//! Effects: what `App::update` asks the adapters to do, as plain data.

use crate::action::FocusMode;
use crate::ids::{EventId, RoomId, SendId, TxnId};
use crate::keys::Key;
use crate::mention::RenderedBody;
use crate::resolver::TimerId;
use crate::timeline::{ItemRef, MediaBytes};

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
    /// Send through the room's timeline; answered with `Msg::SendQueued`
    /// once the room's send queue has it, or `Msg::SendFailed` when it
    /// cannot be queued. The local echo shows the send state after that. An
    /// edit of a sent message has no local echo, so it is answered when it
    /// leaves the queue: `SendQueued` once sent, `SendFailed` when refused.
    Send {
        room: RoomId,
        id: SendId,
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
    /// Mute the room, or remove its room rule so the account default applies
    /// again (design, "Mute"). The room's mode changes when the SDK reports it.
    SetMute {
        room: RoomId,
        muted: bool,
    },
    /// Save the item's image or file in the download directory; answered with
    /// `Msg::Downloaded`.
    Download {
        room: RoomId,
        item: ItemRef,
    },
    /// Send the file at `path` as an image or file message.
    Upload {
        room: RoomId,
        path: String,
    },
    /// The item's image for the message view; answered with
    /// `Msg::ImageFetched` or `Msg::ImageUnavailable`.
    FetchImage {
        room: RoomId,
        item: ItemRef,
    },
}

/// What the message view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewContent {
    /// The message's text for the scratch editor.
    Text(String),
    /// An image message: its name, size and dimensions, and the image once
    /// `UiEffect::ShowImage` brings it.
    Image {
        item: ItemRef,
        name: String,
        size: Option<u64>,
        dimensions: Option<(u32, u32)>,
    },
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
    /// Pasted text for the focused editor (the composer's or the message
    /// view's), inserted as text rather than keys; line breaks are `\n`.
    Paste(String),
    /// Open the full-screen message view.
    OpenView(ViewContent),
    /// Close the message view; edits in its editor are discarded.
    CloseView,
    /// The image of `item` for the open message view, which shows that item.
    ShowImage {
        item: ItemRef,
        bytes: MediaBytes,
    },
    /// Run `files.opener` with the path of a downloaded file appended.
    OpenPath(String),
    /// Run `files.opener` with the URL appended.
    OpenUrl(String),
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
