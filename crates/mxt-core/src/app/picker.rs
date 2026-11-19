//! Pickers: a query, its ranked items and a selection. The emoji picker
//! toggles a reaction on a timeline item.

use std::collections::HashSet;

use super::command_line::Edit;
use super::{App, StatusKind};
use crate::effect::{Effect, MatrixEffect};
use crate::emoji::{self, Emoji};
use crate::ids::RoomId;
use crate::keys::Key;
use crate::timeline::{ItemRef, MessageContent};

/// The open picker (`PICKER` mode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    /// Ranked for `query`.
    pub items: Vec<PickerItem>,
    /// An index into `items`, 0 when they are empty.
    pub selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerKind {
    /// A reaction on `item` of `room`.
    Emoji { room: RoomId, item: ItemRef },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerItem {
    pub label: String,
    /// What accepting the item uses: the reaction key for emoji.
    pub key: String,
}

/// The picker and what its items are ranked from.
pub(super) struct OpenPicker {
    picker: Picker,
    /// The keys the user reacted with on the item.
    reacted: Vec<String>,
}

impl App {
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref().map(|open| &open.picker)
    }

    /// `message.react`: the emoji picker for the item under the cursor,
    /// unless it is deleted.
    pub(super) fn react(&mut self) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        if item.content == MessageContent::Redacted {
            return self.set_status(StatusKind::Error, "The message is deleted");
        }
        let reacted: Vec<String> = item
            .reactions
            .iter()
            .filter(|group| group.mine)
            .map(|group| group.key.clone())
            .collect();
        let picker = Picker {
            kind: PickerKind::Emoji {
                room: room.clone(),
                item: item.id.clone(),
            },
            query: String::new(),
            items: emoji_items("", &reacted),
            selected: 0,
        };
        self.picker = Some(OpenPicker { picker, reacted });
    }

    /// A key released to the picker: edits the query and ranks the items
    /// again; other keys are ignored.
    pub(super) fn type_into_picker(&mut self, key: Key) {
        let (Some(open), Some(edit)) = (&mut self.picker, Edit::of(key)) else {
            return;
        };
        if edit.apply(&mut open.picker.query) {
            open.picker.items = emoji_items(&open.picker.query, &open.reacted);
            open.picker.selected = 0;
        }
    }

    /// `picker.next` / `picker.prev`, wrapping around.
    pub(super) fn move_picker(&mut self, forward: bool) {
        if let Some(open) = &mut self.picker
            && let Some(last) = open.picker.items.len().checked_sub(1)
        {
            let picker = &mut open.picker;
            picker.selected = match (forward, picker.selected) {
                (true, selected) if selected >= last => 0,
                (true, selected) => selected + 1,
                (false, 0) => last,
                (false, selected) => selected - 1,
            };
        }
    }

    /// `picker.accept`: toggles the selected reaction and closes the picker;
    /// without items it stays open.
    pub(super) fn accept_picker(&mut self, effects: &mut Vec<Effect>) {
        let Some(open) = &self.picker else {
            return;
        };
        let Some(selected) = open.picker.items.get(open.picker.selected) else {
            return;
        };
        let PickerKind::Emoji { room, item } = &open.picker.kind;
        effects.push(Effect::Matrix(MatrixEffect::ToggleReaction {
            room: room.clone(),
            item: item.clone(),
            key: selected.key.clone(),
        }));
        self.picker = None;
    }

    /// `picker.cancel`.
    pub(super) fn cancel_picker(&mut self) {
        self.picker = None;
    }
}

/// The emoji for `query`. An empty query lists the user's reactions on the
/// item first, then the most common reactions, then every other emoji in
/// `emojis` order. An emoji the user reacted with keeps the key they used,
/// which may lack the variation selector, so accepting it removes their
/// reaction instead of adding a second one.
fn emoji_items(query: &str, reacted: &[String]) -> Vec<PickerItem> {
    let key = |emoji: Emoji| {
        reacted
            .iter()
            .find(|key| Emoji::get(key) == Some(emoji))
            .map_or_else(|| emoji.as_str().to_owned(), Clone::clone)
    };
    let item = |emoji: Emoji| PickerItem {
        label: emoji.label(),
        key: key(emoji),
    };
    if !query.trim().is_empty() {
        return emoji::search(query).into_iter().map(item).collect();
    }
    let mut listed: HashSet<&str> = HashSet::new();
    let mut items: Vec<PickerItem> = reacted
        .iter()
        .map(|key| {
            let emoji = Emoji::get(key);
            if let Some(emoji) = emoji {
                listed.insert(emoji.as_str());
            }
            PickerItem {
                label: emoji.map_or_else(|| key.clone(), Emoji::label),
                key: key.clone(),
            }
        })
        .collect();
    for emoji in emoji::common().chain(emoji::all()) {
        if listed.insert(emoji.as_str()) {
            items.push(item(emoji));
        }
    }
    items
}
