//! Temporary directories for the file system tests.

use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};

/// A fresh directory under the system's temporary directory, removed on drop.
/// `name` must be unique among the tests; the process ID separates test runs.
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(name: &str) -> TestDir {
        let path = std::env::temp_dir().join(format!("mxt-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TestDir(path)
    }
}

impl Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
