//! The open room's timeline: the matrix-sdk-ui `Timeline` subscription, its
//! back pagination, and the conversion of its items into core entries.

use std::{collections::HashSet, pin::pin, sync::Arc};

use futures_util::{Stream, StreamExt as _};
use matrix_sdk::{
    Room,
    room::edit::EditedContent,
    ruma::{
        self, MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedUserId,
        events::{
            Mentions,
            room::message::{
                FormattedBody, MessageFormat, MessageType, RoomMessageEventContent,
                RoomMessageEventContentWithoutRelation,
            },
        },
    },
};
use matrix_sdk_ui::{
    Timeline,
    eyeball_im::VectorDiff,
    timeline::{
        self as sdk, EventSendState, EventTimelineItem, InReplyToDetails, MsgLikeKind, Profile,
        ReactionsByKeyBySender, RoomExt as _, TimelineDetails, TimelineEventItemId,
        TimelineItemContent, TimelineItemKind, VirtualTimelineItem,
    },
};
use mxt_core::{
    effect::{Outgoing, OutgoingKind},
    ids::{EventId, RoomId, Timestamp, TxnId, UserId},
    mention::RenderedBody,
    msg::Msg,
    rich::{Block, Inline, RichText},
    timeline::{
        ItemRef, MessageContent, ReactionGroup, ReplyPreview, SendState, TimelineDiff,
        TimelineEntry, TimelineItem,
    },
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{JoinError, JoinSet},
};
use tracing::{debug, warn};

use crate::{
    Error,
    error::{brief_sdk, timeline_error},
    html::render_html,
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

/// The open room's timeline, and its tasks: the one that forwards its diffs,
/// the one that fetches reply details, the one that loads the room's
/// members (see `MEMBERS_FETCH_LIMIT`), and a back pagination in flight.
pub(crate) struct Subscription {
    room: RoomId,
    timeline: Arc<Timeline>,
    tasks: JoinSet<()>,
}

impl Subscription {
    /// Builds `sdk_room`'s timeline, sends its items as one `Msg::Timeline`
    /// with a `Reset`, then every batch of its diffs as one `Msg::Timeline`.
    /// The SDK's diffs are forwarded as they are: a remote echo replaces its
    /// local echo with `Remove` and `Insert` or `PushBack`. Members are
    /// lazy-loaded, so the SDK may lack the profiles of senders, the user's
    /// own included; in rooms of up to `MEMBERS_FETCH_LIMIT` joined members
    /// the room's members are loaded in the background, and the senders'
    /// names arrive as `Set` diffs.
    pub(crate) async fn start(
        sdk_room: &Room,
        room: RoomId,
        to_core: UnboundedSender<Msg>,
    ) -> Result<Subscription, Error> {
        let timeline = Arc::new(sdk_room.timeline().await.map_err(timeline_error)?);
        let me = sdk_room.own_user_id().to_owned();
        let (items, diffs) = timeline.subscribe().await;
        let (to_fetch, fetches) = mpsc::unbounded_channel();
        let mut replies = ReplyDetails {
            asked: HashSet::new(),
            to_fetch,
        };
        items.iter().for_each(|item| replies.see(item));
        let entries = items.iter().map(|item| entry(item, &me)).collect();
        let reset = vec![TimelineDiff::Reset(entries)];
        let _ = to_core.send(Msg::Timeline {
            room: room.clone(),
            diffs: reset,
        });
        let mut tasks = JoinSet::new();
        tasks.spawn(forward(room.clone(), me, diffs, replies, to_core));
        tasks.spawn(fetch_reply_details(timeline.clone(), fetches));
        if sdk_room.joined_members_count() <= MEMBERS_FETCH_LIMIT {
            let members = timeline.clone();
            tasks.spawn(async move { members.fetch_members().await });
        }
        Ok(Subscription {
            room,
            timeline,
            tasks,
        })
    }

    pub(crate) fn room(&self) -> &RoomId {
        &self.room
    }

    /// The timeline, for the effects that act on its items. They run outside
    /// the subscription's tasks, so closing the room does not abort a send.
    pub(crate) fn timeline(&self) -> Arc<Timeline> {
        self.timeline.clone()
    }

    /// Loads older events in the background, answered with
    /// `Msg::PaginationDone` or `Msg::PaginationFailed`.
    pub(crate) fn paginate_backwards(&mut self, to_core: UnboundedSender<Msg>) {
        let (room, timeline) = (self.room.clone(), self.timeline.clone());
        self.tasks.spawn(async move {
            let answer = match timeline.paginate_backwards(PAGE).await {
                Ok(reached_start) => Msg::PaginationDone {
                    room,
                    reached_start,
                },
                Err(error) => {
                    let error = timeline_error(error);
                    let brief = error.brief().to_string();
                    warn!("Loading older messages failed: {brief}");
                    debug!("Loading older messages failed: {error}");
                    Msg::PaginationFailed { room, error: brief }
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
    room: RoomId,
    me: OwnedUserId,
    diffs: impl Stream<Item = Vec<VectorDiff<Arc<sdk::TimelineItem>>>>,
    mut replies: ReplyDetails,
    to_core: UnboundedSender<Msg>,
) {
    let mut diffs = pin!(diffs);
    while let Some(batch) = diffs.next().await {
        let diffs = batch
            .into_iter()
            .map(|diff| {
                replies.see_diff(&diff);
                timeline_diff(diff, &me)
            })
            .collect();
        let changed = Msg::Timeline {
            room: room.clone(),
            diffs,
        };
        if to_core.send(changed).is_err() {
            return;
        }
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
        match diff {
            VectorDiff::Clear => self.asked.clear(),
            VectorDiff::Reset { values } => {
                self.asked.clear();
                values.iter().for_each(|item| self.see(item));
            }
            VectorDiff::Append { values } => values.iter().for_each(|item| self.see(item)),
            VectorDiff::PushFront { value }
            | VectorDiff::PushBack { value }
            | VectorDiff::Insert { value, .. }
            | VectorDiff::Set { value, .. } => self.see(value),
            VectorDiff::PopFront
            | VectorDiff::PopBack
            | VectorDiff::Remove { .. }
            | VectorDiff::Truncate { .. } => {}
        }
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

/// Queues `message` in the room's send queue: a new message, a reply, or an
/// edit, which keeps its mentions in the new content. The local echo shows
/// the send state, so only a failure to queue is an error here.
pub(crate) async fn send_message(timeline: &Timeline, message: Outgoing) -> Result<(), Error> {
    enable_sending(timeline);
    let content = outgoing_content(&message.body);
    let queued = match message.kind {
        OutgoingKind::New => timeline
            .send(RoomMessageEventContent::from(content).into())
            .await
            .map(drop),
        OutgoingKind::Reply { to } => timeline.send_reply(content, event_id(&to)?).await.map(drop),
        OutgoingKind::Edit { target } => {
            let content = EditedContent::RoomMessage(content);
            timeline.edit(&item_id(&target)?, content).await
        }
    };
    queued.map_err(timeline_error)
}

/// Adds the user's reaction with `key` to `item`, or removes it if they
/// reacted with it.
pub(crate) async fn toggle_reaction(
    timeline: &Timeline,
    item: &ItemRef,
    key: &str,
) -> Result<(), Error> {
    enable_sending(timeline);
    timeline
        .toggle_reaction(&item_id(item)?, key)
        .await
        .map(drop)
        .map_err(timeline_error)
}

/// Redacts the user's own `item`, or stops sending it while it is a local
/// echo.
pub(crate) async fn redact(timeline: &Timeline, item: &ItemRef) -> Result<(), Error> {
    enable_sending(timeline);
    timeline
        .redact(&item_id(item)?, None)
        .await
        .map_err(timeline_error)
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
    enable_sending(timeline);
    Ok(())
}

/// The SDK disables a room's send queue after any failed request, and only
/// `Client::resume` enables it again, so every write the user starts enables
/// it; otherwise later messages, reactions and redactions would wait for
/// good. A request that failed with a recoverable error (the network, for
/// example) stays queued and goes out first, which keeps the order; one that
/// failed otherwise holds back the ones after it until it is resent or
/// deleted.
fn enable_sending(timeline: &Timeline) {
    timeline.room().send_queue().set_enabled(true);
}

/// The content of a message from the composer (design, "Matrix adapter",
/// Sending): an HTML body from the Markdown when it has any formatting, the
/// plain body alone otherwise, and `m.mentions` with the users still
/// mentioned.
fn outgoing_content(body: &RenderedBody) -> RoomMessageEventContentWithoutRelation {
    let content = match FormattedBody::markdown(&body.markdown) {
        Some(html) => RoomMessageEventContentWithoutRelation::text_html(&body.plain, html.body),
        None => RoomMessageEventContentWithoutRelation::text_plain(&body.plain),
    };
    if body.user_ids.is_empty() {
        return content;
    }
    // The core's user IDs come from the SDK's members, so they parse.
    let users = body
        .user_ids
        .iter()
        .filter_map(|user| ruma::UserId::parse(user.as_str()).ok());
    content.add_mentions(Mentions::with_user_ids(users))
}

fn timeline_diff(diff: VectorDiff<Arc<sdk::TimelineItem>>, me: &ruma::UserId) -> TimelineDiff {
    let entries = |items: matrix_sdk_ui::eyeball_im::Vector<Arc<sdk::TimelineItem>>| {
        items.iter().map(|item| entry(item, me)).collect()
    };
    match diff {
        VectorDiff::Append { values } => TimelineDiff::Append(entries(values)),
        VectorDiff::Clear => TimelineDiff::Clear,
        VectorDiff::PushFront { value } => TimelineDiff::PushFront(entry(&value, me)),
        VectorDiff::PushBack { value } => TimelineDiff::PushBack(entry(&value, me)),
        VectorDiff::PopFront => TimelineDiff::PopFront,
        VectorDiff::PopBack => TimelineDiff::PopBack,
        VectorDiff::Insert { index, value } => TimelineDiff::Insert(index, entry(&value, me)),
        VectorDiff::Set { index, value } => TimelineDiff::Set(index, entry(&value, me)),
        VectorDiff::Remove { index } => TimelineDiff::Remove(index),
        VectorDiff::Truncate { length } => TimelineDiff::Truncate(length),
        VectorDiff::Reset { values } => TimelineDiff::Reset(entries(values)),
    }
}

/// Every SDK item becomes one entry, so indices stay the SDK's: what the
/// first release does not show becomes `Hidden`.
fn entry(item: &sdk::TimelineItem, me: &ruma::UserId) -> TimelineEntry {
    match item.kind() {
        TimelineItemKind::Virtual(VirtualTimelineItem::DateDivider(day)) => {
            TimelineEntry::DateDivider(timestamp(*day))
        }
        TimelineItemKind::Virtual(VirtualTimelineItem::ReadMarker) => TimelineEntry::ReadMarker,
        TimelineItemKind::Virtual(VirtualTimelineItem::TimelineStart) => TimelineEntry::Hidden,
        TimelineItemKind::Event(event) => event_entry(event, me),
    }
}

/// Messages and other message-like events, calls included, are items; state
/// events and membership changes are hidden.
fn event_entry(event: &EventTimelineItem, me: &ruma::UserId) -> TimelineEntry {
    let (content, reply, reactions, edited) = match event.content() {
        TimelineItemContent::MsgLike(msg_like) => (
            msg_like_content(&msg_like.kind),
            msg_like.in_reply_to.as_ref().map(reply_preview),
            reaction_groups(&msg_like.reactions, me),
            matches!(&msg_like.kind, MsgLikeKind::Message(message) if message.is_edited()),
        ),
        other => match other_content(other) {
            Some(content) => (content, None, Vec::new(), false),
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
        MsgLikeKind::Sticker(sticker) => MessageContent::Sticker {
            body: sticker.content().body.clone(),
        },
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
            let dimension = |side: Option<ruma::UInt>| u32::try_from(u64::from(side?)).ok();
            MessageContent::Image {
                name: image.filename().to_owned(),
                size: info.and_then(|info| info.size).map(u64::from),
                dimensions: info
                    .and_then(|info| Some((dimension(info.width)?, dimension(info.height)?))),
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

/// The HTML body when there is one, otherwise the plain body. An HTML body
/// with nothing but blank lines falls back to the plain body, so the message
/// is not blank.
fn rich_body(body: &str, formatted: Option<&FormattedBody>) -> RichText {
    let html = formatted.filter(|formatted| formatted.format == MessageFormat::Html);
    match html.map(|html| render_html(&html.body)) {
        Some(rich) if !is_blank(&rich) => rich,
        _ => RichText::plain(body),
    }
}

fn is_blank(rich: &RichText) -> bool {
    rich.blocks.iter().all(|block| {
        matches!(block, Block::Paragraph(inlines)
            if inlines.iter().all(|inline| *inline == Inline::LineBreak))
    })
}

/// The replied-to event's sender and plain body, once the SDK has them.
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
        MsgLikeKind::Message(message) => message.body().to_owned(),
        kind => msg_like_content(kind).plain_text(),
    })
}

/// One group per key, in the SDK's order.
fn reaction_groups(reactions: &ReactionsByKeyBySender, me: &ruma::UserId) -> Vec<ReactionGroup> {
    reactions
        .iter()
        .map(|(key, senders)| ReactionGroup {
            key: key.clone(),
            count: u32::try_from(senders.len()).unwrap_or(u32::MAX),
            mine: senders.contains_key(me),
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
    use matrix_sdk::ruma::{
        OwnedMxcUri, UInt,
        events::{
            StateEventType,
            room::{
                ImageInfo,
                message::{
                    AudioInfo, AudioMessageEventContent, EmoteMessageEventContent, FileInfo,
                    FileMessageEventContent, ImageMessageEventContent, LocationMessageEventContent,
                    NoticeMessageEventContent, TextMessageEventContent,
                },
            },
        },
        owned_event_id, owned_user_id,
    };
    use matrix_sdk_ui::timeline::{EmbeddedEvent, MsgLikeContent, ReactionInfo};
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
    fn reactions_are_grouped_by_key_with_the_own_reaction() {
        let me = owned_user_id!("@me:example.org");
        let info = || ReactionInfo {
            timestamp: MilliSecondsSinceUnixEpoch(UInt::from(0u32)),
            send_state: None,
        };
        let mut reactions = ReactionsByKeyBySender::default();
        let thumbs = reactions.entry("👍".to_owned()).or_default();
        thumbs.insert(owned_user_id!("@alice:example.org"), info());
        thumbs.insert(owned_user_id!("@bob:example.org"), info());
        let party = reactions.entry("🎉".to_owned()).or_default();
        party.insert(owned_user_id!("@alice:example.org"), info());
        party.insert(me.clone(), info());
        assert_eq!(
            reaction_groups(&reactions, &me),
            vec![
                ReactionGroup {
                    key: "👍".into(),
                    count: 2,
                    mine: false,
                },
                ReactionGroup {
                    key: "🎉".into(),
                    count: 2,
                    mine: true,
                },
            ]
        );
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
        // Line breaks alone are no formatting.
        assert_eq!(
            sent("hello\nworld", &[]),
            json!({ "msgtype": "m.text", "body": "hello\nworld" })
        );
        // The plain body stays as typed, and line breaks stay in the HTML.
        assert_eq!(
            sent("**hi**\nthere", &[]),
            json!({
                "msgtype": "m.text",
                "body": "**hi**\nthere",
                "format": "org.matrix.custom.html",
                "formatted_body": "<strong>hi</strong><br />\nthere",
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
