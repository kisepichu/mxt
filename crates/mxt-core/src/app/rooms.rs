//! Room list updates and the room list actions.

use std::cmp::Ordering;

use super::{App, Focus, Motion, StatusKind};
use crate::effect::{Effect, MatrixEffect};
use crate::ids::RoomId;
use crate::msg::Pane;
use crate::room::{NotificationMode, RoomSummary};
use crate::rooms::{RoomTarget, resolve_room};

/// When no room matches a target or the picker's query.
pub(super) const NO_MATCHING_ROOM: &str = "No matching room";

/// Whether opening another room records a jump.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RoomSwitch {
    /// An action or an answer opens the room, by whatever path: the
    /// position before the switch goes into the jump list.
    Recorded,
    /// The jump list goes to one of its entries.
    FromJumpList,
}

impl App {
    /// The cursor stays on its room when the order changes.
    pub(super) fn upsert_room(&mut self, summary: RoomSummary) {
        let (old, new) = self.rooms.upsert(summary);
        if let Some(cursor) = self.room_cursor {
            self.room_cursor = Some(follow(cursor, old, new));
        }
    }

    /// Optimistic clearing (design, "Matrix adapter"): the room's counts go
    /// to zero at once, and the room moves when the sort keys depend on
    /// them, the cursor staying on its room.
    pub(super) fn clear_room_unread(&mut self, id: &RoomId) {
        if let Some((old, new)) = self.rooms.clear_unread(id)
            && let Some(cursor) = self.room_cursor
        {
            self.room_cursor = Some(follow(cursor, Some(old), new));
        }
    }

    /// A cursor on the removed room moves to the room now at its index.
    pub(super) fn remove_room(&mut self, id: RoomId, effects: &mut Vec<Effect>) {
        if let Some((index, _)) = self.rooms.remove(&id)
            && let Some(cursor) = self.room_cursor
        {
            self.room_cursor = match cursor.cmp(&index) {
                Ordering::Less => Some(cursor),
                Ordering::Greater => Some(cursor - 1),
                Ordering::Equal => self.rooms.len().checked_sub(1).map(|last| cursor.min(last)),
            };
        }
        if let Some(open) = self.open_room.take_if(|open| *open == id) {
            self.reset_timeline(effects);
            effects.push(Effect::Matrix(MatrixEffect::UnsubscribeTimeline(open)));
        }
    }

    /// Moves the room list cursor, clamped, without wrapping.
    pub(super) fn move_room_cursor(&mut self, motion: Motion) {
        let (Some(cursor), Some(last)) = (self.room_cursor(), self.rooms.len().checked_sub(1))
        else {
            return;
        };
        let half_pages =
            |count: u32| (count as usize).saturating_mul(self.half_page(Pane::RoomList));
        let target = match motion {
            Motion::Down(count) => cursor.saturating_add(count as usize),
            Motion::Up(count) => cursor.saturating_sub(count as usize),
            Motion::HalfPageDown(count) => cursor.saturating_add(half_pages(count)),
            Motion::HalfPageUp(count) => cursor.saturating_sub(half_pages(count)),
            Motion::First => 0,
            Motion::Last => last,
        };
        self.room_cursor = Some(target.min(last));
    }

    pub(super) fn open_selected(&mut self, effects: &mut Vec<Effect>) {
        if let Some(index) = self.room_cursor() {
            self.open_at(index, RoomSwitch::Recorded, effects);
        }
    }

    pub(super) fn open_target(&mut self, target: &RoomTarget, effects: &mut Vec<Effect>) {
        match resolve_room(&self.rooms, self.open_room.as_ref(), target) {
            Some(id) => self.open_id(&id, effects),
            None => self.set_status(StatusKind::Error, NO_MATCHING_ROOM),
        }
    }

    /// Opens the room with `id` as `room.open` does.
    pub(super) fn open_id(&mut self, id: &RoomId, effects: &mut Vec<Effect>) {
        match self.rooms.position(id) {
            Some(index) => self.open_at(index, RoomSwitch::Recorded, effects),
            None => self.set_status(StatusKind::Error, NO_MATCHING_ROOM),
        }
    }

    /// Opens the room at `index` of the room list: unsubscribes the previous
    /// room, subscribes the new one with a new timeline, and moves the cursor
    /// to it. An open room only gets the focus. This is the one place where
    /// the open room becomes another room, so a recorded switch puts the
    /// position before it into the jump list whichever action or answer
    /// opens the room (design, "Jump list").
    pub(super) fn open_at(&mut self, index: usize, switch: RoomSwitch, effects: &mut Vec<Effect>) {
        let id = &self.rooms.rooms()[index].id;
        if self.open_room.as_ref() != Some(id) {
            let id = id.clone();
            if switch == RoomSwitch::Recorded {
                self.record_jump();
            }
            if let Some(previous) = self.open_room.replace(id.clone()) {
                effects.push(Effect::Matrix(MatrixEffect::UnsubscribeTimeline(previous)));
            }
            self.reset_timeline(effects);
            effects.push(Effect::Matrix(MatrixEffect::SubscribeTimeline(id)));
            self.room_cursor = Some(index);
        }
        self.focus = Focus::Timeline;
    }

    /// The room that `room.mark_read`, `room.mute` and `room.unmute` act on:
    /// the room under the cursor when the room list has focus, otherwise the
    /// open room.
    fn target_room(&self) -> Option<&RoomSummary> {
        match self.focus {
            Focus::RoomList => self.room_cursor().map(|index| &self.rooms.rooms()[index]),
            Focus::Timeline | Focus::Composer => {
                self.open_room.as_ref().and_then(|id| self.rooms.get(id))
            }
        }
    }

    /// `room.mark_read`. The room's counts are cleared at once (design,
    /// "Optimistic clearing"), so a following action sees it read.
    pub(super) fn mark_read(&mut self, effects: &mut Vec<Effect>) {
        let Some(id) = self.target_room().map(|room| room.id.clone()) else {
            return self.set_status(StatusKind::Error, "No room");
        };
        self.clear_room_unread(&id);
        effects.push(Effect::Matrix(MatrixEffect::MarkRoomRead(id)));
    }

    /// `room.mute` / `room.unmute`. The room's mode label changes when the
    /// SDK reports the new mode. Unmuting removes the room's rule, so
    /// `room.unmute` on a room that is not muted does nothing: a "Mentions
    /// only" rule set elsewhere stays.
    pub(super) fn set_mute(&mut self, muted: bool, effects: &mut Vec<Effect>) {
        let Some(room) = self.target_room() else {
            return self.set_status(StatusKind::Error, "No room");
        };
        if !muted && room.mode != NotificationMode::Mute {
            let status = format!("{} is not muted", room.name);
            return self.set_status(StatusKind::Error, status);
        }
        let verb = if muted { "Muted" } else { "Unmuted" };
        let status = format!("{verb} {}", room.name);
        effects.push(Effect::Matrix(MatrixEffect::SetMute {
            room: room.id.clone(),
            muted,
        }));
        self.set_status(StatusKind::Info, status);
    }
}

/// The cursor index after the room at `old` (`None` for a new room) moved to
/// `new`, such that the cursor stays on its room.
fn follow(cursor: usize, old: Option<usize>, new: usize) -> usize {
    if old == Some(cursor) {
        return new;
    }
    let without = match old {
        Some(old) if old < cursor => cursor - 1,
        _ => cursor,
    };
    if new <= without { without + 1 } else { without }
}
