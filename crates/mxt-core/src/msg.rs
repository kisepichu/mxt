//! Messages: what the adapters report to `App::update`.

use crate::ids::{RoomId, SendId};
use crate::keys::Key;
use crate::mention::Member;
use crate::resolver::TimerId;
use crate::room::RoomSummary;
use crate::timeline::{ItemRef, MediaBytes, TimelineDiff};

/// The mode of the edtui editor that has focus, reported with every key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Normal,
    Visual,
    Insert,
    Search,
    /// Normal or visual mode while a key waits for its character (`f`,
    /// `t`, `r`): the next key goes to the editor without the keymap. Only
    /// keys carry it; the status line shows the editor's own mode.
    AwaitingChar,
}

/// A pane that shows a list of items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    RoomList,
    Timeline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SyncState {
    #[default]
    Starting,
    Synced,
    Offline,
    Retrying,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// `editor` is `Some` while an editor (composer or message view) has focus.
    Key {
        key: Key,
        editor: Option<EditorMode>,
    },
    /// Text pasted into the terminal (bracketed paste), with its line breaks
    /// as `\n`; `editor` as with `Key`.
    Paste {
        text: String,
        editor: Option<EditorMode>,
    },
    /// A timer from `UiEffect::StartTimer` expired.
    Timeout {
        id: TimerId,
    },
    RoomUpserted(RoomSummary),
    RoomRemoved(RoomId),
    SyncState(SyncState),
    /// Changes of a room's timeline, in order; ignored unless the room is open.
    Timeline {
        room: RoomId,
        diffs: Vec<TimelineDiff>,
    },
    /// A `PaginateBackwards` request finished.
    PaginationDone {
        room: RoomId,
        reached_start: bool,
    },
    PaginationFailed {
        room: RoomId,
        error: String,
    },
    /// Sent by the tui adapter after a render changed which items are visible;
    /// `first` and `last` are inclusive indices of the pane's items, which for
    /// the timeline are its entries (`App::timeline`).
    Viewport {
        pane: Pane,
        first: usize,
        last: usize,
    },
    TerminalFocus(bool),
    EffectFailed {
        error: String,
    },
    /// The message of `MatrixEffect::Send` is safe without its draft: the
    /// room's send queue has it, and its local echo shows it, or, for an
    /// edit of a sent message, which has no local echo of its own, the
    /// homeserver accepted it.
    SendQueued {
        id: SendId,
    },
    /// The message of `MatrixEffect::Send` could not be queued, or the
    /// homeserver refused an edit of a sent message for good; `error` is for
    /// the status line.
    SendFailed {
        id: SendId,
        error: String,
    },
    /// The composer's text, answering `UiEffect::RequestSubmit`.
    Submit {
        room: RoomId,
        body: String,
    },
    /// The room's composer became empty or non-empty.
    ComposerEdited {
        room: RoomId,
        empty: bool,
    },
    /// The `@` token at the composer's cursor in insert mode changed;
    /// `query` is the text after `@`, `None` without a token.
    MentionQuery {
        room: RoomId,
        query: Option<String>,
    },
    /// The room's members, answering `MatrixEffect::LoadMembers`.
    MembersLoaded {
        room: RoomId,
        members: Vec<Member>,
    },
    /// A `MatrixEffect::Download` saved the file at `path`.
    Downloaded {
        path: String,
    },
    /// The image of `item`, answering `MatrixEffect::FetchImage`.
    ImageFetched {
        item: ItemRef,
        bytes: MediaBytes,
    },
    /// `MatrixEffect::FetchImage` for `item` brought no image, for example
    /// because the file is too large to preview.
    ImageUnavailable {
        item: ItemRef,
        reason: String,
    },
    /// `MatrixEffect::Recover` imported the secrets.
    Recovered,
}
