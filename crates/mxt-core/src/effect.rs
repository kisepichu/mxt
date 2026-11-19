//! Effects: what `App::update` asks the adapters to do, as plain data.

use crate::action::FocusMode;
use crate::ids::{EventId, RoomId, TxnId};
use crate::keys::Key;
use crate::mention::RenderedBody;
use crate::resolver::TimerId;
use crate::timeline::ItemRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Executed by the matrix adapter.
    Matrix(MatrixEffect),
    /// Executed by the tui adapter.
    Ui(UiEffect),
}

/// A message to send from the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub body: RenderedBody,
    pub kind: OutgoingKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutgoingKind {
    New,
    /// A reply needs the replied-to event's ID, so a local echo cannot be
    /// replied to until it is sent.
    Reply {
        to: EventId,
    },
    /// Replaces the content of the user's own `target`.
    Edit {
        target: ItemRef,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatrixEffect {
    SubscribeTimeline(RoomId),
    UnsubscribeTimeline(RoomId),
    /// `room.mark_read`: a receipt for the room's latest event, and clear its
    /// manual unread mark.
    MarkRoomRead(RoomId),
    /// Load older events of the open room's timeline; answered with
    /// `Msg::PaginationDone` or `Msg::PaginationFailed`.
    PaginateBackwards(RoomId),
    /// A receipt of the configured kind and the fully read marker, both on `event`.
    SendReceipt {
        room: RoomId,
        event: EventId,
    },
    /// Send through the room's timeline; the local echo shows the send
    /// state, and a failure to queue it arrives as `Msg::EffectFailed`.
    Send {
        room: RoomId,
        message: Outgoing,
    },
    /// The room's joined members; answered with `Msg::MembersLoaded`.
    LoadMembers(RoomId),
    /// Delete (redact) the user's own item.
    Redact {
        room: RoomId,
        item: ItemRef,
    },
    /// Add the user's reaction with `key`, or remove it if they sent it.
    ToggleReaction {
        room: RoomId,
        item: ItemRef,
        key: String,
    },
    /// Send a failed local echo again.
    Resend {
        room: RoomId,
        txn: TxnId,
    },
}

/// What the message view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewContent {
    /// The message's text for the scratch editor.
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEffect {
    /// Report `Msg::Timeout { id }` after `after_ms` milliseconds.
    StartTimer {
        id: TimerId,
        after_ms: u64,
    },
    /// Keys for the focused editor (the composer's or the message view's), in order.
    ReplayKeys(Vec<Key>),
    /// Open the full-screen message view.
    OpenView(ViewContent),
    /// Close the message view; edits in its editor are discarded.
    CloseView,
    /// Copy to the clipboard (OSC 52) and the shared register.
    Yank(String),
    /// The room's composer gets the keys, in insert mode unless `mode` is `Keep`.
    FocusComposer {
        room: RoomId,
        mode: FocusMode,
    },
    /// Read the room's composer text and answer `Msg::Submit`.
    RequestSubmit(RoomId),
    /// Insert a line break at the composer's cursor.
    InsertNewline(RoomId),
    /// Replace the composer's text (the message being edited).
    LoadComposer {
        room: RoomId,
        text: String,
    },
    /// Empty the composer.
    ClearComposer(RoomId),
    /// Replace the `@` token at the composer's cursor with `text` and a space.
    ReplaceMentionToken {
        room: RoomId,
        text: String,
    },
    /// Edit the composer's text in `$VISUAL` or `$EDITOR`.
    ExternalEditor(RoomId),
    Quit,
}
