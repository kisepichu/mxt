//! The exclusive lock on the data directory: two processes must never share
//! the crypto store.

use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
};

/// Holds the lock until it is dropped (closing the file releases it).
#[derive(Debug)]
pub struct DataLock {
    _file: File,
}

#[allow(dead_code)] // Used by `main` once the startup flow is wired.
#[derive(Debug)]
pub enum LockError {
    /// Another process holds the lock.
    Held,
    Io(io::Error),
}

#[allow(dead_code)] // Used by `main` once the startup flow is wired.
impl DataLock {
    /// `File::try_lock` on `path`, which is created with mode 0600 if needed.
    pub fn acquire(path: &Path) -> Result<DataLock, LockError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => Ok(DataLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(LockError::Held),
            Err(TryLockError::Error(error)) => Err(LockError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn a_held_lock_cannot_be_taken_again_until_it_is_dropped() {
        let dir = TestDir::new("lock");
        let path = dir.join("lock");

        let first = DataLock::acquire(&path).unwrap();
        assert!(matches!(DataLock::acquire(&path), Err(LockError::Held)));

        drop(first);
        assert!(DataLock::acquire(&path).is_ok());
    }
}
