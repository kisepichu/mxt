//! Effects: what `App::update` asks the adapters to do, as plain data.

use crate::ids::{EventId, RoomId};
use crate::keys::Key;
use crate::resolver::TimerId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Executed by the matrix adapter.
    Matrix(MatrixEffect),
    /// Executed by the tui adapter.
    Ui(UiEffect),
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
    Quit,
}
