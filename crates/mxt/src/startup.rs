//! The startup flow (design, "Startup"): the session, restored or from a
//! new login, the recovery after a first login, then the matrix adapter and
//! the terminal UI until `:q`. `main` prints the outcome, and sets the data
//! aside when the session is unusable, after the runtime has stopped and the
//! stores are closed.

use std::error::Error;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use mxt_core::app::App;
use mxt_core::ids::UserId;
use mxt_core::keymap::Keymap;
use mxt_core::settings::Settings;
use mxt_matrix::{Client, Homeserver, Matrix, RecoveryStatus, SessionWatch, SyncService};
use mxt_tui::Tui;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::event_loop;
use crate::invalidate::{set_aside_orphaned_store, unix_seconds};
use crate::paths::{Paths, with_path};
use crate::prompt::prompt_secret;

/// How long a first login waits for its device keys to reach the server
/// before it asks for the recovery key anyway.
const DEVICE_KEYS_TIMEOUT: Duration = Duration::from_secs(90);

/// How a run ended.
#[derive(Debug)]
pub enum Outcome {
    /// `:q`, or the terminal input ended.
    Quit,
    /// The session cannot be used any more; `main` moves the data aside.
    Invalid(Invalid),
    /// Printed as `mxt: <message>`.
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Invalid {
    /// The homeserver answered `M_UNKNOWN_TOKEN`.
    UnknownToken,
    /// The session file exists but the store does not: a new crypto store
    /// would create a new Olm account for an existing device.
    MissingStore,
}

/// Runs mxt from the session to the end of the terminal UI. The caller holds
/// the data directory lock and drops the runtime afterwards.
pub async fn run(config: Config, paths: &Paths) -> Outcome {
    let Config {
        user_id,
        homeserver,
        keymap,
        settings,
        ..
    } = config;
    match connect(&user_id, homeserver.as_deref(), paths).await {
        Ok((client, sync, mut session)) => {
            run_ui(client, sync, &mut session, keymap, settings).await
        }
        Err(outcome) => outcome,
    }
}

/// The client with a restored or new session, and the sync service, which
/// runs already after a first login. The session watch is created before
/// any request with the access token.
async fn connect(
    user_id: &UserId,
    configured: Option<&str>,
    paths: &Paths,
) -> Result<(Client, Arc<SyncService>, SessionWatch), Outcome> {
    let saved = mxt_matrix::load_session(&paths.session_file())
        .map_err(|error| failure("reading the session", &error))?;
    if saved.is_some() {
        let store = paths.store_dir();
        let exists = store
            .try_exists()
            .map_err(|error| Outcome::Failed(with_path(error, &store).to_string()))?;
        if !exists {
            return Err(Outcome::Invalid(Invalid::MissingStore));
        }
    } else if let Some(moved) = set_aside_orphaned_store(paths, unix_seconds())
        .map_err(|error| Outcome::Failed(format!("setting the old store aside: {error}")))?
    {
        warn!(
            "no session file: moved the store of an earlier session to {}",
            moved.display()
        );
    }

    let server = homeserver(
        saved.as_ref().map(|saved| saved.homeserver.as_str()),
        configured,
        user_id,
    );
    let client = mxt_matrix::build_client(&server, &paths.store_dirs())
        .await
        .map_err(|error| failure("starting the client", &error))?;
    let session = SessionWatch::new(&client);
    let first_login = match saved {
        Some(saved) => {
            mxt_matrix::restore(&client, saved)
                .await
                .map_err(|error| failure("restoring the session", &error))?;
            info!("restored the session");
            false
        }
        None => {
            log_in(&client, user_id, paths).await?;
            true
        }
    };
    let sync = mxt_matrix::build_sync_service(&client)
        .await
        .map_err(|error| failure("starting the sync", &error))?;
    if first_login {
        recover(&client, &sync).await?;
    }
    Ok((client, sync, session))
}

/// Asks for the password, logs in as a new device and saves the session at
/// once, before anything else can fail.
async fn log_in(client: &Client, user_id: &UserId, paths: &Paths) -> Result<(), Outcome> {
    println!("Logging in as {user_id} on {}", client.homeserver());
    let password = ask_secret("Password: ").await?;
    let saved = mxt_matrix::login(client, user_id.as_str(), &password)
        .await
        .map_err(|error| failure("login failed", &error))?;
    drop(password);
    mxt_matrix::save_session(&paths.session_file(), &saved)
        .map_err(|error| failure("logged in, but saving the session failed", &error))?;
    info!("logged in as a new device");
    Ok(())
}

/// Right after a first login: starts the sync, waits for the device keys to
/// reach the server (recovery signs the device, which needs them), then asks
/// for the recovery key. Only `M_UNKNOWN_TOKEN` stops the startup here.
async fn recover(client: &Client, sync: &SyncService) -> Result<(), Outcome> {
    sync.start().await;
    println!("Waiting for this device's keys to reach the server...");
    match mxt_matrix::wait_for_device_keys(client, DEVICE_KEYS_TIMEOUT).await {
        Ok(true) => {}
        Ok(false) => {
            warn!("the device keys did not reach the server in time");
            println!(
                "They did not arrive within {} s; recovery may fail.",
                DEVICE_KEYS_TIMEOUT.as_secs()
            );
        }
        Err(error) if error.is_unknown_token() => {
            return Err(Outcome::Invalid(Invalid::UnknownToken));
        }
        Err(error) => {
            warn!("checking the device keys failed: {error}");
            println!(
                "Checking them failed ({}); recovery may fail.",
                chain(&error)
            );
        }
    }

    if mxt_matrix::recovery_status(client).await == RecoveryStatus::Disabled {
        println!(
            "The account has no secret storage, so there is nothing to recover. \
             Set up recovery in another client; mxt continues without it."
        );
        return Ok(());
    }
    let key = ask_secret("Recovery key (empty to skip): ").await?;
    if key.trim().is_empty() {
        println!("Skipped; :recover asks again later.");
        return Ok(());
    }
    match mxt_matrix::recover(client, &key).await {
        Ok(()) => println!("Recovery complete"),
        Err(error) if error.is_unknown_token() => {
            return Err(Outcome::Invalid(Invalid::UnknownToken));
        }
        Err(error) => {
            warn!("recovery failed: {error}");
            println!("Recovery failed: {}", chain(&error));
        }
    }
    Ok(())
}

/// The matrix adapter and the terminal UI until `:q`, an `M_UNKNOWN_TOKEN`
/// or a panic in one of the adapter's tasks. The terminal is restored and
/// the adapter stopped before this returns.
async fn run_ui(
    client: Client,
    sync: Arc<SyncService>,
    session: &mut SessionWatch,
    keymap: Keymap,
    settings: Settings,
) -> Outcome {
    let receipts = settings.read_receipts;
    let mut app = App::new(keymap, settings);
    let (to_core, from_matrix) = mpsc::unbounded_channel();
    let (to_matrix, effects) = mpsc::unbounded_channel();
    // Before the UI, so its failure is printed like the others.
    let mut matrix = match Matrix::start(client, sync, receipts, to_core, effects).await {
        Ok(matrix) => matrix,
        Err(error) => return failure("starting the matrix adapter", &error),
    };
    mxt_tui::install_panic_hook();
    let mut tui = match Tui::new() {
        Ok(tui) => tui,
        Err(error) => {
            matrix.stop().await;
            return Outcome::Failed(format!("cannot set up the terminal: {error}"));
        }
    };
    let outcome = tokio::select! {
        ended = event_loop::run(&mut app, &mut tui, from_matrix, to_matrix) => match ended {
            Ok(()) => Outcome::Quit,
            Err(error) => Outcome::Failed(format!("terminal: {error}")),
        },
        () = session.unknown_token() => Outcome::Invalid(Invalid::UnknownToken),
        // The panic hook has restored the terminal and printed the panic.
        panic = matrix.panicked() => {
            error!("a task of the matrix adapter panicked");
            Outcome::Failed(format!("the matrix adapter stopped: {panic}"))
        }
    };
    let restored = tui.restore();
    matrix.stop().await;
    match (outcome, restored) {
        (Outcome::Quit, Err(error)) => Outcome::Failed(format!("terminal: {error}")),
        (outcome, _) => outcome,
    }
}

/// `prompt_secret` on a blocking thread: the sync may be running.
async fn ask_secret(prompt: &'static str) -> Result<String, Outcome> {
    let read = match tokio::task::spawn_blocking(move || prompt_secret(prompt)).await {
        Ok(read) => read,
        Err(error) => Err(io::Error::from(error)),
    };
    read.map_err(|error| Outcome::Failed(format!("reading from the terminal: {error}")))
}

/// `M_UNKNOWN_TOKEN` makes the session invalid; any other error is printed
/// after `context`.
fn failure(context: &str, error: &mxt_matrix::Error) -> Outcome {
    if error.is_unknown_token() {
        Outcome::Invalid(Invalid::UnknownToken)
    } else {
        Outcome::Failed(format!("{context}: {}", chain(error)))
    }
}

/// The error and its sources, separated by `: `.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// The server name of `@localpart:server`: everything after the first colon,
/// port included (a localpart has no colon). The configuration loader has
/// checked that there is one.
fn server_name(user_id: &UserId) -> &str {
    user_id
        .as_str()
        .split_once(':')
        .map_or("", |(_, server)| server)
}

/// The saved session's homeserver, which needs no request; otherwise the
/// configured one; otherwise discovery from the user ID's server name.
fn homeserver(saved: Option<&str>, configured: Option<&str>, user_id: &UserId) -> Homeserver {
    match (saved, configured) {
        (Some(saved), _) => Homeserver::Saved(saved.to_owned()),
        (None, Some(url)) => Homeserver::Url(url.to_owned()),
        (None, None) => Homeserver::Discover(server_name(user_id).to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_name_follows_the_first_colon() {
        let id = |id: &str| UserId::new(id);
        assert_eq!(server_name(&id("@alice:example.org")), "example.org");
        assert_eq!(
            server_name(&id("@alice:example.org:8448")),
            "example.org:8448"
        );
        assert_eq!(server_name(&id("@alice:[::1]:8448")), "[::1]:8448");
    }

    #[test]
    fn the_saved_homeserver_wins_then_the_configured_one() {
        let alice = UserId::new("@alice:example.org");
        let saved = Some("https://matrix.example.org/");
        let configured = Some("https://other.example.org");
        assert_eq!(
            homeserver(saved, configured, &alice),
            Homeserver::Saved("https://matrix.example.org/".to_owned())
        );
        assert_eq!(
            homeserver(None, configured, &alice),
            Homeserver::Url("https://other.example.org".to_owned())
        );
        assert_eq!(
            homeserver(None, None, &alice),
            Homeserver::Discover("example.org".to_owned())
        );
    }
}
