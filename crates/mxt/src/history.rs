//! The history file of the `:` command line and the `/` search lines
//! (design, "Line editing"): read once at the start, and replaced by a
//! writer task after each executed line.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use mxt_core::effect::StoreEffect;
use mxt_core::msg::Msg;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// The log target of everything about the file.
const TARGET: &str = "mxt::history";

/// The file's first line, which a later format would change.
const HEADER: &str = "mxt history 1";

/// The two histories as the file holds them, each oldest first.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Histories {
    pub commands: Vec<String>,
    pub searches: Vec<String>,
}

/// Reads the file at `path`. A missing file gives empty histories, logged at
/// `info`: there is none until the first line is executed. A file that
/// cannot be read or is broken gives empty histories too, with a warning;
/// the next write replaces it.
pub fn load(path: &Path) -> Histories {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info!(
                target: TARGET,
                "no command line history at {}; starting with an empty one",
                path.display()
            );
            return Histories::default();
        }
        Err(error) => {
            warn!(
                target: TARGET,
                "cannot read the command line history {}: {error}; starting with an empty one",
                path.display()
            );
            return Histories::default();
        }
    };
    parse(&bytes).unwrap_or_else(|reason| {
        warn!(
            target: TARGET,
            "the command line history {} is broken ({reason}); starting with an empty one",
            path.display()
        );
        Histories::default()
    })
}

/// The file's contents: [`HEADER`], then a line per entry, `:` and a
/// command line's text, then `/` and a search line's text. The core's lines
/// hold no line break.
pub fn format(commands: &[String], searches: &[String]) -> String {
    let mut text = format!("{HEADER}\n");
    for (prefix, lines) in [(':', commands), ('/', searches)] {
        for line in lines {
            text.push(prefix);
            text.push_str(line);
            text.push('\n');
        }
    }
    text
}

/// The histories in a file's contents, or why they are broken. The reason
/// names a line by its number, never its text.
fn parse(bytes: &[u8]) -> Result<Histories, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_owned())?;
    let mut lines = text.strip_suffix('\n').unwrap_or(text).split('\n');
    if lines.next() != Some(HEADER) {
        return Err(format!("its first line is not \"{HEADER}\""));
    }
    let mut histories = Histories::default();
    // The header is line 1.
    for (number, line) in (2..).zip(lines) {
        if let Some(command) = line.strip_prefix(':') {
            histories.commands.push(command.to_owned());
        } else if let Some(search) = line.strip_prefix('/') {
            histories.searches.push(search.to_owned());
        } else {
            return Err(format!("line {number} starts with neither ':' nor '/'"));
        }
    }
    Ok(histories)
}

/// A failed write: the file it failed on, and the error.
#[derive(Debug)]
pub struct WriteError {
    path: PathBuf,
    error: io::Error,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}

/// Replaces the file at `path` with `text` as `mxt-matrix`'s `save_session`
/// writes the session file: through `<path>.tmp` with mode 0600, renamed
/// over `path`, then the directory synced, so a crash never leaves a
/// partial file. A leftover `<path>.tmp` is removed first, so `create_new`
/// makes a fresh file with that mode and never follows a symlink placed
/// there; when writing or renaming fails, `<path>.tmp` is removed.
pub fn write(path: &Path, text: &str) -> Result<(), WriteError> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    match fs::remove_file(&temporary) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(failed(&temporary)(error));
        }
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(failed(&temporary))?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(failed(&temporary))
        .and_then(|()| fs::rename(&temporary, path).map_err(failed(path)));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(failed(dir))
}

fn failed(path: &Path) -> impl FnOnce(io::Error) -> WriteError + '_ {
    move |error| WriteError {
        path: path.to_owned(),
        error,
    }
}

/// The task that executes `Effect::Store`: it writes the snapshots in
/// order, on a blocking thread, a newer one replacing any still waiting.
pub struct Writer {
    snapshots: UnboundedSender<StoreEffect>,
    task: JoinHandle<()>,
}

impl Writer {
    /// Starts the task, which writes to `path`. The first failed write of
    /// the run goes to the status line as `Msg::EffectFailed` on `to_core`,
    /// which the event loop logs at `warn`, so the task logs its full error
    /// only at `debug`; later failures are logged at `warn` and not shown,
    /// so they do not hide each command's own status.
    pub fn spawn(path: PathBuf, to_core: UnboundedSender<Msg>) -> Writer {
        let (snapshots, mut waiting) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut reported = false;
            while let Some(mut effect) = waiting.recv().await {
                while let Ok(newer) = waiting.try_recv() {
                    effect = newer;
                }
                let StoreEffect::SaveHistory { commands, searches } = effect;
                let text = format(&commands, &searches);
                let file = path.clone();
                let written = tokio::task::spawn_blocking(move || write(&file, &text))
                    .await
                    .unwrap_or_else(|error| {
                        Err(WriteError {
                            path: path.clone(),
                            error: io::Error::from(error),
                        })
                    });
                let Err(error) = written else {
                    continue;
                };
                if reported {
                    warn!(target: TARGET, "saving the command line history failed: {error}");
                    continue;
                }
                reported = true;
                debug!(target: TARGET, "saving the command line history failed: {error}");
                // The event loop has stopped when this fails, and with it
                // the status line.
                let _ = to_core.send(Msg::EffectFailed {
                    error: format!("Saving the command line history failed: {}", error.error),
                });
            }
        });
        Writer { snapshots, task }
    }

    /// Where the event loop sends `Effect::Store`.
    pub fn sender(&self) -> UnboundedSender<StoreEffect> {
        self.snapshots.clone()
    }

    /// Closes the channel and waits until the task has written what it was
    /// sent; the event loop's sender must be gone.
    pub async fn finish(self) {
        drop(self.snapshots);
        if let Err(error) = self.task.await {
            warn!(target: TARGET, "the command line history writer stopped: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use tokio::sync::mpsc::error::TryRecvError;

    use super::*;
    use crate::test_dir::TestDir;

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }
    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
    fn save(commands: &[&str], searches: &[&str]) -> StoreEffect {
        StoreEffect::SaveHistory {
            commands: owned(commands),
            searches: owned(searches),
        }
    }

    #[test]
    fn a_written_history_loads_back_with_mode_0600_and_no_temporary_file() {
        let dir = TestDir::new("history-round-trip");
        let path = dir.join("history");
        let histories = Histories {
            commands: owned(&["open foo bar", "join #日本語:example.org", "react 👍🏽"]),
            searches: owned(&["テキスト", " a b ", "/:"]),
        };

        write(&path, &format(&histories.commands, &histories.searches)).unwrap();

        assert_eq!(load(&path), histories);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "mxt history 1\n\
             :open foo bar\n\
             :join #日本語:example.org\n\
             :react 👍🏽\n\
             /テキスト\n\
             / a b \n\
             //:\n"
        );
        assert_eq!(mode(&path), 0o600);
        assert!(!dir.join("history.tmp").exists());
    }

    #[test]
    fn a_leftover_temporary_file_is_replaced_with_mode_0600() {
        let dir = TestDir::new("history-leftover");
        let path = dir.join("history");
        let temporary = dir.join("history.tmp");
        fs::write(&temporary, "leftover").unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).unwrap();

        write(&path, &format(&owned(&["quit"]), &[])).unwrap();

        assert_eq!(load(&path).commands, owned(&["quit"]));
        assert_eq!(mode(&path), 0o600);
        assert!(!temporary.exists());
    }

    #[test]
    fn a_symlink_at_the_temporary_file_is_removed_not_followed() {
        let dir = TestDir::new("history-symlink");
        let path = dir.join("history");
        let other = dir.join("other");
        fs::write(&other, "keep").unwrap();
        symlink(&other, dir.join("history.tmp")).unwrap();

        write(&path, &format(&owned(&["quit"]), &[])).unwrap();

        assert_eq!(fs::read_to_string(&other).unwrap(), "keep");
        assert_eq!(load(&path).commands, owned(&["quit"]));
        assert!(fs::symlink_metadata(dir.join("history.tmp")).is_err());
    }

    #[test]
    fn a_missing_or_broken_file_gives_empty_histories() {
        let dir = TestDir::new("history-broken");
        let path = dir.join("history");
        assert_eq!(load(&path), Histories::default());
        // Not a file.
        assert_eq!(load(&dir), Histories::default());
        for broken in [
            &b"mxt history 1\n:open\xff\n"[..],
            b"mxt history 2\n:quit\n",
            b"",
            b"mxt history 1\n:quit\nx\n",
            b"mxt history 1\n:quit\n\n/a\n",
        ] {
            fs::write(&path, broken).unwrap();
            assert_eq!(load(&path), Histories::default(), "{broken:?}");
        }
        // A file without its last line break is whole.
        fs::write(&path, "mxt history 1\n:quit\n/a").unwrap();
        assert_eq!(
            load(&path),
            Histories {
                commands: owned(&["quit"]),
                searches: owned(&["a"]),
            }
        );
    }

    #[tokio::test]
    async fn the_writer_reports_only_its_first_failure() {
        let dir = TestDir::new("history-writer-failure");
        // The directory does not exist, so no write can succeed.
        let path = dir.join("missing/history");
        let (to_core, mut from_writer) = mpsc::unbounded_channel();
        let writer = Writer::spawn(path, to_core);
        let snapshots = writer.sender();

        snapshots.send(save(&["quit"], &[])).unwrap();
        match from_writer.recv().await {
            Some(Msg::EffectFailed { error }) => assert!(
                error.starts_with("Saving the command line history failed: "),
                "{error}"
            ),
            other => panic!("{other:?}"),
        }
        snapshots.send(save(&["quit", "help"], &[])).unwrap();
        drop(snapshots);
        writer.finish().await;

        assert_eq!(from_writer.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[tokio::test]
    async fn a_newer_snapshot_replaces_a_waiting_one() {
        let dir = TestDir::new("history-writer-newest");
        let path = dir.join("history");
        let (to_core, _from_writer) = mpsc::unbounded_channel();
        let writer = Writer::spawn(path.clone(), to_core);
        let snapshots = writer.sender();

        // The single-threaded test runtime runs the task only once this
        // test waits.
        snapshots.send(save(&["quit"], &[])).unwrap();
        snapshots.send(save(&["quit", "help"], &["a"])).unwrap();
        drop(snapshots);
        writer.finish().await;

        assert_eq!(
            load(&path),
            Histories {
                commands: owned(&["quit", "help"]),
                searches: owned(&["a"]),
            }
        );
    }
}
