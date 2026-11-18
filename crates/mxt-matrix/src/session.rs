//! The session file, password login and session restore.

use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use matrix_sdk::{
    Client, authentication::matrix::MatrixSession,
    ruma::api::client::session::get_login_types::v3::LoginType,
};
use serde::{Deserialize, Serialize};

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

impl fmt::Debug for SavedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedSession")
            .field("homeserver", &self.homeserver)
            .field("user_id", &self.session.meta.user_id)
            .field("device_id", &self.session.meta.device_id)
            .finish_non_exhaustive()
    }
}

/// `Ok(None)` when the file does not exist.
pub fn load_session(path: &Path) -> Result<Option<SavedSession>, Error> {
    match fs::read(path) {
        Ok(json) => Ok(Some(serde_json::from_slice(&json)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Writes a temporary file with mode 0600 next to `path`, then renames it, so
/// a crash never leaves a partial session file.
pub fn save_session(path: &Path, session: &SavedSession) -> Result<(), Error> {
    let json = serde_json::to_vec_pretty(session)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    // Remove a leftover of an interrupted save, so `create_new` makes a fresh
    // file with this mode and never follows a symlink.
    match fs::remove_file(&temporary) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&json)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    Ok(())
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
/// `Homeserver::Saved(session.homeserver)`. Separate from `build_client` so
/// the caller can subscribe to `Client::subscribe_to_session_changes` in
/// between: the E2EE tasks that restoring spawns send requests at once, and
/// an `M_UNKNOWN_TOKEN` from them is only reported there.
pub async fn restore(client: &Client, session: SavedSession) -> Result<(), Error> {
    client.restore_session(session.session).await?;
    Ok(())
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
