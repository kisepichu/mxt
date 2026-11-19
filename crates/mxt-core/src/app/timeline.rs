//! The open room's timeline: its diffs, the cursor, back pagination, read
//! receipts, and the message actions on the item under the cursor.

use super::{App, Confirm, Focus, Motion, StatusKind};
use crate::effect::{Effect, MatrixEffect, UiEffect, ViewContent};
use crate::ids::{EventId, RoomId};
use crate::msg::Pane;
use crate::settings::ImagePreview;
use crate::timeline::{
    ItemRef, MediaBytes, MessageContent, OwnEvent, SendState, TimelineDiff, TimelineEntry,
    TimelineItem, apply_diff,
};

/// Back pagination starts when the cursor has at most this many older items
/// (design, "Matrix adapter").
const PAGINATE_WITHIN: usize = 10;

/// The open room's timeline state; opening a room starts a new one.
#[derive(Default)]
pub(super) struct Timeline {
    entries: Vec<TimelineEntry>,
    /// The item under the cursor; `None` while the cursor follows the newest item.
    anchor: Option<Anchor>,
    pagination: Pagination,
    /// The event of the last receipt requested.
    receipt: Option<EventId>,
}

/// An item and its index in the entries.
struct Anchor {
    index: usize,
    item: ItemRef,
}

/// Back pagination of the open room's timeline.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Pagination {
    #[default]
    Idle,
    /// A `PaginateBackwards` request is in flight.
    InFlight,
    /// The room's start is loaded.
    ReachedStart,
}

impl Timeline {
    pub(super) fn entries(&self) -> &[TimelineEntry] {
        &self.entries
    }

    pub(super) fn cursor(&self) -> Option<usize> {
        match &self.anchor {
            Some(anchor) => Some(anchor.index),
            None => newest_item(&self.entries),
        }
    }

    pub(super) fn follows(&self) -> bool {
        self.anchor.is_none()
    }

    pub(super) fn cursor_item(&self) -> Option<&TimelineItem> {
        self.cursor()
            .and_then(|index| as_item(&self.entries[index]))
    }

    /// Anchors the cursor to the item at `index`.
    fn place(&mut self, index: usize) {
        if let Some(item) = self.entries.get(index).and_then(as_item) {
            self.anchor = Some(Anchor {
                index,
                item: item.id.clone(),
            });
        }
    }

    /// Applies `diffs` in order. An anchored cursor follows its item's index
    /// through them, so inserted older items do not move it to another
    /// message, and a local echo that the SDK removes and replaces by its
    /// remote event in the same batch keeps it.
    fn apply(&mut self, diffs: Vec<TimelineDiff>) {
        let mut index = self.anchor.as_ref().map(|anchor| anchor.index);
        for diff in diffs {
            if matches!(diff, TimelineDiff::Clear | TimelineDiff::Reset(_))
                && self.pagination == Pagination::ReachedStart
            {
                // The new entries may not reach back to the room's start.
                self.pagination = Pagination::Idle;
            }
            if let Some(index) = &mut index {
                *index = shifted(*index, &diff, self.entries.len());
            }
            apply_diff(&mut self.entries, diff);
        }
        if let (Some(anchor), Some(index)) = (self.anchor.take(), index) {
            self.anchor = self.settle(anchor.item, index);
        }
    }

    /// The anchor after a batch of diffs: the item at `index` (where the
    /// anchored item would be), else the item wherever it moved, else the
    /// nearest item at or after `index`, else none (the cursor follows).
    fn settle(&self, item: ItemRef, index: usize) -> Option<Anchor> {
        let is_anchored = |entry: &TimelineEntry| as_item(entry).is_some_and(|i| i.id == item);
        let index = if self.entries.get(index).is_some_and(is_anchored) {
            index
        } else {
            self.entries.iter().position(is_anchored).or_else(|| {
                let after = self.entries.get(index..)?.iter().position(is_item)?;
                Some(index + after)
            })?
        };
        let item = as_item(&self.entries[index])?.id.clone();
        Some(Anchor { index, item })
    }

    fn event_position(&self, event: &EventId) -> Option<usize> {
        self.entries.iter().position(|entry| {
            as_item(entry)
                .is_some_and(|item| matches!(&item.id, ItemRef::Remote(id) if id == event))
        })
    }
}

impl App {
    /// `Msg::Timeline`: only the open room's diffs count.
    pub(super) fn timeline_diffs(
        &mut self,
        room: RoomId,
        diffs: Vec<TimelineDiff>,
        effects: &mut Vec<Effect>,
    ) {
        if self.open_room.as_ref() != Some(&room) {
            return;
        }
        self.timeline.apply(diffs);
        self.check_pagination(effects);
        self.check_receipt(effects);
    }

    /// `Msg::PaginationDone`. Pagination goes on while the cursor is still
    /// near the oldest item, so a cursor that stays there keeps loading.
    pub(super) fn pagination_done(
        &mut self,
        room: RoomId,
        reached_start: bool,
        effects: &mut Vec<Effect>,
    ) {
        if self.open_room.as_ref() != Some(&room) {
            return;
        }
        self.timeline.pagination = if reached_start {
            Pagination::ReachedStart
        } else {
            Pagination::Idle
        };
        self.check_pagination(effects);
    }

    /// `Msg::PaginationFailed`. The next cursor move or timeline change may
    /// request again, so a persistent failure does not repeat in a loop.
    pub(super) fn pagination_failed(&mut self, room: RoomId, error: String) {
        if self.open_room.as_ref() != Some(&room) {
            return;
        }
        if self.timeline.pagination == Pagination::InFlight {
            self.timeline.pagination = Pagination::Idle;
        }
        self.set_status(
            StatusKind::Error,
            format!("Could not load older messages: {error}"),
        );
    }

    /// Moves the timeline cursor among items, clamped, without wrapping.
    /// Moving up leaves the follow mode; reaching the newest item moving down
    /// returns to it.
    pub(super) fn move_timeline_cursor(&mut self, motion: Motion, effects: &mut Vec<Effect>) {
        let Some(cursor) = self.timeline.cursor() else {
            return;
        };
        let half_pages =
            |count: u32| (count as usize).saturating_mul(self.half_page(Pane::Timeline));
        let entries = self.timeline.entries();
        let newest = newest_item(entries);
        let (target, down) = match motion {
            Motion::Up(count) => (older(entries, cursor, count as usize), false),
            Motion::HalfPageUp(count) => (older(entries, cursor, half_pages(count)), false),
            Motion::Down(count) => (newer(entries, cursor, count as usize), true),
            Motion::HalfPageDown(count) => (newer(entries, cursor, half_pages(count)), true),
            Motion::First => (entries.iter().position(is_item).unwrap_or(cursor), false),
            Motion::Last => (newest.unwrap_or(cursor), true),
        };
        if down && Some(target) == newest {
            self.timeline.anchor = None;
        } else {
            self.timeline.place(target);
        }
        self.check_pagination(effects);
    }

    /// Anchors the timeline cursor to the item at `index` (a search match, a
    /// reply target).
    pub(super) fn place_timeline_cursor(&mut self, index: usize, effects: &mut Vec<Effect>) {
        self.timeline.place(index);
        self.check_pagination(effects);
    }

    /// Requests back pagination when the cursor has at most
    /// [`PAGINATE_WITHIN`] older items, or there are no items yet, unless the
    /// room's start is loaded or a request is in flight.
    fn check_pagination(&mut self, effects: &mut Vec<Effect>) {
        let Some(room) = &self.open_room else {
            return;
        };
        let timeline = &mut self.timeline;
        if timeline.pagination != Pagination::Idle {
            return;
        }
        let older = timeline.cursor().map_or(0, |cursor| {
            timeline.entries[..cursor]
                .iter()
                .filter(|entry| is_item(entry))
                .take(PAGINATE_WITHIN + 1)
                .count()
        });
        if older <= PAGINATE_WITHIN {
            timeline.pagination = Pagination::InFlight;
            effects.push(Effect::Matrix(MatrixEffect::PaginateBackwards(
                room.clone(),
            )));
        }
    }

    /// Requests a receipt for the newest remote event once (design, "Read
    /// receipts") while the terminal is focused and the timeline's last
    /// viewport includes the newest item, and clears the room's counts
    /// (design, "Optimistic clearing").
    pub(super) fn check_receipt(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(viewport)) = (&self.open_room, self.timeline_viewport) else {
            return;
        };
        if !self.terminal_focused {
            return;
        }
        let entries = self.timeline.entries();
        let Some(newest) = newest_item(entries) else {
            return;
        };
        if !(viewport.first..=viewport.last).contains(&newest) {
            return;
        }
        let event = entries
            .iter()
            .rev()
            .find_map(|entry| match &as_item(entry)?.id {
                ItemRef::Remote(event) => Some(event),
                ItemRef::Local(_) => None,
            });
        let Some(event) = event.filter(|&event| self.timeline.receipt.as_ref() != Some(event))
        else {
            return;
        };
        let (room, event) = (room.clone(), event.clone());
        self.timeline.receipt = Some(event.clone());
        self.rooms.clear_unread(&room);
        effects.push(Effect::Matrix(MatrixEffect::SendReceipt { room, event }));
    }

    /// `message.view`: the item in the full-screen view. An image shows its
    /// details, and its image is fetched unless previews are off; other
    /// items show their text.
    pub(super) fn view_message(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        let content = match &item.content {
            MessageContent::Image {
                name,
                size,
                dimensions,
            } => ViewContent::Image {
                item: item.id.clone(),
                name: name.clone(),
                size: *size,
                dimensions: *dimensions,
            },
            content => ViewContent::Text(content.plain_text()),
        };
        let fetch = matches!(content, ViewContent::Image { .. })
            && self.settings.image_preview != ImagePreview::Off;
        effects.push(Effect::Ui(UiEffect::OpenView(content)));
        if fetch {
            effects.push(Effect::Matrix(MatrixEffect::FetchImage {
                room: room.clone(),
                item: item.id.clone(),
            }));
        }
        self.view = Some(item.id.clone());
    }

    /// `view.close`.
    pub(super) fn close_view(&mut self, effects: &mut Vec<Effect>) {
        if self.view.take().is_some() {
            effects.push(Effect::Ui(UiEffect::CloseView));
        }
    }

    /// `Msg::ImageFetched`: only for the item the open view shows; an image
    /// for a closed view or another item is dropped.
    pub(super) fn image_fetched(
        &mut self,
        item: ItemRef,
        bytes: MediaBytes,
        effects: &mut Vec<Effect>,
    ) {
        if self.view.as_ref() == Some(&item) {
            effects.push(Effect::Ui(UiEffect::ShowImage { item, bytes }));
        }
    }

    /// `Msg::ImageUnavailable`: the reason, while the view shows the item.
    pub(super) fn image_unavailable(&mut self, item: &ItemRef, reason: &str) {
        if self.view.as_ref() == Some(item) {
            self.set_status(StatusKind::Error, format!("No image preview: {reason}"));
        }
    }

    /// `message.open`: downloads an attachment, opens the only link, or picks
    /// one of several links.
    pub(super) fn open_message(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        let mut links: Vec<String> = match &item.content {
            MessageContent::Image { .. } | MessageContent::File { .. } => {
                return effects.push(Effect::Matrix(MatrixEffect::Download {
                    room: room.clone(),
                    item: item.id.clone(),
                }));
            }
            MessageContent::Text { rich, .. }
            | MessageContent::Notice(rich)
            | MessageContent::Emote(rich) => rich.links().into_iter().map(str::to_owned).collect(),
            MessageContent::Sticker { .. }
            | MessageContent::Redacted
            | MessageContent::Undecryptable
            | MessageContent::Unsupported { .. } => Vec::new(),
        };
        if links.len() > 1 {
            return self.pick_link(links);
        }
        match links.pop() {
            Some(url) => effects.push(Effect::Ui(UiEffect::OpenUrl(url))),
            None => self.set_status(StatusKind::Error, "Nothing to open"),
        }
    }

    /// `Msg::Downloaded`: opens the saved file.
    pub(super) fn downloaded(&mut self, path: String, effects: &mut Vec<Effect>) {
        self.set_status(StatusKind::Info, format!("Saved {path}"));
        effects.push(Effect::Ui(UiEffect::OpenPath(path)));
    }

    /// `file.upload`: sends the file at `path` to the open room.
    pub(super) fn upload(&mut self, path: &str, effects: &mut Vec<Effect>) {
        match &self.open_room {
            Some(room) => effects.push(Effect::Matrix(MatrixEffect::Upload {
                room: room.clone(),
                path: path.to_owned(),
            })),
            None => self.set_status(StatusKind::Error, "No room is open"),
        }
    }

    /// Starts a new timeline when another room or no room is open, and closes
    /// what belongs to the previous one: the message view, the picker, a
    /// confirmation, and the focus and mention menu of its composer.
    pub(super) fn reset_timeline(&mut self, effects: &mut Vec<Effect>) {
        self.timeline = Timeline::default();
        self.close_view(effects);
        self.picker = None;
        self.confirm = None;
        self.mention_menu = None;
        if self.focus == Focus::Composer {
            self.focus = Focus::Timeline;
        }
    }

    /// `message.yank`: the item's plain text.
    pub(super) fn yank_message(&mut self, effects: &mut Vec<Effect>) {
        if let Some(item) = self.timeline.cursor_item() {
            effects.push(Effect::Ui(UiEffect::Yank(item.content.plain_text())));
        }
    }

    /// `message.goto_reply`: the cursor moves to the replied-to message if it
    /// is loaded.
    pub(super) fn goto_reply(&mut self, effects: &mut Vec<Effect>) {
        let Some(item) = self.timeline.cursor_item() else {
            return;
        };
        let target = item
            .reply
            .as_ref()
            .map(|reply| self.timeline.event_position(&reply.event));
        match target {
            Some(Some(index)) => self.place_timeline_cursor(index, effects),
            Some(None) => {
                self.set_status(StatusKind::Error, "The replied-to message is not loaded");
            }
            None => self.set_status(StatusKind::Error, "The message is not a reply"),
        }
    }

    /// `message.delete`: the user's own item under the cursor, after `y` in
    /// `CONFIRM` mode, because a redaction cannot be undone. A deleted item
    /// is refused.
    pub(super) fn delete_message(&mut self) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        if item.content == MessageContent::Redacted {
            return self.set_status(StatusKind::Error, "The message is deleted");
        }
        let Some(own) = OwnEvent::new(item, &self.me) else {
            return self.set_status(StatusKind::Error, "You can only delete your own messages");
        };
        self.confirm = Some(Confirm::Redact {
            room: room.clone(),
            item: own.item().clone(),
        });
    }

    /// `message.resend`: the failed local echo under the cursor.
    pub(super) fn resend_message(&mut self, effects: &mut Vec<Effect>) {
        let failed = match (&self.open_room, self.timeline.cursor_item()) {
            (
                Some(room),
                Some(TimelineItem {
                    id: ItemRef::Local(txn),
                    send_state: Some(SendState::Failed { .. }),
                    ..
                }),
            ) => Some((room.clone(), txn.clone())),
            _ => None,
        };
        match failed {
            Some((room, txn)) => effects.push(Effect::Matrix(MatrixEffect::Resend { room, txn })),
            None => self.set_status(StatusKind::Error, "Nothing to resend"),
        }
    }
}

fn as_item(entry: &TimelineEntry) -> Option<&TimelineItem> {
    match entry {
        TimelineEntry::Item(item) => Some(item),
        _ => None,
    }
}

fn is_item(entry: &TimelineEntry) -> bool {
    matches!(entry, TimelineEntry::Item(_))
}

fn newest_item(entries: &[TimelineEntry]) -> Option<usize> {
    entries.iter().rposition(is_item)
}

/// The item `count` items older than the one at `from`, or the oldest item.
fn older(entries: &[TimelineEntry], from: usize, count: usize) -> usize {
    (0..from)
        .rev()
        .filter(|&index| is_item(&entries[index]))
        .take(count)
        .last()
        .unwrap_or(from)
}

/// The item `count` items newer than the one at `from`, or the newest item.
fn newer(entries: &[TimelineEntry], from: usize, count: usize) -> usize {
    (from + 1..entries.len())
        .filter(|&index| is_item(&entries[index]))
        .take(count)
        .last()
        .unwrap_or(from)
}

/// Where the entry at `index` is after `diff`, which applies to `len`
/// entries. If the diff removes the entry, the index of the entry that takes
/// its place. An insert at `index` itself does not shift it: the SDK replaces
/// a local echo by removing it and inserting its remote event at the same
/// index, and an entry still there after the insert is found by its ID.
fn shifted(index: usize, diff: &TimelineDiff, len: usize) -> usize {
    match *diff {
        TimelineDiff::PushFront(_) => index + 1,
        TimelineDiff::PopFront if len > 0 => index.saturating_sub(1),
        TimelineDiff::Insert(at, _) if at <= len && at < index => index + 1,
        TimelineDiff::Remove(at) if at < len && at < index => index - 1,
        _ => index,
    }
}
