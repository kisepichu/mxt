//! The session file, password login and session restore.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use matrix_sdk::{
    Client, SessionChange, authentication::matrix::MatrixSession,
    ruma::api::client::session::get_login_types::v3::LoginType,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::Error;

/// What `session.json` holds. The access token is in here; it is never
/// printed or logged.
#[derive(Serialize, Deserialize)]
pub struct SavedSession {
    /// The homeserver URL after login (the login response's `.well-known` can
    /// change it), so restoring needs no discovery.
    pub homeserver: String,
    pub session: MatrixSession,
}

impl SavedSession {
    /// The account the session belongs to.
    pub fn user_id(&self) -> &str {
        self.session.meta.user_id.as_str()
    }
}

impl fmt::Debug for SavedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedSession")
            .field("homeserver", &self.homeserver)
            .field("user_id", &self.session.meta.user_id)
            .field("device_id", &self.session.meta.device_id)
            .finish_non_exhaustive()
    }
}

/// `Ok(None)` when the file does not exist. Errors name the file.
pub fn load_session(path: &Path) -> Result<Option<SavedSession>, Error> {
    match fs::read(path) {
        Ok(json) => serde_json::from_slice(&json)
            .map(Some)
            .map_err(json_error(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(path)(error)),
    }
}

/// Writes a temporary file with mode 0600 next to `path`, renames it and syncs
/// the directory, so a crash never leaves a partial session file and a saved
/// one stays saved. When writing or renaming fails, the temporary file (which
/// holds the access token) is removed. Errors name the file.
pub fn save_session(path: &Path, session: &SavedSession) -> Result<(), Error> {
    let json = serde_json::to_vec_pretty(session).map_err(json_error(path))?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    // Remove a leftover of an interrupted save, so `create_new` makes a fresh
    // file with this mode and never follows a symlink.
    match fs::remove_file(&temporary) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(io_error(&temporary)(error));
        }
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(io_error(&temporary))?;
    let saved = file
        .write_all(&json)
        .and_then(|()| file.sync_all())
        .map_err(io_error(&temporary))
        .and_then(|()| fs::rename(&temporary, path).map_err(io_error(path)));
    if let Err(error) = saved {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(io_error(dir))
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |error| Error::Io {
        path: path.to_owned(),
        error,
    }
}

fn json_error(path: &Path) -> impl FnOnce(serde_json::Error) -> Error + '_ {
    move |error| Error::Json {
        path: path.to_owned(),
        error,
    }
}

/// Password login as a new device with the display name "mxt". No refresh
/// token is requested, so the saved access token never changes.
pub async fn login(client: &Client, user_id: &str, password: &str) -> Result<SavedSession, Error> {
    let auth = client.matrix_auth();
    let login_types = auth.get_login_types().await?;
    if !login_types
        .flows
        .iter()
        .any(|flow| matches!(flow, LoginType::Password(_)))
    {
        return Err(Error::PasswordLoginUnsupported {
            offered: login_types
                .flows
                .iter()
                .map(|flow| flow.login_type().to_owned())
                .collect(),
        });
    }
    let response = auth
        .login_username(user_id, password)
        .initial_device_display_name("mxt")
        .send()
        .await?;
    Ok(SavedSession {
        homeserver: client.homeserver().to_string(),
        session: MatrixSession::from(&response),
    })
}

/// Restores a saved session on a client built with
/// `Homeserver::Url(session.homeserver)`. Separate from `build_client` so
/// the caller can create a `SessionWatch` in between: the E2EE tasks that
/// restoring spawns send requests at once, and an `M_UNKNOWN_TOKEN` from
/// them is only reported there.
pub async fn restore(client: &Client, session: SavedSession) -> Result<(), Error> {
    client.restore_session(session.session).await?;
    Ok(())
}

/// Reports when the homeserver answers `M_UNKNOWN_TOKEN` to any request of
/// the client, including those of the SDK's own tasks and the sync.
pub struct SessionWatch(broadcast::Receiver<SessionChange>);

impl SessionWatch {
    /// Create it before `restore` or `login`: the E2EE tasks that restoring
    /// spawns send requests at once, and only changes after this call are
    /// seen.
    pub fn new(client: &Client) -> SessionWatch {
        SessionWatch(client.subscribe_to_session_changes())
    }

    /// Waits for the next `M_UNKNOWN_TOKEN`; pending forever once the client
    /// is gone. Cancel safe.
    pub async fn unknown_token(&mut self) {
        loop {
            match self.0.recv().await {
                Ok(SessionChange::UnknownToken(_)) => return,
                // mxt uses no refresh tokens, so this does not come.
                Ok(SessionChange::TokensRefreshed) => {}
                // The newest changes are still delivered after a lag.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => std::future::pending().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use matrix_sdk::{
        SessionMeta, SessionTokens,
        ruma::{owned_device_id, owned_user_id},
    };

    use super::*;

    /// A fresh directory under the system's temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path =
                std::env::temp_dir().join(format!("mxt-matrix-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn made_up_session() -> SavedSession {
        SavedSession {
            homeserver: "https://matrix.example.org/".to_owned(),
            session: MatrixSession {
                meta: SessionMeta {
                    user_id: owned_user_id!("@alice:example.org"),
                    device_id: owned_device_id!("ABCDEFGHIJ"),
                },
                tokens: SessionTokens {
                    access_token: "not-a-real-token".to_owned(),
                    refresh_token: None,
                },
            },
        }
    }

    #[test]
    fn a_saved_session_loads_back_and_is_private() {
        let dir = TempDir::new("round-trip");
        let path = dir.0.join("session.json");

        save_session(&path, &made_up_session()).unwrap();
        let loaded = load_session(&path)
            .unwrap()
            .expect("the session file exists");

        assert_eq!(loaded.homeserver, "https://matrix.example.org/");
        assert_eq!(loaded.session, made_up_session().session);
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn saving_replaces_an_existing_session_file() {
        let dir = TempDir::new("replace");
        let path = dir.0.join("session.json");
        fs::write(&path, "old").unwrap();

        save_session(&path, &made_up_session()).unwrap();

        assert!(load_session(&path).unwrap().is_some());
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            1,
            "no temporary file is left"
        );
    }

    #[test]
    fn a_leftover_temporary_file_is_replaced_by_a_private_one() {
        let dir = TempDir::new("leftover");
        let path = dir.0.join("session.json");
        let temporary = dir.0.join("session.json.tmp");
        fs::write(&temporary, "partial").unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).unwrap();

        save_session(&path, &made_up_session()).unwrap();

        assert!(load_session(&path).unwrap().is_some());
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!temporary.exists());
    }

    #[test]
    fn a_failed_save_names_the_file_and_leaves_no_temporary_file() {
        let dir = TempDir::new("failed-save");
        // Renaming a file onto a non-empty directory fails.
        let path = dir.0.join("session.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("occupied"), "").unwrap();

        let error = save_session(&path, &made_up_session()).unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with(&format!("{}: ", path.display())),
            "{error}"
        );
        assert!(!dir.0.join("session.json.tmp").exists());
    }

    #[test]
    fn a_missing_session_file_loads_as_none() {
        let dir = TempDir::new("missing");
        assert!(load_session(&dir.0.join("session.json")).unwrap().is_none());
    }

    #[test]
    fn debug_output_has_no_token() {
        let debug = format!("{:?}", made_up_session());
        assert!(!debug.contains("not-a-real-token"), "{debug}");
        assert!(debug.contains("@alice:example.org"), "{debug}");
    }
}
