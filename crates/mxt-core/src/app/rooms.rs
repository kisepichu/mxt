//! Room list updates and the room list actions.

use std::cmp::Ordering;

use super::{App, Focus, Motion, StatusKind};
use crate::action::NotifyMode;
use crate::effect::{Effect, MatrixEffect};
use crate::ids::{RoomId, Timestamp};
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
    /// The cursor stays on its room when the order changes, but for the
    /// open room's first move (see `rooms_moved`), which only an update of
    /// the open room itself makes.
    pub(super) fn upsert_room(&mut self, summary: RoomSummary) {
        let open = self.open_room.as_ref() == Some(&summary.id);
        let (old, new) = self.rooms.upsert(summary);
        let open_room_moved = open && old.is_some_and(|old| old != new);
        self.rooms_moved(open_room_moved, |row| Some(follow(row, old, new)));
    }

    /// Optimistic clearing (design, "Matrix adapter"): the room's counts go
    /// to zero at once and, when it has something to read, its `last_read`
    /// to the top of the read rooms (`receipt_at` is the time of the event
    /// the receipt goes on; see `RoomList::clear_unread`), and the room
    /// moves when the sort keys depend on them, the cursor staying on its
    /// room but for the open room's first move (see `rooms_moved`).
    pub(super) fn clear_room_unread(&mut self, id: &RoomId, receipt_at: Option<Timestamp>) {
        if let Some((old, new)) = self.rooms.clear_unread(id, receipt_at) {
            let open_room_moved = old != new && self.open_room.as_ref() == Some(id);
            self.rooms_moved(open_room_moved, |row| Some(follow(row, Some(old), new)));
        }
    }

    /// The cursor after a change of the list: `row_after` takes a row
    /// before the change to the row of the same room after it, or, for a
    /// removed room, of the room now in its place (`None` when the list is
    /// empty). The cursor stays on its room, except on the open room's
    /// first move with the cursor that opening put on it (design, "Room
    /// list"): `open_room_moved` says that the change was one of the open
    /// room itself (its opening, its clearing, its update) and changed its
    /// row, so the cursor goes to the room that was next to it before the
    /// change, the one after it or, when it was the last, the one before
    /// it. Such a change keeps the list's length. A change of another room
    /// that shifts the open room is not its move: the cursor follows it,
    /// and the rule stays. Either way the cursor follows its room from then
    /// on.
    fn rooms_moved(&mut self, open_room_moved: bool, row_after: impl Fn(usize) -> Option<usize>) {
        let Some(cursor) = self.room_cursor else {
            return;
        };
        let after = row_after(cursor);
        self.room_cursor = if self.cursor_on_unmoved_open_room && open_room_moved {
            self.cursor_on_unmoved_open_room = false;
            next_to(cursor, self.rooms.len()).map_or(after, row_after)
        } else {
            after
        };
    }

    /// Puts the room list cursor on `index` for a cursor action. A cursor
    /// that leaves its row is off the open room that opening put it on, so
    /// that room's moves no longer move it to the room next to it.
    pub(super) fn place_room_cursor(&mut self, index: usize) {
        if self.room_cursor() != Some(index) {
            self.cursor_on_unmoved_open_room = false;
        }
        self.room_cursor = Some(index);
    }

    /// A cursor on the removed room moves to the room now at its index. A
    /// room removed above the open room shifts it without moving it, so the
    /// cursor follows it (see `rooms_moved`).
    pub(super) fn remove_room(&mut self, id: RoomId, effects: &mut Vec<Effect>) {
        if let Some((index, _)) = self.rooms.remove(&id) {
            let last = self.rooms.len().checked_sub(1);
            self.rooms_moved(false, |row| match row.cmp(&index) {
                Ordering::Less => Some(row),
                Ordering::Greater => Some(row - 1),
                Ordering::Equal => last.map(|last| row.min(last)),
            });
        }
        if self.open_room.as_ref() == Some(&id) {
            // Before the room stops being the open one, for its thread.
            self.reset_timeline(effects);
            self.open_room = None;
            self.cursor_on_unmoved_open_room = false;
            effects.push(Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id)));
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
        self.place_room_cursor(target.min(last));
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
    /// room, its thread first, and subscribes the new one with a new
    /// timeline. Both rooms move when the sort keys depend on their counts,
    /// or under `read` on their `last_read` and latest activity, since the
    /// open room sorts as read. Opening puts the cursor on the room at
    /// `index`, before the rooms move, so the opened room's own move as it
    /// opens is its first, while the previously open room going back to
    /// its place only shifts it (see `rooms_moved`). An open room only gets
    /// the focus. This is the one place where the open room becomes another
    /// room, so a recorded switch puts the position before it into the jump
    /// list whichever action or answer opens the room (design, "Jump
    /// list").
    pub(super) fn open_at(&mut self, index: usize, switch: RoomSwitch, effects: &mut Vec<Effect>) {
        let id = &self.rooms.rooms()[index].id;
        if self.open_room.as_ref() != Some(id) {
            let id = id.clone();
            if switch == RoomSwitch::Recorded {
                self.record_jump();
            }
            self.reset_timeline(effects);
            if let Some(previous) = self.open_room.replace(id.clone()) {
                effects.push(Effect::Matrix(MatrixEffect::UnsubscribeTimeline(previous)));
            }
            let [previous, opened] = self.rooms.open(&id);
            self.room_cursor = Some(index);
            self.cursor_on_unmoved_open_room = true;
            let open_room_moved = opened.is_some_and(|(old, new)| old != new);
            self.rooms_moved(open_room_moved, |row| {
                let moved = [previous, opened].into_iter().flatten();
                Some(moved.fold(row, |row, (old, new)| follow(row, Some(old), new)))
            });
            effects.push(Effect::Matrix(MatrixEffect::SubscribeTimeline(id)));
        }
        self.focus = Focus::Timeline;
    }

    /// The room that `room.mark_read`, `room.mute`, `room.unmute` and
    /// `room.notify` act on: the room under the cursor when the room list
    /// has focus, otherwise the open room.
    fn target_room(&self) -> Option<&RoomSummary> {
        match self.focus {
            Focus::RoomList => self.room_cursor().map(|index| &self.rooms.rooms()[index]),
            Focus::Timeline | Focus::Composer => {
                self.open_room.as_ref().and_then(|id| self.rooms.get(id))
            }
        }
    }

    /// `room.mark_read`. The room's counts are cleared at once (design,
    /// "Optimistic clearing"), so a following action sees it read; the
    /// receipt goes on the room's latest event, at its latest activity. On
    /// the open room, the thread roots' marks go up to the room's latest
    /// activity, the time of the latest event the receipt marks read.
    pub(super) fn mark_read(&mut self, effects: &mut Vec<Effect>) {
        let Some((id, latest)) = self
            .target_room()
            .map(|room| (room.id.clone(), room.latest_activity))
        else {
            return self.set_status(StatusKind::Error, "No room");
        };
        if self.open_room.as_ref() == Some(&id)
            && let Some(latest) = latest
        {
            self.main_timeline_mut().read_up_to_at_least(latest);
        }
        self.clear_room_unread(&id, latest);
        effects.push(Effect::Matrix(MatrixEffect::MarkRoomRead(id)));
    }

    /// `room.mute` / `room.unmute`. The room's `Muted` label changes at
    /// once, and a failed change puts it back (see `send_mode`). Unmuting
    /// removes the room's rules, so `room.unmute` on a room that is not
    /// muted does nothing: a rule for all messages or for mentions and
    /// keywords only, set elsewhere or with `room.notify`, stays.
    pub(super) fn set_mute(&mut self, muted: bool, effects: &mut Vec<Effect>) {
        let Some(room) = self.target_room() else {
            return self.set_status(StatusKind::Error, "No room");
        };
        if !muted && room.mode != NotificationMode::Mute {
            let status = format!("{} is not muted", room.name);
            return self.set_status(StatusKind::Error, status);
        }
        let (verb, mode) = if muted {
            ("Muted", NotificationMode::Mute)
        } else {
            ("Unmuted", NotificationMode::Default)
        };
        let status = format!("{verb} {}", room.name);
        let id = room.id.clone();
        self.send_mode(id, mode, effects);
        self.set_status(StatusKind::Info, status);
    }

    /// `room.notify` (design, "Notification modes"): without `mode` it
    /// shows the room's mode. With one it compares the target with the
    /// room's mode, which the changes sent before have set already: the
    /// same mode sends nothing and says so, another sends the change.
    pub(super) fn notify(&mut self, mode: Option<NotifyMode>, effects: &mut Vec<Effect>) {
        let Some(room) = self.target_room() else {
            return self.set_status(StatusKind::Error, "No room");
        };
        let Some(mode) = mode else {
            let status = format!("{} {}", room.name, setting(room.mode));
            return self.set_status(StatusKind::Info, status);
        };
        let target = match mode {
            NotifyMode::All => NotificationMode::AllMessages,
            NotifyMode::Mentions => NotificationMode::MentionsAndKeywords,
        };
        if room.mode == target {
            let status = format!("{} already {}", room.name, setting(target));
            return self.set_status(StatusKind::Info, status);
        }
        let status = format!("{} now {}", room.name, setting(target));
        let id = room.id.clone();
        self.send_mode(id, target, effects);
        self.set_status(StatusKind::Info, status);
    }

    /// Sends the change of the room's mode and sets the room's mode to it
    /// at once, as optimistic clearing sets the counts (design,
    /// "Notification modes"): the `Muted` label follows, and the next
    /// action compares with the target. A change that fails is answered
    /// with `Msg::RoomModeChanged`, which puts the SDK's mode back. No sort
    /// key reads the mode, so no room moves.
    fn send_mode(&mut self, room: RoomId, mode: NotificationMode, effects: &mut Vec<Effect>) {
        self.rooms.set_mode(&room, mode);
        effects.push(Effect::Matrix(MatrixEffect::SetMode { room, mode }));
    }
}

/// How a room in `mode` notifies, after its name in the status line.
fn setting(mode: NotificationMode) -> &'static str {
    match mode {
        NotificationMode::AllMessages => "notifies for every message",
        NotificationMode::MentionsAndKeywords => "notifies for mentions and keywords only",
        NotificationMode::Mute => "is muted",
        NotificationMode::Default => "follows the account's default",
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

/// The row next to `row` in a list of `len` rooms: the one after it, or the
/// one before it when it is the last; `None` when it is the only room.
fn next_to(row: usize, len: usize) -> Option<usize> {
    if row + 1 < len {
        Some(row + 1)
    } else {
        row.checked_sub(1)
    }
}
