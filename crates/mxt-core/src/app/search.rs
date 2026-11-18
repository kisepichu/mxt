//! `/` search in the focused pane and its repetition with `n` and `N`.

use super::{App, Focus, LineKind, StatusKind};
use crate::effect::Effect;
use crate::msg::Pane;
use crate::timeline::TimelineEntry;

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Forward,
    Backward,
}

impl App {
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
    /// forward is towards newer items.
    fn search(&mut self, pane: Pane, direction: Direction, effects: &mut Vec<Effect>) {
        let Some(pattern) = self.last_search.as_deref() else {
            return self.set_status(StatusKind::Error, "No previous search");
        };
        let needle: String = pattern.chars().flat_map(char::to_lowercase).collect();
        let found = match pane {
            Pane::RoomList => self.find_room(&needle, direction),
            Pane::Timeline => self.find_item(&needle, direction),
        };
        match (found, pane) {
            (Some(index), Pane::RoomList) => self.room_cursor = Some(index),
            (Some(index), Pane::Timeline) => self.place_timeline_cursor(index, effects),
            (None, _) => {
                let text = format!("Pattern not found: {pattern}");
                self.set_status(StatusKind::Error, text);
            }
        }
    }

    /// The index of the next room whose displayed name contains `needle`
    /// (lowercase).
    fn find_room(&self, needle: &str, direction: Direction) -> Option<usize> {
        let cursor = self.room_cursor()?;
        let rooms = self.rooms.rooms();
        let mut name = String::new();
        wrapping(cursor, rooms.len(), direction).find(|&index| {
            name.clear();
            name.extend(
                rooms[index]
                    .name
                    .display_chars()
                    .flat_map(char::to_lowercase),
            );
            name.contains(needle)
        })
    }

    /// The index of the next timeline item whose plain text contains
    /// `needle` (lowercase).
    fn find_item(&self, needle: &str, direction: Direction) -> Option<usize> {
        let cursor = self.timeline_cursor()?;
        let entries = self.timeline();
        let mut text = String::new();
        wrapping(cursor, entries.len(), direction).find(|&index| {
            let TimelineEntry::Item(item) = &entries[index] else {
                return false;
            };
            text.clear();
            text.extend(
                item.content
                    .plain_text()
                    .chars()
                    .flat_map(char::to_lowercase),
            );
            text.contains(needle)
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
