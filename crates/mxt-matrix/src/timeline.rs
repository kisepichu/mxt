//! The open room's timelines, its main timeline and its open thread's: the
//! matrix-sdk-ui `Timeline` subscriptions, their back pagination, and the
//! conversion of their items into core entries.

use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU32,
    pin::pin,
    sync::Arc,
};

use futures_util::{Stream, StreamExt as _};
use matrix_sdk::{
    Room,
    room::edit::EditedContent,
    ruma::{
        self, MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedUserId,
        events::{
            Mentions,
            room::{
                ImageInfo, MediaSource,
                message::{
                    FormattedBody, MessageFormat, MessageType, RoomMessageEventContent,
                    RoomMessageEventContentWithoutRelation,
                },
            },
        },
    },
};
use matrix_sdk_ui::{
    Timeline,
    eyeball_im::VectorDiff,
    timeline::{
        self as sdk, EventSendState, EventTimelineItem, InReplyToDetails, MsgLikeKind, Profile,
        ReactionsByKeyBySender, RoomExt as _, TimelineDetails, TimelineEventItemId, TimelineFocus,
        TimelineItemContent, TimelineItemKind, VirtualTimelineItem,
    },
};
use mxt_core::{
    effect::{Outgoing, OutgoingKind},
    ids::{EventId, RoomId, SendId, TimelineId, Timestamp, TxnId, UserId},
    mention::RenderedBody,
    msg::Msg,
    rich::{Block, Inline, RichText},
    timeline::{
        ItemRef, MessageContent, ReactionGroup, ReactionSender, ReplyPreview, SendState,
        ThreadSummary, TimelineDiff, TimelineEntry, TimelineItem,
    },
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{JoinError, JoinSet},
};
use tracing::{debug, warn};

use crate::{
    Error,
    error::{brief_sdk, timeline_error, without_url},
    html::render_html,
    send_queue::Edits,
};

/// How many events one back pagination asks for.
const PAGE: u16 = 20;
/// The most joined members a room may have for its members to be loaded
/// when its timeline opens. matrix-sdk-base marks a room's members missing
/// on every limited sync (`mark_members_missing`), so `fetch_members` asks
/// for the full `/members` list again on the first open after one, and the
/// SDK rotates the room's outbound megolm session after each full member
/// reload. Above this, senders keep what lazy loading gives them.
const MEMBERS_FETCH_LIMIT: u64 = 500;

/// A subscribed timeline, the open room's main timeline or its open
/// thread's, and its tasks: the one that forwards its diffs, the one that
/// fetches reply details, for the main timeline the one that loads the
/// room's members (see `MEMBERS_FETCH_LIMIT`), and a back pagination in
/// flight.
pub(crate) struct Subscription {
    id: TimelineId,
    timeline: Arc<Timeline>,
    tasks: JoinSet<()>,
}

impl Subscription {
    /// Builds the timeline `id` of `sdk_room`: the main timeline without
    /// the replies in threads, or a thread's, root first once its start is
    /// loaded (design, "Threads"). Sends its items as one `Msg::Timeline`
    /// with a `Reset`, then every batch of its diffs as one `Msg::Timeline`,
    /// each for `id`. The SDK's diffs are forwarded as they are: a remote
    /// echo replaces its local echo with `Remove` and `Insert` or
    /// `PushBack`. Members are lazy-loaded, so the SDK may lack the
    /// profiles of senders, the user's own included; with the main timeline
    /// of a room of up to `MEMBERS_FETCH_LIMIT` joined members the room's
    /// members are loaded in the background, and the senders' names arrive
    /// as `Set` diffs; a thread's timeline does not load them again. The
    /// names of the reactions' senders are read from the store before the
    /// items they are on are converted (see `ReactorNames`).
    pub(crate) async fn start(
        sdk_room: &Room,
        id: TimelineId,
        to_core: UnboundedSender<Msg>,
    ) -> Result<Subscription, Error> {
        let focus = match &id.thread {
            // The SDK makes no item of an event in a thread, remote or
            // local.
            None => TimelineFocus::Live {
                hide_threaded_events: true,
            },
            Some(root) => TimelineFocus::Thread {
                root_event_id: event_id(root)?,
            },
        };
        let built = sdk_room.timeline_builder().with_focus(focus).build().await;
        let timeline = Arc::new(built.map_err(timeline_error)?);
        let me = sdk_room.own_user_id().to_owned();
        let (items, diffs) = timeline.subscribe().await;
        let (to_fetch, fetches) = mpsc::unbounded_channel();
        let mut replies = ReplyDetails {
            asked: HashSet::new(),
            to_fetch,
        };
        items.iter().for_each(|item| replies.see(item));
        let mut names = ReactorNames::default();
        names
            .learn(sdk_room, &me, items.iter().map(|item| &**item))
            .await;
        let entries = items.iter().map(|item| entry(item, &me, &names)).collect();
        let reset = vec![TimelineDiff::Reset(entries)];
        let _ = to_core.send(Msg::Timeline {
            timeline: id.clone(),
            diffs: reset,
        });
        let mut tasks = JoinSet::new();
        let room = sdk_room.clone();
        tasks.spawn(forward(
            id.clone(),
            room,
            me,
            diffs,
            replies,
            names,
            to_core,
        ));
        tasks.spawn(fetch_reply_details(timeline.clone(), fetches));
        if id.thread.is_none() && sdk_room.joined_members_count() <= MEMBERS_FETCH_LIMIT {
            let members = timeline.clone();
            tasks.spawn(async move { members.fetch_members().await });
        }
        Ok(Subscription {
            id,
            timeline,
            tasks,
        })
    }

    /// The timeline the core names in its messages and effects.
    pub(crate) fn id(&self) -> &TimelineId {
        &self.id
    }

    pub(crate) fn room(&self) -> &RoomId {
        &self.id.room
    }

    /// The timeline, for the effects that act on its items. They run outside
    /// the subscription's tasks, so closing the room does not abort a send.
    pub(crate) fn timeline(&self) -> Arc<Timeline> {
        self.timeline.clone()
    }

    /// Loads older events in the background, answered with
    /// `Msg::PaginationDone` or `Msg::PaginationFailed`.
    pub(crate) fn paginate_backwards(&mut self, to_core: UnboundedSender<Msg>) {
        let (id, timeline) = (self.id.clone(), self.timeline.clone());
        self.tasks.spawn(async move {
            let answer = match timeline.paginate_backwards(PAGE).await {
                Ok(reached_start) => Msg::PaginationDone {
                    timeline: id,
                    reached_start,
                },
                Err(error) => {
                    let error = timeline_error(error);
                    let brief = error.brief().to_string();
                    warn!("Loading older messages failed: {brief}");
                    debug!("Loading older messages failed: {error}");
                    Msg::PaginationFailed {
                        timeline: id,
                        error: brief,
                    }
                }
            };
            let _ = to_core.send(answer);
        });
    }

    /// The next task that ended; pending while they run, `None` without
    /// tasks. Cancel safe.
    pub(crate) async fn join_next(&mut self) -> Option<Result<(), JoinError>> {
        self.tasks.join_next().await
    }

    /// Stops the tasks and waits until they have stopped, so nothing of this
    /// subscription reaches the core afterwards (a late `PaginationDone`
    /// would count for the room's next subscription), then drops the
    /// timeline. A panic in a task is passed on.
    pub(crate) async fn stop(mut self) {
        self.tasks.abort_all();
        while let Some(finished) = self.tasks.join_next().await {
            if let Err(error) = finished
                && error.is_panic()
            {
                std::panic::resume_unwind(error.into_panic());
            }
        }
    }
}

async fn forward(
    id: TimelineId,
    room: Room,
    me: OwnedUserId,
    diffs: impl Stream<Item = Vec<VectorDiff<Arc<sdk::TimelineItem>>>>,
    mut replies: ReplyDetails,
    mut names: ReactorNames,
    to_core: UnboundedSender<Msg>,
) {
    let mut diffs = pin!(diffs);
    while let Some(batch) = diffs.next().await {
        names
            .learn(&room, &me, batch.iter().flat_map(diff_items))
            .await;
        let diffs = batch
            .into_iter()
            .map(|diff| {
                replies.see_diff(&diff);
                timeline_diff(diff, &me, &names)
            })
            .collect();
        let changed = Msg::Timeline {
            timeline: id.clone(),
            diffs,
        };
        if to_core.send(changed).is_err() {
            return;
        }
    }
}

/// The items `diff` adds or replaces.
fn diff_items(
    diff: &VectorDiff<Arc<sdk::TimelineItem>>,
) -> impl Iterator<Item = &sdk::TimelineItem> {
    let (values, value) = match diff {
        VectorDiff::Append { values } | VectorDiff::Reset { values } => (Some(values), None),
        VectorDiff::PushFront { value }
        | VectorDiff::PushBack { value }
        | VectorDiff::Insert { value, .. }
        | VectorDiff::Set { value, .. } => (None, Some(value)),
        VectorDiff::Clear
        | VectorDiff::PopFront
        | VectorDiff::PopBack
        | VectorDiff::Remove { .. }
        | VectorDiff::Truncate { .. } => (None, None),
    };
    values
        .into_iter()
        .flatten()
        .chain(value)
        .map(|item| &**item)
}

/// The display names of the reactions' senders (design, "Reactions" in
/// "Matrix adapter"), each read once from the SDK's store, which the SDK's
/// timeline takes the senders' own names from, and kept for the
/// subscription's life: a member renamed meanwhile keeps the old name. A
/// sender the store has no name for is not kept, so their name is looked
/// up again with the next batch that carries an item with their reaction,
/// and a member event that reaches the store later shows then.
#[derive(Default)]
struct ReactorNames(HashMap<OwnedUserId, String>);

impl ReactorNames {
    /// Reads the names of the senders of the reactions on `items`' events,
    /// other than `me` and those already known, each once. From the store
    /// only (`Room::get_member_no_sync`): no request. A member without a
    /// display name or with an empty one has none; a failed read, which the
    /// user cannot act on, goes to `debug` only.
    async fn learn<'a>(
        &mut self,
        room: &Room,
        me: &ruma::UserId,
        items: impl Iterator<Item = &'a sdk::TimelineItem>,
    ) {
        let unknown: HashSet<&ruma::UserId> = items
            .filter_map(|item| item.as_event()?.content().reactions())
            .flat_map(|reactions| reactions.values().flat_map(|senders| senders.keys()))
            .map(|user| &**user)
            .filter(|user| *user != me && !self.0.contains_key(*user))
            .collect();
        for user in unknown {
            match room.get_member_no_sync(user).await {
                Ok(member) => {
                    let name = member.as_ref().and_then(|member| member.display_name());
                    if let Some(name) = name.filter(|name| !name.is_empty()) {
                        self.0.insert(user.to_owned(), name.to_owned());
                    }
                }
                Err(error) => debug!("Reading a reaction sender's name failed: {error}"),
            }
        }
    }

    fn get(&self, user: &ruma::UserId) -> Option<String> {
        self.0.get(user).cloned()
    }
}

/// The SDK resolves a reply only against the items loaded when the reply
/// arrives, and does not look again when the replied-to event is loaded
/// later (by back pagination, for example). So every reply in the loaded
/// timeline whose replied-to event the SDK has not looked up is sent to
/// `fetch_reply_details`.
struct ReplyDetails {
    /// Reply events sent since the last `Clear` or `Reset`. The SDK fetches
    /// an `Error` again on every call, and every receipt or reaction sends the
    /// item again with `Set`; on `Clear` and `Reset` it rebuilds the items,
    /// which come back `Unavailable`.
    asked: HashSet<OwnedEventId>,
    to_fetch: UnboundedSender<OwnedEventId>,
}

impl ReplyDetails {
    fn see_diff(&mut self, diff: &VectorDiff<Arc<sdk::TimelineItem>>) {
        if matches!(diff, VectorDiff::Clear | VectorDiff::Reset { .. }) {
            self.asked.clear();
        }
        diff_items(diff).for_each(|item| self.see(item));
    }

    /// The SDK fetches details only for remote events.
    fn see(&mut self, item: &sdk::TimelineItem) {
        let Some(event) = item.as_event().filter(|event| event.is_remote_event()) else {
            return;
        };
        let TimelineItemContent::MsgLike(msg_like) = event.content() else {
            return;
        };
        if let Some(reply) = &msg_like.in_reply_to
            && needs_details(reply)
            && let Some(id) = event.event_id()
            && self.asked.insert(id.to_owned())
        {
            let _ = self.to_fetch.send(id.to_owned());
        }
    }
}

/// Only details the SDK has not looked up: `Pending` ones are being fetched,
/// and `Error` ones failed already.
fn needs_details(reply: &InReplyToDetails) -> bool {
    matches!(reply.event, TimelineDetails::Unavailable)
}

/// Fetches the replied-to events of the replies in `ids` one at a time, so a
/// page of replies to older messages does not send its requests at once.
/// The SDK looks in the loaded items and the event cache first, and sets the
/// result on the reply item, which reaches the core as a `Set`. A failure is
/// not something the user can act on, so it goes to `debug` only.
async fn fetch_reply_details(timeline: Arc<Timeline>, mut ids: UnboundedReceiver<OwnedEventId>) {
    while let Some(id) = ids.recv().await {
        if let Err(error) = timeline.fetch_details_for_event(&id).await {
            debug!("Loading a replied-to message failed: {error}");
        }
    }
}

/// When a queued message is answered with `Msg::SendQueued`.
pub(crate) enum Answer {
    /// At once: its local echo holds its text.
    Now,
    /// When it is sent: an edit of a sent message, recorded in `Edits`, has
    /// no local echo of its own.
    WhenSent,
}

/// Queues `message`, the core's send `id`, in the room's send queue: a new
/// message, a reply, or an edit, which keeps its mentions in the new
/// content. The local echo shows the send state, so only a failure to queue
/// is an error here; an edit of a sent message is recorded in `edits`.
pub(crate) async fn send_message(
    timeline: &Timeline,
    id: SendId,
    message: Outgoing,
    edits: &Edits,
) -> Result<Answer, Error> {
    enable_sending(timeline.room());
    let content = outgoing_content(&message.body);
    let answer = match message.kind {
        OutgoingKind::New => timeline
            .send(RoomMessageEventContent::from(content).into())
            .await
            .map(|_| Answer::Now),
        OutgoingKind::Reply { to } => timeline
            .send_reply(content, event_id(&to)?)
            .await
            .map(|_| Answer::Now),
        // What `Timeline::edit` does for a sent message, without looking for
        // it in the timeline's items, which may no longer hold it.
        OutgoingKind::Edit {
            target: ItemRef::Remote(target),
        } => {
            let edit = timeline
                .room()
                .make_edit_event(&event_id(&target)?, EditedContent::RoomMessage(content))
                .await
                .map_err(|error| timeline_error(sdk::EditError::RoomError(error).into()))?;
            edits.send(timeline, edit, id).await
        }
        // A local echo is replaced in the send queue.
        OutgoingKind::Edit {
            target: target @ ItemRef::Local(_),
        } => {
            let content = EditedContent::RoomMessage(content);
            timeline
                .edit(&item_id(&target)?, content)
                .await
                .map(|()| Answer::Now)
        }
    };
    answer.map_err(timeline_error)
}

/// Adds the user's reaction with `key` to `item`, or removes it if they
/// reacted with it.
pub(crate) async fn toggle_reaction(
    timeline: &Timeline,
    item: &ItemRef,
    key: &str,
) -> Result<(), Error> {
    enable_sending(timeline.room());
    timeline
        .toggle_reaction(&item_id(item)?, key)
        .await
        .map(drop)
        .map_err(timeline_error)
}

/// Redacts the user's own sent `item`, or stops sending it while it is a
/// local echo. A sent item is redacted at once, not through the room's send
/// queue as `Timeline::redact` does: the SDK would show it deleted before
/// the homeserver accepts the redaction, and a refused redaction would hold
/// back the room's later requests. The item changes when the redaction
/// arrives with the sync.
pub(crate) async fn redact(timeline: &Timeline, item: &ItemRef) -> Result<(), Error> {
    match item {
        ItemRef::Remote(event) => timeline
            .room()
            .redact(&event_id(event)?, None, None)
            .await
            .map(drop)
            .map_err(|error| without_url(error.into()).into()),
        ItemRef::Local(_) => {
            enable_sending(timeline.room());
            timeline
                .redact(&item_id(item)?, None)
                .await
                .map_err(timeline_error)
        }
    }
}

/// Sends the failed local echo `txn` again, from its place in the room's
/// send queue.
pub(crate) async fn resend(timeline: &Timeline, txn: &TxnId) -> Result<(), Error> {
    let id = TimelineEventItemId::TransactionId(txn.as_str().into());
    let handle = timeline.items().await.iter().rev().find_map(|item| {
        item.as_event()
            .filter(|event| event.identifier() == id)?
            .local_echo_send_handle()
    });
    let handle = handle.ok_or_else(|| Error::Timeline(sdk::Error::EventNotInTimeline(id)))?;
    handle
        .unwedge()
        .await
        .map_err(|error| Error::Timeline(error.into()))?;
    enable_sending(timeline.room());
    Ok(())
}

/// The message type of `item`, with its latest edit applied, for the
/// attachment it carries; `None` when the item is not a message.
pub(crate) async fn message_type(
    timeline: &Timeline,
    item: &ItemRef,
) -> Result<Option<MessageType>, Error> {
    with_event(timeline, item, |event| {
        let message = event.content().as_message()?;
        Some(message.msgtype().clone())
    })
    .await
}

/// The image of `item`, for the message view and the timeline's previews:
/// the info and the source of an image message, with its latest edit
/// applied, or of a sticker (ruma-events `sticker.rs:59-97`); `None` for
/// any other item.
pub(crate) async fn preview_media(
    timeline: &Timeline,
    item: &ItemRef,
) -> Result<Option<(Option<ImageInfo>, MediaSource)>, Error> {
    with_event(timeline, item, |event| {
        let content = event.content();
        if let Some(sticker) = content.as_sticker() {
            let sticker = sticker.content();
            return Some((Some(sticker.info.clone()), sticker.source.clone().into()));
        }
        match content.as_message()?.msgtype() {
            MessageType::Image(image) => {
                Some((image.info.as_deref().cloned(), image.source.clone()))
            }
            _ => None,
        }
    })
    .await
}

/// `f` of the event of `item` in `timeline`.
async fn with_event<T>(
    timeline: &Timeline,
    item: &ItemRef,
    f: impl FnOnce(&EventTimelineItem) -> T,
) -> Result<T, Error> {
    let id = item_id(item)?;
    let items = timeline.items().await;
    let event = items
        .iter()
        .rev()
        .find_map(|item| item.as_event().filter(|event| event.identifier() == id));
    let event = event.ok_or_else(|| Error::Timeline(sdk::Error::EventNotInTimeline(id)))?;
    Ok(f(event))
}

/// Whether `timeline` has `item` among its items.
pub(crate) async fn holds(timeline: &Timeline, item: &ItemRef) -> bool {
    let Ok(id) = item_id(item) else {
        return false;
    };
    timeline.items().await.iter().any(|item| {
        item.as_event()
            .is_some_and(|event| event.identifier() == id)
    })
}

/// The SDK disables a room's send queue after any failed request, and only
/// `Client::resume` enables it again, so every request the user queues or
/// deletes enables it (the adapter also enables every queue when the sync
/// connects); otherwise later messages, uploads, reactions and redactions
/// would wait for good. A request that failed with a recoverable error (the
/// network, for example) stays queued and goes out first, which keeps the
/// order; one that failed otherwise holds back the ones after it until it is
/// resent or deleted, or, for an edit or a reaction, dropped (see
/// `send_queue`).
pub(crate) fn enable_sending(room: &Room) {
    room.send_queue().set_enabled(true);
}

/// The content of a message from the composer (design, "Matrix adapter",
/// Sending): an HTML body from the Markdown when it has any formatting, the
/// plain body alone otherwise, and `m.mentions` with the users still
/// mentioned. `m.mentions` is there also when it is empty: the push rules
/// from before it apply only to events without it, and would notify the
/// members whose display name the message contains, or the whole room for
/// `@room`.
fn outgoing_content(body: &RenderedBody) -> RoomMessageEventContentWithoutRelation {
    let content = match FormattedBody::markdown(&body.markdown) {
        Some(html) => RoomMessageEventContentWithoutRelation::text_html(&body.plain, html.body),
        None => RoomMessageEventContentWithoutRelation::text_plain(&body.plain),
    };
    // The core's user IDs come from the SDK's members, so they parse.
    let users = body
        .user_ids
        .iter()
        .filter_map(|user| ruma::UserId::parse(user.as_str()).ok());
    content.add_mentions(Mentions::with_user_ids(users))
}

fn timeline_diff(
    diff: VectorDiff<Arc<sdk::TimelineItem>>,
    me: &ruma::UserId,
    names: &ReactorNames,
) -> TimelineDiff {
    let entries = |items: matrix_sdk_ui::eyeball_im::Vector<Arc<sdk::TimelineItem>>| {
        items.iter().map(|item| entry(item, me, names)).collect()
    };
    match diff {
        VectorDiff::Append { values } => TimelineDiff::Append(entries(values)),
        VectorDiff::Clear => TimelineDiff::Clear,
        VectorDiff::PushFront { value } => TimelineDiff::PushFront(entry(&value, me, names)),
        VectorDiff::PushBack { value } => TimelineDiff::PushBack(entry(&value, me, names)),
        VectorDiff::PopFront => TimelineDiff::PopFront,
        VectorDiff::PopBack => TimelineDiff::PopBack,
        VectorDiff::Insert { index, value } => {
            TimelineDiff::Insert(index, entry(&value, me, names))
        }
        VectorDiff::Set { index, value } => TimelineDiff::Set(index, entry(&value, me, names)),
        VectorDiff::Remove { index } => TimelineDiff::Remove(index),
        VectorDiff::Truncate { length } => TimelineDiff::Truncate(length),
        VectorDiff::Reset { values } => TimelineDiff::Reset(entries(values)),
    }
}

/// Every SDK item becomes one entry, so indices stay the SDK's: what the
/// first release does not show becomes `Hidden`.
fn entry(item: &sdk::TimelineItem, me: &ruma::UserId, names: &ReactorNames) -> TimelineEntry {
    match item.kind() {
        TimelineItemKind::Virtual(VirtualTimelineItem::DateDivider(day)) => {
            TimelineEntry::DateDivider(timestamp(*day))
        }
        TimelineItemKind::Virtual(VirtualTimelineItem::ReadMarker) => TimelineEntry::ReadMarker,
        TimelineItemKind::Virtual(VirtualTimelineItem::TimelineStart) => TimelineEntry::Hidden,
        TimelineItemKind::Event(event) => event_entry(event, me, names),
    }
}

/// Messages and other message-like events, calls included, are items; state
/// events and membership changes are hidden.
fn event_entry(
    event: &EventTimelineItem,
    me: &ruma::UserId,
    names: &ReactorNames,
) -> TimelineEntry {
    let (content, reply, reactions, edited, thread) = match event.content() {
        TimelineItemContent::MsgLike(msg_like) => (
            msg_like_content(&msg_like.kind),
            msg_like.in_reply_to.as_ref().map(reply_preview),
            reaction_groups(&msg_like.reactions, me, names),
            matches!(&msg_like.kind, MsgLikeKind::Message(message) if message.is_edited()),
            thread_summary(msg_like.thread_summary.as_ref()),
        ),
        other => match other_content(other) {
            Some(content) => (content, None, Vec::new(), false, None),
            None => return TimelineEntry::Hidden,
        },
    };
    TimelineEntry::Item(TimelineItem {
        id: item_ref(event.identifier()),
        sender: UserId::new(event.sender().as_str()),
        sender_name: profile_name(event.sender_profile(), event.sender()),
        timestamp: timestamp(event.timestamp()),
        content,
        reply,
        reactions,
        edited,
        send_state: event.send_state().map(send_state),
        thread_summary: thread,
    })
}

/// The summary of the thread a message is the root of (design, "Threads"):
/// none without one, and none for one that counts no reply, which the SDK
/// gives when every reply was redacted and a summary the server bundled may
/// say. The latest reply's time once the SDK has loaded that reply.
fn thread_summary(summary: Option<&sdk::ThreadSummary>) -> Option<ThreadSummary> {
    let summary = summary?;
    let latest = match &summary.latest_event {
        TimelineDetails::Ready(event) => Some(timestamp(event.timestamp)),
        _ => None,
    };
    Some(ThreadSummary {
        replies: NonZeroU32::new(summary.num_replies)?,
        latest,
    })
}

/// The content of an event that is not `MsgLike`. Calls and message-like
/// events the SDK could not parse are unsupported items, so the reader
/// learns of them; state events are hidden (`None`).
fn other_content(content: &TimelineItemContent) -> Option<MessageContent> {
    let kind = match content {
        TimelineItemContent::FailedToParseMessageLike { event_type, .. } => event_type.to_string(),
        TimelineItemContent::CallInvite | TimelineItemContent::RtcNotification { .. } => {
            "call".to_owned()
        }
        _ => return None,
    };
    Some(MessageContent::Unsupported { kind })
}

fn msg_like_content(kind: &MsgLikeKind) -> MessageContent {
    match kind {
        MsgLikeKind::Message(message) => message_content(message.msgtype(), message.body()),
        MsgLikeKind::Sticker(sticker) => {
            let sticker = sticker.content();
            MessageContent::Sticker {
                body: sticker.body.clone(),
                dimensions: dimensions(&sticker.info),
            }
        }
        MsgLikeKind::Redacted => MessageContent::Redacted,
        MsgLikeKind::UnableToDecrypt(_) => MessageContent::Undecryptable,
        MsgLikeKind::Poll(_) => MessageContent::Unsupported {
            kind: "poll".to_owned(),
        },
        MsgLikeKind::LiveLocation(_) => MessageContent::Unsupported {
            kind: "live location".to_owned(),
        },
        MsgLikeKind::Other(other) => MessageContent::Unsupported {
            kind: other.event_type().to_string(),
        },
    }
}

/// The latest edit's content, as the SDK applies edits to `msgtype`. `body`
/// is `Message::body()`: the latest edit's plain body, with the reply
/// fallback already stripped by the SDK.
fn message_content(msgtype: &MessageType, body: &str) -> MessageContent {
    match msgtype {
        MessageType::Text(text) => MessageContent::Text {
            body: body.to_owned(),
            rich: rich_body(body, text.formatted.as_ref()),
        },
        MessageType::Notice(notice) => {
            MessageContent::Notice(rich_body(&notice.body, notice.formatted.as_ref()))
        }
        MessageType::Emote(emote) => {
            MessageContent::Emote(rich_body(&emote.body, emote.formatted.as_ref()))
        }
        MessageType::ServerNotice(notice) => MessageContent::Notice(RichText::plain(&notice.body)),
        MessageType::Image(image) => {
            let info = image.info.as_deref();
            MessageContent::Image {
                name: image.filename().to_owned(),
                size: info.and_then(|info| info.size).map(u64::from),
                dimensions: info.and_then(dimensions),
            }
        }
        MessageType::File(file) => {
            let info = file.info.as_deref();
            MessageContent::File {
                name: file.filename().to_owned(),
                size: info.and_then(|info| info.size).map(u64::from),
                mime: info.and_then(|info| info.mimetype.clone()),
            }
        }
        MessageType::Video(video) => {
            let info = video.info.as_deref();
            MessageContent::File {
                name: video.filename().to_owned(),
                size: info.and_then(|info| info.size).map(u64::from),
                mime: info.and_then(|info| info.mimetype.clone()),
            }
        }
        MessageType::Audio(audio) => {
            let info = audio.info.as_deref();
            MessageContent::File {
                name: audio.filename().to_owned(),
                size: info.and_then(|info| info.size).map(u64::from),
                mime: info.and_then(|info| info.mimetype.clone()),
            }
        }
        // The specification's fallback for message types a client does not
        // show (locations, verification requests, custom types): the body.
        // Not as `Text`, which the core lets the sender edit: the edit would
        // turn the message into `m.text` for every client.
        _ => MessageContent::Notice(RichText::plain(body)),
    }
}

/// An image's width and height from its info, when it gives both and they
/// fit in `u32`.
fn dimensions(info: &ImageInfo) -> Option<(u32, u32)> {
    let side = |side: Option<ruma::UInt>| u32::try_from(u64::from(side?)).ok();
    Some((side(info.width)?, side(info.height)?))
}

/// The HTML body when there is one, otherwise the plain body. An HTML body
/// with nothing but blank lines falls back to the plain body, so the message
/// is not blank.
fn rich_body(body: &str, formatted: Option<&FormattedBody>) -> RichText {
    html_body(formatted).unwrap_or_else(|| RichText::plain(body))
}

/// `formatted` as rich text when it is HTML with more than blank lines.
fn html_body(formatted: Option<&FormattedBody>) -> Option<RichText> {
    let html = formatted.filter(|formatted| formatted.format == MessageFormat::Html)?;
    Some(render_html(&html.body)).filter(|rich| !is_blank(rich))
}

fn is_blank(rich: &RichText) -> bool {
    rich.blocks.iter().all(|block| {
        matches!(block, Block::Paragraph(inlines)
            if inlines.iter().all(|inline| *inline == Inline::LineBreak))
    })
}

/// The replied-to event's sender and text, once the SDK has them.
fn reply_preview(reply: &InReplyToDetails) -> ReplyPreview {
    let (sender_name, body) = match &reply.event {
        TimelineDetails::Ready(event) => (
            Some(profile_name(&event.sender_profile, &event.sender)),
            reply_body(&event.content),
        ),
        _ => (None, None),
    };
    ReplyPreview {
        event: EventId::new(reply.event_id.as_str()),
        sender_name,
        body,
    }
}

fn reply_body(content: &TimelineItemContent) -> Option<String> {
    let TimelineItemContent::MsgLike(msg_like) = content else {
        return None;
    };
    Some(match &msg_like.kind {
        MsgLikeKind::Message(message) => message_preview(message.msgtype(), message.body()),
        kind => msg_like_content(kind).plain_text(),
    })
}

/// A replied-to message's text: its HTML body (or a caption's), rendered as
/// the timeline renders HTML bodies, with every spoiler as `[spoiler]`,
/// since the plain body has the spoiler text in the clear; the plain body
/// without HTML. `body` is `Message::body()`, as for `message_content`.
fn message_preview(msgtype: &MessageType, body: &str) -> String {
    let formatted = match msgtype {
        MessageType::Text(content) => content.formatted.as_ref(),
        MessageType::Notice(content) => content.formatted.as_ref(),
        MessageType::Emote(content) => content.formatted.as_ref(),
        MessageType::Image(content) => content.formatted.as_ref(),
        MessageType::File(content) => content.formatted.as_ref(),
        MessageType::Video(content) => content.formatted.as_ref(),
        MessageType::Audio(content) => content.formatted.as_ref(),
        _ => None,
    };
    html_body(formatted).map_or_else(|| body.to_owned(), |rich| rich.to_plain_hiding_spoilers())
}

/// One group per key, in the SDK's order, with its senders sorted by the
/// reaction's timestamp, oldest first (design, "Reactions" in "Matrix
/// adapter"). The sort is stable, so the SDK's order, the order in which
/// the timeline applied the reactions, breaks ties.
fn reaction_groups(
    reactions: &ReactionsByKeyBySender,
    me: &ruma::UserId,
    names: &ReactorNames,
) -> Vec<ReactionGroup> {
    reactions
        .iter()
        .map(|(key, by_sender)| {
            let mut sorted: Vec<_> = by_sender.iter().collect();
            sorted.sort_by_key(|(_, info)| info.timestamp);
            let senders = sorted.into_iter().map(|(user, _)| ReactionSender {
                user: UserId::new(user.as_str()),
                name: names.get(user),
            });
            ReactionGroup {
                key: key.clone(),
                senders: senders.collect(),
                mine: by_sender.contains_key(me),
            }
        })
        .collect()
}

/// The display name, or the user ID while the profile is not loaded or has
/// no name.
fn profile_name(profile: &TimelineDetails<Profile>, user: &ruma::UserId) -> String {
    match profile {
        TimelineDetails::Ready(Profile {
            display_name: Some(name),
            ..
        }) if !name.is_empty() => name.clone(),
        _ => user.to_string(),
    }
}

fn item_ref(id: TimelineEventItemId) -> ItemRef {
    match id {
        TimelineEventItemId::TransactionId(txn) => ItemRef::Local(TxnId::new(txn.as_str())),
        TimelineEventItemId::EventId(event) => ItemRef::Remote(EventId::new(event.as_str())),
    }
}

/// The SDK's identifier of `item`, as `item_ref` made it.
fn item_id(item: &ItemRef) -> Result<TimelineEventItemId, Error> {
    Ok(match item {
        ItemRef::Local(txn) => TimelineEventItemId::TransactionId(txn.as_str().into()),
        ItemRef::Remote(event) => TimelineEventItemId::EventId(event_id(event)?),
    })
}

pub(crate) fn event_id(event: &EventId) -> Result<OwnedEventId, Error> {
    ruma::EventId::parse(event.as_str()).map_err(|_| Error::InvalidEventId)
}

fn send_state(state: &EventSendState) -> SendState {
    match state {
        EventSendState::NotSentYet { .. } => SendState::Sending,
        EventSendState::SendingFailed { error, .. } => SendState::Failed {
            error: brief_sdk(error).to_string(),
        },
        EventSendState::Sent { .. } => SendState::Sent,
    }
}

fn timestamp(ts: MilliSecondsSinceUnixEpoch) -> Timestamp {
    Timestamp::from_millis(ts.0.into())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use matrix_sdk::{
        ruma::{
            OwnedMxcUri, UInt, event_id,
            events::{
                StateEventType,
                room::{
                    ImageInfo,
                    message::{
                        AudioInfo, AudioMessageEventContent, EmoteMessageEventContent, FileInfo,
                        FileMessageEventContent, ImageMessageEventContent,
                        LocationMessageEventContent, NoticeMessageEventContent,
                        TextMessageEventContent,
                    },
                },
            },
            owned_device_id, owned_event_id, owned_user_id, room_id, user_id,
        },
        test_utils::mocks::MatrixMockServer,
    };
    use matrix_sdk_test::{JoinedRoomBuilder, event_factory::EventFactory};
    use matrix_sdk_ui::{
        eyeball_im::Vector,
        timeline::{EmbeddedEvent, MsgLikeContent, ReactionInfo},
    };
    use mxt_core::{
        mention::{MentionRef, render_mentions},
        rich::Style,
    };
    use serde_json::json;

    use super::*;

    fn mxc() -> OwnedMxcUri {
        OwnedMxcUri::from("mxc://example.org/abc")
    }

    /// As the SDK's `Message::body()` is its `msgtype().body()`.
    fn content(msgtype: MessageType) -> MessageContent {
        message_content(&msgtype, msgtype.body())
    }

    fn text(body: &str, rich: RichText) -> MessageContent {
        MessageContent::Text {
            body: body.into(),
            rich,
        }
    }

    /// A replied-to event by Bob, with `profile`.
    fn replied(profile: TimelineDetails<Profile>) -> TimelineDetails<Box<EmbeddedEvent>> {
        TimelineDetails::Ready(Box::new(EmbeddedEvent {
            content: TimelineItemContent::MsgLike(MsgLikeContent::redacted()),
            sender: owned_user_id!("@bob:example.org"),
            sender_profile: profile,
            timestamp: MilliSecondsSinceUnixEpoch(UInt::from(0u32)),
            identifier: TimelineEventItemId::EventId(owned_event_id!("$event:example.org")),
        }))
    }

    fn reply(event: TimelineDetails<Box<EmbeddedEvent>>) -> InReplyToDetails {
        InReplyToDetails {
            event_id: owned_event_id!("$event:example.org"),
            event,
        }
    }

    #[test]
    fn html_bodies_are_rendered_and_other_formats_use_the_plain_body() {
        let bold = Inline::Text {
            text: "hi".into(),
            style: Style {
                bold: true,
                ..Style::default()
            },
        };
        // The plain body stays as sent, for editing.
        assert_eq!(
            content(MessageType::text_html("**hi**", "<b>hi</b>")),
            text(
                "**hi**",
                RichText {
                    blocks: vec![Block::Paragraph(vec![bold])],
                }
            )
        );
        let mut other_format = TextMessageEventContent::plain("plain\nbody");
        other_format.formatted = Some(FormattedBody {
            format: MessageFormat::from("org.example.markup"),
            body: "<b>ignored</b>".into(),
        });
        assert_eq!(
            content(MessageType::Text(other_format)),
            text("plain\nbody", RichText::plain("plain\nbody"))
        );
        // An HTML body without text, or with only blank lines, would leave
        // the message blank.
        let blank = TextMessageEventContent::html("fallback", "<p> </p><p><br></p>");
        assert_eq!(
            content(MessageType::Text(blank)),
            text("fallback", RichText::plain("fallback"))
        );
        let notice = NoticeMessageEventContent::html("n", "<i>n</i>");
        assert!(matches!(
            content(MessageType::Notice(notice)),
            MessageContent::Notice(rich) if rich.to_plain() == "n"
        ));
        let emote = EmoteMessageEventContent::plain("waves");
        assert_eq!(
            content(MessageType::Emote(emote)),
            MessageContent::Emote(RichText::plain("waves"))
        );
    }

    #[test]
    fn images_and_files_keep_name_size_dimensions_and_mime_type() {
        let mut image = ImageMessageEventContent::plain("a caption".into(), mxc());
        image.filename = Some("cat.png".into());
        let mut info = ImageInfo::new();
        info.width = Some(UInt::from(640u32));
        info.height = Some(UInt::from(480u32));
        info.size = Some(UInt::from(12_345u32));
        image.info = Some(Box::new(info));
        assert_eq!(
            content(MessageType::Image(image)),
            MessageContent::Image {
                name: "cat.png".into(),
                size: Some(12_345),
                dimensions: Some((640, 480)),
            }
        );
        // Without a filename the body is the name; one side alone is no
        // dimension.
        let mut image = ImageMessageEventContent::plain("dog.jpg".into(), mxc());
        let mut info = ImageInfo::new();
        info.width = Some(UInt::from(10u32));
        image.info = Some(Box::new(info));
        assert_eq!(
            content(MessageType::Image(image)),
            MessageContent::Image {
                name: "dog.jpg".into(),
                size: None,
                dimensions: None,
            }
        );
        let mut file = FileMessageEventContent::plain("report.pdf".into(), mxc());
        let mut info = FileInfo::new();
        info.mimetype = Some("application/pdf".into());
        info.size = Some(UInt::from(2_048u32));
        file.info = Some(Box::new(info));
        assert_eq!(
            content(MessageType::File(file)),
            MessageContent::File {
                name: "report.pdf".into(),
                size: Some(2_048),
                mime: Some("application/pdf".into()),
            }
        );
        let mut audio = AudioMessageEventContent::plain("voice.ogg".into(), mxc());
        let mut info = AudioInfo::new();
        info.mimetype = Some("audio/ogg".into());
        audio.info = Some(Box::new(info));
        assert_eq!(
            content(MessageType::Audio(audio)),
            MessageContent::File {
                name: "voice.ogg".into(),
                size: None,
                mime: Some("audio/ogg".into()),
            }
        );
    }

    #[test]
    fn message_types_mxt_does_not_show_are_notices_with_their_body() {
        let location =
            LocationMessageEventContent::new("Big Ben".into(), "geo:51.5008,0.1247".into());
        assert_eq!(
            content(MessageType::Location(location)),
            MessageContent::Notice(RichText::plain("Big Ben"))
        );
    }

    #[test]
    fn calls_and_unparsable_message_like_events_are_unsupported_and_state_is_hidden() {
        let unsupported = |kind: &str| {
            Some(MessageContent::Unsupported {
                kind: kind.to_owned(),
            })
        };
        assert_eq!(
            other_content(&TimelineItemContent::CallInvite),
            unsupported("call")
        );
        let notification = TimelineItemContent::RtcNotification {
            call_intent: None,
            declined_by: Vec::new(),
            active_call_info: None,
        };
        assert_eq!(other_content(&notification), unsupported("call"));
        let error = || Arc::new(serde_json::from_str::<u8>("x").unwrap_err());
        let message_like = TimelineItemContent::FailedToParseMessageLike {
            event_type: "org.example.custom".into(),
            error: error(),
        };
        assert_eq!(
            other_content(&message_like),
            unsupported("org.example.custom")
        );
        let state = TimelineItemContent::FailedToParseState {
            event_type: StateEventType::RoomTopic,
            state_key: String::new(),
            error: error(),
        };
        assert_eq!(other_content(&state), None);
    }

    #[test]
    fn reactions_are_grouped_by_key_with_their_senders_oldest_first() {
        let me = owned_user_id!("@me:example.org");
        let bob = owned_user_id!("@bob:example.org");
        let carol = owned_user_id!("@carol:example.org");
        let dave = owned_user_id!("@dave:example.org");
        let at = |millis: u32| ReactionInfo {
            timestamp: MilliSecondsSinceUnixEpoch(UInt::from(millis)),
            send_state: None,
        };
        let mut reactions = ReactionsByKeyBySender::default();
        let thumbs = reactions.entry("👍".to_owned()).or_default();
        thumbs.insert(carol.clone(), at(30));
        thumbs.insert(bob.clone(), at(10));
        thumbs.insert(me.clone(), at(20));
        // Equal timestamps keep the SDK's order.
        let party = reactions.entry("🎉".to_owned()).or_default();
        party.insert(dave.clone(), at(10));
        party.insert(bob.clone(), at(10));
        let names = ReactorNames(HashMap::from([(bob.clone(), "Bob".to_owned())]));
        let sender = |user: &ruma::UserId, name: Option<&str>| ReactionSender {
            user: UserId::new(user.as_str()),
            name: name.map(Into::into),
        };
        let groups = reaction_groups(&reactions, &me, &names);
        assert_eq!(
            groups,
            vec![
                ReactionGroup {
                    key: "👍".into(),
                    senders: vec![
                        sender(&bob, Some("Bob")),
                        sender(&me, None),
                        sender(&carol, None),
                    ],
                    mine: true,
                },
                ReactionGroup {
                    key: "🎉".into(),
                    senders: vec![sender(&dave, None), sender(&bob, Some("Bob"))],
                    mine: false,
                },
            ]
        );
        assert_eq!(groups[0].count(), 3);
    }

    /// The items of `timeline` once its items carry `reactions` reactions
    /// in all: the event cache takes a sync's events in a task of its own.
    async fn items_once_reacted(
        timeline: &Timeline,
        reactions: usize,
    ) -> Vector<Arc<sdk::TimelineItem>> {
        let count = |items: &Vector<Arc<sdk::TimelineItem>>| -> usize {
            let reactions = items
                .iter()
                .filter_map(|item| item.as_event()?.content().reactions());
            reactions
                .flat_map(|reactions| reactions.values())
                .map(|senders| senders.len())
                .sum()
        };
        let (mut items, diffs) = timeline.subscribe().await;
        let mut diffs = pin!(diffs);
        while count(&items) < reactions {
            let batch = tokio::time::timeout(Duration::from_secs(10), diffs.next());
            let batch = batch.await.expect("waited for the reactions").unwrap();
            batch.into_iter().for_each(|diff| diff.apply(&mut items));
        }
        items
    }

    /// Names from the member events in the store, none for a member with an
    /// empty one or without one, the user's own not looked up; a member
    /// event that reaches the store later counts with the next lookup.
    #[tokio::test]
    async fn reactor_names_are_the_display_names_of_the_member_events_in_the_store() {
        let server = MatrixMockServer::new().await;
        let me = user_id!("@alice:example.org");
        let client = server
            .client_builder()
            .logged_in_with_token("1234".to_owned(), me.to_owned(), owned_device_id!("DEVICE"))
            // Plain HTTP; see the adapter's tests.
            .on_builder(|builder| builder.disable_ssl_verification())
            .build()
            .await;
        server.mock_room_state_encryption().plain().mount().await;
        client.event_cache().subscribe().unwrap();
        let bob = user_id!("@bob:example.org");
        let carol = user_id!("@carol:example.org");
        let dave = user_id!("@dave:example.org");
        let room_id = room_id!("!room:example.org");
        let message = event_id!("$message:example.org");
        let f = EventFactory::new();
        let reactions = [me, bob, carol, dave]
            .map(|user| f.reaction(message, "👍").sender(user).into_raw_sync());
        let joined = JoinedRoomBuilder::new(room_id)
            .add_state_event(f.member(me).display_name("Alice"))
            .add_state_event(f.member(bob).display_name("Bob"))
            .add_state_event(f.member(dave).display_name(""))
            .add_timeline_event(f.text_msg("hello").sender(bob).event_id(message))
            .add_timeline_bulk(reactions);
        let room = server.sync_room(&client, joined).await;
        let timeline = room.timeline().await.unwrap();
        let items = items_once_reacted(&timeline, 4).await;
        let items = || items.iter().map(|item| &**item);

        let mut names = ReactorNames::default();
        names.learn(&room, me, items()).await;
        assert_eq!(names.get(bob).as_deref(), Some("Bob"));
        assert_eq!(names.get(dave), None);
        assert_eq!(names.get(carol), None);
        assert_eq!(names.get(me), None);

        let carol_joins =
            JoinedRoomBuilder::new(room_id).add_state_event(f.member(carol).display_name("Carol"));
        server.sync_room(&client, carol_joins).await;
        names.learn(&room, me, items()).await;
        assert_eq!(names.get(carol).as_deref(), Some("Carol"));
        assert_eq!(names.get(bob).as_deref(), Some("Bob"));
    }

    #[test]
    fn reply_previews_show_the_sender_and_body_once_ready() {
        let preview = |sender_name: Option<&str>, body: Option<&str>| ReplyPreview {
            event: EventId::new("$event:example.org"),
            sender_name: sender_name.map(Into::into),
            body: body.map(Into::into),
        };
        assert_eq!(
            reply_preview(&reply(TimelineDetails::Pending)),
            preview(None, None)
        );
        let named = TimelineDetails::Ready(Profile {
            display_name: Some("Bob".into()),
            ..Profile::default()
        });
        assert_eq!(
            reply_preview(&reply(replied(named))),
            preview(Some("Bob"), Some("(deleted)"))
        );
        // Without a loaded profile, the user ID names the sender.
        assert_eq!(
            reply_preview(&reply(replied(TimelineDetails::Unavailable))),
            preview(Some("@bob:example.org"), Some("(deleted)"))
        );
    }

    #[test]
    fn thread_summaries_count_replies_and_keep_the_latest_reply_time_once_loaded() {
        let summary = |num_replies, latest_event| sdk::ThreadSummary {
            latest_event,
            num_replies,
        };
        let three = NonZeroU32::new(3).unwrap();
        assert_eq!(thread_summary(None), None);
        // Every reply redacted, or a bundled summary that says so.
        let none = summary(0, replied(TimelineDetails::Unavailable));
        assert_eq!(thread_summary(Some(&none)), None);
        for latest_event in [TimelineDetails::Unavailable, TimelineDetails::Pending] {
            assert_eq!(
                thread_summary(Some(&summary(3, latest_event))),
                Some(ThreadSummary {
                    replies: three,
                    latest: None,
                })
            );
        }
        let mut latest = replied(TimelineDetails::Unavailable);
        if let TimelineDetails::Ready(event) = &mut latest {
            event.timestamp = MilliSecondsSinceUnixEpoch(UInt::from(1_500u32));
        }
        assert_eq!(
            thread_summary(Some(&summary(3, latest))),
            Some(ThreadSummary {
                replies: three,
                latest: Some(Timestamp::from_millis(1_500)),
            })
        );
    }

    /// As the SDK's `Message::body()` is its `msgtype().body()`.
    fn preview_of(msgtype: MessageType) -> String {
        message_preview(&msgtype, msgtype.body())
    }

    #[test]
    fn reply_previews_hide_the_spoilers_of_html_bodies_and_keep_plain_bodies() {
        // Text, a mention and an emoticon in one spoiler are one
        // placeholder; the plain body would show all of them.
        let spoilers = MessageType::text_html(
            "the end: Bob was @Alice :cat:, and more",
            "the end: <span data-mx-spoiler>Bob <b>was</b> \
             <a href=\"https://matrix.to/#/@alice:example.org\">@Alice</a> \
             <img data-mx-emoticon src=\"mxc://example.org/cat\" alt=\":cat:\"></span>, \
             and <span data-mx-spoiler=\"plot\">more</span>",
        );
        assert_eq!(preview_of(spoilers), "the end: [spoiler], and [spoiler]");
        // A caption's spoiler too.
        let mut image = ImageMessageEventContent::plain("a secret caption".into(), mxc());
        image.filename = Some("cat.png".into());
        image.formatted = Some(FormattedBody::html(
            "a <span data-mx-spoiler>secret</span> caption",
        ));
        assert_eq!(preview_of(MessageType::Image(image)), "a [spoiler] caption");
        // Without HTML, the plain body as sent.
        assert_eq!(
            preview_of(MessageType::text_plain("**hi** there\nsecond line")),
            "**hi** there\nsecond line"
        );
        let image = ImageMessageEventContent::plain("dog.jpg".into(), mxc());
        assert_eq!(preview_of(MessageType::Image(image)), "dog.jpg");
    }

    #[test]
    fn only_reply_details_the_sdk_has_not_looked_up_are_fetched() {
        assert!(needs_details(&reply(TimelineDetails::Unavailable)));
        assert!(!needs_details(&reply(TimelineDetails::Pending)));
        assert!(!needs_details(&reply(replied(
            TimelineDetails::Unavailable
        ))));
        let failed = TimelineDetails::Error(Arc::new(matrix_sdk::Error::InsufficientData));
        assert!(!needs_details(&reply(failed)));
    }

    fn sent(text: &str, mentions: &[MentionRef]) -> serde_json::Value {
        serde_json::to_value(outgoing_content(&render_mentions(text, mentions))).unwrap()
    }

    #[test]
    fn text_without_formatting_is_sent_plain_and_markdown_with_an_html_body() {
        // Line breaks alone are no formatting. Without mentions, `m.mentions`
        // is empty, which keeps the legacy push rules from notifying anyone.
        assert_eq!(
            sent("hello\nworld", &[]),
            json!({ "msgtype": "m.text", "body": "hello\nworld", "m.mentions": {} })
        );
        // The plain body stays as typed, and line breaks stay in the HTML.
        assert_eq!(
            sent("**hi**\nthere", &[]),
            json!({
                "msgtype": "m.text",
                "body": "**hi**\nthere",
                "format": "org.matrix.custom.html",
                "formatted_body": "<strong>hi</strong><br />\nthere",
                "m.mentions": {},
            })
        );
    }

    #[test]
    fn mentions_are_links_in_the_html_body_and_listed_in_m_mentions() {
        let alice = MentionRef {
            user: UserId::new("@alice:example.org"),
            text: "@A*b*_c".into(),
        };
        // The escaped Markdown in the name comes out as the name.
        assert_eq!(
            sent("hi @A*b*_c and @A*b*_c", &[alice]),
            json!({
                "msgtype": "m.text",
                "body": "hi @A*b*_c and @A*b*_c",
                "format": "org.matrix.custom.html",
                "formatted_body": "hi <a href=\"https://matrix.to/#/@alice:example.org\">@A*b*_c</a> \
                    and <a href=\"https://matrix.to/#/@alice:example.org\">@A*b*_c</a>",
                "m.mentions": { "user_ids": ["@alice:example.org"] },
            })
        );
    }
}
