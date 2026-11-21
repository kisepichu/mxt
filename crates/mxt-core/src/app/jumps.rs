//! The jump list (design, "Jump list"): the positions before jumps, and
//! `jump.back` and `jump.forward`, which go back to them as nvim's `<C-o>`
//! and `<C-i>` do.

use std::collections::VecDeque;

use super::rooms::RoomSwitch;
use super::{App, StatusKind};
use crate::effect::{Effect, UiEffect};
use crate::ids::{EventId, RoomId};
use crate::resolver::TimerId;
use crate::timeline::ItemRef;

/// How many entries the jump list keeps; the oldest drops first.
const JUMP_LIMIT: usize = 100;

/// The status while older messages load to reach an entry's event.
const LOADING: &str = "Loading older messages…";

/// How long the wait goes on once the room's start is loaded without the
/// event: the SDK may deliver that page's diffs after its
/// `Msg::PaginationDone`.
const GRACE_MS: u64 = 1000;

/// A position: a room and the remote event under its timeline cursor, or
/// the room alone when there is none (an empty timeline, a local echo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Jump {
    room: RoomId,
    event: Option<EventId>,
}

/// The entries, oldest first, for the session only.
#[derive(Default)]
pub(super) struct JumpList {
    entries: VecDeque<Jump>,
    /// The entry the last `jump.back` or `jump.forward` went to;
    /// `entries.len()` at the newest position, after every entry.
    index: usize,
}

/// What [`JumpList::push`] changed, for [`JumpList::take_back`].
struct Pushed {
    added: bool,
    dropped: Option<Jump>,
}

impl JumpList {
    /// Records `jump`, the position before a jump: the entries after the one
    /// `jump.back` or `jump.forward` went to are dropped, `jump` is added
    /// unless the newest entry equals it, and the list is at its newest
    /// position again.
    fn record(&mut self, jump: Jump) {
        self.entries.truncate(self.index.saturating_add(1));
        self.push(jump);
        self.index = self.entries.len();
    }

    /// Adds `jump` unless the newest entry equals it, dropping the oldest
    /// beyond [`JUMP_LIMIT`].
    fn push(&mut self, jump: Jump) -> Pushed {
        if self.entries.back() == Some(&jump) {
            return Pushed {
                added: false,
                dropped: None,
            };
        }
        let dropped = if self.entries.len() == JUMP_LIMIT {
            self.entries.pop_front()
        } else {
            None
        };
        self.entries.push_back(jump);
        Pushed {
            added: true,
            dropped,
        }
    }

    fn take_back(&mut self, pushed: Pushed) {
        if pushed.added {
            self.entries.pop_back();
        }
        if let Some(dropped) = pushed.dropped {
            self.entries.push_front(dropped);
        }
    }

    /// The index after `from` in `direction`, if there is an entry there.
    fn next(&self, from: usize, direction: JumpDirection) -> Option<usize> {
        match direction {
            JumpDirection::Back => from.checked_sub(1),
            JumpDirection::Forward => Some(from + 1).filter(|&next| next < self.entries.len()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JumpDirection {
    /// `jump.back`, to older entries.
    Back,
    /// `jump.forward`, to newer entries.
    Forward,
}

/// An entry whose event is not loaded: older messages load until it is, or
/// until the room's start is loaded without it and a grace period passes.
/// The entry is the one the list's index is on, in the open room. The next
/// key or paste, a failed load and opening another room end the wait, and
/// its grace period with it.
pub(super) struct JumpWait {
    event: EventId,
    direction: JumpDirection,
    skipped: Skipped,
    /// The grace period's timer, while the room's start is loaded without
    /// the event.
    grace: Option<TimerId>,
}

/// The entries skipped on the way, for the status line.
#[derive(Clone, Copy, Default)]
struct Skipped {
    /// Entries of rooms that are not in the room list.
    left: usize,
    /// Entries whose event is not in their room's timeline from its start.
    gone: usize,
}

impl Skipped {
    /// `Skipped a jump to a room you left`, `Skipped 2 jumps to messages
    /// that are gone`, …; `None` when nothing was skipped.
    fn text(self) -> Option<String> {
        let count = self.left + self.gone;
        let jumps = match count {
            0 => return None,
            1 => "a jump".to_owned(),
            _ => format!("{count} jumps"),
        };
        let to = match (self.left > 0, self.gone > 0, count == 1) {
            (true, false, true) => "a room you left",
            (true, false, false) => "rooms you left",
            (false, true, true) => "a message that is gone",
            (false, true, false) => "messages that are gone",
            _ => "rooms you left and messages that are gone",
        };
        Some(format!("Skipped {jumps} to {to}"))
    }
}

/// Whether an entry can be gone to now.
enum Reach {
    /// Its room is at this index of the room list.
    Room(usize),
    /// The room is not in the room list.
    Left,
    /// The room is open, its start is loaded, and the event is not there.
    Gone,
}

impl App {
    /// The position a jump records: the open room and the remote event
    /// under its timeline cursor; `None` while no room is open.
    fn position(&self) -> Option<Jump> {
        let room = self.open_room.clone()?;
        let event = match self.timeline.cursor_item().map(|item| &item.id) {
            Some(ItemRef::Remote(event)) => Some(event.clone()),
            Some(ItemRef::Local(_)) | None => None,
        };
        Some(Jump { room, event })
    }

    /// Records the position before a jump: opening another room, `gg`,
    /// `G`, a search or `gd` in the timeline.
    pub(super) fn record_jump(&mut self) {
        if let Some(position) = self.position() {
            self.jumps.record(position);
        }
    }

    /// `jump.back` and `jump.forward`. Back from the newest position first
    /// adds the current one, so that `jump.forward` returns to it, as in
    /// nvim. When no entry is left to go to, nothing changes.
    pub(super) fn jump(&mut self, direction: JumpDirection, effects: &mut Vec<Effect>) {
        let pushed =
            if direction == JumpDirection::Back && self.jumps.index == self.jumps.entries.len() {
                self.position().map(|current| self.jumps.push(current))
            } else {
                None
            };
        // The current position is the newest entry now.
        let from = match pushed {
            Some(_) => self.jumps.entries.len() - 1,
            None => self.jumps.index,
        };
        if !self.jump_from(from, direction, Skipped::default(), effects)
            && let Some(pushed) = pushed
        {
            self.jumps.take_back(pushed);
        }
    }

    /// Goes to the first entry after `from` in `direction` that can be gone
    /// to, skipping the others. Whether there was one; otherwise the status
    /// line says so.
    fn jump_from(
        &mut self,
        from: usize,
        direction: JumpDirection,
        mut skipped: Skipped,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let mut index = from;
        while let Some(next) = self.jumps.next(index, direction) {
            index = next;
            match self.reach(&self.jumps.entries[index]) {
                Reach::Room(room) => {
                    self.go_to(index, room, direction, skipped, effects);
                    return true;
                }
                Reach::Left => skipped.left += 1,
                Reach::Gone => skipped.gone += 1,
            }
        }
        let (none, after_skipping) = match direction {
            JumpDirection::Back => ("No older jump", "no older jump"),
            JumpDirection::Forward => ("No newer jump", "no newer jump"),
        };
        let text = match skipped.text() {
            Some(skipped) => format!("{skipped}; {after_skipping}"),
            None => none.to_owned(),
        };
        self.set_status(StatusKind::Error, text);
        false
    }

    fn reach(&self, jump: &Jump) -> Reach {
        let Some(room) = self.rooms.position(&jump.room) else {
            return Reach::Left;
        };
        match &jump.event {
            Some(event)
                if self.open_room.as_ref() == Some(&jump.room)
                    && self.timeline.reached_start()
                    && self.timeline.event_position(event).is_none() =>
            {
                Reach::Gone
            }
            _ => Reach::Room(room),
        }
    }

    /// Goes to the entry at `index`, whose room is at `room` in the room
    /// list: opens it without recording a jump, which focuses the timeline,
    /// and puts the cursor on the entry's event, or makes it follow the
    /// newest message for a room alone. An event that is not loaded is
    /// waited for while older messages load.
    fn go_to(
        &mut self,
        index: usize,
        room: usize,
        direction: JumpDirection,
        skipped: Skipped,
        effects: &mut Vec<Effect>,
    ) {
        self.jumps.index = index;
        let event = self.jumps.entries[index].event.clone();
        self.open_at(room, RoomSwitch::FromJumpList, effects);
        let Some(event) = event else {
            self.timeline.follow();
            return self.arrived(skipped);
        };
        if let Some(at) = self.timeline.event_position(&event) {
            self.place_timeline_cursor(at, effects);
            return self.arrived(skipped);
        }
        let text = match skipped.text() {
            Some(skipped) => format!("{skipped}; loading older messages…"),
            None => LOADING.to_owned(),
        };
        self.set_status(StatusKind::Info, text);
        self.jump_wait = Some(JumpWait {
            event,
            direction,
            skipped,
            grace: None,
        });
        self.paginate(effects);
    }

    /// The status once the cursor is on the entry: what was skipped, or
    /// nothing, and no [`LOADING`] left on the status line.
    fn arrived(&mut self, skipped: Skipped) {
        match skipped.text() {
            Some(text) => self.set_status(StatusKind::Info, text),
            None => {
                if self.status().is_some_and(|status| status.text == LOADING) {
                    self.status_shown = false;
                }
            }
        }
    }

    /// After the open room's timeline changed or a page of older messages
    /// arrived: the waited-for event gets the cursor once it is loaded; once
    /// the room's start is loaded without it, the grace period starts, after
    /// which the event is gone (see [`App::jump_grace_over`]); otherwise more
    /// older messages load.
    pub(super) fn check_jump_wait(&mut self, effects: &mut Vec<Effect>) {
        let Some(wait) = &mut self.jump_wait else {
            return;
        };
        if let Some(at) = self.timeline.event_position(&wait.event) {
            let skipped = wait.skipped;
            self.jump_wait = None;
            self.place_timeline_cursor(at, effects);
            return self.arrived(skipped);
        }
        if !self.timeline.reached_start() {
            // A reset timeline has its start to load again; the grace
            // period starts anew once it is loaded.
            wait.grace = None;
            return self.paginate(effects);
        }
        if wait.grace.is_none() {
            let id = self.resolver.new_timer();
            wait.grace = Some(id);
            effects.push(Effect::Ui(UiEffect::StartTimer {
                id,
                after_ms: GRACE_MS,
            }));
        }
    }

    /// `Msg::Timeout`: when `id` is the wait's grace timer, the event is
    /// gone, and the next entry in the same direction is gone to. Whether
    /// it was.
    pub(super) fn jump_grace_over(&mut self, id: TimerId, effects: &mut Vec<Effect>) -> bool {
        let Some(wait) = self.jump_wait.take_if(|wait| wait.grace == Some(id)) else {
            return false;
        };
        let mut skipped = wait.skipped;
        skipped.gone += 1;
        // With no entry left, the room stays open: the jump went there.
        self.jump_from(self.jumps.index, wait.direction, skipped, effects);
        true
    }
}
