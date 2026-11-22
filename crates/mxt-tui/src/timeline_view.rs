//! The timeline pane (design, "Timeline"): the shown timeline's entries, the
//! open room's or its thread's, oldest at the top. The cursor item stays in
//! view: when the cursor moves off the pane, the view scrolls just far
//! enough to bring its item in at the edge it moved towards, so it never
//! jumps. The newest entry is at the bottom while the cursor follows the
//! newest item and after a frame that showed the newest entry's last row. A
//! pane taller than in the last frame shows its new rows below the cursor
//! item first, so the newest entry is back at the bottom when it fits. Only
//! the entries around the cursor are laid out, so a long timeline costs no
//! more per frame. With previews on, an image item reserves rows for its
//! preview below its line (see [`crate::previews`]). After a search, the
//! matches of its pattern in the messages are marked (design, "Search and
//! jumps").

use std::borrow::Cow;
use std::collections::VecDeque;
use std::iter;
use std::ops::Range;

use jiff::fmt::strtime;
use jiff::tz::TimeZone;
use mxt_core::app::{App, Focus};
use mxt_core::ids::{TimelineId, Timestamp};
use mxt_core::rich::{self, Block, Inline, RichText};
use mxt_core::search::{self, Needle};
use mxt_core::timeline::{
    ItemRef, MessageContent, ReactionGroup, ReplyPreview, SendState, ThreadSummary, TimelineEntry,
    TimelineItem,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::previews::{Placed, PreviewLayout, Previews, Target};
use crate::view::{BOLD, DIM, ERROR, PLAIN, fit, grapheme_width, text_width};

/// `ui.time_format` when the configuration has none.
const DEFAULT_TIME_FORMAT: &str = "%H:%M";
/// The date on a date divider.
const DATE_FORMAT: &str = "%Y-%m-%d (%a)";
/// The date before a thread's latest reply time on another day than its
/// root.
const REPLY_DATE_FORMAT: &str = "%Y-%m-%d";
const READ_MARKER: &str = "new messages";
/// Before every row of a quote.
const QUOTE: &str = "│ ";
const CODE_BLOCK: Style = DIM;
/// Columns between tab stops.
const TAB_WIDTH: usize = 4;
/// Each column of spoiler text.
const SPOILER: char = '█';
/// Each column of spoiler text that a search match touches (design, "Search
/// and jumps"): reverse video or an underline would not show on `█`.
const MATCHED_SPOILER: char = '▒';
/// What image, file and sticker lines start with; the search marks the name
/// after it.
const IMAGE: &str = "[image: ";
const FILE: &str = "[file: ";
const STICKER: &str = "[sticker: ";
/// The newer-messages hint shows counts above this as `99+`.
pub(crate) const MAX_SHOWN_NEWER: usize = 99;
/// The fewest entries above and below the pane whose previews are fetched
/// ahead (design, "Fetching").
const MIN_MARGIN: usize = 5;

/// A checked `ui.time_format`, the `strftime`-style format of the item
/// headers' times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeFormat(String);

impl TimeFormat {
    /// Checks `ui.time_format`: an empty format and one that jiff cannot
    /// format are rejected. The configuration loader reports an empty one as
    /// an error and ignores the other with a warning, since a newer jiff may
    /// accept it (design, "Layers").
    pub fn parse(format: String) -> Result<TimeFormat, String> {
        if format.is_empty() {
            return Err("must not be empty".to_owned());
        }
        let sample = jiff::Timestamp::UNIX_EPOCH.to_zoned_datetime(TimeZone::UTC);
        strtime::format(&format, &sample).map_err(|error| error.to_string())?;
        Ok(TimeFormat(format))
    }
}

/// `%H:%M`, the default of `ui.time_format`.
impl Default for TimeFormat {
    fn default() -> TimeFormat {
        TimeFormat(DEFAULT_TIME_FORMAT.to_owned())
    }
}

/// What the timeline pane keeps between frames.
pub(crate) struct TimelinePane {
    time_format: TimeFormat,
    zone: TimeZone,
    /// Where the last frame left the view, so the next one keeps it while
    /// the cursor does not follow.
    anchor: Option<Anchor>,
    /// The timeline the last frame showed: another one (a thread opened or
    /// closed, another room) is placed as on a first frame.
    shown: Option<TimelineId>,
}

enum Anchor {
    /// The frame showed the newest entry, so the next one keeps the newest
    /// entry at the bottom: entries that arrive below then appear.
    End,
    /// The frame showed older entries only: the cursor item then, the rows
    /// below it and the pane's rows, so the view stays put while the cursor
    /// moves inside it and while entries arrive above or below it, and a
    /// taller pane shows its new rows below the cursor item first.
    Item {
        index: usize,
        item: ItemRef,
        below: usize,
        height: usize,
    },
}

impl TimelinePane {
    pub(crate) fn new() -> TimelinePane {
        TimelinePane {
            time_format: TimeFormat::default(),
            zone: TimeZone::system(),
            anchor: None,
            shown: None,
        }
    }

    pub(crate) fn set_time_format(&mut self, format: TimeFormat) {
        self.time_format = format;
    }

    /// Draws the shown timeline's entries into `area` and returns the first
    /// and last entry indices with a visible row. With `previews`, image
    /// items reserve rows for their previews, the kept previews are drawn
    /// unless they meet a rectangle of `covered` (the overlays drawn over
    /// the pane), and the targets on and around the pane are laid out.
    pub(crate) fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        app: &App,
        previews: Option<&mut Previews>,
        covered: &[Rect],
    ) -> Option<(usize, usize)> {
        // Compared without building the shown `TimelineId` on every frame.
        let same = match &self.shown {
            Some(shown) => app.shows(shown),
            None => app.open_room().is_none(),
        };
        if !same {
            self.shown = app.shown_timeline();
            self.anchor = None;
        }
        let entries = app.timeline();
        if entries.is_empty() {
            self.anchor = None;
            if let Some(previews) = previews {
                previews.set_targets(app, Vec::new());
            }
            return None;
        }
        // The cursor bar and a space on the left, a space on the right.
        let width = usize::from(area.width).saturating_sub(3);
        let height = usize::from(area.height);
        if width == 0 || height == 0 {
            return None;
        }
        let cursor = app.timeline_cursor();
        let render = Render {
            entries,
            width,
            height,
            time_format: &self.time_format.0,
            zone: &self.zone,
            thread_marks_after: app.thread_marks_after(),
            previews: previews.as_deref().map(Previews::layout),
            highlight: app.search_highlight(),
            cursor,
        };
        let window = render.lay_out(app.timeline_follows(), self.anchor.as_ref());

        let bar = match app.focus() {
            Focus::Timeline => Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
            Focus::RoomList | Focus::Composer => Span::raw("│"),
        };
        let buffer = frame.buffer_mut();
        let mut y = window.top;
        for (index, rows) in (window.first..).zip(&window.rows) {
            for row in &rows.lines {
                if let Ok(row_y) = u16::try_from(y)
                    && row_y < area.height
                {
                    let row_y = area.y + row_y;
                    if Some(index) == cursor {
                        buffer.set_span(area.x, row_y, &bar, 1);
                    }
                    buffer.set_line(area.x + 2, row_y, row, area.width - 3);
                }
                y += 1;
            }
        }

        self.anchor = render.anchor(&window);
        let pane_rows = render.pane_rows();
        let visible = window.visible(pane_rows);
        let targets = previews.as_deref().map(|previews| match visible {
            Some(range) => previews.choose_targets(window.on_pane(pane_rows), render.margin(range)),
            None => Vec::new(),
        });
        if let (Some(previews), Some(targets)) = (previews, targets) {
            let column = Rect {
                x: area.x + 2,
                width: area.width - 3,
                ..area
            };
            previews.draw(buffer, column, window.placed(), covered);
            previews.set_targets(app, targets);
        }
        visible
    }
}

/// The newer-messages hint's count (design, "Timeline"): the items after
/// the entry at `last`, the last with a row on the pane (as
/// `TimelinePane::draw` returns it). Messages of every content kind count,
/// local echoes too; date dividers, the read marker and hidden entries do
/// not. Counting stops at the 100th item after `last` (one past
/// `MAX_SHOWN_NEWER`).
pub(crate) fn newer_items(entries: &[TimelineEntry], last: usize) -> usize {
    entries
        .iter()
        .skip(last + 1)
        .filter(|entry| matches!(entry, TimelineEntry::Item(_)))
        .take(MAX_SHOWN_NEWER + 1)
        .count()
}

/// Consecutive entries and their rows, placed on the pane's rows.
struct Window<'a> {
    first: usize,
    rows: VecDeque<Rows<'a>>,
    /// The pane row of the first entry's first row; negative when that row
    /// is above the pane.
    top: isize,
    /// All rows of the window.
    len: isize,
}

impl<'a> Window<'a> {
    /// The window of the entry at `index` with its first row at `top`.
    fn new(index: usize, rows: Rows<'a>, top: isize) -> Window<'a> {
        let len = rows.height();
        Window {
            first: index,
            rows: VecDeque::from([rows]),
            top,
            len,
        }
    }

    fn last(&self) -> usize {
        self.first + self.rows.len() - 1
    }

    fn bottom(&self) -> isize {
        self.top + self.len
    }

    fn push_back(&mut self, rows: Rows<'a>) {
        self.len += rows.height();
        self.rows.push_back(rows);
    }

    fn push_front(&mut self, rows: Rows<'a>) {
        let len = rows.height();
        self.first -= 1;
        self.top -= len;
        self.len += len;
        self.rows.push_front(rows);
    }

    fn shift(&mut self, rows: isize) {
        self.top += rows;
    }

    fn len_of(&self, index: usize) -> Option<isize> {
        let rows = self.rows.get(index.checked_sub(self.first)?)?;
        Some(rows.height())
    }

    fn top_of(&self, index: usize) -> Option<isize> {
        let before = index.checked_sub(self.first)?;
        let above: isize = self.rows.iter().take(before).map(Rows::height).sum();
        (before < self.rows.len()).then_some(self.top + above)
    }

    /// Drops the entries without a row on the pane's `height` rows, keeping
    /// at least one.
    fn trim(&mut self, height: isize) {
        while self.rows.len() > 1
            && let Some(rows) = self.rows.front()
            && self.top + rows.height() <= 0
        {
            let len = rows.height();
            self.rows.pop_front();
            self.first += 1;
            self.top += len;
            self.len -= len;
        }
        while self.rows.len() > 1
            && let Some(rows) = self.rows.back()
            && self.bottom() - rows.height() >= height
        {
            self.len -= rows.height();
            self.rows.pop_back();
        }
    }

    /// The first and last entries with a row on the pane's `height` rows.
    fn visible(&self, height: isize) -> Option<(usize, usize)> {
        let mut y = self.top;
        let mut range: Option<(usize, usize)> = None;
        for (index, rows) in (self.first..).zip(&self.rows) {
            let len = rows.height();
            if len > 0 && y + len > 0 && y < height {
                range = Some((range.map_or(index, |(first, _)| first), index));
            }
            y += len;
        }
        range
    }

    /// The previews' slots, each with its entry's top row and rows.
    fn slots(&self) -> impl Iterator<Item = (isize, isize, &Slot<'a>)> {
        let mut y = self.top;
        self.rows.iter().filter_map(move |rows| {
            let (top, len) = (y, rows.height());
            y += len;
            rows.preview.as_ref().map(|slot| (top, len, slot))
        })
    }

    /// Where the previews go on the pane.
    fn placed(&self) -> impl Iterator<Item = Placed<'a>> {
        self.slots().map(|(top, _, slot)| Placed {
            item: slot.item,
            top: top + isize::try_from(slot.offset).unwrap_or(isize::MAX),
            target: slot.target,
        })
    }

    /// The targets of the remote items among the entries with a row on the
    /// pane's `height` rows, top to bottom.
    fn on_pane(&self, height: isize) -> impl Iterator<Item = (&'a ItemRef, Target)> {
        self.slots()
            .filter(move |&(top, len, slot)| {
                len > 0 && top + len > 0 && top < height && matches!(slot.item, ItemRef::Remote(_))
            })
            .map(|(_, _, slot)| (slot.item, slot.target))
    }
}

/// An entry's rows, and where its preview goes among them.
struct Rows<'a> {
    lines: Vec<Line<'a>>,
    preview: Option<Slot<'a>>,
}

impl<'a> Rows<'a> {
    fn plain(lines: Vec<Line<'a>>) -> Rows<'a> {
        Rows {
            lines,
            preview: None,
        }
    }

    fn height(&self) -> isize {
        isize::try_from(self.lines.len()).unwrap_or(isize::MAX)
    }
}

/// The blank rows an image item reserves for its preview: from `offset`,
/// as many as `target`'s height.
#[derive(Clone, Copy)]
struct Slot<'a> {
    item: &'a ItemRef,
    offset: usize,
    target: Target,
}

/// Renders entries for one frame.
struct Render<'a, 'p> {
    entries: &'a [TimelineEntry],
    width: usize,
    height: usize,
    time_format: &'a str,
    zone: &'a TimeZone,
    /// `App::thread_marks_after`: thread roots whose latest reply is newer
    /// are marked unread.
    thread_marks_after: Option<Timestamp>,
    /// With previews on, the rows they take.
    previews: Option<PreviewLayout<'p>>,
    /// `App::search_highlight`: its matches in the messages are marked.
    highlight: Option<&'a Needle>,
    /// The cursor's entry: it stays on the pane, and its matches are bold
    /// too.
    cursor: Option<usize>,
}

impl<'a> Render<'a, '_> {
    fn pane_rows(&self) -> isize {
        isize::try_from(self.height).unwrap_or(isize::MAX)
    }

    /// Places the entries around the cursor.
    fn lay_out(&self, follows: bool, anchor: Option<&Anchor>) -> Window<'a> {
        let height = self.pane_rows();
        let last = self.entries.len() - 1;
        let mut window = match self.cursor.filter(|_| !follows) {
            Some(cursor) => self.around(cursor, anchor),
            // Following: the newest entry at the bottom.
            None => self.at_bottom(last),
        };

        // The cursor item fully on the pane; its rows never exceed the pane.
        if let Some(cursor) = self.cursor
            && let (Some(top), Some(len)) = (window.top_of(cursor), window.len_of(cursor))
        {
            if top < 0 {
                window.shift(self.top_with_lead(cursor, len) - top);
            } else if top + len > height {
                window.shift(height - top - len);
            }
        }
        // Rows below, and the last entry at the bottom when they run out.
        while window.bottom() < height && window.last() < last {
            window.push_back(self.rows(window.last() + 1));
        }
        if window.bottom() < height {
            window.shift(height - window.bottom());
        }
        // Rows above. When they run out, the entries move up as far as the
        // ones below the pane allow; a timeline shorter than the pane stays
        // at the bottom.
        while window.top > 0 && window.first > 0 {
            window.push_front(self.rows(window.first - 1));
        }
        if window.top > 0 {
            while window.bottom() < height + window.top && window.last() < last {
                window.push_back(self.rows(window.last() + 1));
            }
            window.shift(-window.top.min(window.bottom() - height));
        }
        window.trim(height);
        window
    }

    /// The anchor for the next frame: the end, or the cursor item and the
    /// rows below it.
    fn anchor(&self, window: &Window) -> Option<Anchor> {
        let cursor = self.cursor?;
        let TimelineEntry::Item(item) = &self.entries[cursor] else {
            return None;
        };
        if self.shows_end(window) {
            return Some(Anchor::End);
        }
        let bottom = window.top_of(cursor)? + window.len_of(cursor)?;
        Some(Anchor::Item {
            index: cursor,
            item: item.id.clone(),
            below: usize::try_from(self.pane_rows() - bottom).unwrap_or(0),
            height: self.height,
        })
    }

    /// Whether the last entry with rows is in `window` with its last row on
    /// the pane; only hidden entries, which have none, may follow it.
    fn shows_end(&self, window: &Window) -> bool {
        window.bottom() <= self.pane_rows()
            && self.entries[window.last() + 1..]
                .iter()
                .all(|entry| matches!(entry, TimelineEntry::Hidden))
    }

    /// A window that holds the cursor item. The newest entry is at the
    /// bottom when the last frame showed it. Otherwise the cursor item is
    /// placed where the last frame had the anchor: with as many rows below
    /// it, or on a taller pane at the same top row, so that the new rows
    /// show the entries below it first. Without an anchor it is at the
    /// bottom. A lost anchor (its item went away or changed its
    /// `ItemRef`, as a local echo does once the server has it) is replaced
    /// by the entry at the anchor's index, clamped to the entries, which is
    /// where the core's `settle` keeps the cursor. A cursor more than a pane
    /// away from the anchor goes to the pane's edge it moved towards.
    fn around(&self, cursor: usize, anchor: Option<&Anchor>) -> Window<'a> {
        let height = self.pane_rows();
        let mut window = if let Some(Anchor::End) = anchor {
            self.at_bottom(self.entries.len() - 1)
        } else if let Some(Anchor::Item {
            index,
            item: id,
            below,
            height: last_height,
        }) = anchor
        {
            let is_anchor = |entry: &TimelineEntry| matches!(entry, TimelineEntry::Item(item) if item.id == *id);
            let index = if self.entries.get(*index).is_some_and(is_anchor) {
                *index
            } else {
                let clamped = (*index).min(self.entries.len() - 1);
                self.entries.iter().position(is_anchor).unwrap_or(clamped)
            };
            let rows = self.rows(index);
            let below = isize::try_from(*below).unwrap_or(isize::MAX);
            let last_height = isize::try_from(*last_height).unwrap_or(isize::MAX);
            let top = height.min(last_height) - below - rows.height();
            Window::new(index, rows, top)
        } else {
            return self.at_bottom(cursor);
        };
        while cursor > window.last() {
            if window.bottom() > 2 * height {
                return self.at_bottom(cursor);
            }
            window.push_back(self.rows(window.last() + 1));
        }
        while cursor < window.first {
            if window.top < -height {
                let rows = self.rows(cursor);
                let top = self.top_with_lead(cursor, rows.height());
                return Window::new(cursor, rows, top);
            }
            window.push_front(self.rows(window.first - 1));
        }
        window
    }

    /// The window of the entry at `index` with its last row at the bottom.
    fn at_bottom(&self, index: usize) -> Window<'a> {
        let rows = self.rows(index);
        let top = self.pane_rows() - rows.height();
        Window::new(index, rows, top)
    }

    /// The top row for the cursor item coming in at the top: below the
    /// dividers and markers right above it, so a day's first message shows
    /// its date.
    fn top_with_lead(&self, cursor: usize, len: isize) -> isize {
        let lead: isize = self.entries[..cursor]
            .iter()
            .rev()
            .map_while(|entry| match entry {
                TimelineEntry::Item(_) => None,
                TimelineEntry::DateDivider(_) | TimelineEntry::ReadMarker => Some(1),
                TimelineEntry::Hidden => Some(0),
            })
            .sum();
        lead.min(self.pane_rows() - len)
    }

    /// The rows of the entry at `index`. An item that would be taller than
    /// the pane with its preview has none. An item taller than the pane is
    /// cut with `[+N lines]` as its last row; a thread root keeps its
    /// summary row below the marker, on a pane with a row for both.
    fn rows(&self, index: usize) -> Rows<'a> {
        match &self.entries[index] {
            TimelineEntry::Item(item) => {
                let mut rows = self.item(item, Some(index) == self.cursor);
                if rows.lines.len() > self.height
                    && let Some(slot) = rows.preview.take()
                {
                    let end = slot.offset + usize::from(slot.target.size.height);
                    rows.lines.drain(slot.offset..end);
                }
                let lines = &mut rows.lines;
                if lines.len() > self.height {
                    // `item` ends with the summary row.
                    let summary = if item.thread_summary.is_some() && self.height > 1 {
                        lines.pop()
                    } else {
                        None
                    };
                    let kept = self.height - 1 - usize::from(summary.is_some());
                    let hidden = lines.len() - kept;
                    lines.truncate(kept);
                    lines.push(Line::styled(format!("[+{hidden} lines]"), DIM));
                    lines.extend(summary);
                }
                rows
            }
            TimelineEntry::DateDivider(timestamp) => {
                let date = self.format(*timestamp, DATE_FORMAT);
                Rows::plain(vec![rule(&date, DIM, self.width)])
            }
            TimelineEntry::ReadMarker => Rows::plain(vec![rule(
                READ_MARKER,
                Style::new().fg(Color::Red),
                self.width,
            )]),
            TimelineEntry::Hidden => Rows::plain(Vec::new()),
        }
    }

    /// The targets around the entries `first` to `last`, those with a row on
    /// the pane (design, "Fetching"): among as many entries again above and
    /// below them, at least `MIN_MARGIN` each way, by distance from the
    /// pane, below first.
    fn margin(&self, (first, last): (usize, usize)) -> impl Iterator<Item = (&'a ItemRef, Target)> {
        let reach = (last - first + 1).max(MIN_MARGIN);
        (1..=reach)
            .flat_map(move |distance| [Some(last + distance), first.checked_sub(distance)])
            .flatten()
            .filter(|&index| index < self.entries.len())
            .filter_map(|index| self.target(index))
    }

    /// The target of the preview of the remote image or sticker item at
    /// `index`, unless its preview failed or the item with it would not fit
    /// the pane.
    fn target(&self, index: usize) -> Option<(&'a ItemRef, Target)> {
        let TimelineEntry::Item(item) = &self.entries[index] else {
            return None;
        };
        if !matches!(item.id, ItemRef::Remote(_))
            || !matches!(
                item.content,
                MessageContent::Image { .. } | MessageContent::Sticker { .. }
            )
        {
            return None;
        }
        let slot = self.rows(index).preview?;
        Some((slot.item, slot.target))
    }

    /// The header, the reply preview, the body, an image's or a sticker's
    /// preview rows, the reactions, and a thread root's summary. With the
    /// search highlight, the body's matches are marked, bold too in the
    /// cursor's message (`cursor`).
    fn item(&self, item: &'a TimelineItem, cursor: bool) -> Rows<'a> {
        let mut rows = vec![self.header(item)];
        if let Some(reply) = &item.reply {
            rows.push(reply_line(reply, self.width));
        }
        let mut body = Body::new(self.width, rows);
        body.cursor = cursor;
        let mut preview = None;
        match &item.content {
            MessageContent::Text { rich, .. } => body.rich(rich, self.line_matches(&item.content)),
            MessageContent::Notice(text) => {
                body.base = DIM;
                body.rich(text, self.line_matches(&item.content));
            }
            MessageContent::Emote(text) => {
                body.lead = vec![
                    (Cow::Borrowed("* "), PLAIN),
                    (Cow::Borrowed(item.sender_name.as_str()), PLAIN),
                    (Cow::Borrowed(" "), PLAIN),
                ];
                body.rich(text, self.line_matches(&item.content));
            }
            MessageContent::Image {
                name, dimensions, ..
            } => {
                let text = match dimensions {
                    Some((width, height)) => format!("{IMAGE}{name} {width}x{height}]"),
                    None => format!("{IMAGE}{name}]"),
                };
                let matches = self.matches_in(name, IMAGE.len());
                body.line(vec![Part::new(text, PLAIN)], &matches);
                preview = self.reserve(&mut body, &item.id, *dimensions);
            }
            MessageContent::File { name, size, .. } => {
                let text = match size {
                    Some(size) => format!("{FILE}{name} {}]", file_size(*size)),
                    None => format!("{FILE}{name}]"),
                };
                let matches = self.matches_in(name, FILE.len());
                body.line(vec![Part::new(text, PLAIN)], &matches);
            }
            MessageContent::Sticker {
                body: text,
                dimensions,
            } => {
                let matches = self.matches_in(text, STICKER.len());
                let line = format!("{STICKER}{text}]");
                body.line(vec![Part::new(line, PLAIN)], &matches);
                preview = self.reserve(&mut body, &item.id, *dimensions);
            }
            MessageContent::Redacted
            | MessageContent::Undecryptable
            | MessageContent::Unsupported { .. } => {
                let text = item.content.plain_text();
                let matches = self.matches_in(&text, 0);
                body.line(vec![Part::new(text, DIM)], &matches);
            }
        }
        let shown = item.shown_reactions();
        if !shown.is_empty() {
            body.base = PLAIN;
            body.line(reactions(shown), &[]);
        }
        let mut rows = body.rows;
        if let Some(summary) = &item.thread_summary {
            rows.push(self.thread_row(item.timestamp, summary));
        }
        Rows {
            lines: rows,
            preview,
        }
    }

    /// With previews on, the blank rows after an image's or a sticker's
    /// line, which the preview of `item` is drawn over once kept, unless it
    /// failed; where they start among the rows.
    fn reserve(
        &self,
        body: &mut Body<'a>,
        item: &'a ItemRef,
        dimensions: Option<(u32, u32)>,
    ) -> Option<Slot<'a>> {
        let width = u16::try_from(self.width).unwrap_or(u16::MAX);
        let target = self.previews?.target(item, dimensions, width)?;
        let slot = Slot {
            item,
            offset: body.rows.len(),
            target,
        };
        let rows = usize::from(target.size.height);
        body.rows.extend(iter::repeat_n(Line::default(), rows));
        Some(slot)
    }

    /// The search's matches on each plain line of `content`'s rich text
    /// (see `search::plain_line_matches`); none without the highlight.
    fn line_matches(&self, content: &MessageContent) -> Vec<Vec<Range<usize>>> {
        self.highlight.map_or_else(Vec::new, |needle| {
            search::plain_line_matches(content, needle)
        })
    }

    /// The search's matches in `text`, which its line has after `offset`
    /// bytes, as the line's byte ranges; none without the highlight. `text`
    /// is the item's plain text (design, "Search and jumps").
    fn matches_in(&self, text: &str, offset: usize) -> Vec<Range<usize>> {
        let Some(needle) = self.highlight else {
            return Vec::new();
        };
        let mut matches = needle.find_in(text);
        for range in &mut matches {
            range.start += offset;
            range.end += offset;
        }
        matches
    }

    /// A thread root's last row (design, "Threads"): `💬 N · <time>`, dim,
    /// with the date before the time when the latest reply is on another
    /// local day than the root, and `💬 N` while the latest reply is not
    /// loaded. A root whose latest reply is newer than `thread_marks_after`
    /// ends with ` · unread`, the mark bold.
    fn thread_row(&self, root: Timestamp, summary: &ThreadSummary) -> Line<'a> {
        let mut text = format!("💬 {}", summary.replies);
        if let Some(latest) = summary.latest {
            text.push_str(" · ");
            if self.date(latest) != self.date(root) {
                text.push_str(&self.format(latest, REPLY_DATE_FORMAT));
                text.push(' ');
            }
            text.push_str(&self.format(latest, self.time_format));
        }
        let mut segments = vec![(text.as_str(), DIM)];
        if self
            .thread_marks_after
            .is_some_and(|after| summary.unread_after(after))
        {
            segments.extend([(" · ", DIM), ("unread", BOLD)]);
        }
        owned_line(&segments, self.width)
    }

    /// The local time, the sender, `(edited)` and the send state.
    fn header(&self, item: &'a TimelineItem) -> Line<'a> {
        let time = self.format(item.timestamp, self.time_format);
        let mut segments = vec![
            (time.as_str(), DIM),
            (" ", PLAIN),
            (item.sender_name.as_str(), BOLD),
        ];
        if item.edited {
            segments.push((" (edited)", DIM));
        }
        match &item.send_state {
            Some(SendState::Sending) => segments.push((" sending…", DIM)),
            Some(SendState::Failed { .. }) => segments.push((" failed", ERROR)),
            Some(SendState::Sent) | None => {}
        }
        owned_line(&segments, self.width)
    }

    /// `timestamp` in the local time zone; `None` if it cannot be shown.
    fn local(&self, timestamp: Timestamp) -> Option<jiff::ZonedDateTime> {
        let ms = i64::try_from(timestamp.as_millis()).ok()?;
        let time = jiff::Timestamp::from_millisecond(ms).ok()?;
        Some(time.to_zoned_datetime(self.zone.clone()))
    }

    /// The local date of `timestamp`.
    fn date(&self, timestamp: Timestamp) -> Option<jiff::civil::Date> {
        self.local(timestamp).map(|time| time.date())
    }

    /// `timestamp` in the local time zone; `?` if it cannot be shown.
    fn format(&self, timestamp: Timestamp, format: &str) -> String {
        self.local(timestamp)
            .and_then(|time| strtime::format(format, &time).ok())
            .unwrap_or_else(|| "?".to_owned())
    }
}

/// `segments` on one row of `width` columns, cut with `…`, as a line that
/// owns its text.
fn owned_line<'a>(segments: &[(&str, Style)], width: usize) -> Line<'a> {
    let (spans, _) = fit(segments, width);
    Line::from(
        spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect::<Vec<_>>(),
    )
}

/// `── label ──` across `width` columns.
fn rule<'a>(label: &str, style: Style, width: usize) -> Line<'a> {
    let label = format!(" {label} ");
    let Some(fill) = width.checked_sub(text_width(&label)) else {
        let (spans, _) = fit(&[(label.trim(), style)], width);
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        return Line::styled(text, style);
    };
    let left = fill / 2;
    Line::from(vec![
        Span::styled("─".repeat(left), style),
        Span::styled(label, style),
        Span::styled("─".repeat(fill - left), style),
    ])
}

/// `> name: body`, dim, on one row: the first line of the body.
fn reply_line(reply: &ReplyPreview, width: usize) -> Line<'_> {
    let body = reply
        .body
        .as_deref()
        .map(|body| body.lines().next().unwrap_or_default());
    let mut segments = vec![("> ", DIM)];
    match (reply.sender_name.as_deref(), body) {
        (Some(name), Some(body)) => segments.extend([(name, DIM), (": ", DIM), (body, DIM)]),
        (Some(text), None) | (None, Some(text)) => segments.push((text, DIM)),
        (None, None) => segments.push(("(not loaded)", DIM)),
    }
    Line::from(fit(&segments, width).0)
}

/// `key count` chips in [`TimelineItem::shown_reactions`]'s order; the
/// user's own reactions are bold.
fn reactions(groups: &[ReactionGroup]) -> Vec<Part<'_>> {
    let mut parts = Vec::with_capacity(groups.len() * 3);
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            parts.push(Part::new("  ", PLAIN));
        }
        let style = if group.mine { BOLD } else { PLAIN };
        parts.push(Part::new(group.key.as_str(), style));
        parts.push(Part::new(format!(" {}", group.count), style));
    }
    parts
}

/// `512 B`, `1.5 KiB`, `3.2 MiB`.
pub(crate) fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)] // One decimal is shown.
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Styled text of one line before wrapping.
type Piece<'a> = (Cow<'a, str>, Style);

/// Text of one line of a message as its plain text has it (design, "Search
/// and jumps"), with its style. Spoiler text is drawn as blocks (see
/// `hidden`).
struct Part<'a> {
    text: Cow<'a, str>,
    style: Style,
    spoiler: bool,
    /// For spoiler text: whether a search match touches its run (see
    /// `mark_spoilers`), which is then drawn as `▒`.
    matched: bool,
}

impl<'a> Part<'a> {
    /// Text that is no spoiler.
    fn new(text: impl Into<Cow<'a, str>>, style: Style) -> Part<'a> {
        Part {
            text: text.into(),
            style,
            spoiler: false,
            matched: false,
        }
    }
}

/// Builds the rows of a message body: rich text wrapped to the width, after
/// the prefixes of the quotes and lists it is in, with the search's matches
/// marked.
struct Body<'a> {
    width: usize,
    rows: Vec<Line<'a>>,
    /// Under every piece's own style: dim for notices.
    base: Style,
    /// Put before the first line: `* name ` of an emote.
    lead: Vec<Piece<'a>>,
    /// Outermost first.
    prefixes: Vec<Prefix>,
    /// Whether the body is the cursor's message, whose matches are bold too.
    cursor: bool,
}

enum Prefix {
    Quote,
    /// A list item: its marker on its first row, as many spaces on the others.
    Item {
        marker: String,
        started: bool,
    },
}

impl<'a> Body<'a> {
    fn new(width: usize, rows: Vec<Line<'a>>) -> Body<'a> {
        Body {
            width,
            rows,
            base: PLAIN,
            lead: Vec::new(),
            prefixes: Vec::new(),
            cursor: false,
        }
    }

    /// The lines of `text`, one per line of its plain text, with `matches`
    /// on each (see `search::plain_line_matches`).
    fn rich(&mut self, text: &'a RichText, matches: Vec<Vec<Range<usize>>>) {
        let mut steps = Steps {
            steps: Vec::new(),
            matches: matches.into_iter(),
        };
        steps.blocks(&text.blocks);
        mark_spoilers(&mut steps.steps);
        for step in steps.steps {
            match step {
                Step::Enter(prefix) => self.prefixes.push(prefix),
                Step::Leave => {
                    self.prefixes.pop();
                }
                Step::Line(parts, matches) => self.line(parts, &matches),
            }
        }
        if !self.lead.is_empty() {
            self.line(Vec::new(), &[]);
        }
    }

    /// One line of text with the search's `matches` on it, byte ranges in
    /// the text of `parts`, wrapped after the prefixes.
    fn line(&mut self, parts: Vec<Part<'a>>, matches: &[Range<usize>]) {
        let mut pieces = std::mem::take(&mut self.lead);
        push_parts(&mut pieces, parts, matches, self.cursor);
        let prefix_width: usize = self.prefixes.iter().map(Prefix::width).sum();
        let width = self.width.saturating_sub(prefix_width).max(1);
        for row in wrap(pieces, width) {
            let mut spans = Vec::with_capacity(self.prefixes.len() + row.len());
            for prefix in &mut self.prefixes {
                spans.push(prefix.span());
            }
            spans.extend(
                row.into_iter()
                    .map(|(text, style)| Span::styled(text, self.base.patch(style))),
            );
            self.rows.push(Line::from(spans));
        }
    }
}

/// A step of drawing rich text: one of its plain lines, with its parts and
/// the search's matches on it (see `search::plain_line_matches`), or where
/// a quote or a list item around the lines starts or ends.
enum Step<'a> {
    Enter(Prefix),
    Leave,
    Line(Vec<Part<'a>>, Vec<Range<usize>>),
}

/// The steps of rich text, in order. They are all known before the first is
/// drawn, so a spoiler's run is marked on every line it is on (see
/// `mark_spoilers`).
struct Steps<'a> {
    steps: Vec<Step<'a>>,
    /// The search's matches on the plain lines still to come: the rich
    /// text's lines follow its plain lines one by one.
    matches: std::vec::IntoIter<Vec<Range<usize>>>,
}

impl<'a> Steps<'a> {
    fn blocks(&mut self, blocks: &'a [Block]) {
        for block in blocks {
            self.block(block);
        }
    }

    fn block(&mut self, block: &'a Block) {
        match block {
            Block::Paragraph(inlines) => {
                for line in paragraph_lines(inlines) {
                    self.line(line);
                }
            }
            Block::Code { runs, .. } => {
                let mut line = Vec::new();
                for run in runs {
                    for (index, part) in run.text.split('\n').enumerate() {
                        if index > 0 {
                            self.line(std::mem::take(&mut line));
                        }
                        if !part.is_empty() {
                            line.push(Part {
                                text: Cow::Borrowed(part),
                                style: CODE_BLOCK,
                                spoiler: run.spoiler,
                                matched: false,
                            });
                        }
                    }
                }
                self.line(line);
            }
            Block::Quote(blocks) => self.nested(Prefix::Quote, blocks),
            // No row, where the plain text has an empty line.
            Block::List { items, .. } if items.is_empty() => {
                self.matches.next();
            }
            Block::List {
                ordered,
                start,
                items,
            } => {
                for (index, item) in (0..).zip(items) {
                    let marker = if *ordered {
                        format!("{}. ", start.saturating_add(index))
                    } else {
                        "- ".to_owned()
                    };
                    let prefix = Prefix::Item {
                        marker,
                        started: false,
                    };
                    self.nested(prefix, item);
                }
            }
        }
    }

    fn nested(&mut self, prefix: Prefix, blocks: &'a [Block]) {
        self.steps.push(Step::Enter(prefix));
        if blocks.is_empty() {
            self.line(Vec::new());
        }
        self.blocks(blocks);
        self.steps.push(Step::Leave);
    }

    /// The next plain line, with its matches.
    fn line(&mut self, parts: Vec<Part<'a>>) {
        let matches = self.matches.next().unwrap_or_default();
        self.steps.push(Step::Line(parts, matches));
    }
}

/// Marks the spoiler text whose run a search match touches
/// (`Part::matched`), so that all of the run's blocks turn to `▒` (design,
/// "Search and jumps"). A run goes on across lines, so no line of a spoiler
/// shows which of them holds the match: line breaks, blank lines and the
/// edges of blocks, quotes and list items do not end it, text that is no
/// spoiler does.
fn mark_spoilers(steps: &mut [Step<'_>]) {
    // The step and part indices of the run's parts so far, and whether a
    // match touches the run.
    let mut run = Vec::new();
    let mut touched = false;
    let mut marked = Vec::new();
    for (step, line) in steps.iter().enumerate() {
        let Step::Line(parts, matches) = line else {
            continue;
        };
        let mut start = 0;
        for (index, part) in parts.iter().enumerate() {
            let end = start + part.text.len();
            if part.spoiler {
                run.push((step, index));
                touched |= matches
                    .iter()
                    .any(|range| range.start < end && range.end > start);
            } else if touched {
                marked.append(&mut run);
                touched = false;
            } else {
                run.clear();
            }
            start = end;
        }
    }
    if touched {
        marked.append(&mut run);
    }
    for (step, index) in marked {
        if let Step::Line(parts, _) = &mut steps[step] {
            parts[index].matched = true;
        }
    }
}

/// Appends `parts` to `pieces` as they are drawn, with the search's
/// `matches` (byte ranges in the text of `parts`) in their style (design,
/// "Search and jumps"): text is split at the matches' edges, and spoiler
/// text, drawn as `█`, is drawn as `▒` in its own style when a match
/// touches its run (`Part::matched`). In the cursor's message (`cursor`)
/// these are bold too.
fn push_parts<'a>(
    pieces: &mut Vec<Piece<'a>>,
    parts: Vec<Part<'a>>,
    matches: &[Range<usize>],
    cursor: bool,
) {
    pieces.reserve(parts.len());
    let mut start = 0;
    for part in parts {
        let len = part.text.len();
        if part.spoiler {
            let (block, style) = match (part.matched, cursor) {
                (false, _) => (SPOILER, part.style),
                (true, false) => (MATCHED_SPOILER, part.style),
                (true, true) => (MATCHED_SPOILER, part.style.add_modifier(Modifier::BOLD)),
            };
            pieces.push((Cow::Owned(hidden(&part.text, block)), style));
        } else {
            push_split(pieces, part.text, part.style, start, matches, cursor);
        }
        start += len;
    }
}

/// Pushes `text` in `style`, which starts at byte `start` of its line, cut
/// at the edges of the `matches` on it, which are in the match style.
fn push_split<'a>(
    pieces: &mut Vec<Piece<'a>>,
    text: Cow<'a, str>,
    style: Style,
    start: usize,
    matches: &[Range<usize>],
    cursor: bool,
) {
    let end = start + text.len();
    // The text before `cut` is pushed.
    let mut cut = 0;
    for range in matches
        .iter()
        .filter(|range| range.start < end && range.end > start)
    {
        let from = range.start.max(start) - start;
        let to = range.end.min(end) - start;
        // Ranges fall on character boundaries; one that does not (the plain
        // text and the drawn text would differ) is left unmarked.
        if from < cut || !text.is_char_boundary(from) || !text.is_char_boundary(to) {
            continue;
        }
        if from > cut {
            pieces.push((slice(&text, cut..from), style));
        }
        pieces.push((slice(&text, from..to), matched(style, cursor)));
        cut = to;
    }
    if cut == 0 {
        pieces.push((text, style));
    } else if cut < text.len() {
        pieces.push((slice(&text, cut..text.len()), style));
    }
}

/// The bytes `range` of `text`, borrowed when `text` is.
fn slice<'a>(text: &Cow<'a, str>, range: Range<usize>) -> Cow<'a, str> {
    match text {
        Cow::Borrowed(text) => Cow::Borrowed(&text[range]),
        Cow::Owned(text) => Cow::Owned(text[range].to_owned()),
    }
}

/// The style of matched text in `style` (design, "Search and jumps"):
/// reverse video added, or for text that is reversed already (inline
/// code) taken off and an underline added; bold too in the cursor's
/// message.
fn matched(mut style: Style, cursor: bool) -> Style {
    if style.add_modifier.contains(Modifier::REVERSED) {
        style.add_modifier.remove(Modifier::REVERSED);
        style.add_modifier.insert(Modifier::UNDERLINED);
    } else {
        style.add_modifier.insert(Modifier::REVERSED);
    }
    if cursor {
        style.add_modifier.insert(Modifier::BOLD);
    }
    style
}

impl Prefix {
    fn width(&self) -> usize {
        match self {
            Prefix::Quote => text_width(QUOTE),
            Prefix::Item { marker, .. } => text_width(marker),
        }
    }

    /// The prefix on the next row.
    fn span(&mut self) -> Span<'static> {
        match self {
            Prefix::Quote => Span::styled(QUOTE, DIM),
            Prefix::Item { marker, started } => {
                if std::mem::replace(started, true) {
                    Span::raw(" ".repeat(text_width(marker)))
                } else {
                    Span::raw(marker.clone())
                }
            }
        }
    }
}

/// The lines of a paragraph: line breaks and newlines in the text start a
/// new line.
fn paragraph_lines(inlines: &[Inline]) -> Vec<Vec<Part<'_>>> {
    let mut lines = vec![Vec::new()];
    for inline in inlines {
        let (text, style, spoiler) = match inline {
            Inline::Text { text, style } => (text, inline_style(*style), style.spoiler),
            Inline::Link { text, style, .. } => (
                text,
                inline_style(*style).add_modifier(Modifier::UNDERLINED),
                style.spoiler,
            ),
            Inline::Mention { text, spoiler, .. } => (text, BOLD, *spoiler),
            Inline::Emoticon { alt, spoiler } => (alt, PLAIN, *spoiler),
            Inline::LineBreak => {
                lines.push(Vec::new());
                continue;
            }
        };
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            if part.is_empty() {
                continue;
            }
            if let Some(line) = lines.last_mut() {
                line.push(Part {
                    text: Cow::Borrowed(part),
                    style,
                    spoiler,
                    matched: false,
                });
            }
        }
    }
    lines
}

/// Spoiler text as `block`s of the same width; the message view shows the
/// text. Tabs stay, so they take as many columns as in the text.
fn hidden(text: &str, block: char) -> String {
    let mut blocks = String::with_capacity(text.len());
    for (index, run) in text.split('\t').enumerate() {
        if index > 0 {
            blocks.push('\t');
        }
        blocks.extend(std::iter::repeat_n(block, text_width(run)));
    }
    blocks
}

fn inline_style(style: rich::Style) -> Style {
    let mut out = PLAIN;
    for (on, modifier) in [
        (style.bold, Modifier::BOLD),
        (style.italic, Modifier::ITALIC),
        (style.strike, Modifier::CROSSED_OUT),
        (style.code, Modifier::REVERSED),
    ] {
        if on {
            out = out.add_modifier(modifier);
        }
    }
    out
}

/// A grapheme cluster of the pieces being wrapped.
struct Cluster {
    piece: usize,
    start: usize,
    end: usize,
    width: usize,
    space: bool,
}

/// Wraps the pieces into rows of at most `width` columns, measured per
/// grapheme cluster as ratatui draws them, after expanding tabs. A row
/// breaks after a space or before or after a wide cluster (CJK text has no
/// spaces), else inside a word too long for a row. Every space at a break
/// is dropped, and a row they would leave empty is left out. Always at
/// least one row.
fn wrap<'a>(mut pieces: Vec<Piece<'a>>, width: usize) -> Vec<Vec<Piece<'a>>> {
    expand_tabs(&mut pieces);
    let clusters: Vec<Cluster> = pieces
        .iter()
        .enumerate()
        .flat_map(|(piece, (text, _))| {
            text.grapheme_indices(true)
                .map(move |(start, grapheme)| Cluster {
                    piece,
                    start,
                    end: start + grapheme.len(),
                    width: grapheme_width(grapheme),
                    space: grapheme.chars().all(char::is_whitespace),
                })
        })
        .collect();
    let mut rows = Vec::new();
    let (mut start, mut used, mut i) = (0, 0, 0);
    // Where the row could end: after its last space or wide cluster.
    let mut last_break = None;
    while let Some(cluster) = clusters.get(i) {
        if used + cluster.width > width && i > start {
            let end = if cluster.space || cluster.width > 1 {
                i
            } else {
                last_break.unwrap_or(i)
            };
            let mut row_end = end;
            while row_end > start && clusters[row_end - 1].space {
                row_end -= 1;
            }
            if row_end > start {
                rows.push(row(&pieces, &clusters[start..row_end]));
            }
            start = end;
            while clusters.get(start).is_some_and(|next| next.space) {
                start += 1;
            }
            (i, used, last_break) = (start, 0, None);
            continue;
        }
        used += cluster.width;
        if cluster.space || cluster.width > 1 {
            last_break = Some(i + 1);
        }
        i += 1;
    }
    if start < clusters.len() || rows.is_empty() {
        rows.push(row(&pieces, &clusters[start..]));
    }
    rows
}

/// The pieces of `clusters`, one per run of clusters from the same piece.
fn row<'a>(pieces: &[Piece<'a>], clusters: &[Cluster]) -> Vec<Piece<'a>> {
    clusters
        .chunk_by(|a, b| a.piece == b.piece)
        .filter_map(|run| {
            let (first, last) = (run.first()?, run.last()?);
            let (text, style) = &pieces[first.piece];
            let range = first.start..last.end;
            let text = match text {
                Cow::Borrowed(text) => Cow::Borrowed(&text[range]),
                Cow::Owned(text) => Cow::Owned(text[range].to_owned()),
            };
            Some((text, *style))
        })
        .collect()
}

/// Replaces every tab with spaces up to the next tab stop, counted in
/// columns from the start of the line: ratatui draws no tab, so code would
/// lose its indentation.
fn expand_tabs(pieces: &mut [Piece<'_>]) {
    if !pieces.iter().any(|(text, _)| text.contains('\t')) {
        return;
    }
    let mut column = 0;
    for (text, _) in pieces {
        if !text.contains('\t') {
            column += text_width(text);
            continue;
        }
        let mut expanded = String::with_capacity(text.len() + TAB_WIDTH);
        for grapheme in text.graphemes(true) {
            if grapheme == "\t" {
                let spaces = TAB_WIDTH - column % TAB_WIDTH;
                expanded.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            } else {
                expanded.push_str(grapheme);
                column += grapheme_width(grapheme);
            }
        }
        *text = Cow::Owned(expanded);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::image_view::PreviewEncoder;
    use crate::previews::{ImageRows, Limits};
    use crate::view::RoomListLayout;
    use crate::view::tests::{self as view, Screen, app, press};
    use mxt_core::ids::{EventId, RoomId, TimelineId, TxnId, UserId};
    use mxt_core::msg::Msg;
    use mxt_core::timeline::TimelineDiff;
    use ratatui::layout::Size;
    use ratatui_image::FontSize;

    const WIDTH: usize = 20;
    const HEIGHT: usize = 10;

    fn wrapped(pieces: &[&str], width: usize) -> Vec<String> {
        let pieces: Vec<Piece> = pieces
            .iter()
            .map(|text| (Cow::Borrowed(*text), PLAIN))
            .collect();
        wrap(pieces, width)
            .into_iter()
            .map(|row| row.iter().map(|(text, _)| text.as_ref()).collect())
            .collect()
    }

    /// A message of a header and `lines` rows of text.
    fn message(id: ItemRef, lines: usize) -> TimelineEntry {
        let text = vec!["text"; lines].join("\n");
        TimelineEntry::Item(TimelineItem {
            id,
            sender: UserId::new("@alice:example.org"),
            sender_name: "alice".to_owned(),
            timestamp: Timestamp::from_millis(0),
            content: MessageContent::Text {
                body: text.clone(),
                rich: RichText::plain(&text),
            },
            reply: None,
            reactions: Vec::new(),
            edited: false,
            send_state: None,
            thread_summary: None,
        })
    }

    fn remote(id: usize, lines: usize) -> TimelineEntry {
        message(ItemRef::Remote(EventId::new(format!("$event{id}"))), lines)
    }

    /// Items of 2 to 15 rows (cut to the pane's height) among date
    /// dividers, a read marker and a hidden entry.
    fn timeline() -> Vec<TimelineEntry> {
        vec![
            TimelineEntry::DateDivider(Timestamp::from_millis(0)),
            remote(0, 1),
            remote(1, 3),
            TimelineEntry::Hidden,
            remote(2, 2),
            TimelineEntry::ReadMarker,
            remote(3, 14),
            remote(4, 1),
            TimelineEntry::DateDivider(Timestamp::from_millis(86_400_000)),
            remote(5, 4),
            remote(6, 2),
            remote(7, 1),
        ]
    }

    /// About seven panes: twenty items of 2 to 5 rows among date dividers,
    /// hidden entries and the read marker.
    fn long_timeline() -> Vec<TimelineEntry> {
        let mut entries = Vec::new();
        for id in 0..20 {
            match id {
                0 | 7 | 14 => entries.push(TimelineEntry::DateDivider(Timestamp::from_millis(0))),
                3 | 8 | 13 | 18 => entries.push(TimelineEntry::Hidden),
                16 => entries.push(TimelineEntry::ReadMarker),
                _ => {}
            }
            entries.push(remote(id, 1 + id % 4));
        }
        entries
    }

    fn items(entries: &[TimelineEntry]) -> Vec<usize> {
        (0..entries.len())
            .filter(|&index| matches!(entries[index], TimelineEntry::Item(_)))
            .collect()
    }

    fn render<'a>(
        entries: &'a [TimelineEntry],
        height: usize,
        zone: &'a TimeZone,
    ) -> Render<'a, 'static> {
        Render {
            entries,
            width: WIDTH,
            height,
            time_format: DEFAULT_TIME_FORMAT,
            zone,
            thread_marks_after: None,
            previews: None,
            highlight: None,
            cursor: None,
        }
    }

    /// The rows of the body of `rich` at `width` columns.
    fn body_rows(rich: &RichText, width: usize) -> Vec<String> {
        let mut body = Body::new(width, Vec::new());
        body.rich(rich, Vec::new());
        body.rows.iter().map(ToString::to_string).collect()
    }

    /// One frame on a pane of `HEIGHT` rows: the entries laid out as
    /// `(index, top row, rows)`, and the anchor for the next frame.
    fn lay_out(
        entries: &[TimelineEntry],
        cursor: usize,
        follows: bool,
        anchor: Option<&Anchor>,
    ) -> (Vec<(usize, isize, isize)>, Option<Anchor>) {
        lay_out_on(entries, HEIGHT, cursor, follows, anchor)
    }

    /// `lay_out` on a pane of `height` rows.
    fn lay_out_on(
        entries: &[TimelineEntry],
        height: usize,
        cursor: usize,
        follows: bool,
        anchor: Option<&Anchor>,
    ) -> (Vec<(usize, isize, isize)>, Option<Anchor>) {
        let zone = TimeZone::UTC;
        let render = Render {
            cursor: Some(cursor),
            ..render(entries, height, &zone)
        };
        let window = render.lay_out(follows, anchor);
        let placed = (window.first..=window.last())
            .map(|index| {
                let top = window.top_of(index).unwrap();
                (index, top, window.len_of(index).unwrap())
            })
            .collect();
        (placed, render.anchor(&window))
    }

    /// The first and last entries laid out.
    fn shown(placed: &[(usize, isize, isize)]) -> (usize, usize) {
        (placed[0].0, placed[placed.len() - 1].0)
    }

    /// The top row and the rows of the entry at `index`.
    fn place(placed: &[(usize, isize, isize)], index: usize) -> (isize, isize) {
        let &(_, top, len) = placed.iter().find(|(i, ..)| *i == index).unwrap();
        (top, len)
    }

    #[test]
    fn time_formats_are_checked() {
        let parse = |format: &str| TimeFormat::parse(format.to_owned());
        assert_eq!(parse("%H:%M"), Ok(TimeFormat("%H:%M".to_owned())));
        assert!(parse("%Y-%m-%d %I:%M %p").is_ok());
        assert!(parse("%H:%K").is_err());
        assert!(parse("%").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn words_move_to_the_next_row_whole() {
        assert_eq!(
            wrapped(&["the quick brown fox"], 10),
            ["the quick", "brown fox"]
        );
        // A word longer than a row is broken inside.
        assert_eq!(wrapped(&["abcdefghij"], 4), ["abcd", "efgh", "ij"]);
        // Pieces of one word stay together.
        assert_eq!(wrapped(&["ab ", "cd", "ef"], 5), ["ab", "cdef"]);
        // Every space at a break is dropped, also across pieces, and a row
        // of spaces only is left out.
        assert_eq!(wrapped(&["abc.  defgh"], 4), ["abc.", "defg", "h"]);
        assert_eq!(wrapped(&["abc.  de"], 4), ["abc.", "de"]);
        assert_eq!(wrapped(&["ab   cdefgh"], 3), ["ab", "cde", "fgh"]);
        assert_eq!(wrapped(&["one  ", " two"], 4), ["one", "two"]);
        assert_eq!(wrapped(&["        x"], 4), ["x"]);
    }

    #[test]
    fn tabs_expand_to_the_next_tab_stop() {
        // An indented code line keeps its indentation, and the wrap counts
        // its columns.
        assert_eq!(wrapped(&["\t\treturn x;"], 40), ["        return x;"]);
        assert_eq!(
            wrapped(&["\tlet x = 1; // a comment"], 20),
            ["    let x = 1; // a", "comment"]
        );
        // Tab stops count display columns from the line's start, across
        // pieces.
        assert_eq!(wrapped(&["ab\tc", "日\td"], 20), ["ab  c日 d"]);
    }

    #[test]
    fn japanese_breaks_between_wide_characters() {
        assert_eq!(
            wrapped(&["日本語の文章です"], 7),
            ["日本語", "の文章", "です"]
        );
        // A wide character never straddles the edge; the next row takes it.
        assert_eq!(wrapped(&["a日本"], 4), ["a日", "本"]);
        assert_eq!(wrapped(&["見て abcdef"], 8), ["見て", "abcdef"]);
    }

    #[test]
    fn emoji_clusters_are_never_split() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let text = format!("{family}{family}{family}");
        assert_eq!(
            wrapped(&[&text], 4),
            [format!("{family}{family}"), family.to_owned()]
        );
    }

    #[test]
    fn empty_text_is_one_empty_row() {
        assert_eq!(wrapped(&[], 10), [""]);
        // Every row takes at least one cluster.
        assert_eq!(wrapped(&["abc"], 0), ["a", "b", "c"]);
    }

    #[test]
    fn spoilers_hide_text_links_mentions_and_emoticons() {
        let spoiler = rich::Style {
            spoiler: true,
            ..rich::Style::default()
        };
        let alice = rich::MentionTarget::User(UserId::new("@alice:example.org"));
        let inlines = [
            Inline::Text {
                text: "plot".into(),
                style: spoiler,
            },
            Inline::Link {
                text: "link".into(),
                url: "https://example.org".into(),
                style: spoiler,
            },
            Inline::Mention {
                text: "@Alice".into(),
                target: alice.clone(),
                spoiler: true,
            },
            Inline::Emoticon {
                alt: ":cat:".into(),
                spoiler: true,
            },
            Inline::Mention {
                text: "@Alice".into(),
                target: alice,
                spoiler: false,
            },
            Inline::Emoticon {
                alt: ":cat:".into(),
                spoiler: false,
            },
        ];
        // Each as blocks of its width.
        let rich = RichText {
            blocks: vec![Block::Paragraph(inlines.to_vec())],
        };
        assert_eq!(body_rows(&rich, 40), ["███████████████████@Alice:cat:"]);
    }

    #[test]
    fn spoiler_text_in_code_blocks_is_hidden() {
        let code = |runs: &[(&str, bool)]| RichText {
            blocks: vec![Block::Code {
                language: None,
                runs: runs
                    .iter()
                    .map(|&(text, spoiler)| rich::CodeRun {
                        text: text.into(),
                        spoiler,
                    })
                    .collect(),
            }],
        };
        let rows = |rich: &RichText| body_rows(rich, WIDTH);
        // A code block in a spoiler.
        let block = "let x;\n\n\tx";
        assert_eq!(rows(&code(&[(block, true)])), ["██████", "", "    █"]);
        assert_eq!(rows(&code(&[(block, false)])), ["let x;", "", "    x"]);
        // Spoilers in a code block, one of them over a line break.
        let runs = [
            ("x = ", false),
            ("secret", true),
            ("\n\ty = ", false),
            ("two\nlines", true),
            (";", false),
        ];
        assert_eq!(rows(&code(&runs)), ["x = ██████", "    y = ███", "█████;"]);
    }

    #[test]
    fn file_sizes() {
        assert_eq!(file_size(512), "512 B");
        assert_eq!(file_size(1536), "1.5 KiB");
        assert_eq!(file_size(3 * 1024 * 1024 + 200 * 1024), "3.2 MiB");
    }

    #[test]
    fn the_cursor_item_is_always_fully_on_the_pane() {
        let entries = timeline();
        let items = items(&entries);
        let newest = items[items.len() - 1];
        let (_, mut anchor) = lay_out(&entries, newest, true, None);
        // Up one item at a time to the oldest and down again, then jumps
        // from one end to the other.
        let mut moves: Vec<usize> = items.iter().rev().chain(&items).copied().collect();
        moves.extend([items[0], newest, items[0]]);
        for cursor in moves {
            let (placed, next) = lay_out(&entries, cursor, false, anchor.as_ref());
            let (top, len) = place(&placed, cursor);
            assert!(
                top >= 0 && top + len <= HEIGHT as isize,
                "cursor {cursor}: {placed:?}"
            );
            anchor = next;
        }
    }

    /// The owner's report: well up with `k`, then down with `j`, the view
    /// scrolled one item at a time until the cursor item and every entry
    /// after it fit on the pane, and then jumped to put the newest entry at
    /// the bottom, moving the cursor from the bottom row to the top one.
    /// `<C-d>`, which moves half a pane of items at a time, jumped the same
    /// way near the end. Each step keeps the view while the cursor item is
    /// on the pane, and otherwise scrolls just far enough to bring it in at
    /// the edge it moves towards, all the way to either end.
    #[test]
    fn moving_the_cursor_by_one_item_or_more_never_jumps_the_view() {
        let entries = long_timeline();
        let items = items(&entries);
        let newest = items[items.len() - 1];
        let zone = TimeZone::UTC;
        let pane = render(&entries, HEIGHT, &zone);
        let reversed: Vec<usize> = items.iter().rev().copied().collect();
        // One item at a time as `j` and `k` move, and more as `<C-d>` and
        // `<C-u>` do.
        for step in 1..=3 {
            // Every `step`th item after the first, and the last.
            let stops = |from: &[usize]| {
                let mut stops: Vec<usize> = from.iter().copied().skip(step).step_by(step).collect();
                if stops.last() != from.last() {
                    stops.extend(from.last());
                }
                stops
            };
            let (mut placed, mut anchor) = lay_out(&entries, newest, true, None);
            // Up to the oldest item, then down to the newest, which follows
            // again as in the core.
            let up = stops(&reversed).into_iter().map(|cursor| (cursor, false));
            let down = stops(&items).into_iter().map(|cursor| (cursor, true));
            for (cursor, down) in up.chain(down) {
                let follows = down && cursor == newest;
                let (next, next_anchor) = lay_out(&entries, cursor, follows, anchor.as_ref());
                let (top, len) = place(&next, cursor);
                let expected = match placed.iter().find(|(index, ..)| *index == cursor) {
                    // On the pane already: the view stays.
                    Some(&(_, top, len)) if top >= 0 && top + len <= HEIGHT as isize => top,
                    // In at the bottom edge.
                    _ if down => HEIGHT as isize - len,
                    // In at the top edge, below the dividers and markers
                    // right above it.
                    _ => pane.top_with_lead(cursor, len),
                };
                let direction = if down { "down" } else { "up" };
                assert_eq!(
                    top, expected,
                    "step {step}, {direction} to {cursor}: {placed:?} then {next:?}"
                );
                (placed, anchor) = (next, next_anchor);
            }
        }
    }

    #[test]
    fn following_puts_the_newest_entry_at_the_bottom() {
        let mut entries = timeline();
        let (placed, anchor) = lay_out(&entries, entries.len() - 1, true, None);
        let (top, len) = place(&placed, entries.len() - 1);
        assert_eq!(top + len, HEIGHT as isize);

        entries.push(remote(8, 2));
        let (placed, _) = lay_out(&entries, entries.len() - 1, true, anchor.as_ref());
        let (top, len) = place(&placed, entries.len() - 1);
        assert_eq!(top + len, HEIGHT as isize);

        // A timeline shorter than the pane, too.
        let short = [remote(0, 1), remote(1, 1)];
        let (placed, _) = lay_out(&short, 1, true, None);
        assert_eq!(placed, [(0, 6, 2), (1, 8, 2)]);
    }

    #[test]
    fn entries_added_above_or_below_the_pane_leave_the_cursor_item_in_place() {
        let entries = timeline();
        let items = items(&entries);
        let (_, anchor) = lay_out(&entries, items[items.len() - 1], true, None);
        // One item up: the view no longer follows.
        let cursor = items[items.len() - 2];
        let (placed, anchor) = lay_out(&entries, cursor, false, anchor.as_ref());
        let rows = place(&placed, cursor);

        // Back pagination.
        let mut paginated = vec![
            remote(100, 2),
            TimelineEntry::DateDivider(Timestamp::from_millis(0)),
            remote(101, 5),
        ];
        paginated.extend(entries.iter().cloned());
        let (placed, _) = lay_out(&paginated, cursor + 3, false, anchor.as_ref());
        assert_eq!(place(&placed, cursor + 3), rows);

        // New messages while the cursor is up where the pane shows older
        // entries only.
        let cursor = items[4];
        let (placed, anchor) = lay_out(&entries, cursor, false, anchor.as_ref());
        assert!(placed.iter().all(|&(index, ..)| index < entries.len() - 1));
        let rows = place(&placed, cursor);
        let mut appended = entries.clone();
        appended.extend([remote(102, 1), remote(103, 3)]);
        let (placed, _) = lay_out(&appended, cursor, false, anchor.as_ref());
        assert_eq!(place(&placed, cursor), rows);
    }

    #[test]
    fn new_entries_appear_at_the_bottom_while_the_pane_shows_the_newest_entry() {
        let mut entries = timeline();
        let items = items(&entries);
        let (_, anchor) = lay_out(&entries, items[items.len() - 1], true, None);
        let cursor = items[items.len() - 2];
        let (placed, anchor) = lay_out(&entries, cursor, false, anchor.as_ref());
        let (top, len) = place(&placed, cursor);

        // A message of two rows: the cursor item moves up by them.
        entries.push(remote(102, 1));
        let (placed, anchor) = lay_out(&entries, cursor, false, anchor.as_ref());
        assert_eq!(place(&placed, entries.len() - 1), (HEIGHT as isize - 2, 2));
        assert_eq!(place(&placed, cursor), (top - 2, len));

        // A state event, which has no rows, then a message taller than the
        // rows above the cursor item (as in a burst): the message appears,
        // and the cursor item stays on the pane, at the top.
        entries.push(TimelineEntry::Hidden);
        let (_, anchor) = lay_out(&entries, cursor, false, anchor.as_ref());
        entries.push(remote(103, 3));
        let (placed, _) = lay_out(&entries, cursor, false, anchor.as_ref());
        assert_eq!(place(&placed, entries.len() - 1), (HEIGHT as isize - 3, 4));
        assert_eq!(place(&placed, cursor), (0, len));
    }

    /// The pane changes its height with the composer's draft. Twenty
    /// messages of two rows on a pane of 16, the cursor five messages up to
    /// reply to one, and a draft of seven rows takes six rows off the pane,
    /// so the cursor's message is at the top and the newest one below the
    /// pane. Sending gives the rows back below the cursor's message: the
    /// newest message is at the bottom again, and the reply and the next
    /// message appear.
    #[test]
    fn a_pane_that_grows_back_after_a_tall_draft_shows_the_newest_entry_again() {
        let mut entries: Vec<_> = (0..20).map(|id| remote(id, 1)).collect();
        let (placed, anchor) = lay_out_on(&entries, 16, 19, true, None);
        assert_eq!(shown(&placed), (12, 19));
        let (placed, anchor) = lay_out_on(&entries, 16, 14, false, anchor.as_ref());
        assert_eq!(shown(&placed), (12, 19));
        let (placed, anchor) = lay_out_on(&entries, 10, 14, false, anchor.as_ref());
        assert_eq!(shown(&placed), (14, 18));
        let (placed, anchor) = lay_out_on(&entries, 16, 14, false, anchor.as_ref());
        assert_eq!(shown(&placed), (12, 19));
        // The reply's local echo, then a message from another.
        entries.push(message(ItemRef::Local(TxnId::new("reply")), 1));
        let (placed, anchor) = lay_out_on(&entries, 16, 14, false, anchor.as_ref());
        assert_eq!(shown(&placed), (13, 20));
        entries.push(remote(20, 1));
        let (placed, _) = lay_out_on(&entries, 16, 14, false, anchor.as_ref());
        assert_eq!(shown(&placed), (14, 21));
    }

    /// Far from the newest entry, a pane that grows back after a tall draft
    /// shows more of the messages after the cursor's, which keeps its row.
    #[test]
    fn a_taller_pane_far_from_the_end_shows_more_rows_below_the_cursor_item() {
        let entries: Vec<_> = (0..40).map(|id| remote(id, 1)).collect();
        // Reading down with `j`: the cursor's message at the bottom edge.
        let (placed, anchor) = lay_out_on(&entries, 16, 23, false, None);
        assert_eq!(shown(&placed), (16, 23));
        let (placed, anchor) = lay_out_on(&entries, 10, 23, false, anchor.as_ref());
        assert_eq!(shown(&placed), (19, 23));
        let (placed, _) = lay_out_on(&entries, 16, 23, false, anchor.as_ref());
        assert_eq!(shown(&placed), (19, 26));
        assert_eq!(place(&placed, 23), (8, 2));
    }

    #[test]
    fn a_local_echo_keeps_its_rows_when_it_gets_its_event_id() {
        let echo = message(ItemRef::Local(TxnId::new("txn")), 2);
        let mut entries = vec![remote(0, 2), remote(1, 2), echo, remote(2, 1)];
        let (_, anchor) = lay_out(&entries, 3, true, None);
        let (placed, anchor) = lay_out(&entries, 2, false, anchor.as_ref());
        let rows = place(&placed, 2);

        entries[2] = remote(3, 2);
        let (placed, _) = lay_out(&entries, 2, false, anchor.as_ref());
        assert_eq!(place(&placed, 2), rows);
    }

    #[test]
    fn an_item_taller_than_the_pane_ends_in_the_count_of_hidden_rows() {
        let entries = [remote(0, 30)];
        let zone = TimeZone::UTC;
        let rows = render(&entries, HEIGHT, &zone).rows(0).lines;
        assert_eq!(rows.len(), HEIGHT);
        // The header and 30 rows of text, of which 22 do not fit.
        assert_eq!(rows[HEIGHT - 1].to_string(), "[+22 lines]");
        // As the cursor item, it fills the pane.
        let (placed, _) = lay_out(&entries, 0, false, None);
        assert_eq!(placed, [(0, 0, 10)]);
    }

    /// An image message of `id`, 4000 by 3000 pixels.
    fn photo(id: ItemRef) -> TimelineEntry {
        let mut entry = message(id, 1);
        if let TimelineEntry::Item(item) = &mut entry {
            item.content = MessageContent::Image {
                name: "photo.jpg".to_owned(),
                size: None,
                dimensions: Some((4000, 3000)),
            };
        }
        entry
    }

    /// The rows' text, and where the preview goes: its first row and size.
    fn with_preview(rows: Rows) -> (Vec<String>, Option<(usize, Size)>) {
        let lines = rows.lines.iter().map(ToString::to_string).collect();
        let preview = rows.preview.map(|slot| (slot.offset, slot.target.size));
        (lines, preview)
    }

    #[test]
    fn an_image_item_reserves_its_preview_s_rows_below_its_line() {
        let failed = ItemRef::Remote(EventId::new("$failed:example.org"));
        let entries = [
            photo(ItemRef::Remote(EventId::new("$photo:example.org"))),
            photo(failed.clone()),
            photo(ItemRef::Local(TxnId::new("echo"))),
        ];
        let mut previews = Previews::new(
            ImageRows::default(),
            PreviewEncoder::TmuxSixel(FontSize::new(10, 20)),
            Limits::DEFAULT,
            Box::new(|| {}),
        )
        .unwrap();
        previews.fail(failed);
        let zone = TimeZone::UTC;
        let render = Render {
            width: 60,
            previews: Some(previews.layout()),
            ..render(&entries, 20, &zone)
        };
        let line = "[image: photo.jpg 4000x3000]";
        // 8 rows and 22 columns of 10 by 20 pixels hold 213 by 160 pixels.
        let (rows, preview) = with_preview(render.rows(0));
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[1], line);
        assert!(rows[2..].iter().all(String::is_empty), "{rows:?}");
        assert_eq!(preview, Some((2, Size::new(22, 8))));
        // A failed preview leaves the line alone.
        let (rows, preview) = with_preview(render.rows(1));
        assert_eq!((rows.len(), preview), (2, None));
        // A local echo reserves the rows too.
        let (rows, preview) = with_preview(render.rows(2));
        assert_eq!((rows.len(), preview), (10, Some((2, Size::new(22, 8)))));
        // On a pane of 6 rows the item with its preview would not fit.
        let short = Render {
            height: 6,
            ..render
        };
        let (rows, preview) = with_preview(short.rows(0));
        assert_eq!(
            (rows, preview),
            (vec!["00:00 alice".to_owned(), line.to_owned()], None)
        );
    }

    #[test]
    fn a_sticker_reserves_its_preview_s_rows_below_its_line() {
        let mut entry = message(ItemRef::Remote(EventId::new("$sticker:example.org")), 1);
        if let TimelineEntry::Item(item) = &mut entry {
            item.content = MessageContent::Sticker {
                body: "a happy cat".to_owned(),
                dimensions: Some((512, 512)),
            };
        }
        let entries = [entry];
        let previews = Previews::new(
            ImageRows::default(),
            PreviewEncoder::TmuxSixel(FontSize::new(10, 20)),
            Limits::DEFAULT,
            Box::new(|| {}),
        )
        .unwrap();
        let zone = TimeZone::UTC;
        let render = Render {
            width: 60,
            previews: Some(previews.layout()),
            ..render(&entries, 20, &zone)
        };
        // 512 by 512 pixels fit 8 rows of 20 pixels as 160 by 160: 16
        // columns.
        let (rows, preview) = with_preview(render.rows(0));
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[1], "[sticker: a happy cat]");
        assert_eq!(preview, Some((2, Size::new(16, 8))));
        assert_eq!(
            render.target(0).map(|(_, target)| target.size),
            Some(Size::new(16, 8))
        );
    }

    fn at(time: &str) -> Timestamp {
        let ms = time.parse::<jiff::Timestamp>().unwrap().as_millisecond();
        Timestamp::from_millis(u64::try_from(ms).unwrap())
    }

    /// A thread root of `lines` rows of text sent on 2026-11-22 at 09:15 UTC,
    /// with three replies, the latest at `latest`.
    fn root(lines: usize, latest: Option<&str>) -> TimelineEntry {
        let mut entry = remote(0, lines);
        if let TimelineEntry::Item(item) = &mut entry {
            item.timestamp = at("2026-11-22T09:15:00Z");
            item.thread_summary = Some(ThreadSummary {
                replies: NonZeroU32::new(3).unwrap(),
                latest: latest.map(at),
            });
        }
        entry
    }

    /// The spans of the last row of `entry` in UTC, with roots marked after
    /// `marks_after`, on a pane wide enough for a reply's date.
    fn last_row(entry: TimelineEntry, marks_after: Option<&str>) -> Vec<(String, Style)> {
        let entries = [entry];
        let zone = TimeZone::UTC;
        let render = Render {
            width: 40,
            thread_marks_after: marks_after.map(at),
            ..render(&entries, HEIGHT, &zone)
        };
        let row = render.rows(0).lines.pop().unwrap();
        row.spans
            .iter()
            .map(|span| (span.content.to_string(), span.style))
            .collect()
    }

    fn text(row: &[(String, Style)]) -> String {
        row.iter().map(|(text, _)| text.as_str()).collect()
    }

    #[test]
    fn a_thread_root_ends_with_its_reply_count_and_latest_reply_time_dim() {
        let row = last_row(root(1, Some("2026-11-22T14:32:00Z")), None);
        assert_eq!(text(&row), "💬 3 · 14:32");
        assert!(row.iter().all(|(_, style)| *style == DIM), "{row:?}");
        // While the latest reply is not loaded.
        assert_eq!(text(&last_row(root(1, None), None)), "💬 3");
        // A reply on another day than the root shows its date.
        let next_day = last_row(root(1, Some("2026-11-23T14:32:00Z")), None);
        assert_eq!(text(&next_day), "💬 3 · 2026-11-23 14:32");
        // A message without replies has no such row.
        assert_eq!(text(&last_row(remote(0, 1), None)), "text");
    }

    #[test]
    fn a_thread_root_with_a_reply_after_the_marks_time_is_marked_unread() {
        let latest = Some("2026-11-22T14:32:00Z");
        let row = last_row(root(1, latest), Some("2026-11-22T14:31:59Z"));
        assert_eq!(text(&row), "💬 3 · 14:32 · unread");
        let (mark, rest) = row.split_last().unwrap();
        assert_eq!(mark, &("unread".to_owned(), BOLD));
        assert!(rest.iter().all(|(_, style)| *style == DIM), "{row:?}");
        for marks_after in [
            Some("2026-11-22T14:32:00Z"),
            Some("2026-11-22T15:00:00Z"),
            None,
        ] {
            let row = last_row(root(1, latest), marks_after);
            assert_eq!(text(&row), "💬 3 · 14:32", "{marks_after:?}");
        }
        // Without the latest reply's time, no mark.
        let row = last_row(root(1, None), Some("2026-11-22T00:00:00Z"));
        assert_eq!(text(&row), "💬 3");
    }

    #[test]
    fn a_thread_root_taller_than_the_pane_keeps_its_reply_row_below_the_cut() {
        let entries = [root(30, Some("2026-11-22T14:32:00Z"))];
        let zone = TimeZone::UTC;
        let rows_on = |height| {
            let render = Render {
                width: 40,
                thread_marks_after: Some(at("2026-11-22T14:00:00Z")),
                ..render(&entries, height, &zone)
            };
            let rows = render.rows(0).lines;
            rows.iter().map(ToString::to_string).collect::<Vec<_>>()
        };
        let rows = rows_on(HEIGHT);
        assert_eq!(rows.len(), HEIGHT);
        // The header and 30 rows of text, of which 23 do not fit above the
        // marker and the reply row.
        assert_eq!(rows[0], "09:15 alice");
        assert_eq!(rows[1..HEIGHT - 2], ["text"; 7]);
        assert_eq!(rows[HEIGHT - 2], "[+23 lines]");
        assert_eq!(rows[HEIGHT - 1], "💬 3 · 14:32 · unread");
        // A pane of one row has room for the marker only.
        assert_eq!(rows_on(1), ["[+32 lines]"]);
    }

    const ROOM: &str = "!a:example.org";

    /// The room `!a` open with `entries` and drawn once; the cursor follows.
    /// The pane has 8 rows, four one-line messages.
    fn open(entries: Vec<TimelineEntry>) -> (App, Screen) {
        open_on(Screen::new(), entries)
    }

    /// `open` on `screen`.
    fn open_on(mut screen: Screen, entries: Vec<TimelineEntry>) -> (App, Screen) {
        let mut app = app();
        press(&mut app, "<CR>");
        app.update(Msg::Timeline {
            timeline: TimelineId::main(RoomId::new(ROOM)),
            diffs: vec![TimelineDiff::Reset(entries)],
        });
        screen.draw(&app, None);
        (app, screen)
    }

    /// `entries` arriving in one batch, as the event loop drains them.
    fn arrive(app: &mut App, entries: impl IntoIterator<Item = TimelineEntry>) {
        app.update(Msg::Timeline {
            timeline: TimelineId::main(RoomId::new(ROOM)),
            diffs: entries.into_iter().map(TimelineDiff::PushBack).collect(),
        });
    }

    /// `key` pressed `times` times, with a frame after each.
    fn repeat(app: &mut App, screen: &mut Screen, key: &str, times: usize) {
        for _ in 0..times {
            press(app, key);
            screen.draw(app, None);
        }
    }

    /// One frame: the last entry with a row on the pane, and the newest.
    fn last_visible(app: &App, screen: &mut Screen) -> (usize, usize) {
        let (_, last) = screen.draw(app, None).timeline.unwrap();
        (last, app.timeline().len() - 1)
    }

    /// The owner's report: the cursor one message up (to reply to it, say),
    /// and every message that arrived afterwards, the user's own reply too,
    /// stayed below the pane, also after `j` put the cursor on the message
    /// at the pane's bottom edge, which looks like the end of the timeline.
    /// Only opening the room again showed them.
    #[test]
    fn messages_arriving_while_the_pane_shows_the_newest_entry_appear() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        repeat(&mut app, &mut screen, "k", 1);
        // The local echo of the user's reply, then a message from another.
        arrive(&mut app, [message(ItemRef::Local(TxnId::new("reply")), 1)]);
        let (last, newest) = last_visible(&app, &mut screen);
        assert_eq!(last, newest);
        arrive(&mut app, [view::message(10)]);
        let (last, newest) = last_visible(&app, &mut screen);
        assert_eq!(last, newest);
        // On a message that is not the newest: the cursor stays where it is.
        repeat(&mut app, &mut screen, "j", 1);
        assert!(!app.timeline_follows());
        arrive(&mut app, [view::message(11)]);
        let (last, newest) = last_visible(&app, &mut screen);
        assert_eq!(last, newest);
    }

    /// Up past the top of the pane to read a little more, a message arrives
    /// meanwhile, and back down to the old newest message to reply: it comes
    /// in at the bottom edge like any other, and the hint counts the message
    /// below it. One more `j` shows that message and follows again.
    #[test]
    fn moving_back_down_to_the_old_newest_message_counts_what_arrived_meanwhile() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        // The pane shows the last four messages; the cursor goes above them.
        repeat(&mut app, &mut screen, "k", 4);
        // The pane shows older entries only: the view stays put.
        arrive(&mut app, [view::message(10)]);
        let (last, newest) = last_visible(&app, &mut screen);
        assert!(last < newest - 1);
        repeat(&mut app, &mut screen, "j", 4);
        assert_eq!(app.timeline_cursor(), Some(newest - 1));
        assert_eq!(last_visible(&app, &mut screen), (newest - 1, newest));
        let rule = screen.composer_rule();
        assert!(rule.ends_with("── ↓ 1 newer ─"), "{rule:?}");
        repeat(&mut app, &mut screen, "j", 1);
        assert!(app.timeline_follows());
        arrive(&mut app, [view::message(11)]);
        let (last, newest) = last_visible(&app, &mut screen);
        assert_eq!(last, newest);
    }

    /// More messages in one batch (a sync catch-up, say) than fit below the
    /// cursor's message keep it at the top of the pane, and the newest ones
    /// below the pane. Moving down keeps the view until the cursor's message
    /// is at the bottom edge, then scrolls one message at a time, and the
    /// newest message ends at the bottom.
    #[test]
    fn moving_down_after_a_burst_scrolls_one_message_at_a_time_to_the_newest() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        repeat(&mut app, &mut screen, "k", 1);
        arrive(&mut app, (10..14).map(view::message));
        let visible = screen.draw(&app, None).timeline;
        assert_eq!(visible, Some((7, 10)));
        repeat(&mut app, &mut screen, "j", 3);
        assert_eq!(screen.draw(&app, None).timeline, Some((7, 10)));
        for visible in [(8, 11), (9, 12)] {
            repeat(&mut app, &mut screen, "j", 1);
            assert_eq!(screen.draw(&app, None).timeline, Some(visible));
        }
        assert!(app.timeline_follows());
    }

    /// After `open` with nine messages, the cursor one message up and a
    /// burst of `count` messages: the pane shows the cursor's message at its
    /// top and the three after it, so `count - 2` are below the pane.
    fn burst(app: &mut App, screen: &mut Screen, count: u32) {
        repeat(app, screen, "k", 1);
        arrive(app, (10..10 + count).map(view::message));
        let newest = 8 + usize::try_from(count).unwrap();
        assert_eq!(last_visible(app, screen), (10, newest));
    }

    #[test]
    fn the_composer_rule_counts_the_messages_below_the_pane_until_it_shows_the_newest() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        burst(&mut app, &mut screen, 5);
        let rule = screen.composer_rule();
        assert!(rule.ends_with("── ↓ 3 newer ─"), "{rule:?}");
        // The view stays put while the cursor moves inside it, down to the
        // bottom edge.
        repeat(&mut app, &mut screen, "j", 3);
        let rule = screen.composer_rule();
        assert!(rule.ends_with("── ↓ 3 newer ─"), "{rule:?}");
        // Then each message it scrolls in leaves the count.
        repeat(&mut app, &mut screen, "j", 1);
        let rule = screen.composer_rule();
        assert!(rule.ends_with("── ↓ 2 newer ─"), "{rule:?}");
        repeat(&mut app, &mut screen, "j", 2);
        assert!(app.timeline_follows());
        let (last, newest) = last_visible(&app, &mut screen);
        assert_eq!(last, newest);
        let rule = screen.composer_rule();
        assert!(!rule.contains('↓'), "{rule:?}");
    }

    #[test]
    fn hidden_entries_date_dividers_and_the_read_marker_below_the_pane_give_no_hint() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        repeat(&mut app, &mut screen, "k", 1);
        let divider = TimelineEntry::DateDivider(Timestamp::from_millis(86_400_000));
        arrive(
            &mut app,
            [
                view::message(10),
                view::message(11),
                TimelineEntry::Hidden,
                divider,
                TimelineEntry::ReadMarker,
            ],
        );
        // The divider and the read marker are below the pane.
        assert_eq!(last_visible(&app, &mut screen), (10, 13));
        let rule = screen.composer_rule();
        assert!(!rule.contains('↓'), "{rule:?}");
    }

    #[test]
    fn more_than_99_newer_messages_read_99_plus() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        burst(&mut app, &mut screen, 101);
        let rule = screen.composer_rule();
        assert!(rule.ends_with("── ↓ 99 newer ─"), "{rule:?}");
        // 150 below the pane.
        arrive(&mut app, (111..162).map(view::message));
        assert_eq!(last_visible(&app, &mut screen), (10, 160));
        let rule = screen.composer_rule();
        assert!(rule.ends_with("── ↓ 99+ newer ─"), "{rule:?}");
    }

    #[test]
    fn a_narrow_rule_keeps_the_draft_label_and_shortens_or_leaves_out_the_hint() {
        // `─ Reply to Alice ` takes 17 columns, ` ↓ 3 newer ─` 12 and
        // ` ↓ 3 ─` 6, and one `─` at least goes between.
        for (width, expected) in [
            (30, "─ Reply to Alice ─ ↓ 3 newer ─"),
            (29, "─ Reply to Alice ────── ↓ 3 ─"),
            (24, "─ Reply to Alice ─ ↓ 3 ─"),
            (23, "─ Reply to Alice ──────"),
        ] {
            let screen = Screen::sized(width, RoomListLayout::default());
            let (mut app, mut screen) = open_on(screen, (1..10).map(view::message).collect());
            // Without the room list the rule takes the terminal's width.
            press(&mut app, "<Space>e");
            burst(&mut app, &mut screen, 5);
            press(&mut app, "r");
            screen.draw(&app, None);
            assert_eq!(screen.composer_rule(), expected, "{width}");
        }
    }

    /// The newest message so tall (a long paste, say) that it and the one
    /// before it do not fit on the pane together, so the cursor on the one
    /// before cuts the newest one's last row: the pane does not show the
    /// newest entry's last row, so the user's reply stays below the pane,
    /// counted by the hint, also after `j` onto the old newest message,
    /// until the cursor reaches the reply.
    #[test]
    fn a_reply_after_a_tall_newest_message_that_the_pane_cuts_stays_below_it() {
        // Four rows and five on the pane's eight.
        let mut entries: Vec<_> = (1..5).map(view::message).collect();
        entries.extend([remote(0, 3), remote(1, 4)]);
        let (mut app, mut screen) = open(entries);
        repeat(&mut app, &mut screen, "k", 1);
        arrive(&mut app, [message(ItemRef::Local(TxnId::new("reply")), 1)]);
        for _ in 0..2 {
            assert_eq!(last_visible(&app, &mut screen), (5, 6));
            let rule = screen.composer_rule();
            assert!(rule.ends_with("── ↓ 1 newer ─"), "{rule:?}");
            repeat(&mut app, &mut screen, "j", 1);
        }
        let (last, newest) = last_visible(&app, &mut screen);
        assert_eq!(last, newest);
    }

    /// A message from alice with `content`.
    fn with_content(content: MessageContent) -> TimelineEntry {
        let mut entry = remote(0, 1);
        if let TimelineEntry::Item(item) = &mut entry {
            item.content = content;
        }
        entry
    }

    fn plain_text(text: &str) -> MessageContent {
        MessageContent::Text {
            body: text.to_owned(),
            rich: RichText::plain(text),
        }
    }

    /// Rich text of `blocks`.
    fn rich_text(blocks: Vec<Block>) -> MessageContent {
        MessageContent::Text {
            body: String::new(),
            rich: RichText { blocks },
        }
    }

    fn styled(text: &str, style: rich::Style) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            style,
        }
    }

    /// Each grapheme cluster of `line` with its style.
    fn cells(line: &Line) -> Vec<(String, Style)> {
        line.spans
            .iter()
            .flat_map(|span| {
                let clusters = span.content.graphemes(true);
                clusters.map(move |cluster| (cluster.to_owned(), span.style))
            })
            .collect()
    }

    /// The body rows of a message of `content` on a pane of 40 by 40, after
    /// a search for `pattern`, as the cursor's message with `cursor`: each
    /// row's grapheme clusters with their styles.
    fn searched(
        content: MessageContent,
        pattern: Option<&str>,
        cursor: bool,
    ) -> Vec<Vec<(String, Style)>> {
        let entries = [with_content(content)];
        let needle = pattern.map(Needle::new);
        let zone = TimeZone::UTC;
        let render = Render {
            width: 40,
            highlight: needle.as_ref(),
            cursor: cursor.then_some(0),
            ..render(&entries, 40, &zone)
        };
        // Without the header.
        render.rows(0).lines[1..].iter().map(cells).collect()
    }

    /// The clusters of `row` whose style has `modifier`, and a space for
    /// each column of the others, without the spaces at the end.
    fn marked(row: &[(String, Style)], modifier: Modifier) -> String {
        let mut marked = String::new();
        for (cluster, style) in row {
            if style.add_modifier.contains(modifier) {
                marked.push_str(cluster);
            } else {
                marked.extend(iter::repeat_n(' ', grapheme_width(cluster)));
            }
        }
        marked.trim_end().to_owned()
    }

    #[test]
    fn matches_are_reversed_and_keep_the_text_s_style() {
        let rows = searched(plain_text("hello world"), Some("WORLD"), false);
        assert_eq!(text(&rows[0]), "hello world");
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "      world");
        let rows = searched(plain_text("hello world"), None, false);
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "");
        // Across a bold span and a plain one.
        let bold = rich::Style {
            bold: true,
            ..rich::Style::default()
        };
        let spans = vec![
            styled("hel", bold),
            styled("lo world", rich::Style::default()),
        ];
        let rows = searched(rich_text(vec![Block::Paragraph(spans)]), Some("llo"), false);
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "  llo");
        assert_eq!(marked(&rows[0], Modifier::BOLD), "hel");
        // Wide characters, and an emoji with its skin tone, whole.
        let content = plain_text("日本語のテキスト 👍🏽 ok");
        let rows = searched(content.clone(), Some("テキ"), false);
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "        テキ");
        let rows = searched(content, Some("👍"), false);
        let thumb = format!("{}👍🏽", " ".repeat(17));
        assert_eq!(marked(&rows[0], Modifier::REVERSED), thumb);
    }

    #[test]
    fn matches_in_quotes_and_lists_leave_their_prefixes_out() {
        let milk = || {
            vec![Block::Paragraph(vec![styled(
                "milk",
                rich::Style::default(),
            )])]
        };
        let list = rich_text(vec![Block::List {
            ordered: false,
            start: 1,
            items: vec![milk()],
        }]);
        let rows = searched(list, Some("milk"), false);
        assert_eq!(text(&rows[0]), "- milk");
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "  milk");
        let quote = rich_text(vec![Block::Quote(milk())]);
        let rows = searched(quote.clone(), Some("milk"), false);
        assert_eq!(text(&rows[0]), "│ milk");
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "  milk");
        // A match that starts in the plain text's `> ` marks its part after
        // it.
        let rows = searched(quote, Some("> m"), false);
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "  m");
        // An empty list has no row but a line in the plain text, so the
        // lines after it keep their own matches.
        let mut blocks = vec![Block::List {
            ordered: false,
            start: 1,
            items: Vec::new(),
        }];
        blocks.extend(milk());
        let rows = searched(rich_text(blocks), Some("milk"), false);
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "milk");
    }

    #[test]
    fn matches_in_inline_code_are_underlined_instead() {
        let plain = rich::Style::default();
        let code = rich::Style {
            code: true,
            ..plain
        };
        let spans = vec![
            styled("run ", plain),
            styled("cat file", code),
            styled(" now", plain),
        ];
        let rows = searched(rich_text(vec![Block::Paragraph(spans)]), Some("cat"), false);
        assert_eq!(text(&rows[0]), "run cat file now");
        // The rest of the code stays reversed.
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "        file");
        assert_eq!(marked(&rows[0], Modifier::UNDERLINED), "    cat");
    }

    #[test]
    fn the_name_on_image_file_and_sticker_lines_is_marked() {
        let image = MessageContent::Image {
            name: "image.png".to_owned(),
            size: None,
            dimensions: Some((64, 64)),
        };
        let file = MessageContent::File {
            name: "file.txt".to_owned(),
            size: Some(2048),
            mime: None,
        };
        let sticker = MessageContent::Sticker {
            body: "sticker".to_owned(),
            dimensions: None,
        };
        let cases = [
            (image, "image", "[image: image.png 64x64]", IMAGE),
            (file, "file", "[file: file.txt 2.0 KiB]", FILE),
            (sticker, "sticker", "[sticker: sticker]", STICKER),
        ];
        for (content, pattern, line, opening) in cases {
            let rows = searched(content, Some(pattern), false);
            assert_eq!(text(&rows[0]), line);
            // Not the word in the opening.
            let name = format!("{}{pattern}", " ".repeat(opening.len()));
            assert_eq!(marked(&rows[0], Modifier::REVERSED), name);
        }
    }

    #[test]
    fn lines_without_text_are_marked_and_an_emote_s_lead_is_not() {
        let rows = searched(MessageContent::Redacted, Some("DEL"), false);
        assert_eq!(marked(&rows[0], Modifier::REVERSED), " del");
        assert_eq!(marked(&rows[0], Modifier::DIM), "(deleted)");
        let emote = MessageContent::Emote(RichText::plain("waves at Alice"));
        let rows = searched(emote, Some("alice"), false);
        assert_eq!(text(&rows[0]), "* alice waves at Alice");
        let alice = format!("{}Alice", " ".repeat(17));
        assert_eq!(marked(&rows[0], Modifier::REVERSED), alice);
    }

    #[test]
    fn a_spoiler_with_a_match_is_drawn_whole_as_light_shade_in_its_style() {
        let plain = rich::Style::default();
        let spoiler = rich::Style {
            spoiler: true,
            ..plain
        };
        let bold = rich::Style {
            bold: true,
            ..spoiler
        };
        let content = rich_text(vec![Block::Paragraph(vec![
            styled("see ", plain),
            styled("secret ", spoiler),
            styled("plot", bold),
            styled(" and ", plain),
            styled("other", spoiler),
        ])]);
        let rows = searched(content.clone(), None, false);
        assert_eq!(text(&rows[0]), "see ███████████ and █████");
        let rows = searched(content.clone(), Some("plot"), false);
        assert_eq!(text(&rows[0]), "see ▒▒▒▒▒▒▒▒▒▒▒ and █████");
        assert_eq!(marked(&rows[0], Modifier::BOLD), "           ▒▒▒▒");
        assert_eq!(marked(&rows[0], Modifier::REVERSED), "");
        let rows = searched(content, Some("other"), false);
        assert_eq!(text(&rows[0]), "see ███████████ and ▒▒▒▒▒");
    }

    #[test]
    fn a_spoiler_over_several_lines_turns_to_light_shade_on_all_of_them() {
        let plain = rich::Style::default();
        let spoiler = rich::Style {
            spoiler: true,
            ..plain
        };
        let texts = |rows: &[Vec<(String, Style)>]| -> Vec<String> {
            rows.iter().map(Vec::as_slice).map(text).collect()
        };
        // A line break in a spoiler, as `<br>` and as a newline in its text.
        let paragraphs = [
            vec![
                styled("see ", plain),
                styled("one", spoiler),
                Inline::LineBreak,
                styled("two", spoiler),
            ],
            vec![styled("see ", plain), styled("one\ntwo", spoiler)],
        ];
        for inlines in paragraphs {
            let content = rich_text(vec![Block::Paragraph(inlines)]);
            let rows = searched(content.clone(), None, false);
            assert_eq!(texts(&rows), ["see ███", "███"]);
            for pattern in ["one", "two"] {
                let rows = searched(content.clone(), Some(pattern), false);
                assert_eq!(texts(&rows), ["see ▒▒▒", "▒▒▒"], "{pattern}");
            }
        }
        let code = |runs: &[(&str, bool)]| Block::Code {
            language: None,
            runs: runs
                .iter()
                .map(|&(text, spoiler)| rich::CodeRun {
                    text: text.into(),
                    spoiler,
                })
                .collect(),
        };
        // In a code block, a run over a line break; text that is no spoiler
        // ends a run.
        let runs = rich_text(vec![code(&[
            ("x = ", false),
            ("secret", true),
            ("\ny = ", false),
            ("two\nlines", true),
            (";", false),
        ])]);
        let rows = searched(runs.clone(), Some("lines"), false);
        assert_eq!(texts(&rows), ["x = ██████", "y = ▒▒▒", "▒▒▒▒▒;"]);
        let rows = searched(runs, Some("secret"), false);
        assert_eq!(texts(&rows), ["x = ▒▒▒▒▒▒", "y = ███", "█████;"]);
        // A spoiler around text and a code block, which are two blocks, with
        // a blank line in the code; in the cursor's message bold on every
        // line.
        let content = rich_text(vec![
            Block::Paragraph(vec![styled("the answer:", spoiler)]),
            code(&[("x\n\nprint(42)", true)]),
        ]);
        let rows = searched(content, Some("print"), true);
        assert_eq!(texts(&rows), ["▒▒▒▒▒▒▒▒▒▒▒", "▒", "", "▒▒▒▒▒▒▒▒▒"]);
        let bold: Vec<String> = rows.iter().map(|row| marked(row, Modifier::BOLD)).collect();
        assert_eq!(bold, texts(&rows));
    }

    #[test]
    fn the_cursor_s_message_has_its_matches_bold_too() {
        let plain = rich::Style::default();
        let code = rich::Style {
            code: true,
            ..plain
        };
        let spoiler = rich::Style {
            spoiler: true,
            ..plain
        };
        let image = MessageContent::Image {
            name: "cat.png".to_owned(),
            size: None,
            dimensions: None,
        };
        let cases = [
            (plain_text("hello world"), "world"),
            (
                rich_text(vec![Block::Paragraph(vec![
                    styled("run ", plain),
                    styled("cat file", code),
                ])]),
                "cat",
            ),
            (
                rich_text(vec![Block::Paragraph(vec![
                    styled("see ", plain),
                    styled("plot", spoiler),
                ])]),
                "plot",
            ),
            (image, "cat"),
            (MessageContent::Redacted, "del"),
        ];
        for (content, pattern) in cases {
            let unsearched = searched(content.clone(), None, true);
            let elsewhere = searched(content.clone(), Some(pattern), false);
            let at_cursor = searched(content, Some(pattern), true);
            assert_ne!(elsewhere, unsearched, "{pattern}");
            let before = unsearched.iter().flatten();
            let cells = before
                .zip(elsewhere.iter().flatten())
                .zip(at_cursor.iter().flatten());
            for ((before, elsewhere), at_cursor) in cells {
                // The cells the search changed, bold too.
                let (text, style) = elsewhere.clone();
                let style = if before == elsewhere {
                    style
                } else {
                    style.add_modifier(Modifier::BOLD)
                };
                assert_eq!(at_cursor, &(text, style), "{pattern}");
            }
        }
    }

    #[test]
    fn matches_in_the_cut_off_part_of_a_tall_message_are_not_drawn() {
        let body = format!("{}needle", "text\n".repeat(29));
        let entries = [with_content(plain_text(&body))];
        let needle = Needle::new("needle");
        let zone = TimeZone::UTC;
        let rows_on = |height| -> Vec<Vec<(String, Style)>> {
            let render = Render {
                highlight: Some(&needle),
                cursor: Some(0),
                ..render(&entries, height, &zone)
            };
            render.rows(0).lines.iter().map(cells).collect()
        };
        let rows = rows_on(HEIGHT);
        assert_eq!(text(&rows[HEIGHT - 1]), "[+22 lines]");
        let reversed = |row: &Vec<(String, Style)>| marked(row, Modifier::REVERSED);
        assert!(rows.iter().map(reversed).all(|row| row.is_empty()));
        // A pane that holds the whole message shows the match.
        let rows = rows_on(40);
        assert_eq!(reversed(&rows[30]), "needle");
    }

    /// The text drawn in reverse video above the composer, without the
    /// cursor bar.
    fn reversed(screen: &Screen) -> String {
        let area = screen.terminal.backend().buffer().area;
        let mut text = String::new();
        for y in 0..area.height - 3 {
            for x in 0..area.width {
                let cell = screen.cell(x, y);
                if cell.modifier.contains(Modifier::REVERSED) && cell.symbol() != " " {
                    text.push_str(cell.symbol());
                }
            }
        }
        text
    }

    #[test]
    fn a_search_highlights_the_messages_on_the_pane_until_noh() {
        let (mut app, mut screen) = open((1..10).map(view::message).collect());
        assert_eq!(reversed(&screen), "");
        press(&mut app, "/MESSAGE<CR>");
        screen.draw(&app, None);
        // The four messages on the pane.
        assert_eq!(reversed(&screen), "message".repeat(4));
        press(&mut app, ":noh<CR>");
        screen.draw(&app, None);
        assert_eq!(reversed(&screen), "");
        press(&mut app, "n");
        screen.draw(&app, None);
        assert_eq!(reversed(&screen), "message".repeat(4));
    }
}
