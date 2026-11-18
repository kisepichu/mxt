//! Effects: what `App::update` asks the adapters to do, as plain data.

use crate::ids::RoomId;
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
    Quit,
}
