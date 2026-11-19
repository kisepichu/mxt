//! The `Matrix` handle: runs the tasks that report to the core while the
//! sync loop runs, and executes matrix effects.

use std::{fmt::Display, path::PathBuf, sync::Arc};

use matrix_sdk::{
    Client, Room, RoomMemberships,
    latest_events::LatestEventValue,
    notification_settings::{NotificationSettings, RoomNotificationMode},
    room::{Receipts, RoomMember},
    ruma::OwnedEventId,
};
use matrix_sdk_ui::Timeline;
use mxt_core::{
    effect::{MatrixEffect, Outgoing},
    ids::{EventId, RoomId, SendId, UserId},
    mention::Member,
    msg::{Msg, SyncState},
    settings::ReceiptKind,
};
use tokio::{
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        watch,
    },
    task::{JoinError, JoinSet},
};
use tracing::{debug, warn};

use crate::{
    Error, SyncLoop,
    error::without_url,
    media, rooms, send_queue,
    timeline::{Subscription, event_id, redact, resend, send_message, toggle_reaction},
};

/// The running matrix adapter.
pub struct Matrix {
    tasks: JoinSet<()>,
    sync: SyncLoop,
}

impl Matrix {
    /// Starts the tasks that turn the joined rooms, the sync responses, room
    /// info updates and notification settings changes into
    /// `Msg::RoomUpserted` / `Msg::RoomRemoved`, the sync loop's state into
    /// `Msg::SyncState`, and edits, reactions and redactions that fail for
    /// good, and uploads, into `Msg::EffectFailed` (see `send_queue`). The
    /// effects sent to `effects` run in the background; failures arrive as
    /// `Msg::EffectFailed`, and sends are answered with `Msg::SendQueued` or
    /// `Msg::SendFailed`. Downloads are saved in `download_dir`
    /// (`files.download_dir`), and a leading `~/` in an upload's path stands
    /// for `home`.
    pub async fn start(
        client: Client,
        sync: SyncLoop,
        receipts: ReceiptKind,
        download_dir: PathBuf,
        home: PathBuf,
        to_core: UnboundedSender<Msg>,
        effects: UnboundedReceiver<MatrixEffect>,
    ) -> Matrix {
        let settings = client.notification_settings().await;
        let rooms = rooms::Rooms::new(client.clone(), settings.clone(), to_core.clone());
        let send_updates = client.send_queue().subscribe();
        let mut tasks = JoinSet::new();
        tasks.spawn(watch_sync(client.clone(), sync.state(), to_core.clone()));
        tasks.spawn(rooms::run(rooms, sync.rooms()));
        tasks.spawn(send_queue::handle_failures(
            client.clone(),
            send_updates,
            to_core.clone(),
        ));
        let files = Files { download_dir, home };
        let effects = run_effects(client, settings, receipts, files, to_core, effects);
        tasks.spawn(effects);
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

/// Reports the sync loop's state when it changes; ends with the loop. When
/// the sync connects, the first time and after being offline or retrying,
/// every room's send queue is enabled: the requests that failed with a
/// recoverable error meanwhile go out, in every room, and so do those left
/// from the previous session, also in rooms that are not open.
async fn watch_sync(
    client: Client,
    mut state: watch::Receiver<SyncState>,
    to_core: UnboundedSender<Msg>,
) {
    // The core starts with `Starting`.
    let mut reported = SyncState::Starting;
    loop {
        let current = *state.borrow_and_update();
        if current != reported {
            reported = current;
            let _ = to_core.send(Msg::SyncState(current));
            if current == SyncState::Synced {
                client.send_queue().set_enabled(true).await;
            }
        }
        if state.changed().await.is_err() {
            return;
        }
    }
}

/// Where downloads go, and what a leading `~/` in an upload's path stands
/// for.
struct Files {
    download_dir: PathBuf,
    home: PathBuf,
}

/// A request for a room's send queue, see `send_messages`.
enum Queued {
    Message(Arc<Timeline>, SendId, Outgoing),
    Upload(Room, PathBuf),
}

/// Subscriptions run in order, so the last one wins; messages and uploads
/// are queued in the order they were sent, by `send_messages`, and
/// notification modes change in order, by `set_mutes`; the rest runs
/// concurrently.
async fn run_effects(
    client: Client,
    settings: NotificationSettings,
    receipts: ReceiptKind,
    files: Files,
    to_core: UnboundedSender<Msg>,
    mut queue: UnboundedReceiver<MatrixEffect>,
) {
    let mut running = JoinSet::new();
    let (sends, to_send) = mpsc::unbounded_channel();
    running.spawn(send_messages(to_send, to_core.clone()));
    let (mutes, to_mute) = mpsc::unbounded_channel();
    running.spawn(set_mutes(
        client.clone(),
        settings,
        to_mute,
        to_core.clone(),
    ));
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
                    let client = client.clone();
                    let marked = async move { mark_read(&client, &room, receipts).await };
                    running.spawn(reported(&to_core, "Marking the room as read failed", marked));
                }
                Some(MatrixEffect::SendReceipt { room, event }) => {
                    let client = client.clone();
                    let sent = async move { send_receipt(&client, &room, &event, receipts).await };
                    running.spawn(reported(&to_core, "Sending the read receipt failed", sent));
                }
                Some(MatrixEffect::PaginateBackwards(room)) => {
                    match timeline.as_mut().filter(|open| *open.room() == room) {
                        Some(open) => open.paginate_backwards(to_core.clone()),
                        // Opening the room failed; the answer ends the core's
                        // request.
                        None => {
                            let _ = to_core.send(Msg::PaginationFailed {
                                room,
                                error: Error::TimelineNotOpen.to_string(),
                            });
                        }
                    }
                }
                Some(MatrixEffect::Send { room, id, message }) => {
                    match open_timeline(&timeline, &room) {
                        Ok(open) => {
                            let _ = sends.send(Queued::Message(open, id, message));
                        }
                        Err(error) => report_send(&to_core, id, &error),
                    }
                }
                Some(MatrixEffect::ToggleReaction { room, item, key }) => {
                    let open = open_timeline(&timeline, &room);
                    let toggled = async move { toggle_reaction(&*open?, &item, &key).await };
                    running.spawn(reported(&to_core, "Reacting failed", toggled));
                }
                Some(MatrixEffect::Redact { room, item }) => {
                    let open = open_timeline(&timeline, &room);
                    let redacted = async move { redact(&*open?, &item).await };
                    running.spawn(reported(&to_core, "Deleting the message failed", redacted));
                }
                Some(MatrixEffect::Resend { room, txn }) => {
                    let open = open_timeline(&timeline, &room);
                    let resent = async move { resend(&*open?, &txn).await };
                    running.spawn(reported(&to_core, "Resending the message failed", resent));
                }
                Some(MatrixEffect::LoadMembers(room)) => {
                    running.spawn(load_members(client.clone(), room, to_core.clone()));
                }
                Some(MatrixEffect::SetMute { room, muted }) => {
                    let _ = mutes.send((room, muted));
                }
                Some(MatrixEffect::Download { room, item }) => {
                    let (client, dir) = (client.clone(), files.download_dir.clone());
                    let open = open_timeline(&timeline, &room);
                    let downloaded =
                        async move { media::download(&client, &*open?, &item, dir).await };
                    running.spawn(answered(&to_core, "Downloading the file failed", downloaded));
                }
                Some(MatrixEffect::Upload { room, path }) => match sdk_room(&client, &room) {
                    Ok(room) => {
                        let path = media::expand_home(&path, &files.home);
                        let _ = sends.send(Queued::Upload(room, path));
                    }
                    Err(error) => report(&to_core, "Uploading the file failed", &error),
                },
                Some(MatrixEffect::FetchImage { room, item }) => {
                    let client = client.clone();
                    let open = open_timeline(&timeline, &room);
                    let fetched = async move { media::fetch_image(&client, &*open?, item).await };
                    running.spawn(answered(&to_core, "Loading the image failed", fetched));
                }
                None => break,
            },
            Some(finished) = running.join_next(), if !running.is_empty() => pass_on(finished),
            Some(finished) = timeline_task(&mut timeline) => pass_on(finished),
        }
    }
}

/// `effect`, with its failure reported as `what`.
fn reported<F>(
    to_core: &UnboundedSender<Msg>,
    what: &'static str,
    effect: F,
) -> impl Future<Output = ()> + use<F>
where
    F: Future<Output = Result<(), Error>>,
{
    let to_core = to_core.clone();
    async move {
        if let Err(error) = effect.await {
            report(&to_core, what, &error);
        }
    }
}

/// `effect`, with its answer sent to the core, or its failure reported as
/// `what`.
fn answered<F>(
    to_core: &UnboundedSender<Msg>,
    what: &'static str,
    effect: F,
) -> impl Future<Output = ()> + use<F>
where
    F: Future<Output = Result<Msg, Error>>,
{
    let to_core = to_core.clone();
    async move {
        match effect.await {
            Ok(answer) => {
                let _ = to_core.send(answer);
            }
            Err(error) => report(&to_core, what, &error),
        }
    }
}

/// Queues the messages and uploads one at a time, in the order they were
/// sent; a message is answered with `Msg::SendQueued` or `Msg::SendFailed`,
/// an upload that cannot be queued is reported. A reply or an edit of a
/// sent message fetches its target event first when the event cache lacks
/// it, which can take up to three attempts of the client's request timeout,
/// and an upload reads its file (and fetches the homeserver's upload limit
/// once); a later message must not overtake either, so it waits. Meanwhile
/// the other effects (switching rooms, pagination, receipts) do not wait.
async fn send_messages(mut requests: UnboundedReceiver<Queued>, to_core: UnboundedSender<Msg>) {
    while let Some(request) = requests.recv().await {
        match request {
            Queued::Message(timeline, id, message) => {
                match send_message(&timeline, message).await {
                    Ok(()) => {
                        let _ = to_core.send(Msg::SendQueued { id });
                    }
                    Err(error) => report_send(&to_core, id, &error),
                }
            }
            Queued::Upload(room, path) => {
                if let Err(error) = media::upload(&room, path).await {
                    report(&to_core, "Uploading the file failed", &error);
                }
            }
        }
    }
}

/// Changes notification modes one at a time, in the order they were asked
/// for: each change reads the push rules the previous one left, so muting
/// and then unmuting a room ends with the room unmuted. The room's mode in
/// the room list changes when the push rules arrive with the sync.
async fn set_mutes(
    client: Client,
    settings: NotificationSettings,
    mut changes: UnboundedReceiver<(RoomId, bool)>,
    to_core: UnboundedSender<Msg>,
) {
    while let Some((room, muted)) = changes.recv().await {
        if let Err(error) = set_mute(&client, &settings, &room, muted).await {
            let what = if muted {
                "Muting the room failed"
            } else {
                "Unmuting the room failed"
            };
            report(&to_core, what, &error);
        }
    }
}

/// Muting sets the room's mode to mute; unmuting removes the room's
/// user-defined rules, so the account default applies again (design,
/// "Mute").
async fn set_mute(
    client: &Client,
    settings: &NotificationSettings,
    room: &RoomId,
    muted: bool,
) -> Result<(), Error> {
    let room = sdk_room(client, room)?;
    let changed = if muted {
        settings
            .set_room_notification_mode(room.room_id(), RoomNotificationMode::Mute)
            .await
    } else {
        settings
            .delete_user_defined_room_rules(room.room_id())
            .await
    };
    changed.map_err(Error::NotificationSettings)
}

/// `room`'s timeline if it is the open one: sending, reactions and
/// redactions act on its items.
fn open_timeline(timeline: &Option<Subscription>, room: &RoomId) -> Result<Arc<Timeline>, Error> {
    timeline
        .as_ref()
        .filter(|open| open.room() == room)
        .map(Subscription::timeline)
        .ok_or(Error::TimelineNotOpen)
}

/// Answers with `Msg::MembersLoaded` at least once, so mention completion
/// does not wait for an answer that never comes. With lazy-loaded members,
/// the ones the store has (the senders of loaded events, for example) go
/// first, so completion works while the full list loads, which takes a
/// while in large bridged rooms; then the full list. This is the one full
/// load above `MEMBERS_FETCH_LIMIT` too: completion needs it, and the core
/// asks once per room. When loading fails, what loaded before stays, or the
/// answer is empty.
async fn load_members(client: Client, room: RoomId, to_core: UnboundedSender<Msg>) {
    let answer = |members: Vec<RoomMember>| {
        let members = members.iter().map(member).collect();
        let _ = to_core.send(Msg::MembersLoaded {
            room: room.clone(),
            members,
        });
    };
    let mut answered = false;
    let loaded = async {
        let sdk_room = sdk_room(&client, &room)?;
        let sdk_error = |error| Error::from(without_url(error));
        if !sdk_room.are_members_synced() {
            let stored = sdk_room.members_no_sync(RoomMemberships::JOIN).await;
            answer(stored.map_err(sdk_error)?);
            answered = true;
        }
        let all = sdk_room.members(RoomMemberships::JOIN).await;
        all.map_err(sdk_error)
    }
    .await;
    match loaded {
        Ok(members) => answer(members),
        Err(error) => {
            report(&to_core, "Loading the room's members failed", &error);
            if !answered {
                answer(Vec::new());
            }
        }
    }
}

fn member(member: &RoomMember) -> Member {
    Member {
        user: UserId::new(member.user_id().as_str()),
        display_name: member.display_name().map(ToOwned::to_owned),
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
    let event = event_id(event)?;
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
    report_brief(to_core, what, &error.brief(), error);
}

/// `report` for a message that could not be queued, as `Msg::SendFailed`,
/// so the core can put its draft back.
fn report_send(to_core: &UnboundedSender<Msg>, id: SendId, error: &Error) {
    let error = logged("Sending the message failed", &error.brief(), error);
    let _ = to_core.send(Msg::SendFailed { id, error });
}

/// `report`, for an error that is not an `Error`: `brief` names no room and
/// holds no event contents.
pub(crate) fn report_brief(
    to_core: &UnboundedSender<Msg>,
    what: &str,
    brief: &dyn Display,
    full: &dyn Display,
) {
    let error = logged(what, brief, full);
    let _ = to_core.send(Msg::EffectFailed { error });
}

/// Logs a failure, `brief` to `warn` and `full` to `debug`, and returns the
/// status line's text.
fn logged(what: &str, brief: &dyn Display, full: &dyn Display) -> String {
    warn!("{what}: {brief}");
    debug!("{what}: {full}");
    format!("{what}: {brief}")
}
