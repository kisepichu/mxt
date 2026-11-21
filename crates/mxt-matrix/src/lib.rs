//! The matrix-rust-sdk adapter of mxt: builds the SDK client, runs the sync,
//! converts SDK data into `mxt-core` domain types and executes matrix
//! effects.

// The future of matrix-sdk-ui's `Room::timeline()` is deeper than the default
// limit allows; matrix-sdk and matrix-sdk-ui raise their own limit to the same
// value.
#![recursion_limit = "256"]

mod adapter;
mod backoff;
mod bridges;
mod client;
mod error;
mod html;
mod join;
mod media;
mod recovery;
mod rooms;
mod send_queue;
mod session;
mod sync;
#[cfg(test)]
mod test_dir;
mod timeline;

pub use adapter::Matrix;
pub use client::{Homeserver, StoreDirs, build_client};
pub use error::Error;
pub use matrix_sdk::Client;
pub use recovery::{RecoveryStatus, recover, recovery_status, wait_for_device_keys};
pub use session::{SavedSession, SessionWatch, load_session, login, restore, save_session};
pub use sync::SyncLoop;
