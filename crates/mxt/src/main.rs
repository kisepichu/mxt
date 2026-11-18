//! The mxt binary: command line, startup flow and the composition root.

mod config;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use config::ConfigError;

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
        // The startup tasks continue from here.
        Ok(_config) => ExitCode::SUCCESS,
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
