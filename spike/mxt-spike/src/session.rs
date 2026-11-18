//! The data directory, the session file, the client builder and session restore.

use std::{
    fs::{self, DirBuilder, File, OpenOptions, Permissions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use matrix_sdk::{
    Client, ClientBuildError, Room,
    authentication::matrix::MatrixSession,
    encryption::{BackupDownloadStrategy, EncryptionSettings},
    ruma::RoomId,
    sliding_sync::VersionBuilder,
};
use matrix_sdk_ui::sync_service::{self, SyncService};
use serde::{Deserialize, Serialize};

pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn session_file(&self) -> PathBuf {
        self.root.join("session.json")
    }

    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }

    pub fn test_room_file(&self) -> PathBuf {
        self.root.join("test-room")
    }

    /// Creates the directory with mode 0700, and resets the mode if it exists.
    pub fn ensure(&self) -> Result<()> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)
            .with_context(|| format!("creating {}", self.root.display()))?;
        fs::set_permissions(&self.root, Permissions::from_mode(0o700))
            .with_context(|| format!("setting the mode of {}", self.root.display()))
    }

    /// Takes an exclusive lock so two processes never share the crypto store.
    pub fn lock(&self) -> Result<File> {
        let path = self.root.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        if file.try_lock().is_err() {
            bail!(
                "another mxt-spike process uses {}; two processes must never share the crypto store",
                self.root.display()
            );
        }
        Ok(file)
    }
}

/// Writes a small file with mode 0600, failing if it already exists.
pub fn write_new_private(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

/// What `session.json` holds. The access token is in here and is never printed.
#[derive(Serialize, Deserialize)]
pub struct StoredSession {
    /// The homeserver URL after discovery, so restoring needs no `.well-known` lookup.
    pub homeserver: String,
    pub session: MatrixSession,
}

pub enum Homeserver<'a> {
    /// Discover the homeserver from a server name (`.well-known`), as `login` does.
    ServerName(&'a str),
    /// Use the homeserver URL stored in the session file.
    Url(&'a str),
}

/// Builds the client with the settings every command uses: the sqlite store in
/// `<data dir>/store` without a passphrase, keys from backup after decryption
/// failures, no automatic backup or cross-signing creation, no refresh tokens,
/// and simplified sliding sync when the server advertises it.
pub async fn build_client(dir: &DataDir, homeserver: Homeserver<'_>) -> Result<Client> {
    let builder = Client::builder()
        .sqlite_store(dir.store_dir(), None)
        .with_encryption_settings(EncryptionSettings {
            auto_enable_cross_signing: false,
            backup_download_strategy: BackupDownloadStrategy::AfterDecryptionFailure,
            auto_enable_backups: false,
        });
    let builder = match homeserver {
        Homeserver::ServerName(name) => builder.server_name_or_homeserver_url(name),
        Homeserver::Url(url) => builder.homeserver_url(url),
    };
    match builder
        .clone()
        .sliding_sync_version_builder(VersionBuilder::DiscoverNative)
        .build()
        .await
    {
        Ok(client) => Ok(client),
        Err(ClientBuildError::SlidingSyncVersion(error)) => {
            eprintln!(
                "note: no simplified sliding sync ({error}); only classic-probe, notifications, \
                 bridge-raw and members can work"
            );
            Ok(builder
                .sliding_sync_version_builder(VersionBuilder::None)
                .build()
                .await?)
        }
        Err(error) => Err(error.into()),
    }
}

/// A restored client plus the data directory lock, which lives as long as the client.
pub struct Session {
    pub client: Client,
    _lock: File,
}

/// Restores the session from `session.json`. Fails before any network access
/// when there is no session.
pub async fn restore(dir: &DataDir) -> Result<Session> {
    let path = dir.session_file();
    if !path.exists() {
        bail!(
            "no session ({} does not exist); run `mxt-spike login <user_id>` first",
            path.display()
        );
    }
    dir.ensure()?;
    let lock = dir.lock()?;
    let stored: StoredSession = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("parsing {}", path.display()))?;
    let client = build_client(dir, Homeserver::Url(&stored.homeserver)).await?;
    client
        .restore_session(stored.session)
        .await
        .context("restoring the session")?;
    Ok(Session {
        client,
        _lock: lock,
    })
}

/// Builds and starts the sync service (simplified sliding sync).
pub async fn start_sync(client: &Client) -> Result<SyncService> {
    if matches!(
        client.sliding_sync_version(),
        matrix_sdk::sliding_sync::Version::None
    ) {
        bail!("this command needs simplified sliding sync, which the homeserver does not offer");
    }
    let sync = SyncService::builder(client.clone()).build().await?;
    sync.start().await;
    Ok(sync)
}

/// Fails if the sync service has stopped with an error.
pub fn check_sync(sync: &SyncService) -> Result<()> {
    match sync.state().get() {
        sync_service::State::Error(error) => bail!("the sync service failed: {error}"),
        sync_service::State::Terminated => bail!("the sync service terminated"),
        _ => Ok(()),
    }
}

/// Subscribes the room in the room list and waits until the client knows it.
pub async fn wait_for_room(sync: &SyncService, room_id: &RoomId, wait: Duration) -> Result<Room> {
    let room_list = sync.room_list_service();
    room_list.set_room_subscriptions(&[room_id]).await;
    let client = room_list.client();
    let start = Instant::now();
    loop {
        if let Some(room) = client.get_room(room_id) {
            return Ok(room);
        }
        check_sync(sync)?;
        if start.elapsed() > wait {
            bail!("room {room_id} did not appear within {wait:?}; is the account a member?");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
