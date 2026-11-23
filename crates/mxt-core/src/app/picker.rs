//! Pickers: a query, its ranked items and a selection. The emoji picker
//! toggles a reaction on a timeline item, the room picker opens a room, and
//! the link picker opens a link of a timeline item.

use std::collections::HashSet;

use super::rooms::NO_MATCHING_ROOM;
use super::{App, StatusKind};
use crate::action::ReactWith;
use crate::effect::{Effect, MatrixEffect};
use crate::emoji::{self, Emoji};
use crate::fuzzy;
use crate::ids::RoomId;
use crate::keys::Key;
use crate::line::{LineChange, LineEditor};
use crate::room::{Badge, RoomSummary};
use crate::rooms::{RoomFilter, RoomList};
use crate::timeline::{ItemRef, MessageContent, ReactionGroup};

/// The open picker (`PICKER` mode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub kind: PickerKind,
    /// The query and its cursor.
    pub query: LineEditor,
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
    /// The room's unread badge, drawn in its own style; `None` for read rooms
    /// and for other pickers' items.
    pub badge: Option<Badge>,
    pub key: PickerKey,
}

/// What accepting an item uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerKey {
    /// The reaction key, for the emoji picker.
    Reaction(String),
    Room(RoomId),
    Url(String),
}

/// The picker and what its items are ranked from.
pub(super) struct OpenPicker {
    picker: Picker,
    source: Source,
}

impl OpenPicker {
    /// Ranks the items again for the query and selects the first.
    fn rank(&mut self, rooms: &RoomList) {
        self.picker.items = self.source.items(self.picker.query.text(), rooms);
        self.picker.selected = 0;
    }
}

/// What a picker's items come from, besides its query.
enum Source {
    /// The reactions on the item, as the timeline shows them.
    Emoji { shown: Vec<ReactionGroup> },
    /// The rooms that pass the filter, as the room list is at each edit.
    Rooms(RoomFilter),
    /// The URLs of the item's links.
    Links(Vec<String>),
}

impl Source {
    fn items(&self, query: &str, rooms: &RoomList) -> Vec<PickerItem> {
        match self {
            Source::Emoji { shown } => emoji_items(query, shown),
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
            query: LineEditor::default(),
            items: source.items("", &self.rooms),
            selected: 0,
        };
        self.picker = Some(OpenPicker { picker, source });
    }

    /// `message.react` on the item under the open room's timeline cursor,
    /// whichever pane has focus: with an emoji, toggles that reaction; with
    /// an index, the reaction shown there under the item, or says it has
    /// none there; otherwise opens the emoji picker. Refused without an
    /// open room, on an empty timeline and on a deleted item.
    pub(super) fn react(&mut self, with: ReactWith, effects: &mut Vec<Effect>) {
        let Some(room) = &self.open_room else {
            return self.set_status(StatusKind::Error, "No room is open");
        };
        let Some(item) = self.timeline.cursor_item() else {
            return self.set_status(StatusKind::Error, "No message to react to");
        };
        if item.content == MessageContent::Redacted {
            return self.set_status(StatusKind::Error, "The message is deleted");
        }
        let shown = item.shown_reactions();
        let (room, id) = (room.clone(), item.id.clone());
        let key = match with {
            ReactWith::Emoji(emoji) => reaction_key(own_keys(shown), emoji),
            ReactWith::Index(index) => match shown.get(index.get() - 1) {
                Some(group) => shown_reaction_key(own_keys(shown), group),
                None => {
                    let text = format!("The message has no reaction {index}");
                    return self.set_status(StatusKind::Error, text);
                }
            },
            ReactWith::Picker => {
                let source = Source::Emoji {
                    shown: shown.to_vec(),
                };
                return self.open_picker(PickerKind::Emoji { room, item: id }, source);
            }
        };
        effects.push(Effect::Matrix(MatrixEffect::ToggleReaction {
            room,
            item: id,
            key,
        }));
    }

    /// `picker.rooms`: the room picker over the rooms that pass `filter`.
    pub(super) fn pick_room(&mut self, filter: RoomFilter) {
        self.open_picker(PickerKind::Rooms { filter }, Source::Rooms(filter));
    }

    /// The link picker over `urls`, the links of the item under the cursor.
    pub(super) fn pick_link(&mut self, urls: Vec<String>) {
        self.open_picker(PickerKind::Links, Source::Links(urls));
    }

    /// A key released to the picker: the line editor's. An edit of the query
    /// ranks the items again; moving the cursor does not.
    pub(super) fn type_into_picker(&mut self, key: Key) {
        if let Some(open) = &mut self.picker
            && open.picker.query.key(key) == LineChange::Text
        {
            open.rank(&self.rooms);
        }
    }

    /// Pasted text without line breaks, inserted into the query at the
    /// cursor, which ranks the items again.
    pub(super) fn paste_into_picker(&mut self, text: &str) {
        if let Some(open) = &mut self.picker
            && open.picker.query.insert(text)
        {
            open.rank(&self.rooms);
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
        match key {
            PickerKey::Reaction(key) => {
                // Only the emoji picker has reaction items.
                if let PickerKind::Emoji { room, item } = open.picker.kind {
                    effects.push(Effect::Matrix(MatrixEffect::ToggleReaction {
                        room,
                        item,
                        key,
                    }));
                }
            }
            PickerKey::Room(id) => self.open_id(&id, effects),
            PickerKey::Url(url) => self.open_url(url, effects),
        }
    }

    /// `picker.cancel`.
    pub(super) fn cancel_picker(&mut self) {
        self.picker = None;
    }
}

/// The rooms that pass `filter`, labelled with the displayed name, with their
/// badges: in room list order for an empty query, otherwise ranked on the
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
            PickerItem {
                label: std::mem::take(&mut names[index]),
                badge: Badge::from_counts(&room.unread),
                key: PickerKey::Room(room.id.clone()),
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
            badge: None,
            key: PickerKey::Url(urls[index].clone()),
        })
        .collect()
}

/// The emoji for `query`, each once. An empty query lists the reactions on
/// the item first, in the order the timeline shows them (others' and the
/// user's own), each toggling its own key and labelled with it, so two
/// reactions with one emoji (`❤` and `❤️`) can be told apart; then the most
/// common reactions, then every other emoji in `emojis` order. A query ranks
/// every emoji, each toggling the key from [`reaction_key`].
fn emoji_items(query: &str, shown: &[ReactionGroup]) -> Vec<PickerItem> {
    let item = |label: String, key: String| PickerItem {
        label,
        badge: None,
        key: PickerKey::Reaction(key),
    };
    if !query.trim().is_empty() {
        return emoji::search(query)
            .into_iter()
            .map(|emoji| item(emoji.label(), reaction_key(own_keys(shown), emoji)))
            .collect();
    }
    let mut listed: HashSet<&str> = HashSet::new();
    let mut items: Vec<PickerItem> = shown
        .iter()
        .map(|group| {
            let emoji = Emoji::get(&group.key);
            if let Some(emoji) = emoji {
                listed.insert(emoji.as_str());
            }
            let label = emoji.map_or_else(|| group.key.clone(), |emoji| emoji.label_as(&group.key));
            item(label, group.key.clone())
        })
        .collect();
    // The user's own reactions are among the shown ones, so every other
    // emoji is a new reaction, whose key is the emoji itself.
    for emoji in emoji::common().chain(emoji::all()) {
        if listed.insert(emoji.as_str()) {
            items.push(item(emoji.label(), emoji.as_str().to_owned()));
        }
    }
    items
}

/// The keys of the user's own reactions among `shown`.
fn own_keys(shown: &[ReactionGroup]) -> impl Iterator<Item = &str> {
    shown
        .iter()
        .filter(|group| group.mine)
        .map(|group| group.key.as_str())
}

/// The key a reaction with `emoji` toggles: the key of the user's own
/// reaction (`own`) with the same emoji, which may lack the variation
/// selector (`❤` for `❤️`), so toggling removes their reaction instead of
/// adding a second one; otherwise the emoji itself.
fn reaction_key<'a>(own: impl Iterator<Item = &'a str>, emoji: Emoji) -> String {
    own_key(own, emoji).unwrap_or(emoji.as_str()).to_owned()
}

/// The key that toggles the reaction shown as `group`: its key when the
/// user is among its senders, so toggling removes it; otherwise, as
/// [`reaction_key`] chooses, the key of the user's own reaction (`own`)
/// with the same emoji, and else the group's key, so the user's reaction
/// joins the group (`❤ 2` becomes `❤ 3`, not `❤ 2  ❤️ 1`).
fn shown_reaction_key<'a>(own: impl Iterator<Item = &'a str>, group: &ReactionGroup) -> String {
    if group.mine {
        return group.key.clone();
    }
    Emoji::get(&group.key)
        .and_then(|emoji| own_key(own, emoji))
        .unwrap_or(&group.key)
        .to_owned()
}

/// The key of the user's own reaction (`own`) with `emoji`, compared
/// through [`Emoji::get`].
fn own_key<'a>(mut own: impl Iterator<Item = &'a str>, emoji: Emoji) -> Option<&'a str> {
    own.find(|key| Emoji::get(key) == Some(emoji))
}
