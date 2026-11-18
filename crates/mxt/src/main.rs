//! The mxt binary: command line, startup flow and the composition root.

mod config;
mod event_loop;
mod invalidate;
mod lock;
mod logging;
mod paths;
mod prompt;
#[cfg(test)]
mod test_dir;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use config::{Config, ConfigError};
use mxt_core::app::App;
use mxt_tui::Tui;
use tokio::sync::mpsc;

const USAGE: &str = "usage: mxt [--config PATH]\n       mxt --version";

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let config_path = match args.as_slice() {
        [] => None,
        [flag] if flag == "--version" => {
            println!("mxt {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        [flag, path] if flag == "--config" => Some(PathBuf::from(path)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let env = |name: &str| std::env::var(name).ok();
    let Some(path) = config_path.or_else(|| config::default_path(&env)) else {
        eprintln!("mxt: neither XDG_CONFIG_HOME nor HOME is set; pass --config PATH");
        return ExitCode::FAILURE;
    };
    let Some(home) = env("HOME").filter(|home| !home.is_empty()) else {
        eprintln!("mxt: HOME is not set");
        return ExitCode::FAILURE;
    };

    match config::load(&path, Path::new(&home)) {
        Ok(config) => run_ui(config),
        Err(ConfigError::NotFound { path }) => {
            eprint!(
                "mxt: {} does not exist. A minimal configuration:\n\n{}",
                path.display(),
                config::MINIMAL_EXAMPLE
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("mxt: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The terminal UI until `:q`.
fn run_ui(config: Config) -> ExitCode {
    let mut app = App::new(config.keymap, config.settings, config.user_id);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("mxt: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    mxt_tui::install_panic_hook();
    let mut tui = match Tui::new() {
        Ok(tui) => tui,
        Err(error) => {
            eprintln!("mxt: cannot set up the terminal: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The matrix adapter (the sync task) takes `_to_core` and
    // `_matrix_effects`; until then they stay open, so the loop sees neither
    // channel closed.
    let (_to_core, from_matrix) = mpsc::unbounded_channel();
    let (to_matrix, _matrix_effects) = mpsc::unbounded_channel();
    let result = runtime.block_on(event_loop::run(&mut app, &mut tui, from_matrix, to_matrix));
    let restored = tui.restore();
    match result.and(restored) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mxt: terminal: {error}");
            ExitCode::FAILURE
        }
    }
}
