//! The `Matrix` handle: runs the sync service and the tasks that report to
//! the core, and executes matrix effects.

use std::{sync::Arc, time::Duration};

use matrix_sdk::{
    Client, HttpError, Room,
    latest_events::LatestEventValue,
    room::Receipts,
    ruma::{self, OwnedEventId, OwnedRoomId},
};
use matrix_sdk_ui::{
    RoomListService, room_list_service,
    sync_service::{self, SyncService},
};
use mxt_core::{
    effect::MatrixEffect,
    ids::{EventId, RoomId},
    msg::{Msg, SyncState},
    settings::ReceiptKind,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep_until},
};
use tracing::{debug, warn};

use crate::{Error, backoff::Backoff, bridges, error::brief_sync, rooms};

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
                Some(MatrixEffect::SendReceipt { room, event }) => {
                    let (client, to_core) = (client.clone(), to_core.clone());
                    running.spawn(async move {
                        if let Err(error) = send_receipt(&client, &room, &event, receipts).await {
                            report(&to_core, "Sending the read receipt failed", &error);
                        }
                    });
                }
                // The adapter keeps no timeline yet (phase 2 Task 4 adds it),
                // so there is nothing to paginate. The answer ends the core's
                // request.
                Some(MatrixEffect::PaginateBackwards(room)) => {
                    let _ = to_core.send(Msg::PaginationFailed {
                        room,
                        error: "older messages cannot be loaded yet: mxt keeps no timeline".to_owned(),
                    });
                }
                // Sending, reactions, redactions and members arrive in
                // phase 3 Task 3.
                Some(
                    MatrixEffect::Send { .. }
                    | MatrixEffect::LoadMembers(_)
                    | MatrixEffect::Redact { .. }
                    | MatrixEffect::ToggleReaction { .. }
                    | MatrixEffect::Resend { .. },
                ) => {}
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
            room.send_multiple_receipts(read_up_to(event, receipts))
                .await
        }
        None => room.set_unread_flag(false).await,
    };
    sent.map_err(|error| without_url(error).into())
}

/// A receipt of the configured kind and the fully read marker for `event`.
async fn send_receipt(
    client: &Client,
    room: &RoomId,
    event: &EventId,
    receipts: ReceiptKind,
) -> Result<(), Error> {
    let room = sdk_room(client, room)?;
    let event = ruma::EventId::parse(event.as_str()).map_err(|_| Error::InvalidEventId)?;
    room.send_multiple_receipts(read_up_to(event, receipts))
        .await
        .map_err(|error| without_url(error).into())
}

/// The fully read marker and a receipt of the configured kind, both on `event`.
fn read_up_to(event: OwnedEventId, kind: ReceiptKind) -> Receipts {
    let marker = Receipts::new().fully_read_marker(event.clone());
    match kind {
        ReceiptKind::Public => marker.public_read_receipt(event),
        ReceiptKind::Private => marker.private_read_receipt(event),
    }
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

/// The status line and the `warn` log get the brief error, without event
/// contents or room IDs; `debug` gets the full one.
fn report(to_core: &UnboundedSender<Msg>, what: &str, error: &Error) {
    let brief = error.brief();
    warn!("{what}: {brief}");
    debug!("{what}: {error}");
    let _ = to_core.send(Msg::EffectFailed {
        error: format!("{what}: {brief}"),
    });
}

/// Reports the sync state, and restarts the sync service with backoff after
/// it stopped with an error, or after it went offline again before the room
/// list responded (see `Restarts`).
async fn watch_sync(sync: Arc<SyncService>, to_core: UnboundedSender<Msg>) {
    let mut service = sync.state();
    let room_list = sync.room_list_service();
    let mut room_list_state = room_list.state();
    let mut restarts = Restarts::new();
    let mut restart_at = None;
    let mut reported = SyncState::Starting;
    // The state at the start counts as a change.
    let mut service_changed = true;
    loop {
        let service_state = service.next_now();
        let list_state = room_list_state.next_now();
        let state = sync_state(&service_state, &list_state, reported);
        if state != reported {
            reported = state;
            let _ = to_core.send(Msg::SyncState(state));
        }
        if state == SyncState::Synced {
            restarts.responded();
        }
        if std::mem::take(&mut service_changed) {
            match restarts.service_changed(&service_state) {
                Restart::None => restart_at = None,
                Restart::Keep => {}
                Restart::After(delay) => {
                    match &service_state {
                        sync_service::State::Error(error) => {
                            warn!(
                                "the sync service stopped ({}); restarting it in {} s",
                                brief_sync(error),
                                delay.as_secs()
                            );
                            debug!("the sync service stopped: {error}");
                        }
                        _ => warn!(
                            "the sync service terminated; restarting it in {} s",
                            delay.as_secs()
                        ),
                    }
                    restart_at = Some(Instant::now() + delay);
                }
                Restart::StopThenAfter(delay) => {
                    warn!(
                        "the sync service went offline before the room list responded; \
                         restarting it in {} s",
                        delay.as_secs()
                    );
                    sync.stop().await;
                    restart_at = Some(Instant::now() + delay);
                }
            }
        }
        tokio::select! {
            changed = service.next() => match changed {
                Some(_) => service_changed = true,
                None => return,
            },
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
        // Before the first start, after `Matrix::stop`, or while
        // `watch_sync` waits to restart the service.
        sync_service::State::Idle => previous,
    }
}

/// When `watch_sync` restarts the sync service: after `Error` or
/// `Terminated`, and after `Offline` when the room list has not responded
/// since the service last stopped or went offline.
///
/// With offline mode, the SDK leaves `Offline` on its own once `/versions`
/// answers, and starts the syncs again at once. That recovers from an
/// unreachable homeserver. But a sync that fails while the homeserver answers
/// (an error response, or a response the SDK cannot read) would fail and
/// start again in a tight loop, so mxt stops the service and backs off.
struct Restarts {
    /// 1 s, doubling, at most 60 s; reset by a room list response.
    backoff: Backoff,
    /// Whether the room list has responded since the service last stopped or
    /// went offline.
    responded: bool,
}

/// What `watch_sync` does after the sync service's state changed.
#[derive(Debug, PartialEq, Eq)]
enum Restart {
    /// No restart: the service runs, or the SDK resumes it from offline mode.
    None,
    /// A scheduled restart stays: the service is idle because `watch_sync`
    /// stopped it (or it has not been started yet).
    Keep,
    /// Start the service after the delay.
    After(Duration),
    /// Stop the service (it leaves offline mode), then start it after the
    /// delay.
    StopThenAfter(Duration),
}

impl Restarts {
    fn new() -> Restarts {
        Restarts {
            backoff: Backoff::new(Duration::from_secs(1), Duration::from_secs(60)),
            responded: false,
        }
    }

    /// The room list has responded: the reported state is `Synced`.
    fn responded(&mut self) {
        self.backoff.reset();
        self.responded = true;
    }

    fn service_changed(&mut self, state: &sync_service::State) -> Restart {
        match state {
            sync_service::State::Running => Restart::None,
            sync_service::State::Idle => Restart::Keep,
            sync_service::State::Offline => {
                if std::mem::take(&mut self.responded) {
                    Restart::None
                } else {
                    Restart::StopThenAfter(self.backoff.next_delay())
                }
            }
            sync_service::State::Error(_) | sync_service::State::Terminated => {
                self.responded = false;
                Restart::After(self.backoff.next_delay())
            }
        }
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
    fn offline_mode_resumes_only_after_a_room_list_response() {
        use sync_service::State as Service;
        let mut restarts = Restarts::new();
        let second = Duration::from_secs(1);
        // No response since the start.
        assert_eq!(restarts.service_changed(&Service::Running), Restart::None);
        assert_eq!(
            restarts.service_changed(&Service::Offline),
            Restart::StopThenAfter(second)
        );
        assert_eq!(restarts.service_changed(&Service::Idle), Restart::Keep);
        assert_eq!(restarts.service_changed(&Service::Running), Restart::None);
        restarts.responded();
        // The SDK waits for `/versions` and resumes.
        assert_eq!(restarts.service_changed(&Service::Offline), Restart::None);
        assert_eq!(restarts.service_changed(&Service::Running), Restart::None);
        // That restart got no response: back off, doubling.
        assert_eq!(
            restarts.service_changed(&Service::Offline),
            Restart::StopThenAfter(second)
        );
        assert_eq!(restarts.service_changed(&Service::Running), Restart::None);
        assert_eq!(
            restarts.service_changed(&Service::Offline),
            Restart::StopThenAfter(2 * second)
        );
    }

    #[test]
    fn errors_restart_with_a_backoff_that_a_response_resets() {
        use sync_service::State as Service;
        let mut restarts = Restarts::new();
        let second = Duration::from_secs(1);
        restarts.responded();
        assert_eq!(restarts.service_changed(&error()), Restart::After(second));
        assert_eq!(restarts.service_changed(&Service::Running), Restart::None);
        assert_eq!(
            restarts.service_changed(&Service::Terminated),
            Restart::After(2 * second)
        );
        // The error ended the response from before it.
        assert_eq!(
            restarts.service_changed(&Service::Offline),
            Restart::StopThenAfter(4 * second)
        );
        restarts.responded();
        assert_eq!(restarts.service_changed(&error()), Restart::After(second));
    }
}
