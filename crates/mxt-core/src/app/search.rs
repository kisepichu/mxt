//! `/` search in the focused pane, its repetition with `n` and `N`, and its
//! highlight in the timeline (design, "Search and jumps").

use super::{App, Focus, LineKind, StatusKind};
use crate::effect::Effect;
use crate::msg::Pane;
use crate::search::Needle;
use crate::timeline::TimelineEntry;

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Forward,
    Backward,
}

impl App {
    /// The last search's pattern while the timeline highlights its matches:
    /// from every search, also one that found nothing, until `:noh`
    /// (`search.clear_highlight`) or another room opens. Entering or leaving
    /// a thread keeps it.
    pub fn search_highlight(&self) -> Option<&Needle> {
        self.highlight.as_ref()
    }

    /// `search.start`: a search line for the focused pane; nothing from the composer.
    pub(super) fn start_search(&mut self) {
        if let Some(pane) = self.focused_pane() {
            self.open_line(LineKind::Search(pane));
        }
    }

    /// An executed search line; an empty text reuses the last pattern.
    pub(super) fn search_line(&mut self, pane: Pane, text: String, effects: &mut Vec<Effect>) {
        if !text.is_empty() {
            self.last_search = Some(text);
        }
        self.search(pane, Direction::Forward, effects);
    }

    /// `search.next` / `search.prev`: the last pattern in the focused pane.
    pub(super) fn search_again(&mut self, direction: Direction, effects: &mut Vec<Effect>) {
        if let Some(pane) = self.focused_pane() {
            self.search(pane, direction, effects);
        }
    }

    fn focused_pane(&self) -> Option<Pane> {
        match self.focus {
            Focus::RoomList => Some(Pane::RoomList),
            Focus::Timeline => Some(Pane::Timeline),
            Focus::Composer => None,
        }
    }

    /// Moves the pane's cursor to the next item that contains the last
    /// pattern, case-insensitively, starting after the cursor and wrapping
    /// around, so the cursor's own item is checked last. In the timeline,
    /// forward is towards newer items. The pattern's matches are
    /// highlighted, as nvim's `hlsearch` does, also when there are none.
    fn search(&mut self, pane: Pane, direction: Direction, effects: &mut Vec<Effect>) {
        let Some(pattern) = self.last_search.as_deref() else {
            return self.set_status(StatusKind::Error, "No previous search");
        };
        let needle = Needle::new(pattern);
        let found = match pane {
            Pane::RoomList => self.find_room(&needle, direction),
            Pane::Timeline => self.find_item(&needle, direction),
        };
        match (found, pane) {
            (Some(index), Pane::RoomList) => self.place_room_cursor(index),
            // A jump in the timeline; the room list's cursor is no position
            // of the jump list (design, "Jump list").
            (Some(index), Pane::Timeline) => {
                self.record_jump();
                self.place_timeline_cursor(index, effects);
            }
            (None, _) => {
                let text = format!("Pattern not found: {pattern}");
                self.set_status(StatusKind::Error, text);
            }
        }
        self.highlight = Some(needle);
    }

    /// The index of the next room whose displayed name contains `needle`.
    fn find_room(&self, needle: &Needle, direction: Direction) -> Option<usize> {
        let cursor = self.room_cursor()?;
        let rooms = self.rooms.rooms();
        wrapping(cursor, rooms.len(), direction)
            .find(|&index| needle.is_in(rooms[index].name.display_chars()))
    }

    /// The index of the next timeline item whose plain text contains
    /// `needle`.
    fn find_item(&self, needle: &Needle, direction: Direction) -> Option<usize> {
        let cursor = self.timeline_cursor()?;
        let entries = self.timeline();
        wrapping(cursor, entries.len(), direction).find(|&index| match &entries[index] {
            TimelineEntry::Item(item) => needle.is_in(item.content.plain_text().chars()),
            _ => false,
        })
    }
}

/// The indices of `len` items in `direction`, starting after `cursor` and
/// wrapping around to it.
fn wrapping(cursor: usize, len: usize, direction: Direction) -> impl Iterator<Item = usize> {
    (1..=len).map(move |step| match direction {
        Direction::Forward => (cursor + step) % len,
        Direction::Backward => (cursor + len - step) % len,
    })
}
