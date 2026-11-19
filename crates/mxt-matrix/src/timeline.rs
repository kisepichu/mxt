//! The open room's timeline: the matrix-sdk-ui `Timeline` subscription, its
//! back pagination, and the conversion of its items into core entries.

use std::{pin::pin, sync::Arc};

use futures_util::{Stream, StreamExt as _};
use matrix_sdk::{
    Room,
    ruma::{
        self, MilliSecondsSinceUnixEpoch, OwnedUserId,
        events::room::message::{FormattedBody, MessageFormat, MessageType},
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
    ids::{EventId, RoomId, Timestamp, TxnId, UserId},
    msg::Msg,
    rich::RichText,
    timeline::{
        ItemRef, MessageContent, ReactionGroup, ReplyPreview, SendState, TimelineDiff,
        TimelineEntry, TimelineItem,
    },
};
use tokio::{
    sync::mpsc::UnboundedSender,
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

/// The open room's timeline, and its tasks: the one that forwards its diffs,
/// and a back pagination in flight.
pub(crate) struct Subscription {
    room: RoomId,
    timeline: Arc<Timeline>,
    tasks: JoinSet<()>,
}

impl Subscription {
    /// Builds `sdk_room`'s timeline, sends its items as one `Msg::Timeline`
    /// with a `Reset`, then every batch of its diffs as one `Msg::Timeline`.
    /// The SDK's diffs are forwarded as they are: a remote echo replaces its
    /// local echo with `Remove` and `Insert` or `PushBack`.
    pub(crate) async fn start(
        sdk_room: &Room,
        room: RoomId,
        to_core: UnboundedSender<Msg>,
    ) -> Result<Subscription, Error> {
        let timeline = Arc::new(sdk_room.timeline().await.map_err(timeline_error)?);
        let me = sdk_room.own_user_id().to_owned();
        let (items, diffs) = timeline.subscribe().await;
        let entries = items.iter().map(|item| entry(item, &me)).collect();
        let reset = vec![TimelineDiff::Reset(entries)];
        let _ = to_core.send(Msg::Timeline {
            room: room.clone(),
            diffs: reset,
        });
        let mut tasks = JoinSet::new();
        tasks.spawn(forward(room.clone(), me, diffs, to_core));
        Ok(Subscription {
            room,
            timeline,
            tasks,
        })
    }

    pub(crate) fn room(&self) -> &RoomId {
        &self.room
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
    to_core: UnboundedSender<Msg>,
) {
    let mut diffs = pin!(diffs);
    while let Some(batch) = diffs.next().await {
        let diffs = batch
            .into_iter()
            .map(|diff| timeline_diff(diff, &me))
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

/// Messages and other message-like events are items; state events,
/// membership changes and calls are hidden.
fn event_entry(event: &EventTimelineItem, me: &ruma::UserId) -> TimelineEntry {
    let (content, reply, reactions, edited) = match event.content() {
        TimelineItemContent::MsgLike(msg_like) => (
            msg_like_content(&msg_like.kind),
            msg_like.in_reply_to.as_ref().map(reply_preview),
            reaction_groups(&msg_like.reactions, me),
            matches!(&msg_like.kind, MsgLikeKind::Message(message) if message.is_edited()),
        ),
        TimelineItemContent::FailedToParseMessageLike { event_type, .. } => {
            let kind = event_type.to_string();
            (
                MessageContent::Unsupported { kind },
                None,
                Vec::new(),
                false,
            )
        }
        _ => return TimelineEntry::Hidden,
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
        _ => MessageContent::Text {
            body: body.to_owned(),
            rich: RichText::plain(body),
        },
    }
}

/// The HTML body when there is one, otherwise the plain body. An HTML body
/// without any text falls back to the plain body, so the message is not
/// blank.
fn rich_body(body: &str, formatted: Option<&FormattedBody>) -> RichText {
    let html = formatted.filter(|formatted| formatted.format == MessageFormat::Html);
    match html.map(|html| render_html(&html.body)) {
        Some(rich) if !rich.blocks.is_empty() => rich,
        _ => RichText::plain(body),
    }
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
        events::room::{
            ImageInfo,
            message::{
                AudioInfo, AudioMessageEventContent, EmoteMessageEventContent, FileInfo,
                FileMessageEventContent, ImageMessageEventContent, LocationMessageEventContent,
                NoticeMessageEventContent, TextMessageEventContent,
            },
        },
        owned_event_id, owned_user_id,
    };
    use matrix_sdk_ui::timeline::{EmbeddedEvent, MsgLikeContent, ReactionInfo};
    use mxt_core::rich::{Block, Inline, Style};

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
        // An HTML body without text would leave the message blank.
        let blank = TextMessageEventContent::html("fallback", "<p> </p>");
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
    fn message_types_mxt_does_not_show_fall_back_to_their_body() {
        let location =
            LocationMessageEventContent::new("Big Ben".into(), "geo:51.5008,0.1247".into());
        assert_eq!(
            content(MessageType::Location(location)),
            text("Big Ben", RichText::plain("Big Ben"))
        );
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
        let replied = |profile| {
            TimelineDetails::Ready(Box::new(EmbeddedEvent {
                content: TimelineItemContent::MsgLike(MsgLikeContent::redacted()),
                sender: owned_user_id!("@bob:example.org"),
                sender_profile: profile,
                timestamp: MilliSecondsSinceUnixEpoch(UInt::from(0u32)),
                identifier: TimelineEventItemId::EventId(owned_event_id!("$event:example.org")),
            }))
        };
        let reply = |event| InReplyToDetails {
            event_id: owned_event_id!("$event:example.org"),
            event,
        };
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
}
