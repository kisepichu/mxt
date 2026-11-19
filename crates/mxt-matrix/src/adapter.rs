//! The `Matrix` handle: runs the tasks that report to the core while the
//! sync loop runs, and executes matrix effects.

use matrix_sdk::{
    Client, Room,
    latest_events::LatestEventValue,
    room::Receipts,
    ruma::{self, OwnedEventId},
};
use mxt_core::{
    effect::MatrixEffect,
    ids::{EventId, RoomId},
    msg::{Msg, SyncState},
    settings::ReceiptKind,
};
use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        watch,
    },
    task::{JoinError, JoinSet},
};
use tracing::{debug, warn};

use crate::{Error, SyncLoop, error::without_url, rooms, timeline::Subscription};

/// The running matrix adapter.
pub struct Matrix {
    tasks: JoinSet<()>,
    sync: SyncLoop,
}

impl Matrix {
    /// Starts the tasks that turn the joined rooms, the sync responses, room
    /// info updates and notification settings changes into
    /// `Msg::RoomUpserted` / `Msg::RoomRemoved`, and the sync loop's state
    /// into `Msg::SyncState`. The effects sent to `effects` run in the
    /// background; failures arrive as `Msg::EffectFailed`.
    pub async fn start(
        client: Client,
        sync: SyncLoop,
        receipts: ReceiptKind,
        to_core: UnboundedSender<Msg>,
        effects: UnboundedReceiver<MatrixEffect>,
    ) -> Matrix {
        let settings = client.notification_settings().await;
        let rooms = rooms::Rooms::new(client.clone(), settings, to_core.clone());
        let mut tasks = JoinSet::new();
        tasks.spawn(watch_sync(sync.state(), to_core.clone()));
        tasks.spawn(rooms::run(rooms, sync.rooms()));
        tasks.spawn(run_effects(client, receipts, to_core, effects));
        Matrix { tasks, sync }
    }

    /// Waits until one of the tasks or the sync loop panics and returns its
    /// error; pending while they run and once they have all ended otherwise.
    /// The panic hook has restored the terminal by then, so the caller ends
    /// the program. Cancel safe.
    pub async fn panicked(&mut self) -> JoinError {
        tokio::select! {
            error = first_panic(&mut self.tasks) => error,
            error = self.sync.panicked() => error,
        }
    }

    /// Stops the tasks and the sync loop.
    pub async fn stop(mut self) {
        self.tasks.shutdown().await;
        self.sync.stop().await;
    }
}

/// The error of the first task in `tasks` that panics; pending while they
/// run and once they have all ended otherwise. Cancel safe.
pub(crate) async fn first_panic(tasks: &mut JoinSet<()>) -> JoinError {
    while let Some(finished) = tasks.join_next().await {
        if let Err(error) = finished
            && error.is_panic()
        {
            return error;
        }
    }
    std::future::pending().await
}

/// Reports the sync loop's state when it changes; ends with the loop.
async fn watch_sync(mut state: watch::Receiver<SyncState>, to_core: UnboundedSender<Msg>) {
    // The core starts with `Starting`.
    let mut reported = SyncState::Starting;
    loop {
        let current = *state.borrow_and_update();
        if current != reported {
            reported = current;
            let _ = to_core.send(Msg::SyncState(current));
        }
        if state.changed().await.is_err() {
            return;
        }
    }
}

/// Subscriptions run in order, so the last one wins; receipts and back
/// pagination run concurrently.
async fn run_effects(
    client: Client,
    receipts: ReceiptKind,
    to_core: UnboundedSender<Msg>,
    mut queue: UnboundedReceiver<MatrixEffect>,
) {
    let mut running = JoinSet::new();
    // The open room's timeline; at most one.
    let mut timeline: Option<Subscription> = None;
    loop {
        tokio::select! {
            effect = queue.recv() => match effect {
                Some(MatrixEffect::SubscribeTimeline(room)) => {
                    if let Some(previous) = timeline.take() {
                        previous.stop().await;
                    }
                    timeline = subscribe(&client, room, &to_core).await;
                }
                Some(MatrixEffect::UnsubscribeTimeline(room)) => {
                    if let Some(open) = timeline.take_if(|open| *open.room() == room) {
                        open.stop().await;
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
                Some(MatrixEffect::PaginateBackwards(room)) => {
                    match timeline.as_mut().filter(|open| *open.room() == room) {
                        Some(open) => open.paginate_backwards(to_core.clone()),
                        // Opening the room failed; the answer ends the core's
                        // request.
                        None => {
                            let _ = to_core.send(Msg::PaginationFailed {
                                room,
                                error: "the room's timeline is not open".to_owned(),
                            });
                        }
                    }
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
            Some(finished) = running.join_next(), if !running.is_empty() => pass_on(finished),
            Some(finished) = timeline_task(&mut timeline) => pass_on(finished),
        }
    }
}

/// The room's timeline; classic sync delivers every room's events already.
async fn subscribe(
    client: &Client,
    room: RoomId,
    to_core: &UnboundedSender<Msg>,
) -> Option<Subscription> {
    let started = match sdk_room(client, &room) {
        Ok(sdk_room) => Subscription::start(&sdk_room, room, to_core.clone()).await,
        Err(error) => Err(error),
    };
    started
        .inspect_err(|error| report(to_core, "Opening the room failed", error))
        .ok()
}

/// The next task of the open timeline that ended; `None` without a timeline
/// or without tasks, which disables the `select!` branch until the next turn.
async fn timeline_task(timeline: &mut Option<Subscription>) -> Option<Result<(), JoinError>> {
    timeline.as_mut()?.join_next().await
}

/// A panic is passed on, so `Matrix::panicked` sees it.
fn pass_on(finished: Result<(), JoinError>) {
    match finished {
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => warn!("a matrix effect task ended abnormally: {error}"),
        Ok(()) => {}
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

fn sdk_room(client: &Client, room: &RoomId) -> Result<Room, Error> {
    ruma::RoomId::parse(room.as_str())
        .ok()
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
