//! The mxt binary: command line, startup flow and the composition root.

// The future of matrix-sdk-ui's `Room::timeline()` is deeper than the default
// limit allows when it is compiled into this crate; matrix-sdk and
// matrix-sdk-ui raise their own limit to the same value.
#![recursion_limit = "256"]

mod config;
mod event_loop;
mod invalidate;
mod lock;
mod logging;
mod paths;
mod prompt;
mod startup;
#[cfg(test)]
mod test_dir;

use std::env::VarError;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use config::{Config, ConfigError};
use lock::{DataLock, LockError};
use paths::Paths;
use startup::{Invalid, Outcome};
use tokio::runtime::Runtime;
use tracing::error;

const USAGE: &str = "usage: mxt [--config PATH]\n       mxt --version";

/// How long the runtime's tasks get to stop at exit. Stopping them drops the
/// last SDK clients, which closes the stores before the data is moved aside.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// The stack size of the runtime's worker and blocking threads, which poll
/// every future, the SDK's included. ruma-html's `Html` tree has no `Drop`
/// implementation, so dropping a tree recurses once per nesting level, and
/// matrix-sdk-ui sanitizes every message body, which drops the part below the
/// sanitizer's maximum depth. A 64 KiB event holds about 21,800 nested
/// three-byte tags; tokio's default of 2 MiB overflows at about 32,800 levels
/// in release builds and about 5,600 in debug builds, and a stack overflow
/// aborts the process. 16 MiB covers debug builds too.
const THREAD_STACK_SIZE: usize = 16 << 20;

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

    let env = |name: &str| std::env::var_os(name);
    let Some(path) = config_path.or_else(|| config::default_path(&env)) else {
        eprintln!(
            "mxt: neither XDG_CONFIG_HOME nor HOME is set to an absolute path; pass --config PATH"
        );
        return ExitCode::FAILURE;
    };
    let Some(home) = env("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
    else {
        eprintln!("mxt: HOME is not set to an absolute path");
        return ExitCode::FAILURE;
    };

    let config = match config::load(&path, &home) {
        Ok(config) => config,
        Err(ConfigError::NotFound { path }) => {
            eprint!(
                "mxt: {} does not exist. A minimal configuration:\n\n{}",
                path.display(),
                config::MINIMAL_EXAMPLE
            );
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("mxt: {error}");
            return ExitCode::FAILURE;
        }
    };
    // `HOME` is absolute, so every directory has its fallback.
    let Some(paths) = Paths::from_env(&env) else {
        eprintln!("mxt: HOME is not set to an absolute path");
        return ExitCode::FAILURE;
    };
    if let Err(message) = paths.check_separate() {
        eprintln!("mxt: {message}");
        return ExitCode::FAILURE;
    }
    run(config, &paths)
}

/// Everything after the configuration: the directories, the lock, the log,
/// then the startup flow until `:q`. The lock is released last.
fn run(config: Config, paths: &Paths) -> ExitCode {
    if let Err(error) = paths.create_dirs() {
        eprintln!("mxt: {error}");
        return ExitCode::FAILURE;
    }
    let lock_file = paths.lock_file();
    let _lock = match DataLock::acquire(&lock_file) {
        Ok(lock) => lock,
        Err(LockError::Held) => {
            eprintln!(
                "mxt: another mxt is running (lock: {})",
                lock_file.display()
            );
            return ExitCode::FAILURE;
        }
        Err(LockError::Io(error)) => {
            eprintln!("mxt: {}: {error}", lock_file.display());
            return ExitCode::FAILURE;
        }
    };
    let filter = match std::env::var("MXT_LOG") {
        Ok(filter) => Some(filter),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(_)) => {
            eprintln!("mxt: MXT_LOG is not valid UTF-8");
            return ExitCode::FAILURE;
        }
    };
    let _log = match logging::init_logging(&paths.state, filter.as_deref()) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("mxt: logging: {error}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("mxt: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The main thread's stack is the OS default, not `THREAD_STACK_SIZE`, so
    // it only waits for the startup flow, which a blocking thread of the
    // runtime polls. Not `spawn`: the terminal UI is not `Send`.
    let task = {
        let handle = runtime.handle().clone();
        let paths = paths.clone();
        runtime.spawn_blocking(move || handle.block_on(startup::run(config, &paths)))
    };
    let outcome = match runtime.block_on(task) {
        Ok(outcome) => outcome,
        // Nothing cancels the task, so this is a panic, which the hook has
        // printed already; it unwinds `main` as if the flow ran here.
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    };
    runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    match outcome {
        Outcome::Quit => ExitCode::SUCCESS,
        Outcome::Failed(message) => {
            error!("{message}");
            eprintln!("mxt: {message}");
            ExitCode::FAILURE
        }
        Outcome::FirstLoginFailed(message) => {
            error!("{message}");
            eprintln!("mxt: {message}");
            if let Err(failure) = invalidate::discard_new_store(paths) {
                error!("deleting the new store failed: {failure}");
                eprintln!("mxt: deleting the new store failed: {failure}");
            }
            ExitCode::FAILURE
        }
        Outcome::Invalid(reason) => {
            set_aside(paths, reason);
            ExitCode::FAILURE
        }
    }
}

/// The runtime everything runs on: multi-threaded, with `THREAD_STACK_SIZE`.
fn runtime() -> io::Result<Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(THREAD_STACK_SIZE)
        .build()
}

/// Moves the data of a session that cannot be used any more aside, so the
/// next start logs in as a new device with fresh stores.
fn set_aside(paths: &Paths, reason: Invalid) {
    let (why, device) = match reason {
        Invalid::UnknownToken => (
            "the homeserver no longer accepts this session (M_UNKNOWN_TOKEN)".to_owned(),
            "",
        ),
        // The access token may still be valid; it moves aside with the data.
        Invalid::MissingStore => (
            format!(
                "the session has no store ({} is missing), and a new store would give this \
                 device new encryption keys",
                paths.store_dir().display()
            ),
            " The old device stays in the account's device list and can be removed from \
             another client.",
        ),
    };
    match invalidate::invalidate(paths, invalidate::unix_seconds()) {
        Ok(moved) => {
            error!("{why}; moved the data to {}", moved.display());
            eprintln!(
                "mxt: {why}. The old data was moved to {}; the next start logs in as a new \
                 device.{device}",
                moved.display()
            );
        }
        Err(failure) => {
            error!("{why}; moving the data aside failed: {failure}");
            eprintln!("mxt: {why}, and moving the old data aside failed: {failure}");
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::html::{HtmlSanitizerMode, RemoveReplyFallback, sanitize_html};

    use super::runtime;

    /// The deepest `formatted_body` an event can carry, 64 KiB of nested
    /// `<b>` (the event size limit) and then text, sanitized the way
    /// matrix-sdk-ui sanitizes every message body (`MessageType::sanitize` in
    /// `Compat` mode). Dropping its tree recurses once per level.
    fn sanitize_deepest_body() -> String {
        let body = format!("{}x", "<b>".repeat(64 * 1024 / 3));
        sanitize_html(&body, HtmlSanitizerMode::Compat, RemoveReplyFallback::No)
    }

    #[test]
    fn runtime_threads_sanitize_the_deepest_message_body() {
        // The sanitizer keeps 100 levels and drops the 21,745 below them.
        let kept = format!("{}{}", "<b>".repeat(100), "</b>".repeat(100));
        let runtime = runtime().unwrap();
        let task = runtime.spawn(async { sanitize_deepest_body() });
        assert_eq!(runtime.block_on(task).unwrap(), kept);
        let blocking = runtime.spawn_blocking(sanitize_deepest_body);
        assert_eq!(runtime.block_on(blocking).unwrap(), kept);
    }
}
