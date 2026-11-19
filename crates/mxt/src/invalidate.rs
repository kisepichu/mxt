//! Setting aside data that belongs to a session the homeserver no longer
//! accepts, or to a device whose session file is gone, and deleting the new
//! store of a first login that failed.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::paths::{Paths, create_private_dir, with_path};

/// `M_UNKNOWN_TOKEN`: deletes the cache directory, then renames the data
/// directory to `<parent>/mxt.invalid-<unix seconds>` and returns the new
/// path. The cache goes first because it can always be rebuilt: an error
/// always means the data directory has not moved. The caller has dropped the
/// SDK client before, so the stores are closed.
pub fn invalidate(paths: &Paths, unix_seconds: u64) -> io::Result<PathBuf> {
    delete_dir(&paths.cache)?;
    let moved = paths
        .data
        .with_file_name(format!("mxt.invalid-{unix_seconds}"));
    rename(&paths.data, &moved)?;
    Ok(moved)
}

/// Without a session file, the stores belong to a device whose session was
/// lost, or to another account: moves the state and crypto store to
/// `data/store.invalid-<unix seconds>`, so a new login never reuses another
/// device's crypto store, and empties the cache directory (left with mode
/// 0700), so the new device never shows the old one's decrypted events and
/// media. Returns the moved store, `None` when there was none. No-op while
/// the session file exists.
pub fn set_aside_orphaned_store(paths: &Paths, unix_seconds: u64) -> io::Result<Option<PathBuf>> {
    if session_file_exists(paths)? {
        return Ok(None);
    }
    // Recreated here: the SDK would create it with the default mode.
    delete_dir(&paths.cache)?;
    create_private_dir(&paths.cache)?;
    let store = paths.store_dir();
    let store_exists = store
        .try_exists()
        .map_err(|error| with_path(error, &store))?;
    if !store_exists {
        return Ok(None);
    }
    let moved = paths.data.join(format!("store.invalid-{unix_seconds}"));
    rename(&store, &moved)?;
    Ok(Some(moved))
}

/// A first login that failed before its session was saved: deletes the new
/// store and the cache directory. Before a login the crypto store holds no
/// Olm account, and after a login whose session was not saved the access
/// token is lost, so no one can use that device again. No-op while the
/// session file exists: saving can fail after the rename. The caller has
/// dropped the SDK client before, so the stores are closed.
pub fn discard_new_store(paths: &Paths) -> io::Result<()> {
    if session_file_exists(paths)? {
        return Ok(());
    }
    delete_dir(&paths.store_dir())?;
    delete_dir(&paths.cache)
}

fn session_file_exists(paths: &Paths) -> io::Result<bool> {
    let session_file = paths.session_file();
    session_file
        .try_exists()
        .map_err(|error| with_path(error, &session_file))
}

/// The current time for the names of set-aside directories.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `fs::remove_dir_all`; a missing directory is fine.
fn delete_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(with_path(error, dir)),
        _ => Ok(()),
    }
}

fn rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("moving {} to {}: {error}", from.display(), to.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt as _,
    };

    use super::*;
    use crate::test_dir::TestDir;

    fn paths_in(root: &TestDir) -> Paths {
        let paths = Paths {
            data: root.join("data/mxt"),
            cache: root.join("cache/mxt"),
            state: root.join("state/mxt"),
        };
        paths.create_dirs().unwrap();
        paths
    }

    #[test]
    fn invalidate_moves_the_data_directory_and_deletes_the_cache() {
        let root = TestDir::new("invalidate");
        let paths = paths_in(&root);
        fs::write(paths.session_file(), "{}").unwrap();
        fs::write(paths.cache.join("event-cache"), "").unwrap();

        let moved = invalidate(&paths, 1_700_000_000).unwrap();

        assert_eq!(moved, root.join("data/mxt.invalid-1700000000"));
        assert!(moved.join("session.json").is_file());
        assert!(!paths.data.exists());
        assert!(!paths.cache.exists());
        assert!(paths.state.exists());
    }

    /// Empty, and private even if it was not before.
    fn assert_empty_and_private(dir: &Path) {
        assert_eq!(fs::read_dir(dir).unwrap().count(), 0, "{}", dir.display());
        let mode = fs::metadata(dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
    }

    #[test]
    fn an_orphaned_store_is_set_aside_and_the_cache_emptied() {
        let root = TestDir::new("orphaned-store");
        let paths = paths_in(&root);
        fs::create_dir(paths.store_dir()).unwrap();
        fs::write(paths.store_dir().join("crypto"), "").unwrap();
        fs::write(paths.cache.join("event-cache"), "").unwrap();
        fs::set_permissions(&paths.cache, Permissions::from_mode(0o755)).unwrap();

        let moved = set_aside_orphaned_store(&paths, 1_700_000_000).unwrap();

        let expected = paths.data.join("store.invalid-1700000000");
        assert_eq!(moved.as_ref(), Some(&expected));
        assert!(expected.join("crypto").is_file());
        assert!(!paths.store_dir().exists());
        assert_empty_and_private(&paths.cache);
    }

    #[test]
    fn a_store_and_cache_with_a_session_file_stay() {
        let root = TestDir::new("store-with-session");
        let paths = paths_in(&root);
        fs::create_dir(paths.store_dir()).unwrap();
        fs::write(paths.session_file(), "{}").unwrap();
        fs::write(paths.cache.join("event-cache"), "").unwrap();

        assert_eq!(
            set_aside_orphaned_store(&paths, 1_700_000_000).unwrap(),
            None
        );
        discard_new_store(&paths).unwrap();
        assert!(paths.store_dir().exists());
        assert!(paths.cache.join("event-cache").exists());
    }

    #[test]
    fn the_new_store_of_a_failed_first_login_is_deleted_with_the_cache() {
        let root = TestDir::new("discard-new-store");
        let paths = paths_in(&root);
        fs::create_dir(paths.store_dir()).unwrap();
        fs::write(paths.store_dir().join("crypto"), "").unwrap();
        fs::write(paths.lock_file(), "").unwrap();
        fs::write(paths.cache.join("event-cache"), "").unwrap();

        discard_new_store(&paths).unwrap();

        assert!(!paths.store_dir().exists());
        assert!(!paths.cache.exists());
        assert!(paths.lock_file().is_file());
    }

    #[test]
    fn without_a_store_the_cache_is_still_emptied() {
        let root = TestDir::new("no-store");
        let paths = paths_in(&root);
        fs::write(paths.cache.join("event-cache"), "").unwrap();

        assert_eq!(
            set_aside_orphaned_store(&paths, 1_700_000_000).unwrap(),
            None
        );
        assert_empty_and_private(&paths.cache);
    }
}
