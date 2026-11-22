//! The `Matrix` handle: runs the tasks that report to the core while the
//! sync loop runs, and executes matrix effects.

use std::{fmt::Display, path::PathBuf, sync::Arc};

use matrix_sdk::{
    Client, Room, RoomMemberships,
    event_cache::EventCacheError,
    latest_events::LatestEventValue,
    notification_settings::{NotificationSettings, RoomNotificationMode},
    room::{Receipts, RoomMember},
    ruma::{OwnedEventId, api::client::receipt::create_receipt, events::receipt::ReceiptThread},
    serde_helpers::extract_thread_root,
};
use matrix_sdk_ui::Timeline;
use mxt_core::{
    effect::{MatrixEffect, Outgoing},
    ids::{EventId, RoomId, SendId, TimelineId, UserId},
    mention::Member,
    msg::{Msg, SyncState},
    settings::ReceiptKind,
    timeline::ItemRef,
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
    join::{self, DirectMessages},
    media::{self, ThreadUpload},
    recovery, rooms,
    send_queue::{self, Edits},
    timeline::{
        Answer, Subscription, event_id, holds, redact, resend, send_message, toggle_reaction,
    },
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
    /// `Msg::SendFailed`, an edit of a sent message only when it leaves the
    /// send queue. Downloads are saved in `download_dir`
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
        let files = Files { download_dir, home };
        let mut tasks = JoinSet::new();
        // First, so the send queue's updates are watched before `watch_sync`
        // enables the queues.
        spawn_send_paths(
            &mut tasks,
            client.clone(),
            settings,
            receipts,
            files,
            to_core.clone(),
            effects,
        );
        tasks.spawn(watch_sync(client, sync.state(), to_core.clone()));
        tasks.spawn(rooms::run(rooms, sync.rooms()));
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

/// Spawns in `tasks` the effects (`run_effects`) and the task that answers
/// and reports the requests leaving the send queue
/// (`send_queue::handle_failures`); the latter's subscription to the send
/// queue's updates is made before any effect runs, so it sees every
/// request's outcome.
fn spawn_send_paths(
    tasks: &mut JoinSet<()>,
    client: Client,
    settings: NotificationSettings,
    receipts: ReceiptKind,
    files: Files,
    to_core: UnboundedSender<Msg>,
    effects: UnboundedReceiver<MatrixEffect>,
) {
    let edits = Edits::default();
    tasks.spawn(send_queue::handle_failures(
        client.clone(),
        edits.clone(),
        client.send_queue().subscribe(),
        to_core.clone(),
    ));
    let effects = run_effects(client, settings, receipts, files, edits, to_core, effects);
    tasks.spawn(effects);
}

/// A request for a room's send queue, see `send_messages`.
enum Queued {
    Message(Arc<Timeline>, SendId, Outgoing),
    /// An upload, into the thread when it names one.
    Upload(Room, PathBuf, Option<ThreadUpload>),
}

/// Subscriptions run in order, so the last one wins; messages and uploads
/// are queued in the order they were sent, by `send_messages`, and
/// notification modes change in order, by `set_mutes`; direct message
/// lookups and creations run one at a time (see `DirectMessages`); the rest
/// runs concurrently.
async fn run_effects(
    client: Client,
    settings: NotificationSettings,
    receipts: ReceiptKind,
    files: Files,
    edits: Edits,
    to_core: UnboundedSender<Msg>,
    mut queue: UnboundedReceiver<MatrixEffect>,
) {
    let mut running = JoinSet::new();
    let (sends, to_send) = mpsc::unbounded_channel();
    running.spawn(send_messages(to_send, edits, to_core.clone()));
    let (mutes, to_mute) = mpsc::unbounded_channel();
    running.spawn(set_mutes(
        client.clone(),
        settings,
        to_mute,
        to_core.clone(),
    ));
    // The open room's main timeline, and the thread shown in its place; at
    // most one of each.
    let mut main: Option<Subscription> = None;
    let mut thread: Option<Subscription> = None;
    let dms = DirectMessages::default();
    loop {
        tokio::select! {
            effect = queue.recv() => match effect {
                Some(MatrixEffect::SubscribeTimeline(room)) => {
                    if let Some(previous) = main.take() {
                        previous.stop().await;
                    }
                    main = subscribe(&client, TimelineId::main(room), &to_core).await;
                }
                Some(MatrixEffect::UnsubscribeTimeline(room)) => {
                    if let Some(open) = main.take_if(|open| *open.room() == room) {
                        open.stop().await;
                    }
                }
                Some(MatrixEffect::SubscribeThread { room, root }) => {
                    if let Some(previous) = thread.take() {
                        previous.stop().await;
                    }
                    let id = TimelineId { room, thread: Some(root) };
                    thread = subscribe(&client, id, &to_core).await;
                }
                Some(MatrixEffect::UnsubscribeThread { room, root }) => {
                    let id = TimelineId { room, thread: Some(root) };
                    if let Some(open) = thread.take_if(|open| *open.id() == id) {
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
                Some(MatrixEffect::SendThreadReceipt { room, root, event, unthreaded }) => {
                    let client = client.clone();
                    let sent = async move {
                        let thread = (!unthreaded).then_some(&root);
                        send_thread_receipt(&client, &room, thread, &event, receipts).await
                    };
                    running.spawn(reported(&to_core, "Sending the read receipt failed", sent));
                }
                Some(MatrixEffect::PaginateBackwards(id)) => {
                    let open = [main.as_mut(), thread.as_mut()]
                        .into_iter()
                        .flatten()
                        .find(|open| *open.id() == id);
                    match open {
                        Some(open) => open.paginate_backwards(to_core.clone()),
                        // Opening the room or the thread failed, or the
                        // timeline is another; the answer ends the core's
                        // request.
                        None => {
                            let _ = to_core.send(Msg::PaginationFailed {
                                timeline: id,
                                error: Error::TimelineNotOpen.to_string(),
                            });
                        }
                    }
                }
                Some(MatrixEffect::Send { timeline, id, message }) => {
                    match subscribed(&main, &thread, &timeline) {
                        Some(open) => {
                            let _ = sends.send(Queued::Message(open.timeline(), id, message));
                        }
                        None => report_send(&to_core, id, &Error::TimelineNotOpen),
                    }
                }
                Some(MatrixEffect::ToggleReaction { room, item, key }) => {
                    let open = ItemTimelines::of(&main, &thread, &room);
                    let toggled = async move {
                        let timeline = open?.holding(&item).await;
                        toggle_reaction(&timeline, &item, &key).await
                    };
                    running.spawn(reported(&to_core, "Reacting failed", toggled));
                }
                Some(MatrixEffect::Redact { room, item }) => {
                    let open = ItemTimelines::of(&main, &thread, &room);
                    let redacted = async move {
                        let timeline = open?.holding(&item).await;
                        redact(&timeline, &item).await
                    };
                    running.spawn(reported(&to_core, "Deleting the message failed", redacted));
                }
                Some(MatrixEffect::Resend { room, txn }) => {
                    let open = ItemTimelines::of(&main, &thread, &room);
                    let resent = async move {
                        let timeline = open?.holding(&ItemRef::Local(txn.clone())).await;
                        resend(&timeline, &txn).await
                    };
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
                    let open = ItemTimelines::of(&main, &thread, &room);
                    let downloaded = async move {
                        let timeline = open?.holding(&item).await;
                        media::download(&client, &timeline, &item, dir).await
                    };
                    running.spawn(answered(&to_core, "Downloading the file failed", downloaded));
                }
                Some(MatrixEffect::Upload { timeline, path }) => {
                    match upload_target(&client, &thread, &timeline) {
                        Ok((room, into)) => {
                            let path = media::expand_home(&path, &files.home);
                            let _ = sends.send(Queued::Upload(room, path, into));
                        }
                        Err(error) => report(&to_core, "Uploading the file failed", &error),
                    }
                }
                Some(MatrixEffect::FetchImage { room, item }) => {
                    let client = client.clone();
                    let open = ItemTimelines::of(&main, &thread, &room);
                    let fetched = async move {
                        let timeline = open?.holding(&item).await;
                        media::fetch_image(&client, &timeline, item).await
                    };
                    running.spawn(answered(&to_core, "Loading the image failed", fetched));
                }
                Some(MatrixEffect::FetchPreview { room, item }) => {
                    let open = ItemTimelines::of(&main, &thread, &room);
                    let to_core = to_core.clone();
                    running.spawn(fetch_preview(client.clone(), open, room, item, to_core));
                }
                Some(MatrixEffect::FindThread { room, event }) => {
                    running.spawn(find_thread(client.clone(), room, event, to_core.clone()));
                }
                Some(MatrixEffect::Recover(key)) => {
                    let client = client.clone();
                    // The key is dropped with the task.
                    let recovered = async move {
                        recovery::recover(&client, key.expose()).await?;
                        Ok(Msg::Recovered)
                    };
                    running.spawn(answered(&to_core, "Recovery failed", recovered));
                }
                Some(MatrixEffect::Join { target }) => {
                    running.spawn(join::join(client.clone(), dms.clone(), target, to_core.clone()));
                }
                Some(MatrixEffect::CreateDm { user }) => {
                    let dms = dms.clone();
                    running.spawn(join::create_dm(client.clone(), dms, user, to_core.clone()));
                }
                None => break,
            },
            Some(finished) = running.join_next(), if !running.is_empty() => pass_on(finished),
            Some(finished) = timeline_task(&mut main) => pass_on(finished),
            Some(finished) = timeline_task(&mut thread) => pass_on(finished),
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

/// `MatrixEffect::FetchPreview`: answers `Msg::PreviewFetched` with the
/// item's image from the timeline that holds it (see `ItemTimelines`), or
/// `Msg::PreviewFailed` for any failure, which only `debug` logs: a room
/// can hold many, and the item shows its text line instead (design,
/// "Logging").
async fn fetch_preview(
    client: Client,
    open: Result<ItemTimelines, Error>,
    room: RoomId,
    item: ItemRef,
    to_core: UnboundedSender<Msg>,
) {
    let fetched = async {
        let timeline = open?.holding(&item).await;
        media::fetch_preview(&client, &timeline, &item).await
    };
    let answer = match fetched.await {
        Ok(bytes) => Msg::PreviewFetched { room, item, bytes },
        Err(error) => {
            debug!(?item, "a timeline preview could not be fetched: {error}");
            Msg::PreviewFailed { room, item }
        }
    };
    let _ = to_core.send(answer);
}

/// Queues the messages and uploads one at a time, in the order they were
/// sent; a message is answered with `Msg::SendQueued` or `Msg::SendFailed`
/// (an edit of a sent message by `send_queue::handle_failures`, once it
/// leaves the send queue), an upload that cannot be queued is reported. A
/// reply or an edit of a sent message fetches its target event first when
/// the event cache lacks it, which can take up to three attempts of the
/// client's request timeout, and an upload reads its file (and fetches the
/// homeserver's upload limit once); a later message must not overtake
/// either, so it waits. Meanwhile the other effects (switching rooms,
/// pagination, receipts) do not wait.
async fn send_messages(
    mut requests: UnboundedReceiver<Queued>,
    edits: Edits,
    to_core: UnboundedSender<Msg>,
) {
    while let Some(request) = requests.recv().await {
        match request {
            Queued::Message(timeline, id, message) => {
                match send_message(&timeline, id, message, &edits).await {
                    Ok(Answer::Now) => {
                        let _ = to_core.send(Msg::SendQueued { id });
                    }
                    Ok(Answer::WhenSent) => {}
                    Err(error) => report_send(&to_core, id, &error),
                }
            }
            Queued::Upload(room, path, thread) => {
                if let Err(error) = media::upload(&room, path, thread).await {
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

/// `room`'s main timeline if it is the open one.
fn open_timeline(main: &Option<Subscription>, room: &RoomId) -> Result<Arc<Timeline>, Error> {
    main.as_ref()
        .filter(|open| open.room() == room)
        .map(Subscription::timeline)
        .ok_or(Error::TimelineNotOpen)
}

/// The subscription of `id`: the open room's main timeline or its thread.
fn subscribed<'a>(
    main: &'a Option<Subscription>,
    thread: &'a Option<Subscription>,
    id: &TimelineId,
) -> Option<&'a Subscription> {
    [main, thread]
        .into_iter()
        .flatten()
        .find(|open| open.id() == id)
}

/// The timelines an effect on an item of a room acts on (reactions,
/// redactions, resending, downloads, images): the open thread's, which
/// alone holds the replies in the thread, when it holds the item, else the
/// room's main timeline, which holds the root too.
struct ItemTimelines {
    main: Arc<Timeline>,
    thread: Option<Arc<Timeline>>,
}

impl ItemTimelines {
    /// `room`'s timelines while it is the open room.
    fn of(
        main: &Option<Subscription>,
        thread: &Option<Subscription>,
        room: &RoomId,
    ) -> Result<ItemTimelines, Error> {
        let thread = thread.as_ref().filter(|open| open.room() == room);
        Ok(ItemTimelines {
            main: open_timeline(main, room)?,
            thread: thread.map(Subscription::timeline),
        })
    }

    async fn holding(self, item: &ItemRef) -> Arc<Timeline> {
        if let Some(thread) = self.thread
            && holds(&thread, item).await
        {
            return thread;
        }
        self.main
    }
}

/// Where an upload to `timeline` goes: the room, and for a thread its root
/// and timeline, which has to be the open thread's.
fn upload_target(
    client: &Client,
    thread: &Option<Subscription>,
    timeline: &TimelineId,
) -> Result<(Room, Option<ThreadUpload>), Error> {
    let Some(root) = &timeline.thread else {
        return Ok((sdk_room(client, &timeline.room)?, None));
    };
    let open = thread
        .as_ref()
        .filter(|open| open.id() == timeline)
        .ok_or(Error::TimelineNotOpen)?;
    let thread = ThreadUpload {
        root: event_id(root)?,
        timeline: open.timeline(),
    };
    Ok((thread.timeline.room().clone(), Some(thread)))
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

/// The timeline `id`, the room's main timeline or a thread's; classic sync
/// delivers every room's events already.
async fn subscribe(
    client: &Client,
    id: TimelineId,
    to_core: &UnboundedSender<Msg>,
) -> Option<Subscription> {
    let what = match id.thread {
        None => "Opening the room failed",
        Some(_) => "Opening the thread failed",
    };
    let started = match sdk_room(client, &id.room) {
        Ok(sdk_room) => Subscription::start(&sdk_room, id, to_core.clone()).await,
        Err(error) => Err(error),
    };
    started
        .inspect_err(|error| report(to_core, what, error))
        .ok()
}

/// The next task of the subscription that ended; `None` without a
/// subscription or without tasks, which disables the `select!` branch until
/// the next turn.
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

/// One receipt of the configured kind on `event`, threaded in `thread`, or
/// unthreaded without one (design, "Threads"). It moves no fully read
/// marker, and an unthreaded one clears the room's manual unread mark.
async fn send_thread_receipt(
    client: &Client,
    room: &RoomId,
    thread: Option<&EventId>,
    event: &EventId,
    receipts: ReceiptKind,
) -> Result<(), Error> {
    let room = sdk_room(client, room)?;
    let thread = match thread {
        Some(root) => ReceiptThread::Thread(event_id(root)?),
        None => ReceiptThread::Unthreaded,
    };
    let kind = match receipts {
        ReceiptKind::Public => create_receipt::v3::ReceiptType::Read,
        ReceiptKind::Private => create_receipt::v3::ReceiptType::ReadPrivate,
    };
    room.send_single_receipt(kind, thread, event_id(event)?)
        .await
        .map_err(|error| without_url(error).into())
}

/// `MatrixEffect::FindThread`: answers `Msg::ThreadFound` with the root of
/// the thread that holds `event`, from the room's event cache, in memory
/// and in its store, without a request. An event it does not have, or a
/// failure, which goes to `debug` only, answers that the event is in no
/// thread.
async fn find_thread(client: Client, room: RoomId, event: EventId, to_core: UnboundedSender<Msg>) {
    let root = thread_root(&client, &room, &event)
        .await
        .unwrap_or_else(|error| {
            debug!("Looking for the thread of a replied-to message failed: {error}");
            None
        });
    let _ = to_core.send(Msg::ThreadFound { room, event, root });
}

async fn thread_root(
    client: &Client,
    room: &RoomId,
    event: &EventId,
) -> Result<Option<EventId>, Error> {
    let room = sdk_room(client, room)?;
    let event = event_id(event)?;
    let cache_error = |error: EventCacheError| Error::Timeline(error.into());
    let (cache, _drop_handles) = room.event_cache().await.map_err(cache_error)?;
    let found = cache.find_event(&event).await.map_err(cache_error)?;
    let root = found.and_then(|found| extract_thread_root(found.raw()));
    Ok(root.map(|root| EventId::new(root.as_str())))
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

/// The status line and the `warn` log, which the event loop writes for
/// every failure it passes to the core, get the brief error, without event
/// contents or room IDs; `debug` gets the full one.
pub(crate) fn report(to_core: &UnboundedSender<Msg>, what: &str, error: &Error) {
    report_brief(to_core, what, &error.brief(), error);
}

/// `report` for a message that could not be queued, as `Msg::SendFailed`,
/// so the core can put its draft back.
fn report_send(to_core: &UnboundedSender<Msg>, id: SendId, error: &Error) {
    let what = "Sending the message failed";
    report_send_brief(to_core, id, what, &error.brief(), error);
}

/// `report_send`, for an error that is not an `Error` and with what failed:
/// `brief` names no room and holds no event contents.
pub(crate) fn report_send_brief(
    to_core: &UnboundedSender<Msg>,
    id: SendId,
    what: &str,
    brief: &dyn Display,
    full: &dyn Display,
) {
    let error = logged(what, brief, full);
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

/// Logs the full failure to `debug` and returns the status line's text,
/// which the event loop logs to `warn`.
fn logged(what: &str, brief: &dyn Display, full: &dyn Display) -> String {
    debug!("{what}: {full}");
    format!("{what}: {brief}")
}

#[cfg(test)]
mod tests;
