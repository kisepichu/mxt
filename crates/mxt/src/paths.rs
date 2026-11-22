//! The data, cache and state directories (design, "Storage").

use std::{
    ffi::OsString,
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
    /// The logs and the command line history.
    pub state: PathBuf,
}

impl Paths {
    /// `$XDG_DATA_HOME/mxt`, `$XDG_CACHE_HOME/mxt`, `$XDG_STATE_HOME/mxt`, with
    /// `~/.local/share`, `~/.cache`, `~/.local/state` when a variable is unset,
    /// empty or relative (see `base_dir`); the same on macOS. `None` when a
    /// directory falls back to `HOME` and `HOME` is unset, empty or relative.
    pub fn from_env(env: &impl Fn(&str) -> Option<OsString>) -> Option<Paths> {
        let dir = |variable: &str, fallback: &str| {
            base_dir(env, variable, fallback).map(|base| base.join("mxt"))
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

    /// The history of the `:` command line and the `/` search lines. Its
    /// name does not start with `mxt`: tracing-appender's pruning deletes
    /// the files in `state` whose names start with `mxt` and end with `log`.
    pub fn history_file(&self) -> PathBuf {
        self.state.join("history")
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

    /// The message to print when the data or state directory lies inside the
    /// cache directory: `invalidate` and `set_aside_orphaned_store` delete
    /// the cache directory, which would take the crypto store and the held
    /// lock file, or the logs, with it. `Path::starts_with` compares whole
    /// components, so `/xdg/mxt2` is not inside `/xdg/mxt`; symlinks are not
    /// resolved.
    pub fn check_separate(&self) -> Result<(), String> {
        for (dir, name, variable) in [
            (&self.data, "data", "XDG_DATA_HOME"),
            (&self.state, "state", "XDG_STATE_HOME"),
        ] {
            if dir.starts_with(&self.cache) {
                return Err(format!(
                    "the cache directory {} contains the {name} directory {}; \
                     set XDG_CACHE_HOME and {variable} to different directories",
                    self.cache.display(),
                    dir.display()
                ));
            }
        }
        Ok(())
    }
}

/// An XDG base directory: `$variable`, or `$HOME/<fallback>` when the variable
/// is unset, empty or relative (the XDG base directory specification ignores
/// relative paths). `None` when that falls back to `HOME` and `HOME` is unset,
/// empty or relative. `env` is `std::env::var_os`, so a value that is not UTF-8
/// is used as it is instead of falling back to another directory.
pub fn base_dir(
    env: &impl Fn(&str) -> Option<OsString>,
    variable: &str,
    fallback: &str,
) -> Option<PathBuf> {
    let absolute = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute(variable).or_else(|| Some(absolute("HOME")?.join(fallback)))
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
    use std::os::unix::ffi::OsStringExt as _;

    use super::*;
    use crate::test_dir::TestDir;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn values_that_are_not_utf8_are_used_as_they_are() {
        let data = OsString::from_vec(b"/xdg/d\xffta".to_vec());
        let paths = Paths::from_env(&|name: &str| match name {
            "HOME" => Some("/home/alice".into()),
            "XDG_DATA_HOME" => Some(data.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(paths.data, PathBuf::from(data).join("mxt"));
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
        assert_eq!(
            paths.history_file(),
            Path::new("/home/alice/.local/state/mxt/history")
        );
    }

    #[test]
    fn a_data_or_state_directory_inside_the_cache_is_refused() {
        let equal = Paths::from_env(&env(&[
            ("HOME", "/home/alice"),
            ("XDG_DATA_HOME", "/xdg"),
            ("XDG_CACHE_HOME", "/xdg"),
        ]))
        .unwrap();
        assert_eq!(
            equal.check_separate(),
            Err(
                "the cache directory /xdg/mxt contains the data directory /xdg/mxt; \
                 set XDG_CACHE_HOME and XDG_DATA_HOME to different directories"
                    .to_owned()
            )
        );

        let nested = Paths::from_env(&env(&[
            ("HOME", "/home/alice"),
            ("XDG_DATA_HOME", "/xdg/mxt/data"),
            ("XDG_CACHE_HOME", "/xdg"),
        ]))
        .unwrap();
        assert_eq!(
            nested.check_separate(),
            Err(
                "the cache directory /xdg/mxt contains the data directory /xdg/mxt/data/mxt; \
                 set XDG_CACHE_HOME and XDG_DATA_HOME to different directories"
                    .to_owned()
            )
        );

        let state = Paths::from_env(&env(&[
            ("HOME", "/home/alice"),
            ("XDG_STATE_HOME", "/home/alice/.cache"),
        ]))
        .unwrap();
        assert_eq!(
            state.check_separate(),
            Err(
                "the cache directory /home/alice/.cache/mxt contains the state directory \
                 /home/alice/.cache/mxt; set XDG_CACHE_HOME and XDG_STATE_HOME to different \
                 directories"
                    .to_owned()
            )
        );
    }

    #[test]
    fn directories_that_only_share_a_name_prefix_are_separate() {
        let defaults = Paths::from_env(&env(&[("HOME", "/home/alice")])).unwrap();
        assert_eq!(defaults.check_separate(), Ok(()));

        let prefix = Paths::from_env(&env(&[
            ("HOME", "/home/alice"),
            ("XDG_DATA_HOME", "/xdg/mxt2"),
            ("XDG_STATE_HOME", "/xdg/mxt-state"),
            ("XDG_CACHE_HOME", "/xdg"),
        ]))
        .unwrap();
        assert_eq!(prefix.check_separate(), Ok(()));
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
