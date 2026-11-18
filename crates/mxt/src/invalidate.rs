//! Setting aside data that belongs to a session the homeserver no longer
//! accepts, or to a device whose session file is gone.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::paths::{Paths, with_path};

/// `M_UNKNOWN_TOKEN`: renames the data directory to
/// `<parent>/mxt.invalid-<unix seconds>` and deletes the cache directory.
/// Returns the new path. The caller has dropped the SDK client before, so the
/// stores are closed.
#[allow(dead_code)] // Used by `main` once the startup flow is wired.
pub fn invalidate(paths: &Paths, unix_seconds: u64) -> io::Result<PathBuf> {
    let moved = paths
        .data
        .with_file_name(format!("mxt.invalid-{unix_seconds}"));
    rename(&paths.data, &moved)?;
    match fs::remove_dir_all(&paths.cache) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(with_path(error, &paths.cache))
        }
        _ => Ok(moved),
    }
}

/// Moves a store without a session file (left by a device whose session was
/// lost) to `data/store.invalid-<unix seconds>`, so a new login never reuses
/// another device's crypto store. No-op when there is no store.
#[allow(dead_code)] // Used by `main` once the startup flow is wired.
pub fn set_aside_orphaned_store(paths: &Paths, unix_seconds: u64) -> io::Result<Option<PathBuf>> {
    let store = paths.store_dir();
    let session_file = paths.session_file();
    let session_exists = session_file
        .try_exists()
        .map_err(|error| with_path(error, &session_file))?;
    let store_exists = store
        .try_exists()
        .map_err(|error| with_path(error, &store))?;
    if session_exists || !store_exists {
        return Ok(None);
    }
    let moved = paths.data.join(format!("store.invalid-{unix_seconds}"));
    rename(&store, &moved)?;
    Ok(Some(moved))
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
    use std::fs;

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

    #[test]
    fn an_orphaned_store_is_set_aside() {
        let root = TestDir::new("orphaned-store");
        let paths = paths_in(&root);
        fs::create_dir(paths.store_dir()).unwrap();
        fs::write(paths.store_dir().join("crypto"), "").unwrap();

        let moved = set_aside_orphaned_store(&paths, 1_700_000_000).unwrap();

        let expected = paths.data.join("store.invalid-1700000000");
        assert_eq!(moved.as_ref(), Some(&expected));
        assert!(expected.join("crypto").is_file());
        assert!(!paths.store_dir().exists());
    }

    #[test]
    fn a_store_with_a_session_file_stays() {
        let root = TestDir::new("store-with-session");
        let paths = paths_in(&root);
        fs::create_dir(paths.store_dir()).unwrap();
        fs::write(paths.session_file(), "{}").unwrap();

        assert_eq!(
            set_aside_orphaned_store(&paths, 1_700_000_000).unwrap(),
            None
        );
        assert!(paths.store_dir().exists());
    }

    #[test]
    fn no_store_is_a_no_op() {
        let root = TestDir::new("no-store");
        let paths = paths_in(&root);

        assert_eq!(
            set_aside_orphaned_store(&paths, 1_700_000_000).unwrap(),
            None
        );
    }
}
