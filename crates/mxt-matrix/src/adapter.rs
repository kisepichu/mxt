//! The `Matrix` handle: runs the sync service and the tasks that report to
//! the core, and executes matrix effects.

use std::{sync::Arc, time::Duration};

use matrix_sdk::{
    Client, HttpError, Room,
    latest_events::LatestEventValue,
    room::Receipts,
    ruma::{self, OwnedRoomId},
};
use matrix_sdk_ui::{
    RoomListService, room_list_service,
    sync_service::{self, SyncService},
};
use mxt_core::{
    effect::MatrixEffect,
    ids::RoomId,
    msg::{Msg, SyncState},
    settings::ReceiptKind,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep_until},
};
use tracing::warn;

use crate::{Error, bridges, rooms};

/// Builds the sync service (matrix-sdk-ui `SyncService` with offline mode),
/// so startup can start it before the UI (the first login waits for the
/// device keys while it runs).
pub async fn build_sync_service(client: &Client) -> Result<Arc<SyncService>, Error> {
    let sync = SyncService::builder(client.clone())
        .with_offline_mode()
        .build()
        .await?;
    Ok(Arc::new(sync))
}

/// The running matrix adapter.
pub struct Matrix {
    effects: UnboundedSender<MatrixEffect>,
    tasks: Vec<JoinHandle<()>>,
    sync: Arc<SyncService>,
}

impl Matrix {
    /// Starts the sync service if it is not running, and the tasks that turn
    /// the room list, room info updates, notification settings changes and
    /// bridge state into `Msg::RoomUpserted` / `Msg::RoomRemoved`, and the
    /// sync service state into `Msg::SyncState`.
    pub async fn start(
        client: Client,
        sync: Arc<SyncService>,
        receipts: ReceiptKind,
        to_core: UnboundedSender<Msg>,
    ) -> Result<Matrix, Error> {
        let room_list = sync.room_list_service();
        let all_rooms = room_list.all_rooms().await?;
        let settings = client.notification_settings().await;
        let (to_bridges, listed) = mpsc::unbounded_channel();
        let (bridge_states, from_bridges) = mpsc::unbounded_channel();
        let (effects, queue) = mpsc::unbounded_channel();
        let rooms = rooms::Rooms::new(client.clone(), settings, to_core.clone(), to_bridges);
        let tasks = vec![
            tokio::spawn(watch_sync(sync.clone(), to_core.clone())),
            tokio::spawn(rooms::run(all_rooms, rooms, from_bridges)),
            tokio::spawn(bridges::run(
                client.clone(),
                room_list.clone(),
                listed,
                bridge_states,
            )),
            tokio::spawn(run_effects(client, room_list, receipts, to_core, queue)),
        ];
        if !matches!(
            sync.state().get(),
            sync_service::State::Running | sync_service::State::Offline
        ) {
            sync.start().await;
        }
        Ok(Matrix {
            effects,
            tasks,
            sync,
        })
    }

    /// Runs the effect in the background. Failures arrive as `Msg::EffectFailed`.
    pub fn execute(&self, effect: MatrixEffect) {
        // The queue lives as long as `self`.
        let _ = self.effects.send(effect);
    }

    /// Stops the tasks and the sync service.
    pub async fn stop(self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks {
            let _ = task.await;
        }
        self.sync.stop().await;
    }
}

/// Subscriptions run in order, so the last one wins; receipts run
/// concurrently.
async fn run_effects(
    client: Client,
    room_list: Arc<RoomListService>,
    receipts: ReceiptKind,
    to_core: UnboundedSender<Msg>,
    mut queue: UnboundedReceiver<MatrixEffect>,
) {
    let mut running = JoinSet::new();
    loop {
        tokio::select! {
            effect = queue.recv() => match effect {
                Some(MatrixEffect::SubscribeTimeline(room)) => match sdk_room_id(&room) {
                    Some(room) => room_list.set_room_subscriptions(&[&room]).await,
                    None => report(&to_core, "Opening the room failed", &Error::UnknownRoom),
                },
                Some(MatrixEffect::UnsubscribeTimeline(room)) => {
                    if let Some(room) = sdk_room_id(&room) {
                        room_list.remove_room_subscriptions(&[&room]);
                    }
                }
                Some(MatrixEffect::MarkRoomRead(room)) => {
                    let (client, to_core) = (client.clone(), to_core.clone());
                    running.spawn(async move {
                        if let Err(error) = mark_read(&client, &room, receipts).await {
                            report(&to_core, "Marking the room as read failed", &error);
                        }
                    });
                }
                None => break,
            },
            Some(finished) = running.join_next(), if !running.is_empty() => {
                if let Err(error) = finished {
                    warn!("a matrix effect task ended abnormally: {error}");
                }
            }
        }
    }
}

/// A receipt of the configured kind and the fully read marker for the room's
/// latest event as the room list knows it, which also clears the manual
/// unread mark. Without a remote latest event, only the mark is cleared.
async fn mark_read(client: &Client, room: &RoomId, receipts: ReceiptKind) -> Result<(), Error> {
    let room = sdk_room(client, room)?;
    let latest = match room.latest_event() {
        LatestEventValue::Remote(event) => event.event_id().map(ToOwned::to_owned),
        _ => None,
    };
    let sent = match latest {
        Some(event) => {
            let marker = Receipts::new().fully_read_marker(event.clone());
            let receipts = match receipts {
                ReceiptKind::Public => marker.public_read_receipt(event),
                ReceiptKind::Private => marker.private_read_receipt(event),
            };
            room.send_multiple_receipts(receipts).await
        }
        None => room.set_unread_flag(false).await,
    };
    sent.map_err(|error| without_url(error).into())
}

/// A failed request names its URL, which holds the room ID; the status line
/// and the `warn` log get the error without it.
fn without_url(error: matrix_sdk::Error) -> matrix_sdk::Error {
    match error {
        matrix_sdk::Error::Http(error) => match *error {
            HttpError::Reqwest(error) => HttpError::Reqwest(error.without_url()).into(),
            error => error.into(),
        },
        error => error,
    }
}

fn sdk_room_id(room: &RoomId) -> Option<OwnedRoomId> {
    ruma::RoomId::parse(room.as_str()).ok()
}

fn sdk_room(client: &Client, room: &RoomId) -> Result<Room, Error> {
    sdk_room_id(room)
        .and_then(|room| client.get_room(&room))
        .ok_or(Error::UnknownRoom)
}

/// A short message without event contents for the status line, and the same
/// at `warn` for the log.
fn report(to_core: &UnboundedSender<Msg>, what: &str, error: &Error) {
    warn!("{what}: {error}");
    let _ = to_core.send(Msg::EffectFailed {
        error: format!("{what}: {error}"),
    });
}

/// Reports the sync state, and restarts the sync service with backoff after
/// it stopped with an error. (With offline mode, the SDK itself waits for the
/// homeserver after a failed request and resumes.)
async fn watch_sync(sync: Arc<SyncService>, to_core: UnboundedSender<Msg>) {
    let mut service = sync.state();
    let room_list = sync.room_list_service();
    let mut room_list_state = room_list.state();
    let mut backoff = Backoff::new();
    let mut restart_at = None;
    let mut reported = SyncState::Starting;
    loop {
        let service_state = service.next_now();
        let list_state = room_list_state.next_now();
        let state = sync_state(&service_state, &list_state, reported);
        if state != reported {
            reported = state;
            let _ = to_core.send(Msg::SyncState(state));
        }
        if state == SyncState::Synced {
            backoff.reset();
        }
        match &service_state {
            sync_service::State::Error(_) | sync_service::State::Terminated => {
                if restart_at.is_none() {
                    let delay = backoff.next_delay();
                    match &service_state {
                        sync_service::State::Error(error) => warn!(
                            "the sync service stopped ({error}); restarting it in {} s",
                            delay.as_secs()
                        ),
                        _ => warn!(
                            "the sync service terminated; restarting it in {} s",
                            delay.as_secs()
                        ),
                    }
                    restart_at = Some(Instant::now() + delay);
                }
            }
            _ => restart_at = None,
        }
        tokio::select! {
            changed = service.next() => if changed.is_none() { return },
            changed = room_list_state.next() => if changed.is_none() { return },
            () = sleep_until(restart_at.unwrap_or_else(Instant::now)), if restart_at.is_some() => {
                restart_at = None;
                sync.start().await;
            }
        }
    }
}

/// `Synced` only once the room list has had a response since the sync
/// service (re)started: the service reports `Running` as soon as it starts.
fn sync_state(
    service: &sync_service::State,
    room_list: &room_list_service::State,
    previous: SyncState,
) -> SyncState {
    let responded = matches!(
        room_list,
        room_list_service::State::SettingUp
            | room_list_service::State::Recovering
            | room_list_service::State::Running
    );
    match service {
        sync_service::State::Running if responded => SyncState::Synced,
        sync_service::State::Running => match previous {
            SyncState::Starting => SyncState::Starting,
            SyncState::Synced | SyncState::Offline | SyncState::Retrying => SyncState::Retrying,
        },
        sync_service::State::Offline => SyncState::Offline,
        sync_service::State::Error(_) | sync_service::State::Terminated => SyncState::Retrying,
        // Before the first start, or after `Matrix::stop`.
        sync_service::State::Idle => previous,
    }
}

/// 1 s, doubling, at most 60 s.
struct Backoff {
    next: Duration,
}

impl Backoff {
    const FIRST: Duration = Duration::from_secs(1);
    const MAX: Duration = Duration::from_secs(60);

    fn new() -> Backoff {
        Backoff { next: Self::FIRST }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (delay * 2).min(Self::MAX);
        delay
    }

    fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error() -> sync_service::State {
        sync_service::State::Error(Arc::new(sync_service::Error::Supervisor))
    }

    #[test]
    fn synced_needs_a_room_list_response_after_the_start() {
        use room_list_service::State as List;
        use sync_service::State as Service;
        let starting = SyncState::Starting;
        assert_eq!(
            sync_state(&Service::Idle, &List::Init, starting),
            SyncState::Starting
        );
        assert_eq!(
            sync_state(&Service::Running, &List::Init, starting),
            SyncState::Starting
        );
        assert_eq!(
            sync_state(&Service::Running, &List::SettingUp, starting),
            SyncState::Synced
        );
        assert_eq!(
            sync_state(&Service::Offline, &List::Running, SyncState::Synced),
            SyncState::Offline
        );
        // Back from offline mode, before the next response.
        let failed = List::Error {
            from: Box::new(List::Running),
        };
        assert_eq!(
            sync_state(&Service::Running, &failed, SyncState::Offline),
            SyncState::Retrying
        );
        assert_eq!(
            sync_state(&error(), &failed, SyncState::Synced),
            SyncState::Retrying
        );
        assert_eq!(
            sync_state(&Service::Running, &List::Recovering, SyncState::Retrying),
            SyncState::Synced
        );
    }

    #[test]
    fn backoff_doubles_up_to_a_minute_and_resets() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..8).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }
}
