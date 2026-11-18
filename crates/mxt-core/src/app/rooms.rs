//! Room list updates and the room list actions.

use std::cmp::Ordering;

use super::{App, Focus, StatusKind};
use crate::effect::{Effect, MatrixEffect};
use crate::ids::RoomId;
use crate::msg::Pane;
use crate::room::RoomSummary;
use crate::rooms::{RoomTarget, resolve_room};

/// A cursor movement of `cursor.*`, with its count.
pub(super) enum Motion {
    Down(u32),
    Up(u32),
    /// Half pages of the pane's last viewport.
    HalfPageDown(u32),
    HalfPageUp(u32),
    First,
    Last,
}

impl App {
    /// The cursor stays on its room when the order changes.
    pub(super) fn upsert_room(&mut self, summary: RoomSummary) {
        let (old, new) = self.rooms.upsert(summary);
        if let Some(cursor) = self.room_cursor {
            self.room_cursor = Some(follow(cursor, old, new));
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
            effects.push(Effect::Matrix(MatrixEffect::UnsubscribeTimeline(open)));
        }
    }

    /// Moves the cursor of the focused pane, clamped, without wrapping.
    pub(super) fn move_cursor(&mut self, motion: Motion) {
        // The timeline has no items until phase 2, and the composer has no
        // cursor of the core.
        if self.focus != Focus::RoomList {
            return;
        }
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

    /// Half of the items the pane showed in its last viewport, at least one.
    fn half_page(&self, pane: Pane) -> usize {
        let visible = self.viewport(pane).map_or(1, |viewport| {
            viewport.last.saturating_sub(viewport.first) + 1
        });
        (visible / 2).max(1)
    }

    pub(super) fn open_selected(&mut self, effects: &mut Vec<Effect>) {
        if let Some(index) = self.room_cursor() {
            self.open_at(index, effects);
        }
    }

    pub(super) fn open_target(&mut self, target: &RoomTarget, effects: &mut Vec<Effect>) {
        let found = resolve_room(&self.rooms, self.open_room.as_ref(), target)
            .and_then(|id| self.rooms.position(&id));
        match found {
            Some(index) => self.open_at(index, effects),
            None => self.set_status(StatusKind::Error, "No matching room"),
        }
    }

    /// Opens the room at `index` of the room list: unsubscribes the previous
    /// room, subscribes the new one and moves the cursor to it. An open room
    /// only gets the focus.
    fn open_at(&mut self, index: usize, effects: &mut Vec<Effect>) {
        let id = &self.rooms.rooms()[index].id;
        if self.open_room.as_ref() != Some(id) {
            let id = id.clone();
            if let Some(previous) = self.open_room.replace(id.clone()) {
                effects.push(Effect::Matrix(MatrixEffect::UnsubscribeTimeline(previous)));
            }
            effects.push(Effect::Matrix(MatrixEffect::SubscribeTimeline(id)));
            self.room_cursor = Some(index);
        }
        self.focus = Focus::Timeline;
    }

    /// `room.mark_read` on the room under the cursor when the room list has
    /// focus, otherwise on the open room. Its counts are cleared at once
    /// (design, "Optimistic clearing"), so a following action sees it read.
    pub(super) fn mark_read(&mut self, effects: &mut Vec<Effect>) {
        let target = match self.focus {
            Focus::RoomList => self
                .room_cursor()
                .map(|index| self.rooms.rooms()[index].id.clone()),
            Focus::Timeline | Focus::Composer => self.open_room.clone(),
        };
        let Some(id) = target else {
            return self.set_status(StatusKind::Error, "No room");
        };
        self.rooms.clear_unread(&id);
        effects.push(Effect::Matrix(MatrixEffect::MarkRoomRead(id)));
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
