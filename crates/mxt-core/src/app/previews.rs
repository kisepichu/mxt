//! The timeline previews' queue (design, "Images in the timeline"): the
//! previews the tui adapter wants for the shown timeline, fetched at most
//! [`IN_FLIGHT`] at a time across every room, and the answers passed back
//! to it.

use std::collections::{HashSet, VecDeque};

use super::App;
use crate::effect::{Effect, MatrixEffect, UiEffect};
use crate::ids::{RoomId, TimelineId};
use crate::timeline::{ItemRef, MediaBytes, MessageContent, TimelineEntry};

/// The most `MatrixEffect::FetchPreview`s in flight at a time, whatever
/// room they are for.
const IN_FLIGHT: usize = 4;

/// The previews being fetched. Showing another timeline drops the wanted
/// list; the fetches in flight stay counted until their answers arrive, so
/// switching rooms quickly never has more than [`IN_FLIGHT`] in flight.
#[derive(Default)]
pub(super) struct PreviewQueue {
    /// The latest wanted list of the shown timeline, with its room, without
    /// the items in flight, in its order: the next fetch takes the first.
    wanted: Option<(RoomId, VecDeque<ItemRef>)>,
    /// The items whose `FetchPreview` is not answered yet, with their rooms:
    /// the open room's and those of rooms open before.
    in_flight: Vec<(RoomId, ItemRef)>,
}

impl PreviewQueue {
    /// Fetches the first wanted items while fewer than [`IN_FLIGHT`] are in
    /// flight.
    fn start(&mut self, effects: &mut Vec<Effect>) {
        let Some((room, wanted)) = &mut self.wanted else {
            return;
        };
        while self.in_flight.len() < IN_FLIGHT
            && let Some(item) = wanted.pop_front()
        {
            effects.push(Effect::Matrix(MatrixEffect::FetchPreview {
                room: room.clone(),
                item: item.clone(),
            }));
            self.in_flight.push((room.clone(), item));
        }
    }
}

impl App {
    /// `Msg::PreviewsWanted`: for the shown timeline, `items` replace the
    /// wanted list, without the items in flight, which are never fetched
    /// twice (the tui adapter does not know what is in flight), and without
    /// the items that are no remote image or sticker of the shown timeline:
    /// a local echo, which the matrix adapter cannot fetch by its event, and
    /// any other content. Fetches start up to [`IN_FLIGHT`]. A list for a
    /// timeline that is not shown is dropped.
    pub(super) fn previews_wanted(
        &mut self,
        timeline: &TimelineId,
        items: Vec<ItemRef>,
        effects: &mut Vec<Effect>,
    ) {
        if !self.shows(timeline) {
            return;
        }
        let images = images_among(self.timeline.entries(), &items);
        let queue = &mut self.previews;
        let wanted = items
            .into_iter()
            .filter(|item| {
                images.contains(item)
                    && !queue
                        .in_flight
                        .iter()
                        .any(|(room, flying)| *room == timeline.room && flying == item)
            })
            .collect();
        queue.wanted = Some((timeline.room.clone(), wanted));
        queue.start(effects);
    }

    /// `Msg::PreviewFetched` (`bytes`) and `Msg::PreviewFailed` (`None`):
    /// the answer for an item in flight frees its slot, and the next wanted
    /// item is fetched. While its room is the open room it goes to the tui
    /// adapter as `UiEffect::ShowPreview` or `UiEffect::PreviewFailed`,
    /// which drops the bytes of an item it no longer wants; an answer for a
    /// room that is not open is dropped, and so is one for nothing in
    /// flight.
    pub(super) fn preview_answered(
        &mut self,
        room: &RoomId,
        item: ItemRef,
        bytes: Option<MediaBytes>,
        effects: &mut Vec<Effect>,
    ) {
        let queue = &mut self.previews;
        let Some(index) = queue
            .in_flight
            .iter()
            .position(|(flying_room, flying)| flying_room == room && *flying == item)
        else {
            return;
        };
        queue.in_flight.swap_remove(index);
        if self.open_room.as_ref() == Some(room) {
            effects.push(Effect::Ui(match bytes {
                Some(bytes) => UiEffect::ShowPreview { item, bytes },
                None => UiEffect::PreviewFailed(item),
            }));
        }
        self.previews.start(effects);
    }

    /// The pane shows another timeline: a thread opened or closed, another
    /// room or none. Nothing of the last one's wanted list is fetched before
    /// the tui adapter reports the new timeline's. The items in flight keep
    /// their slots until they are answered.
    pub(super) fn drop_wanted_previews(&mut self) {
        self.previews.wanted = None;
    }
}

/// The remote image and sticker items of `entries` that `items` names.
fn images_among<'a>(entries: &'a [TimelineEntry], items: &[ItemRef]) -> HashSet<&'a ItemRef> {
    let items: HashSet<&ItemRef> = items.iter().collect();
    entries
        .iter()
        .filter_map(|entry| match entry {
            TimelineEntry::Item(item)
                if matches!(item.id, ItemRef::Remote(_))
                    && matches!(
                        item.content,
                        MessageContent::Image { .. } | MessageContent::Sticker { .. }
                    )
                    && items.contains(&item.id) =>
            {
                Some(&item.id)
            }
            _ => None,
        })
        .collect()
}
