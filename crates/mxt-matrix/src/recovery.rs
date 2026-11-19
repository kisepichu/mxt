//! Recovery from secret storage, and the device key check the first start
//! needs before it.

use std::time::Duration;

use matrix_sdk::{
    Client,
    encryption::recovery::RecoveryState,
    ruma::{DeviceId, UserId, api::client::keys::get_keys},
};

use crate::Error;

/// How often `wait_for_device_keys` asks the server.
const DEVICE_KEYS_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The account's secret storage as seen by this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStatus {
    /// Secret storage is set up and this device has all its secrets.
    Enabled,
    /// Secret storage is set up, but this device misses some secrets.
    Incomplete,
    /// The account has no secret storage.
    Disabled,
    /// Not known yet.
    Unknown,
}

/// Waits for the E2EE initialization tasks first (the state is `Unknown`
/// before).
pub async fn recovery_status(client: &Client) -> RecoveryStatus {
    let encryption = client.encryption();
    encryption.wait_for_e2ee_initialization_tasks().await;
    match encryption.recovery().state() {
        RecoveryState::Enabled => RecoveryStatus::Enabled,
        RecoveryState::Incomplete => RecoveryStatus::Incomplete,
        RecoveryState::Disabled => RecoveryStatus::Disabled,
        RecoveryState::Unknown => RecoveryStatus::Unknown,
    }
}

/// Imports the secrets from secret storage with the recovery key (or
/// passphrase) and signs this device with them: at startup, and for
/// `MatrixEffect::Recover`. Fails with `Error::WrongRecoveryKey` or
/// `Error::NoSecretStorage` when the user can act on the failure.
pub async fn recover(client: &Client, key: &str) -> Result<(), Error> {
    client.encryption().recovery().recover(key.trim()).await?;
    Ok(())
}

/// Polls a one-off `/keys/query` for the own device until its keys are on the
/// server (`true`) or `timeout` passes (`false`). A failed query is asked
/// again (the SDK logs the error), except `UnknownToken`, which is returned at
/// once. `recover` signs the device, and that upload needs the device keys on
/// the server, so the first start waits for this, with sync running, before
/// asking for the recovery key.
pub async fn wait_for_device_keys(client: &Client, timeout: Duration) -> Result<bool, Error> {
    let (Some(user_id), Some(device_id)) = (client.user_id(), client.device_id()) else {
        return Err(matrix_sdk::Error::AuthenticationRequired.into());
    };
    let poll = async {
        loop {
            match device_keys_on_server(client, user_id, device_id).await {
                Ok(true) => return Ok(true),
                Err(error) if error.is_unknown_token() => return Err(error),
                // Not there yet, or a transient failure such as a network blip.
                Ok(false) | Err(_) => {}
            }
            tokio::time::sleep(DEVICE_KEYS_POLL_INTERVAL).await;
        }
    };
    tokio::time::timeout(timeout, poll)
        .await
        .unwrap_or(Ok(false))
}

/// The response goes only to the caller, never into the SDK's state.
async fn device_keys_on_server(
    client: &Client,
    user_id: &UserId,
    device_id: &DeviceId,
) -> Result<bool, Error> {
    let mut request = get_keys::v3::Request::new();
    request
        .device_keys
        .insert(user_id.to_owned(), vec![device_id.to_owned()]);
    let response = client.send(request).await?;
    Ok(response
        .device_keys
        .get(user_id)
        .is_some_and(|devices| devices.contains_key(device_id)))
}
