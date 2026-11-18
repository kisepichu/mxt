//! Builds the SDK client with the stores and encryption settings of the
//! design.

use std::path::PathBuf;

use matrix_sdk::{
    Client,
    encryption::{BackupDownloadStrategy, EncryptionSettings},
    sliding_sync::VersionBuilder,
};

use crate::Error;

/// Where the client finds its homeserver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Homeserver {
    /// Discover from the user ID's server name via `.well-known`.
    Discover(String),
    /// A homeserver URL, from the configuration or the session file.
    Url(String),
}

/// The directories of the SDK's SQLite stores.
#[derive(Debug, Clone)]
pub struct StoreDirs {
    /// The state and crypto stores.
    pub data: PathBuf,
    /// The event cache and media cache.
    pub cache: PathBuf,
}

/// SQLite state and crypto stores in `dirs.data` without a passphrase, the
/// event cache and media cache in `dirs.cache`. Room keys are fetched from the
/// key backup when an event fails to decrypt; backups and cross-signing are
/// never created. Simplified sliding sync is required: building fails when
/// the homeserver's `/versions` does not advertise it. No refresh tokens.
pub async fn build_client(homeserver: &Homeserver, dirs: &StoreDirs) -> Result<Client, Error> {
    let builder = match homeserver {
        Homeserver::Discover(server_name) => {
            Client::builder().server_name_or_homeserver_url(server_name)
        }
        Homeserver::Url(url) => Client::builder().homeserver_url(url),
    };
    let client = builder
        .sqlite_store_with_cache_path(&dirs.data, &dirs.cache, None)
        .with_encryption_settings(EncryptionSettings {
            auto_enable_cross_signing: false,
            backup_download_strategy: BackupDownloadStrategy::AfterDecryptionFailure,
            auto_enable_backups: false,
        })
        .sliding_sync_version_builder(VersionBuilder::DiscoverNative)
        .build()
        .await?;
    Ok(client)
}
