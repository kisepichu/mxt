//! Log files (design, "Logging"). Nothing is written to the terminal.

use std::path::Path;

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{self, Rotation},
};
use tracing_subscriber::EnvFilter;

/// `info` for mxt's crates, `warn` for matrix-rust-sdk and everything else.
const DEFAULT_FILTER: &str = "warn,mxt=info,mxt_core=info,mxt_matrix=info,mxt_tui=info";

/// Daily files `mxt.<date>.log` in `state_dir`, seven kept; dates and
/// timestamps are UTC. `filter` is `MXT_LOG`, an `EnvFilter` directive; unset
/// or empty means the default. Keep the guard until exit: dropping it flushes
/// the remaining lines.
pub fn init_logging(
    state_dir: &Path,
    filter: Option<&str>,
) -> Result<WorkerGuard, Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_new(
        filter
            .filter(|filter| !filter.is_empty())
            .unwrap_or(DEFAULT_FILTER),
    )?;
    let appender = rolling::Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("mxt")
        .filename_suffix("log")
        .max_log_files(7)
        .build(state_dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
        .map_err(|error| error as Box<dyn std::error::Error>)?;
    Ok(guard)
}
