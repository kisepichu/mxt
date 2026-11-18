//! `/` search in the focused pane and its repetition with `n` and `N`.

use super::{App, Focus, LineKind, StatusKind};
use crate::msg::Pane;

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
    pub(super) fn search_line(&mut self, pane: Pane, text: String) {
        if !text.is_empty() {
            self.last_search = Some(text);
        }
        self.search(pane, Direction::Forward);
    }

    /// `search.next` / `search.prev`: the last pattern in the focused pane.
    pub(super) fn search_again(&mut self, direction: Direction) {
        if let Some(pane) = self.focused_pane() {
            self.search(pane, direction);
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
    /// around, so the cursor's own item is checked last.
    fn search(&mut self, pane: Pane, direction: Direction) {
        let Some(pattern) = self.last_search.as_deref() else {
            return self.set_status(StatusKind::Error, "No previous search");
        };
        let found = match pane {
            Pane::RoomList => self.find_room(pattern, direction),
            // The timeline has no items until phase 2.
            Pane::Timeline => None,
        };
        match found {
            Some(index) => self.room_cursor = Some(index),
            None => {
                let text = format!("Pattern not found: {pattern}");
                self.set_status(StatusKind::Error, text);
            }
        }
    }

    /// The index of the next room whose displayed name contains `pattern`.
    fn find_room(&self, pattern: &str, direction: Direction) -> Option<usize> {
        let cursor = self.room_cursor()?;
        let rooms = self.rooms.rooms();
        let len = rooms.len();
        let needle: String = pattern.chars().flat_map(char::to_lowercase).collect();
        let mut name = String::new();
        (1..=len)
            .map(|step| match direction {
                Direction::Forward => (cursor + step) % len,
                Direction::Backward => (cursor + len - step) % len,
            })
            .find(|&index| {
                name.clear();
                name.extend(
                    rooms[index]
                        .name
                        .display_chars()
                        .flat_map(char::to_lowercase),
                );
                name.contains(&needle)
            })
    }
}
