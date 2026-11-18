//! Messages: what the adapters report to `App::update`.

use crate::ids::RoomId;
use crate::keys::Key;
use crate::resolver::TimerId;
use crate::room::RoomSummary;
use crate::timeline::TimelineDiff;

/// The mode of the edtui editor that has focus, reported with every key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Normal,
    Visual,
    Insert,
    Search,
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
}
