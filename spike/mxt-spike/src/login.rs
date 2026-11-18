//! `login <user_id>`: password login, session file, recovery and the device's trust state.

use std::{
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use matrix_sdk::{
    Client,
    encryption::recovery::RecoveryState,
    ruma::{
        UserId,
        api::client::{keys::get_keys, session::get_login_types::v3::LoginType},
    },
};

use crate::session::{self, DataDir, Homeserver, StoredSession};

pub async fn run(dir: &DataDir, user_id: &UserId) -> Result<ExitCode> {
    let session_file = dir.session_file();
    if session_file.exists() {
        bail!(
            "{} exists. Every login creates a new device, so login refuses to run while a \
             session exists; the other commands use it",
            session_file.display()
        );
    }
    let store = dir.store_dir();
    if store.exists() {
        bail!(
            "{} exists without a session file. It belongs to a device whose session is gone; \
             move it away before logging in",
            store.display()
        );
    }
    dir.ensure()?;
    let _lock = dir.lock()?;

    let client =
        session::build_client(dir, Homeserver::ServerName(user_id.server_name().as_str())).await?;
    println!("homeserver: {}", client.homeserver());
    println!("sliding sync: {:?}", client.sliding_sync_version());

    let login_types = client.matrix_auth().get_login_types().await?;
    if !login_types
        .flows
        .iter()
        .any(|flow| matches!(flow, LoginType::Password(_)))
    {
        let offered: Vec<&str> = login_types
            .flows
            .iter()
            .map(LoginType::login_type)
            .collect();
        eprintln!(
            "the homeserver does not offer m.login.password; it offers: {}",
            offered.join(", ")
        );
        return Ok(ExitCode::FAILURE);
    }

    let password = rpassword::prompt_password(format!("Password for {user_id}: "))?;
    let response = client
        .matrix_auth()
        .login_username(user_id, &password)
        .initial_device_display_name("mxt-spike")
        .send()
        .await
        .context("logging in")?;
    drop(password);

    let stored = StoredSession {
        homeserver: client.homeserver().to_string(),
        session: client
            .matrix_auth()
            .session()
            .context("no session after login")?,
    };
    session::write_new_private(&session_file, &serde_json::to_vec_pretty(&stored)?)?;
    println!(
        "logged in as {} with the new device {}",
        response.user_id, response.device_id
    );
    println!("session saved to {}", session_file.display());

    // Sync first: signing this device during recovery needs its keys on the server.
    let sync = session::start_sync(&client).await?;
    let start = Instant::now();
    let mut keys_uploaded = false;
    while start.elapsed() < Duration::from_secs(90) {
        session::check_sync(&sync)?;
        if device_keys_on_server(&client).await? {
            keys_uploaded = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    println!(
        "device keys on the server: {} (after {:.1}s)",
        yes_no(keys_uploaded),
        start.elapsed().as_secs_f64()
    );

    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    let recovery = client.encryption().recovery();
    if recovery.state() == RecoveryState::Disabled {
        println!(
            "The account has no secret storage. Recovery has to be set up in another client \
             (for example Element); this device stays unverified until then."
        );
    } else {
        let key = rpassword::prompt_password("Recovery key or passphrase (empty to skip): ")?;
        if key.trim().is_empty() {
            println!("recovery skipped");
        } else {
            match recovery.recover(key.trim()).await {
                Ok(()) => println!("recovery: secrets imported from secret storage"),
                Err(error) => println!("recovery failed: {error}"),
            }
        }
    }

    print_trust(&client).await?;
    sync.stop().await;
    Ok(ExitCode::SUCCESS)
}

/// Asks the server (one-off `/keys/query`, not fed to the SDK) whether this
/// device's keys are uploaded.
async fn device_keys_on_server(client: &Client) -> Result<bool> {
    let user_id = client.user_id().context("not logged in")?.to_owned();
    let device_id = client.device_id().context("not logged in")?.to_owned();
    let mut request = get_keys::v3::Request::new();
    request
        .device_keys
        .insert(user_id.clone(), vec![device_id.clone()]);
    let response = client.send(request).await?;
    Ok(response
        .device_keys
        .get(&user_id)
        .is_some_and(|devices| devices.contains_key(&device_id)))
}

async fn print_trust(client: &Client) -> Result<()> {
    let encryption = client.encryption();
    println!("recovery state: {:?}", encryption.recovery().state());
    match encryption.cross_signing_status().await {
        Some(status) => println!(
            "cross-signing keys available: master {}, self-signing {}, user-signing {}",
            yes_no(status.has_master),
            yes_no(status.has_self_signing),
            yes_no(status.has_user_signing)
        ),
        None => println!("cross-signing keys available: unknown (no crypto machine)"),
    }
    let cross_signed = encryption
        .get_own_device()
        .await?
        .map(|device| device.is_cross_signed_by_owner());
    println!(
        "this device cross-signed by its owner: {}",
        cross_signed.map_or("unknown", yes_no)
    );
    println!(
        "key backup enabled on this device: {}",
        yes_no(encryption.backups().are_enabled().await)
    );
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
