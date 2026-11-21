//! The open room's timeline: its diffs, the cursor, back pagination, read
//! receipts, and the message actions on the item under the cursor.

use super::{App, Confirm, Focus, Motion, StatusKind, View};
use crate::effect::{Effect, MatrixEffect, UiEffect, ViewContent};
use crate::ids::{EventId, TimelineId, Timestamp};
use crate::msg::Pane;
use crate::rich::is_web_url;
use crate::settings::ImagePreview;
use crate::timeline::{
    ItemRef, MediaBytes, MessageContent, OwnEvent, SendState, TimelineDiff, TimelineEntry,
    TimelineItem, apply_diff,
};

/// Back pagination starts when the cursor has at most this many older items
/// (design, "Matrix adapter").
const PAGINATE_WITHIN: usize = 10;

/// Why an action that needs the item's remote event refuses a local echo.
pub(super) const NOT_SENT: &str = "Wait until the message is sent";

/// The open room's main timeline state; opening a room starts a new one.
#[derive(Default)]
pub(super) struct Timeline {
    entries: Vec<TimelineEntry>,
    /// The item under the cursor; `None` while the cursor follows the newest item.
    anchor: Option<Anchor>,
    pagination: Pagination,
    /// The event of the last receipt requested.
    receipt: Option<EventId>,
    /// The latest time mxt marked the room read up to since it opened the
    /// room: the timestamp of the item of its latest receipt, or the room's
    /// latest activity at `room.mark_read`. Thread roots with a newer reply
    /// are marked unread (design, "Threads").
    read_up_to: Option<Timestamp>,
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

    /// The cursor follows the newest item again.
    pub(super) fn follow(&mut self) {
        self.anchor = None;
    }

    /// Whether the room's start is loaded.
    pub(super) fn reached_start(&self) -> bool {
        self.pagination == Pagination::ReachedStart
    }

    pub(super) fn read_up_to(&self) -> Option<Timestamp> {
        self.read_up_to
    }

    /// The room is read up to `time`, unless it was read up to a later time.
    pub(super) fn read_up_to_at_least(&mut self, time: Timestamp) {
        self.read_up_to = self.read_up_to.max(Some(time));
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

    pub(super) fn event_position(&self, event: &EventId) -> Option<usize> {
        self.entries.iter().position(|entry| {
            as_item(entry)
                .is_some_and(|item| matches!(&item.id, ItemRef::Remote(id) if id == event))
        })
    }

    /// The newest remote item: the event a receipt goes on, with its
    /// timestamp.
    fn newest_remote_event(&self) -> Option<(&EventId, Timestamp)> {
        self.entries.iter().rev().find_map(|entry| {
            let item = as_item(entry)?;
            match &item.id {
                ItemRef::Remote(event) => Some((event, item.timestamp)),
                ItemRef::Local(_) => None,
            }
        })
    }
}

impl App {
    /// `Msg::Timeline`: only the open room's main timeline's diffs count.
    pub(super) fn timeline_diffs(
        &mut self,
        timeline: &TimelineId,
        diffs: Vec<TimelineDiff>,
        effects: &mut Vec<Effect>,
    ) {
        if !self.shows(timeline) {
            return;
        }
        self.timeline.apply(diffs);
        self.check_jump_wait(effects);
        self.check_pagination(effects);
        self.check_receipt(effects);
    }

    /// `Msg::PaginationDone`. Pagination goes on while the cursor is still
    /// near the oldest item, so a cursor that stays there keeps loading.
    pub(super) fn pagination_done(
        &mut self,
        timeline: &TimelineId,
        reached_start: bool,
        effects: &mut Vec<Effect>,
    ) {
        if !self.shows(timeline) {
            return;
        }
        self.timeline.pagination = if reached_start {
            Pagination::ReachedStart
        } else {
            Pagination::Idle
        };
        self.check_jump_wait(effects);
        self.check_pagination(effects);
    }

    /// `Msg::PaginationFailed`. The next cursor move or timeline change may
    /// request again, so a persistent failure does not repeat in a loop.
    pub(super) fn pagination_failed(&mut self, timeline: &TimelineId, error: String) {
        if !self.shows(timeline) {
            return;
        }
        if self.timeline.pagination == Pagination::InFlight {
            self.timeline.pagination = Pagination::Idle;
        }
        // The failure's status says why the jump stops short.
        self.jump_wait = None;
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
        if matches!(motion, Motion::First | Motion::Last) {
            // `gg` and `G` are jumps (design, "Jump list").
            self.record_jump();
        }
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
        let timeline = &self.timeline;
        let older = timeline.cursor().map_or(0, |cursor| {
            timeline.entries[..cursor]
                .iter()
                .filter(|entry| is_item(entry))
                .take(PAGINATE_WITHIN + 1)
                .count()
        });
        if older <= PAGINATE_WITHIN {
            self.paginate(effects);
        }
    }

    /// Requests back pagination of the shown timeline, unless its start is
    /// loaded or a request is in flight.
    pub(super) fn paginate(&mut self, effects: &mut Vec<Effect>) {
        let Some(timeline) = self.shown_timeline() else {
            return;
        };
        if self.timeline.pagination == Pagination::Idle {
            self.timeline.pagination = Pagination::InFlight;
            effects.push(Effect::Matrix(MatrixEffect::PaginateBackwards(timeline)));
        }
    }

    /// Requests a receipt for the newest remote event once (design, "Read
    /// receipts") while the terminal is focused and the timeline's last
    /// viewport includes the newest item, clears the room's counts (design,
    /// "Optimistic clearing"), and marks the room read up to the event's
    /// time for the thread roots' marks.
    pub(super) fn check_receipt(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(viewport)) = (&self.open_room, self.timeline_viewport) else {
            return;
        };
        if !self.terminal_focused {
            return;
        }
        let Some(newest) = newest_item(self.timeline.entries()) else {
            return;
        };
        if !(viewport.first..=viewport.last).contains(&newest) {
            return;
        }
        let Some((event, time)) = self
            .timeline
            .newest_remote_event()
            .filter(|&(event, _)| self.timeline.receipt.as_ref() != Some(event))
        else {
            return;
        };
        let (room, event) = (room.clone(), event.clone());
        self.timeline.receipt = Some(event.clone());
        self.timeline.read_up_to_at_least(time);
        self.clear_room_unread(&room);
        effects.push(Effect::Matrix(MatrixEffect::SendReceipt { room, event }));
    }

    /// `message.view`: the item in the full-screen view. An image shows its
    /// details, and its image is fetched unless previews are off; other
    /// items show their text. An image that is still a local echo is
    /// refused: the adapter fetches media by remote event.
    pub(super) fn view_message(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        let content = match &item.content {
            MessageContent::Image { .. } if matches!(item.id, ItemRef::Local(_)) => {
                return self.set_status(StatusKind::Error, NOT_SENT);
            }
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
        self.view = Some(View::Message(item.id.clone()));
    }

    /// `help.commands`, `help.keymap` and `status.messages`: `text` in the
    /// full-screen view, in place of an open view.
    pub(super) fn view_help(&mut self, text: String, effects: &mut Vec<Effect>) {
        effects.push(Effect::Ui(UiEffect::OpenView(ViewContent::Text(text))));
        self.view = Some(View::Help);
    }

    /// `view.close`.
    pub(super) fn close_view(&mut self, effects: &mut Vec<Effect>) {
        if self.view.take().is_some() {
            effects.push(Effect::Ui(UiEffect::CloseView));
        }
    }

    /// Whether the open view is the message view of `item`.
    fn shows_message(&self, item: &ItemRef) -> bool {
        matches!(&self.view, Some(View::Message(viewed)) if viewed == item)
    }

    /// `Msg::ImageFetched`: only for the item the open view shows; an image
    /// for a closed view or another item is dropped.
    pub(super) fn image_fetched(
        &mut self,
        item: ItemRef,
        bytes: MediaBytes,
        effects: &mut Vec<Effect>,
    ) {
        if self.shows_message(&item) {
            effects.push(Effect::Ui(UiEffect::ShowImage { item, bytes }));
        }
    }

    /// `Msg::ImageUnavailable`: the reason, while the view shows the item.
    /// Info, because the reason is expected (the file is too large to
    /// preview); a failed fetch comes as `Msg::EffectFailed`.
    pub(super) fn image_unavailable(&mut self, item: &ItemRef, reason: &str) {
        if self.shows_message(item) {
            self.set_status(StatusKind::Info, format!("No image preview: {reason}"));
        }
    }

    /// `message.open`: downloads an attachment, opens the only `http://` or
    /// `https://` link, or picks one of several. Links with other schemes are
    /// left out (see [`is_web_url`]). An attachment that is still a local
    /// echo is refused: the adapter downloads by remote event. A download
    /// shows `Downloading <name>` until it is saved, which takes a while for
    /// a large file.
    pub(super) fn open_message(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        let mut links: Vec<String> = match &item.content {
            MessageContent::Image { name, .. } | MessageContent::File { name, .. } => {
                if let ItemRef::Local(_) = item.id {
                    return self.set_status(StatusKind::Error, NOT_SENT);
                }
                effects.push(Effect::Matrix(MatrixEffect::Download {
                    room: room.clone(),
                    item: item.id.clone(),
                }));
                return self.set_status(StatusKind::Info, format!("Downloading {name}"));
            }
            MessageContent::Text { rich, .. }
            | MessageContent::Notice(rich)
            | MessageContent::Emote(rich) => rich
                .links()
                .into_iter()
                .filter(|url| is_web_url(url))
                .map(str::to_owned)
                .collect(),
            MessageContent::Sticker { .. }
            | MessageContent::Redacted
            | MessageContent::Undecryptable
            | MessageContent::Unsupported { .. } => Vec::new(),
        };
        if links.len() > 1 {
            return self.pick_link(links);
        }
        match links.pop() {
            Some(url) => self.open_url(url, effects),
            None => self.set_status(StatusKind::Error, "Nothing to open"),
        }
    }

    /// Opens `url`, which the timeline may show only as link text, so the
    /// status line shows where it goes.
    pub(super) fn open_url(&mut self, url: String, effects: &mut Vec<Effect>) {
        self.set_status(StatusKind::Info, format!("Opening {url}"));
        effects.push(Effect::Ui(UiEffect::OpenUrl(url)));
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
    /// what belongs to the previous one: its viewport, which must not count
    /// for the new room's entries before the new room is drawn, the wait for
    /// a jump's message, a message view (a help view stays), the picker, a
    /// confirmation, and the focus and mention menu of its composer.
    pub(super) fn reset_timeline(&mut self, effects: &mut Vec<Effect>) {
        self.timeline = Timeline::default();
        self.timeline_viewport = None;
        self.jump_wait = None;
        if matches!(self.view, Some(View::Message(_))) {
            self.close_view(effects);
        }
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
            Some(Some(index)) => {
                self.record_jump();
                self.place_timeline_cursor(index, effects);
            }
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
