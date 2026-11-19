//! Builds the SDK client with the stores and encryption settings of the
//! design.

use std::path::PathBuf;

use matrix_sdk::{
    Client,
    config::RequestConfig,
    encryption::{BackupDownloadStrategy, EncryptionSettings},
    sliding_sync::VersionBuilder,
};

use crate::Error;

/// Where the client finds its homeserver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Homeserver {
    /// Discover from the user ID's server name via `.well-known`.
    Discover(String),
    /// A homeserver URL from the session file or the configuration; building
    /// sends no request, so a restore works offline.
    Url(String),
}

/// The directories of the SDK's SQLite stores.
#[derive(Debug, Clone)]
pub struct StoreDirs {
    /// The state and crypto stores: a directory of their own, not the data
    /// directory itself, so the store can be set aside without the session
    /// file.
    pub store: PathBuf,
    /// The event cache and media cache.
    pub cache: PathBuf,
}

/// SQLite state and crypto stores in `dirs.store` without a passphrase, the
/// event cache and media cache in `dirs.cache`. Room keys are fetched from the
/// key backup when an event fails to decrypt; backups and cross-signing are
/// never created. mxt syncs with classic `/sync`, so the client neither
/// checks nor uses sliding sync. No refresh tokens.
///
/// Requests that use the client's request config, the sync among them, are
/// sent at most 3 times. The SDK's default retries 5xx and 429 answers for up
/// to 15 min within one request, so the sync state would show `synced` while
/// the homeserver is down behind its proxy. With a limit, the SDK also
/// retries network failures, which it otherwise reports at once. No
/// `max_retry_time`: matrix-sdk 0.19 caps each delay between attempts with
/// it, not their sum, and the delays of 3 attempts are 0.5 s and 1 s.
pub async fn build_client(homeserver: &Homeserver, dirs: &StoreDirs) -> Result<Client, Error> {
    let builder = match homeserver {
        Homeserver::Discover(server_name) => {
            Client::builder().server_name_or_homeserver_url(server_name)
        }
        Homeserver::Url(url) => Client::builder().homeserver_url(url),
    };
    let client = builder
        .sqlite_store_with_cache_path(&dirs.store, &dirs.cache, None)
        .with_encryption_settings(EncryptionSettings {
            auto_enable_cross_signing: false,
            backup_download_strategy: BackupDownloadStrategy::AfterDecryptionFailure,
            auto_enable_backups: false,
        })
        .sliding_sync_version_builder(VersionBuilder::None)
        .request_config(RequestConfig::new().retry_limit(3))
        .build()
        .await?;
    Ok(client)
}
