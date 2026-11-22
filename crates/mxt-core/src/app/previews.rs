//! The timeline previews' queue (design, "Images in the timeline"): the
//! previews the tui adapter wants for the shown timeline, fetched at most
//! [`IN_FLIGHT`] at a time, and the answers passed back to it.

use std::collections::{HashSet, VecDeque};

use super::App;
use crate::effect::{Effect, MatrixEffect, UiEffect};
use crate::ids::{RoomId, TimelineId};
use crate::timeline::{ItemRef, MediaBytes, MessageContent, TimelineEntry};

/// The most `MatrixEffect::FetchPreview`s in flight at a time.
const IN_FLIGHT: usize = 4;

/// The open room's previews being fetched. Opening another room or losing
/// the open one drops it, and the answers for it with it.
pub(super) struct PreviewQueue {
    room: RoomId,
    /// The latest wanted list of the shown timeline, without the items in
    /// flight, in its order: the next fetch takes the first.
    wanted: VecDeque<ItemRef>,
    /// The items whose `FetchPreview` is not answered yet.
    in_flight: Vec<ItemRef>,
}

impl PreviewQueue {
    /// Fetches the first wanted items while fewer than [`IN_FLIGHT`] are in
    /// flight.
    fn start(&mut self, effects: &mut Vec<Effect>) {
        while self.in_flight.len() < IN_FLIGHT
            && let Some(item) = self.wanted.pop_front()
        {
            effects.push(Effect::Matrix(MatrixEffect::FetchPreview {
                room: self.room.clone(),
                item: item.clone(),
            }));
            self.in_flight.push(item);
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
        let queue = match &mut self.previews {
            Some(queue) if queue.room == timeline.room => queue,
            unset => unset.insert(PreviewQueue {
                room: timeline.room.clone(),
                wanted: VecDeque::new(),
                in_flight: Vec::new(),
            }),
        };
        queue.wanted = items
            .into_iter()
            .filter(|item| images.contains(item) && !queue.in_flight.contains(item))
            .collect();
        queue.start(effects);
    }

    /// `Msg::PreviewFetched` (`bytes`) and `Msg::PreviewFailed` (`None`):
    /// the answer for an item in flight in the open room's queue goes to
    /// the tui adapter as `UiEffect::ShowPreview` or
    /// `UiEffect::PreviewFailed`, and the next wanted item is fetched. An
    /// answer for another room, or for a queue that was dropped since, is
    /// dropped.
    pub(super) fn preview_answered(
        &mut self,
        room: &RoomId,
        item: ItemRef,
        bytes: Option<MediaBytes>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(queue) = self.previews.as_mut().filter(|queue| queue.room == *room) else {
            return;
        };
        let Some(index) = queue.in_flight.iter().position(|flying| *flying == item) else {
            return;
        };
        queue.in_flight.swap_remove(index);
        effects.push(Effect::Ui(match bytes {
            Some(bytes) => UiEffect::ShowPreview { item, bytes },
            None => UiEffect::PreviewFailed(item),
        }));
        queue.start(effects);
    }

    /// The pane shows another timeline of the open room (a thread opened or
    /// closed): nothing of the last one's wanted list is fetched before the
    /// tui adapter reports the new timeline's. The items in flight are
    /// answered as before.
    pub(super) fn drop_wanted_previews(&mut self) {
        if let Some(queue) = &mut self.previews {
            queue.wanted.clear();
        }
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
