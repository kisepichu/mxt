//! The open room's timeline as plain data: entries, their items and content,
//! and the diffs the matrix adapter forwards from the SDK.

use crate::ids::{EventId, Timestamp, TxnId, UserId};
use crate::rich::RichText;

/// A timeline item: a local echo before the server assigns an event ID, or a
/// remote event. Mirrors the SDK's `TimelineEventItemId`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemRef {
    Local(TxnId),
    Remote(EventId),
}

/// The send state of a local echo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendState {
    Sending,
    Sent,
    Failed { error: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyPreview {
    pub event: EventId,
    /// `None` while the replied-to event is not available.
    pub sender_name: Option<String>,
    pub body: Option<String>,
}

/// The reactions with one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionGroup {
    pub key: String,
    pub count: u32,
    /// Whether the user is among the senders.
    pub mine: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageContent {
    Text {
        /// The event's plain `body`, without a reply fallback. For messages
        /// mxt sent, it is the Markdown the user typed; `message.edit` loads
        /// it.
        body: String,
        /// The formatted body, or `body` as plain text without one.
        rich: RichText,
    },
    Notice(RichText),
    Emote(RichText),
    Image {
        name: String,
        size: Option<u64>,
        dimensions: Option<(u32, u32)>,
    },
    /// Also video and audio.
    File {
        name: String,
        size: Option<u64>,
        mime: Option<String>,
    },
    Sticker {
        body: String,
    },
    Redacted,
    Undecryptable,
    Unsupported {
        kind: String,
    },
}

impl MessageContent {
    /// Searchable and yankable text: the rich text's plain form (what the
    /// timeline shows, not a text message's Markdown `body`), the file or
    /// image name, the sticker body; `(deleted)`, `(unable to decrypt)`,
    /// `(unsupported: <kind>)` for the others.
    pub fn plain_text(&self) -> String {
        match self {
            MessageContent::Text { rich, .. }
            | MessageContent::Notice(rich)
            | MessageContent::Emote(rich) => rich.to_plain(),
            MessageContent::Image { name, .. } | MessageContent::File { name, .. } => name.clone(),
            MessageContent::Sticker { body } => body.clone(),
            MessageContent::Redacted => "(deleted)".into(),
            MessageContent::Undecryptable => "(unable to decrypt)".into(),
            MessageContent::Unsupported { kind } => format!("(unsupported: {kind})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineItem {
    pub id: ItemRef,
    pub sender: UserId,
    pub sender_name: String,
    pub timestamp: Timestamp,
    pub content: MessageContent,
    pub reply: Option<ReplyPreview>,
    pub reactions: Vec<ReactionGroup>,
    pub edited: bool,
    /// `Some` for local echoes.
    pub send_state: Option<SendState>,
}

/// One entry of the timeline, in the SDK's order (oldest first).
#[derive(Clone, Debug, PartialEq, Eq)]
// Most entries are items, so boxing them would cost an allocation per item
// and save little memory.
#[allow(clippy::large_enum_variant)]
pub enum TimelineEntry {
    Item(TimelineItem),
    /// Start of a day; the timestamp of the first item of that day.
    DateDivider(Timestamp),
    ReadMarker,
    /// Anything the first release does not show (state events, membership
    /// changes). Kept so indices match the SDK's.
    Hidden,
}

/// Mirrors eyeball-im's `VectorDiff`, so the adapter can forward SDK diffs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineDiff {
    Append(Vec<TimelineEntry>),
    Clear,
    PushFront(TimelineEntry),
    PushBack(TimelineEntry),
    PopFront,
    PopBack,
    Insert(usize, TimelineEntry),
    Set(usize, TimelineEntry),
    Remove(usize),
    Truncate(usize),
    Reset(Vec<TimelineEntry>),
}

/// Applies one diff. Out-of-range indices are ignored (and are a bug in the
/// sender), never a panic.
pub fn apply_diff(entries: &mut Vec<TimelineEntry>, diff: TimelineDiff) {
    match diff {
        TimelineDiff::Append(new) => entries.extend(new),
        TimelineDiff::Clear => entries.clear(),
        TimelineDiff::PushFront(entry) => entries.insert(0, entry),
        TimelineDiff::PushBack(entry) => entries.push(entry),
        TimelineDiff::PopFront => {
            if !entries.is_empty() {
                entries.remove(0);
            }
        }
        TimelineDiff::PopBack => {
            entries.pop();
        }
        TimelineDiff::Insert(index, entry) => {
            if index <= entries.len() {
                entries.insert(index, entry);
            }
        }
        TimelineDiff::Set(index, entry) => {
            if let Some(slot) = entries.get_mut(index) {
                *slot = entry;
            }
        }
        TimelineDiff::Remove(index) => {
            if index < entries.len() {
                entries.remove(index);
            }
        }
        TimelineDiff::Truncate(len) => entries.truncate(len),
        TimelineDiff::Reset(new) => *entries = new,
    }
}

/// An item sent by the current user (design, "Functional core"). The sender
/// is taken from the item itself, so a caller cannot pass the wrong one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnEvent(ItemRef);

impl OwnEvent {
    pub fn new(item: &TimelineItem, me: &UserId) -> Option<OwnEvent> {
        (item.sender == *me).then(|| OwnEvent(item.id.clone()))
    }

    pub fn item(&self) -> &ItemRef {
        &self.0
    }
}

/// What the composer's text becomes when it is sent (design, "Functional
/// core"). Replying and editing at the same time, editing another user's
/// message, and replying to a local echo (which has no event ID to reply to)
/// cannot be expressed.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Draft {
    #[default]
    New,
    Reply {
        to: EventId,
    },
    Edit {
        target: OwnEvent,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: ItemRef, sender: &str) -> TimelineItem {
        TimelineItem {
            id,
            sender: UserId::new(sender),
            sender_name: sender.into(),
            timestamp: Timestamp::from_millis(0),
            content: MessageContent::Text {
                body: "hi".into(),
                rich: RichText::plain("hi"),
            },
            reply: None,
            reactions: Vec::new(),
            edited: false,
            send_state: None,
        }
    }
    /// Distinct entries, told apart by `n`.
    fn entries(ns: &[u64]) -> Vec<TimelineEntry> {
        ns.iter()
            .map(|&n| TimelineEntry::DateDivider(Timestamp::from_millis(n)))
            .collect()
    }
    fn applied(start: &[u64], diff: TimelineDiff) -> Vec<TimelineEntry> {
        let mut entries = entries(start);
        apply_diff(&mut entries, diff);
        entries
    }
    fn entry(n: u64) -> TimelineEntry {
        TimelineEntry::DateDivider(Timestamp::from_millis(n))
    }

    #[test]
    fn plain_text_of_each_content_variant() {
        let cases = [
            (
                MessageContent::Text {
                    body: "a\nb".into(),
                    rich: RichText::plain("a\nb"),
                },
                "a\nb",
            ),
            // The rendered text, not the Markdown source.
            (
                MessageContent::Text {
                    body: "**bold**".into(),
                    rich: RichText::plain("bold"),
                },
                "bold",
            ),
            (MessageContent::Notice(RichText::plain("notice")), "notice"),
            (MessageContent::Emote(RichText::plain("waves")), "waves"),
            (
                MessageContent::Image {
                    name: "cat.png".into(),
                    size: Some(1024),
                    dimensions: Some((640, 480)),
                },
                "cat.png",
            ),
            (
                MessageContent::File {
                    name: "notes.pdf".into(),
                    size: None,
                    mime: Some("application/pdf".into()),
                },
                "notes.pdf",
            ),
            (
                MessageContent::Sticker {
                    body: "a happy cat".into(),
                },
                "a happy cat",
            ),
            (MessageContent::Redacted, "(deleted)"),
            (MessageContent::Undecryptable, "(unable to decrypt)"),
            (
                MessageContent::Unsupported {
                    kind: "m.poll.start".into(),
                },
                "(unsupported: m.poll.start)",
            ),
        ];
        for (content, plain) in cases {
            assert_eq!(content.plain_text(), plain, "{content:?}");
        }
    }

    #[test]
    fn apply_diff_for_every_variant() {
        use TimelineDiff::*;
        let cases = [
            (Append(entries(&[3, 4])), &[1, 2, 3, 4][..]),
            (Clear, &[]),
            (PushFront(entry(0)), &[0, 1, 2]),
            (PushBack(entry(3)), &[1, 2, 3]),
            (PopFront, &[2]),
            (PopBack, &[1]),
            (Insert(1, entry(9)), &[1, 9, 2]),
            (Insert(2, entry(9)), &[1, 2, 9]),
            (Set(1, entry(9)), &[1, 9]),
            (Remove(0), &[2]),
            (Truncate(1), &[1]),
            (Truncate(5), &[1, 2]),
            (Reset(entries(&[7])), &[7]),
        ];
        for (diff, expected) in cases {
            let label = format!("{diff:?}");
            assert_eq!(applied(&[1, 2], diff), entries(expected), "{label}");
        }
    }

    #[test]
    fn apply_diff_ignores_out_of_range_indices() {
        use TimelineDiff::*;
        for diff in [Insert(3, entry(9)), Set(2, entry(9)), Remove(2)] {
            let label = format!("{diff:?}");
            assert_eq!(applied(&[1, 2], diff), entries(&[1, 2]), "{label}");
        }
        for diff in [PopFront, PopBack, Set(0, entry(9)), Remove(0)] {
            let label = format!("{diff:?}");
            assert_eq!(applied(&[], diff), entries(&[]), "{label}");
        }
    }

    #[test]
    fn own_event_only_for_the_users_own_items() {
        let me = UserId::new("@alice:example.org");
        let local = ItemRef::Local(TxnId::new("txn1"));
        let remote = ItemRef::Remote(EventId::new("$event1:example.org"));
        for id in [local, remote] {
            let own = OwnEvent::new(&item(id.clone(), "@alice:example.org"), &me);
            assert_eq!(own.as_ref().map(OwnEvent::item), Some(&id));
            assert_eq!(OwnEvent::new(&item(id, "@bob:example.org"), &me), None);
        }
    }
}
