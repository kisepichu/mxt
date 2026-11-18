//! The data, cache and state directories (design, "Storage").

use std::{
    fs::{self, DirBuilder, Permissions},
    io,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use mxt_matrix::StoreDirs;

/// `$XDG_DATA_HOME/mxt`, `$XDG_CACHE_HOME/mxt` and `$XDG_STATE_HOME/mxt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The session file, the lock file and `store_dir` (the SDK's state and
    /// crypto stores).
    pub data: PathBuf,
    /// The SDK's event cache and media cache.
    pub cache: PathBuf,
    /// The logs.
    pub state: PathBuf,
}

#[allow(dead_code)] // Used by `main` once the startup flow is wired.
impl Paths {
    /// `$XDG_DATA_HOME/mxt`, `$XDG_CACHE_HOME/mxt`, `$XDG_STATE_HOME/mxt`, with
    /// `~/.local/share`, `~/.cache`, `~/.local/state` when a variable is unset,
    /// empty or relative (the XDG base directory specification ignores
    /// relative paths); the same on macOS. `None` when a directory falls back
    /// to `HOME` and `HOME` is unset, empty or relative.
    pub fn from_env(env: &impl Fn(&str) -> Option<String>) -> Option<Paths> {
        let absolute = |name: &str| {
            env(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let dir = |variable: &str, fallback: &str| {
            let base = match absolute(variable) {
                Some(base) => base,
                None => absolute("HOME")?.join(fallback),
            };
            Some(base.join("mxt"))
        };
        Some(Paths {
            data: dir("XDG_DATA_HOME", ".local/share")?,
            cache: dir("XDG_CACHE_HOME", ".cache")?,
            state: dir("XDG_STATE_HOME", ".local/state")?,
        })
    }

    pub fn session_file(&self) -> PathBuf {
        self.data.join("session.json")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data.join("lock")
    }

    pub fn store_dir(&self) -> PathBuf {
        self.data.join("store")
    }

    /// The SDK's stores: state and crypto in `store_dir`, where
    /// `set_aside_orphaned_store` looks for them, never in `data` itself.
    pub fn store_dirs(&self) -> StoreDirs {
        StoreDirs {
            store: self.store_dir(),
            cache: self.cache.clone(),
        }
    }

    /// Creates the three directories with mode 0700, and tightens existing
    /// ones to 0700: the cache holds decrypted events and media in SQLite
    /// files without a passphrase. Errors name the directory.
    pub fn create_dirs(&self) -> io::Result<()> {
        for dir in [&self.data, &self.cache, &self.state] {
            create_private_dir(dir)?;
        }
        Ok(())
    }
}

/// Creates `dir` and its missing parents with mode 0700, and tightens `dir`
/// to 0700 when it exists already. Errors name the directory.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .and_then(|()| fs::set_permissions(dir, Permissions::from_mode(0o700)))
        .map_err(|error| with_path(error, dir))
}

/// `error` with `path` in its message, for startup failures that are printed.
pub fn with_path(error: io::Error, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn every_variable_set() {
        let paths = Paths::from_env(&env(&[
            ("HOME", "/home/alice"),
            ("XDG_DATA_HOME", "/xdg/data"),
            ("XDG_CACHE_HOME", "/xdg/cache"),
            ("XDG_STATE_HOME", "/xdg/state"),
        ]));
        assert_eq!(
            paths,
            Some(Paths {
                data: "/xdg/data/mxt".into(),
                cache: "/xdg/cache/mxt".into(),
                state: "/xdg/state/mxt".into(),
            })
        );
    }

    #[test]
    fn the_xdg_variables_do_not_need_home() {
        let paths = Paths::from_env(&env(&[
            ("XDG_DATA_HOME", "/xdg/data"),
            ("XDG_CACHE_HOME", "/xdg/cache"),
            ("XDG_STATE_HOME", "/xdg/state"),
        ]));
        assert!(paths.is_some());
    }

    #[test]
    fn empty_and_relative_values_are_treated_as_unset() {
        let paths = Paths::from_env(&env(&[
            ("HOME", "/home/alice"),
            ("XDG_DATA_HOME", ""),
            ("XDG_CACHE_HOME", "relative/cache"),
        ]));
        assert_eq!(
            paths,
            Some(Paths {
                data: "/home/alice/.local/share/mxt".into(),
                cache: "/home/alice/.cache/mxt".into(),
                state: "/home/alice/.local/state/mxt".into(),
            })
        );
    }

    #[test]
    fn no_home_gives_none() {
        assert_eq!(Paths::from_env(&env(&[])), None);
        assert_eq!(Paths::from_env(&env(&[("HOME", "")])), None);
        assert_eq!(
            Paths::from_env(&env(&[
                ("XDG_DATA_HOME", "/xdg/data"),
                ("XDG_CACHE_HOME", "/xdg/cache"),
            ])),
            None
        );
    }

    #[test]
    fn derived_file_paths() {
        let paths = Paths::from_env(&env(&[("HOME", "/home/alice")])).unwrap();
        assert_eq!(
            paths.session_file(),
            Path::new("/home/alice/.local/share/mxt/session.json")
        );
        assert_eq!(
            paths.lock_file(),
            Path::new("/home/alice/.local/share/mxt/lock")
        );
        assert_eq!(
            paths.store_dir(),
            Path::new("/home/alice/.local/share/mxt/store")
        );
    }

    #[test]
    fn create_dirs_makes_private_directories_and_tightens_existing_ones() {
        let root = TestDir::new("paths-create-dirs");
        let paths = Paths {
            data: root.join("data/mxt"),
            cache: root.join("cache/mxt"),
            state: root.join("state/mxt"),
        };
        for existing in [&paths.data, &paths.cache] {
            fs::create_dir_all(existing).unwrap();
            fs::set_permissions(existing, Permissions::from_mode(0o755)).unwrap();
        }

        paths.create_dirs().unwrap();

        for dir in [&paths.data, &paths.cache, &paths.state] {
            let mode = fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
        }
    }
}
