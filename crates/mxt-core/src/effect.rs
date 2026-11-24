//! Effects: what `App::update` asks the adapters to do, as plain data.

use std::borrow::{Borrow, BorrowMut};
use std::fmt;
use std::ops::Range;

use crate::action::FocusMode;
use crate::ids::{EventId, RoomId, SendId, TimelineId, TxnId, UserId};
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
    /// Executed by the binary, which owns the files mxt keeps beside the
    /// SDK's stores.
    Store(StoreEffect),
}

/// What the binary writes to its files (design, "Functional core").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreEffect {
    /// Replaces the history file with both histories, each oldest first
    /// (design, "Line editing"). A newer one replaces one still waiting.
    SaveHistory {
        commands: Vec<String>,
        searches: Vec<String>,
    },
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

/// The recovery key (or passphrase) typed in `SECRET` mode, on its way to
/// the matrix adapter. `Debug` shows nothing of it, so effects can be
/// printed, in test failures for example, without it. The line editor edits
/// it in its own buffer while it is typed (design, "Line editing").
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryKey(String);

impl RecoveryKey {
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Empty or only whitespace: entering it skips recovery.
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl Borrow<String> for RecoveryKey {
    fn borrow(&self) -> &String {
        &self.0
    }
}

impl BorrowMut<String> for RecoveryKey {
    fn borrow_mut(&mut self) -> &mut String {
        &mut self.0
    }
}

impl From<String> for RecoveryKey {
    fn from(key: String) -> RecoveryKey {
        RecoveryKey(key)
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey(..)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatrixEffect {
    SubscribeTimeline(RoomId),
    UnsubscribeTimeline(RoomId),
    /// The thread of `room` whose root is `root`, shown in place of the
    /// room's main timeline: its diffs come as `Msg::Timeline` for the
    /// thread's `TimelineId`. Replaces a thread subscribed before.
    SubscribeThread {
        room: RoomId,
        root: EventId,
    },
    UnsubscribeThread {
        room: RoomId,
        root: EventId,
    },
    /// `room.mark_read`: a receipt for the room's latest event, and clear its
    /// manual unread mark.
    MarkRoomRead(RoomId),
    /// Load older events of the timeline, a room's main timeline or one of
    /// its threads; answered with `Msg::PaginationDone` or
    /// `Msg::PaginationFailed` for it.
    PaginateBackwards(TimelineId),
    /// A receipt of the configured kind and the fully read marker, both on `event`.
    SendReceipt {
        room: RoomId,
        event: EventId,
    },
    /// One receipt of the configured kind on `event`, the newest remote
    /// event of the thread rooted at `root` (design, "Threads"): unthreaded
    /// when `unthreaded` (the main timeline is read and older than `event`),
    /// else threaded with `root` as its `thread_id`. Never both: `/sync`
    /// carries one receipt per user, kind and event, and Synapse keeps the
    /// unthreaded one. The fully read marker stays the main timeline's.
    SendThreadReceipt {
        room: RoomId,
        root: EventId,
        event: EventId,
        unthreaded: bool,
    },
    /// Send through the timeline, a room's main timeline or one of its
    /// threads, which adds the thread relation; answered with
    /// `Msg::SendQueued` once the room's send queue has it, or
    /// `Msg::SendFailed` when it cannot be queued (the timeline is not
    /// open, for example). The local echo shows the send state after that.
    /// An edit of a sent message has no local echo, so it is answered when
    /// it leaves the queue: `SendQueued` once sent, `SendFailed` when
    /// refused.
    Send {
        timeline: TimelineId,
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
    /// Send the file at `path` as an image or file message into the
    /// timeline, with the thread relation for a thread.
    Upload {
        timeline: TimelineId,
        path: String,
    },
    /// The item's image for the message view, an image message's or a
    /// sticker's; answered with `Msg::ImageFetched` or
    /// `Msg::ImageUnavailable`.
    FetchImage {
        room: RoomId,
        item: ItemRef,
    },
    /// The item's image for its preview in the timeline (design, "Images in
    /// the timeline"), an image message's or a sticker's, a thumbnail when
    /// there is one; answered with `Msg::PreviewFetched`, or with
    /// `Msg::PreviewFailed` for any failure, never `Msg::EffectFailed`.
    FetchPreview {
        room: RoomId,
        item: ItemRef,
    },
    /// The thread that holds `event`, from the room's event cache without a
    /// request; answered with `Msg::ThreadFound`.
    FindThread {
        room: RoomId,
        event: EventId,
    },
    /// Import the secrets from secret storage with the key, which signs this
    /// device (design, "Startup"); answered with `Msg::Recovered`, or with
    /// `Msg::EffectFailed` for a wrong key or an account without secret
    /// storage.
    Recover(RecoveryKey),
    /// `:join`: join the room `target` names (an alias, a room ID or a
    /// matrix.to link to one), which accepts its invite, or find the direct
    /// message with the user it names; answered with `Msg::Joined`, with
    /// `Msg::NoDirectMessage` when no direct message is found, or with
    /// `Msg::EffectFailed`.
    Join {
        target: String,
    },
    /// Create an encrypted direct message with `user`, unless the lookup of
    /// `Join` finds one by now; answered with `Msg::Joined` with the user ID
    /// as the target, or with `Msg::EffectFailed`.
    CreateDm {
        user: UserId,
    },
}

/// What the message view shows. A message's `reactions` are one line per
/// reaction shown under it, in that order, with who reacted (design, "Who
/// reacted"), which the view appends after an empty line; empty for the
/// help views and for a message without reactions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewContent {
    /// The text for the scratch editor: a message's or a help view's. A
    /// message's has the byte ranges of the search's matches on it while
    /// the highlight is shown (design, "Search and jumps"); a help view's
    /// has none. The matches are in `text`, which comes first, never in the
    /// reaction lines.
    Text {
        text: String,
        matches: Vec<Range<usize>>,
        reactions: Vec<String>,
    },
    /// An image message or a sticker: its name (a sticker's body), size and
    /// dimensions, which the reaction lines follow, and the image once
    /// `UiEffect::ShowImage` brings it.
    Image {
        item: ItemRef,
        name: String,
        size: Option<u64>,
        dimensions: Option<(u32, u32)>,
        reactions: Vec<String>,
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
    /// Put the message view's editor in normal mode as edtui's `<Esc>`
    /// does; nothing in normal mode. A key waiting for the next one is
    /// cancelled.
    ViewNormal,
    /// The image of `item` for the open message view, which shows that item.
    ShowImage {
        item: ItemRef,
        bytes: MediaBytes,
    },
    /// The fetched image of `item` for its preview in the timeline (design,
    /// "Images in the timeline"), which the tui adapter decodes off the UI
    /// thread.
    ShowPreview {
        item: ItemRef,
        bytes: MediaBytes,
    },
    /// The preview of `item` could not be fetched: the item shows its text
    /// line alone until the shown timeline changes.
    PreviewFailed(ItemRef),
    /// Run `files.opener` with the path of a downloaded file appended.
    OpenPath(String),
    /// Run `files.opener` with the URL appended.
    OpenUrl(String),
    /// Copy to the clipboard (OSC 52) and the shared register.
    Yank(String),
    /// The timeline's composer gets the keys, in insert mode unless `mode`
    /// is `Keep`.
    FocusComposer {
        timeline: TimelineId,
        mode: FocusMode,
    },
    /// Read the timeline's composer text and answer `Msg::Submit`.
    RequestSubmit(TimelineId),
    /// Insert a line break at the composer's cursor.
    InsertNewline(TimelineId),
    /// Replace the composer's text (the message being edited).
    LoadComposer {
        timeline: TimelineId,
        text: String,
    },
    /// Empty the composer.
    ClearComposer(TimelineId),
    /// Replace the `@` token at the composer's cursor with `text` and a space.
    ReplaceMentionToken {
        timeline: TimelineId,
        text: String,
    },
    /// Edit the composer's text in `$VISUAL` or `$EDITOR`.
    ExternalEditor(TimelineId),
    /// Put the composer's editor in normal mode as edtui's `<Esc>` does;
    /// nothing in normal mode. A key waiting for the next one is cancelled.
    ComposerNormal(TimelineId),
    /// In insert mode, delete the word before the composer's cursor with
    /// the blanks after it (from `line::word_start`), or at the start of a
    /// line the line break, as one undo state without a register write;
    /// nothing in other modes.
    DeleteWordBefore(TimelineId),
    /// Empty the composer as one undo state without a register write,
    /// keeping its mode (a visual selection is dropped); nothing when it is
    /// empty. Unlike `ClearComposer`, the editor state stays, so `u` brings
    /// the text back.
    EmptyComposer(TimelineId),
    Quit,
}
