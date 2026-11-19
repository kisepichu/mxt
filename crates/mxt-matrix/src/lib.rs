//! The matrix-rust-sdk adapter of mxt: builds the SDK client, converts SDK
//! data into `mxt-core` domain types and executes matrix effects.

mod adapter;
mod backoff;
mod bridges;
mod client;
mod error;
mod recovery;
mod rooms;
mod session;

pub use adapter::{Matrix, build_sync_service};
pub use client::{Homeserver, StoreDirs, build_client};
pub use error::Error;
pub use matrix_sdk::Client;
pub use matrix_sdk_ui::sync_service::SyncService;
pub use recovery::{RecoveryStatus, recover, recovery_status, wait_for_device_keys};
pub use session::{SavedSession, SessionWatch, load_session, login, restore, save_session};
