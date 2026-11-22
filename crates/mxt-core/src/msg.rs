//! Messages: what the adapters report to `App::update`.

use crate::ids::{EventId, RoomId, SendId, TimelineId, UserId};
use crate::keys::Key;
use crate::mention::Member;
use crate::resolver::TimerId;
use crate::room::{Invite, RoomSummary};
use crate::timeline::{ItemRef, MediaBytes, TimelineDiff};

/// The mode of the edtui editor that has focus, reported with every key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Normal,
    Visual,
    Insert,
    Search,
    /// Normal or visual mode while a key waits for the next one: `f`, `t`
    /// or `r` for its character, an operator (`d`, `c`, `y`) for the rest
    /// of its command, a text object's `i` or `a` for the object, `g` for
    /// `gg`, `"` for the register's name; and after `"_` until its command
    /// ends. The next key goes to the editor without the keymap. Only keys
    /// carry it; the status line shows the editor's own mode.
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

/// How the user is in the room a `MatrixEffect::Join` or
/// `MatrixEffect::CreateDm` answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinedHow {
    /// The user was in the room before.
    Already,
    /// The user joined it, or accepted its invite.
    Joined,
    /// A new direct message.
    Created,
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
    /// Changes of a timeline, in order; ignored unless it is the open room's
    /// main timeline or its open thread.
    Timeline {
        timeline: TimelineId,
        diffs: Vec<TimelineDiff>,
    },
    /// A `PaginateBackwards` request for `timeline` finished.
    PaginationDone {
        timeline: TimelineId,
        reached_start: bool,
    },
    PaginationFailed {
        timeline: TimelineId,
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
        timeline: TimelineId,
        body: String,
    },
    /// The timeline's composer became empty or non-empty.
    ComposerEdited {
        timeline: TimelineId,
        empty: bool,
    },
    /// The `@` token at the composer's cursor in insert mode changed;
    /// `query` is the text after `@`, `None` without a token.
    MentionQuery {
        timeline: TimelineId,
        query: Option<String>,
    },
    /// An editor, the composer's or the message view's, refused a key, for
    /// the status line as an error, not logged: a register other than the
    /// black hole register `"_`.
    EditorRefused(String),
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
    /// The tui adapter's list of the previews it wants for the shown
    /// timeline (design, "Images in the timeline"): the remote image items
    /// on or near the timeline pane whose previews it neither keeps nor is
    /// decoding nor has seen fail, those on the pane first, then the others
    /// by their distance from it. Sent when the list changes; it replaces
    /// the one before.
    PreviewsWanted {
        timeline: TimelineId,
        items: Vec<ItemRef>,
    },
    /// `MatrixEffect::Recover` imported the secrets.
    Recovered,
    /// The configuration's warnings, each as the log has it, sent once
    /// right after `App::new`: each goes into the status line's history
    /// without being shown, then the status line says how many settings
    /// were ignored (design, "Layers").
    ConfigWarnings {
        warnings: Vec<String>,
    },
    /// The pending invites, sorted by name; replaces the ones before.
    Invites(Vec<Invite>),
    /// The room of `MatrixEffect::Join` or `MatrixEffect::CreateDm` with
    /// `target` (for `CreateDm`, the user ID).
    Joined {
        target: String,
        room: RoomId,
        how: JoinedHow,
    },
    /// `MatrixEffect::Join` with `target`, a user ID or a link to one, found
    /// no direct message with `user`.
    NoDirectMessage {
        target: String,
        user: UserId,
    },
    /// The thread that holds `event` in `room`, answering
    /// `MatrixEffect::FindThread`: its root, or `None` for an event in no
    /// thread or one the event cache does not have.
    ThreadFound {
        room: RoomId,
        event: EventId,
        root: Option<EventId>,
    },
}
