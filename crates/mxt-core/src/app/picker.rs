//! Pickers: a query, its ranked items and a selection. The emoji picker
//! toggles a reaction on a timeline item, the room picker opens a room, and
//! the link picker opens a link of a timeline item.

use std::collections::HashSet;

use super::command_line::Edit;
use super::rooms::NO_MATCHING_ROOM;
use super::{App, StatusKind};
use crate::effect::{Effect, MatrixEffect, UiEffect};
use crate::emoji::{self, Emoji};
use crate::fuzzy;
use crate::ids::RoomId;
use crate::keys::Key;
use crate::room::{Badge, RoomSummary};
use crate::rooms::{RoomFilter, RoomList};
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

impl Picker {
    /// What the picker shows while it has no items.
    pub fn empty_text(&self) -> &'static str {
        match self.kind {
            PickerKind::Rooms { .. } => NO_MATCHING_ROOM,
            PickerKind::Emoji { .. } | PickerKind::Links => "No match",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerKind {
    /// A reaction on `item` of `room`.
    Emoji { room: RoomId, item: ItemRef },
    /// A room that passes `filter`.
    Rooms { filter: RoomFilter },
    /// A link of the item under the cursor.
    Links,
}

/// Items carry their key, so two rooms with the same displayed name are both
/// reachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerItem {
    pub label: String,
    /// What accepting the item uses: the reaction key for emoji, the room ID
    /// for rooms, the URL for links.
    pub key: String,
}

/// The picker and what its items are ranked from.
pub(super) struct OpenPicker {
    picker: Picker,
    source: Source,
}

/// What a picker's items come from, besides its query.
enum Source {
    /// The keys the user reacted with on the item.
    Emoji { reacted: Vec<String> },
    /// The rooms that pass the filter, as the room list is at each edit.
    Rooms(RoomFilter),
    /// The URLs of the item's links.
    Links(Vec<String>),
}

impl Source {
    fn items(&self, query: &str, rooms: &RoomList) -> Vec<PickerItem> {
        match self {
            Source::Emoji { reacted } => emoji_items(query, reacted),
            Source::Rooms(filter) => room_items(rooms, *filter, query),
            Source::Links(urls) => link_items(urls, query),
        }
    }
}

impl App {
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref().map(|open| &open.picker)
    }

    fn open_picker(&mut self, kind: PickerKind, source: Source) {
        let picker = Picker {
            kind,
            query: String::new(),
            items: source.items("", &self.rooms),
            selected: 0,
        };
        self.picker = Some(OpenPicker { picker, source });
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
        let kind = PickerKind::Emoji {
            room: room.clone(),
            item: item.id.clone(),
        };
        self.open_picker(kind, Source::Emoji { reacted });
    }

    /// `picker.rooms`: the room picker over the rooms that pass `filter`.
    pub(super) fn pick_room(&mut self, filter: RoomFilter) {
        self.open_picker(PickerKind::Rooms { filter }, Source::Rooms(filter));
    }

    /// The link picker over `urls`, the links of the item under the cursor.
    pub(super) fn pick_link(&mut self, urls: Vec<String>) {
        self.open_picker(PickerKind::Links, Source::Links(urls));
    }

    /// A key released to the picker: edits the query and ranks the items
    /// again; other keys are ignored.
    pub(super) fn type_into_picker(&mut self, key: Key) {
        let (Some(open), Some(edit)) = (&mut self.picker, Edit::of(key)) else {
            return;
        };
        if edit.apply(&mut open.picker.query) {
            open.picker.items = open.source.items(&open.picker.query, &self.rooms);
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

    /// `picker.accept`: closes the picker and toggles the selected reaction,
    /// opens the selected room by its ID, or opens the selected link. Without
    /// items it stays open.
    pub(super) fn accept_picker(&mut self, effects: &mut Vec<Effect>) {
        let Some(mut open) = self
            .picker
            .take_if(|open| open.picker.selected < open.picker.items.len())
        else {
            return;
        };
        let key = open.picker.items.swap_remove(open.picker.selected).key;
        match open.picker.kind {
            PickerKind::Emoji { room, item } => {
                effects.push(Effect::Matrix(MatrixEffect::ToggleReaction {
                    room,
                    item,
                    key,
                }));
            }
            PickerKind::Rooms { .. } => self.open_id(&RoomId::new(key), effects),
            PickerKind::Links => effects.push(Effect::Ui(UiEffect::OpenUrl(key))),
        }
    }

    /// `picker.cancel`.
    pub(super) fn cancel_picker(&mut self) {
        self.picker = None;
    }
}

/// The rooms that pass `filter`, labelled with the displayed name and the
/// badge: in room list order for an empty query, otherwise ranked on the
/// displayed name.
fn room_items(rooms: &RoomList, filter: RoomFilter, query: &str) -> Vec<PickerItem> {
    let passing: Vec<&RoomSummary> = rooms
        .rooms()
        .iter()
        .filter(|room| filter.matches(&room.unread))
        .collect();
    let mut names: Vec<String> = passing.iter().map(|room| room.name.to_string()).collect();
    fuzzy::rank(query, &names)
        .into_iter()
        .map(|index| {
            let room = passing[index];
            // Each index comes once.
            let mut label = std::mem::take(&mut names[index]);
            if let Some(badge) = Badge::from_counts(&room.unread) {
                label.push_str("  ");
                label.push_str(&badge.text());
            }
            PickerItem {
                label,
                key: room.id.to_string(),
            }
        })
        .collect()
}

/// The URLs as their own labels: in order for an empty query, otherwise
/// ranked.
fn link_items(urls: &[String], query: &str) -> Vec<PickerItem> {
    fuzzy::rank(query, urls)
        .into_iter()
        .map(|index| PickerItem {
            label: urls[index].clone(),
            key: urls[index].clone(),
        })
        .collect()
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
