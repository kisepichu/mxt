//! The timeline pane (design, "Timeline"): the open room's entries, oldest at
//! the top. The cursor item stays in view, and while the cursor follows the
//! newest item the newest entry is at the bottom. Only the entries around the
//! cursor are laid out, so a long timeline costs no more per frame.

use std::borrow::Cow;
use std::collections::VecDeque;

use jiff::fmt::strtime;
use jiff::tz::TimeZone;
use mxt_core::app::{App, Focus};
use mxt_core::ids::Timestamp;
use mxt_core::rich::{self, Block, Inline, RichText};
use mxt_core::timeline::{
    ItemRef, MessageContent, ReactionGroup, ReplyPreview, SendState, TimelineEntry, TimelineItem,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::view::{BOLD, DIM, ERROR, PLAIN, fit, grapheme_width, text_width};

/// `ui.time_format` when the configuration has none.
const DEFAULT_TIME_FORMAT: &str = "%H:%M";
/// The date on a date divider.
const DATE_FORMAT: &str = "%Y-%m-%d (%a)";
const READ_MARKER: &str = "new messages";
/// Before every row of a quote.
const QUOTE: &str = "│ ";
const CODE_BLOCK: Style = DIM;
/// Columns between tab stops.
const TAB_WIDTH: usize = 4;

/// A checked `ui.time_format`, the `strftime`-style format of the item
/// headers' times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeFormat(String);

impl TimeFormat {
    /// Checks `ui.time_format`: an empty format or an invalid directive is
    /// an error, so the configuration loader reports it before the UI starts
    /// (design, "Layers": a typo is never silently ignored).
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
    /// The cursor item of the last frame and the rows below it then, so the
    /// view stays put while the cursor moves inside it and while entries
    /// arrive above or below.
    anchor: Option<Anchor>,
}

struct Anchor {
    index: usize,
    item: ItemRef,
    below: usize,
}

impl TimelinePane {
    pub(crate) fn new() -> TimelinePane {
        TimelinePane {
            time_format: TimeFormat::default(),
            zone: TimeZone::system(),
            anchor: None,
        }
    }

    pub(crate) fn set_time_format(&mut self, format: TimeFormat) {
        self.time_format = format;
    }

    /// Draws the open room's entries into `area` and returns the first and
    /// last entry indices with a visible row.
    pub(crate) fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        app: &App,
    ) -> Option<(usize, usize)> {
        let entries = app.timeline();
        if entries.is_empty() {
            self.anchor = None;
            return None;
        }
        // The cursor bar and a space on the left, a space on the right.
        let width = usize::from(area.width).saturating_sub(3);
        let height = usize::from(area.height);
        if width == 0 || height == 0 {
            return None;
        }
        let render = Render {
            entries,
            width,
            height,
            time_format: &self.time_format.0,
            zone: &self.zone,
        };
        let cursor = app.timeline_cursor();
        let window = render.lay_out(cursor, app.timeline_follows(), self.anchor.as_ref());

        let bar = match app.focus() {
            Focus::Timeline => Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
            Focus::RoomList | Focus::Composer => Span::raw("│"),
        };
        let buffer = frame.buffer_mut();
        let mut y = window.top;
        for (index, rows) in (window.first..).zip(&window.rows) {
            for row in rows {
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

        self.anchor = render.anchor(&window, cursor);
        window.visible(render.pane_rows())
    }
}

/// Consecutive entries and their rows, placed on the pane's rows.
struct Window<'a> {
    first: usize,
    rows: VecDeque<Vec<Line<'a>>>,
    /// The pane row of the first entry's first row; negative when that row
    /// is above the pane.
    top: isize,
    /// All rows of the window.
    len: isize,
}

impl<'a> Window<'a> {
    /// The window of the entry at `index` with its first row at `top`.
    fn new(index: usize, rows: Vec<Line<'a>>, top: isize) -> Window<'a> {
        let len = rows_isize(&rows);
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

    fn push_back(&mut self, rows: Vec<Line<'a>>) {
        self.len += rows_isize(&rows);
        self.rows.push_back(rows);
    }

    fn push_front(&mut self, rows: Vec<Line<'a>>) {
        let len = rows_isize(&rows);
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
        Some(rows_isize(rows))
    }

    fn top_of(&self, index: usize) -> Option<isize> {
        let before = index.checked_sub(self.first)?;
        let above: isize = self
            .rows
            .iter()
            .take(before)
            .map(|rows| rows_isize(rows))
            .sum();
        (before < self.rows.len()).then_some(self.top + above)
    }

    /// Drops the entries without a row on the pane's `height` rows, keeping
    /// at least one.
    fn trim(&mut self, height: isize) {
        while self.rows.len() > 1
            && let Some(rows) = self.rows.front()
            && self.top + rows_isize(rows) <= 0
        {
            let len = rows_isize(rows);
            self.rows.pop_front();
            self.first += 1;
            self.top += len;
            self.len -= len;
        }
        while self.rows.len() > 1
            && let Some(rows) = self.rows.back()
            && self.bottom() - rows_isize(rows) >= height
        {
            self.len -= rows_isize(rows);
            self.rows.pop_back();
        }
    }

    /// The first and last entries with a row on the pane's `height` rows.
    fn visible(&self, height: isize) -> Option<(usize, usize)> {
        let mut y = self.top;
        let mut range: Option<(usize, usize)> = None;
        for (index, rows) in (self.first..).zip(&self.rows) {
            let len = rows_isize(rows);
            if len > 0 && y + len > 0 && y < height {
                range = Some((range.map_or(index, |(first, _)| first), index));
            }
            y += len;
        }
        range
    }
}

/// Renders entries for one frame.
struct Render<'a> {
    entries: &'a [TimelineEntry],
    width: usize,
    height: usize,
    time_format: &'a str,
    zone: &'a TimeZone,
}

impl<'a> Render<'a> {
    fn pane_rows(&self) -> isize {
        isize::try_from(self.height).unwrap_or(isize::MAX)
    }

    /// Places the entries around the cursor.
    fn lay_out(&self, cursor: Option<usize>, follows: bool, anchor: Option<&Anchor>) -> Window<'a> {
        let height = self.pane_rows();
        let last = self.entries.len() - 1;
        let mut window = match cursor.filter(|_| !follows) {
            Some(cursor) => self.around(cursor, anchor),
            // Following: the newest entry at the bottom.
            None => self.at_bottom(last),
        };

        // The cursor item fully on the pane; its rows never exceed the pane.
        if let Some(cursor) = cursor
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

    /// The anchor for the next frame: the cursor item and the rows below it.
    fn anchor(&self, window: &Window, cursor: Option<usize>) -> Option<Anchor> {
        let cursor = cursor?;
        let TimelineEntry::Item(item) = &self.entries[cursor] else {
            return None;
        };
        let bottom = window.top_of(cursor)? + window.len_of(cursor)?;
        Some(Anchor {
            index: cursor,
            item: item.id.clone(),
            below: usize::try_from(self.pane_rows() - bottom).unwrap_or(0),
        })
    }

    /// A window that holds the cursor item, placed where the last frame had
    /// the anchor. A lost anchor (its item went away or changed its
    /// `ItemRef`, as a local echo does once the server has it) is replaced
    /// by the entry at the anchor's index, clamped to the entries, which is
    /// where the core's `settle` keeps the cursor. A cursor more than a pane
    /// away from the anchor goes to the pane's edge it moved towards.
    fn around(&self, cursor: usize, anchor: Option<&Anchor>) -> Window<'a> {
        let height = self.pane_rows();
        let Some(anchor) = anchor else {
            return self.at_bottom(cursor);
        };
        let is_anchor = |entry: &TimelineEntry| matches!(entry, TimelineEntry::Item(item) if item.id == anchor.item);
        let index = if self.entries.get(anchor.index).is_some_and(is_anchor) {
            anchor.index
        } else {
            let clamped = anchor.index.min(self.entries.len() - 1);
            self.entries.iter().position(is_anchor).unwrap_or(clamped)
        };
        let rows = self.rows(index);
        let below = isize::try_from(anchor.below).unwrap_or(isize::MAX);
        let top = height - below - rows_isize(&rows);
        let mut window = Window::new(index, rows, top);
        while cursor > window.last() {
            if window.bottom() > 2 * height {
                return self.at_bottom(cursor);
            }
            window.push_back(self.rows(window.last() + 1));
        }
        while cursor < window.first {
            if window.top < -height {
                let rows = self.rows(cursor);
                let top = self.top_with_lead(cursor, rows_isize(&rows));
                return Window::new(cursor, rows, top);
            }
            window.push_front(self.rows(window.first - 1));
        }
        window
    }

    /// The window of the entry at `index` with its last row at the bottom.
    fn at_bottom(&self, index: usize) -> Window<'a> {
        let rows = self.rows(index);
        let top = self.pane_rows() - rows_isize(&rows);
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

    /// The rows of the entry at `index`. An item taller than the pane ends
    /// with `[+N lines]`.
    fn rows(&self, index: usize) -> Vec<Line<'a>> {
        match &self.entries[index] {
            TimelineEntry::Item(item) => {
                let mut rows = self.item(item);
                if rows.len() > self.height {
                    let hidden = rows.len() - (self.height - 1);
                    rows.truncate(self.height - 1);
                    rows.push(Line::styled(format!("[+{hidden} lines]"), DIM));
                }
                rows
            }
            TimelineEntry::DateDivider(timestamp) => {
                let date = self.format(*timestamp, DATE_FORMAT);
                vec![rule(&date, DIM, self.width)]
            }
            TimelineEntry::ReadMarker => {
                vec![rule(READ_MARKER, Style::new().fg(Color::Red), self.width)]
            }
            TimelineEntry::Hidden => Vec::new(),
        }
    }

    /// The header, the reply preview, the body and the reactions.
    fn item(&self, item: &'a TimelineItem) -> Vec<Line<'a>> {
        let mut rows = vec![self.header(item)];
        if let Some(reply) = &item.reply {
            rows.push(reply_line(reply, self.width));
        }
        let mut body = Body::new(self.width, rows);
        match &item.content {
            MessageContent::Text { rich, .. } => body.rich(rich),
            MessageContent::Notice(text) => {
                body.base = DIM;
                body.rich(text);
            }
            MessageContent::Emote(text) => {
                body.lead = vec![
                    (Cow::Borrowed("* "), PLAIN),
                    (Cow::Borrowed(item.sender_name.as_str()), PLAIN),
                    (Cow::Borrowed(" "), PLAIN),
                ];
                body.rich(text);
            }
            MessageContent::Image {
                name, dimensions, ..
            } => {
                let text = match dimensions {
                    Some((width, height)) => format!("[image: {name} {width}x{height}]"),
                    None => format!("[image: {name}]"),
                };
                body.line(vec![(Cow::Owned(text), PLAIN)]);
            }
            MessageContent::File { name, size, .. } => {
                let text = match size {
                    Some(size) => format!("[file: {name} {}]", file_size(*size)),
                    None => format!("[file: {name}]"),
                };
                body.line(vec![(Cow::Owned(text), PLAIN)]);
            }
            MessageContent::Sticker { body: text } => {
                body.line(vec![(Cow::Owned(format!("[sticker: {text}]")), PLAIN)]);
            }
            MessageContent::Redacted => body.line(vec![(Cow::Borrowed("(deleted)"), DIM)]),
            MessageContent::Undecryptable => {
                body.line(vec![(Cow::Borrowed("(unable to decrypt)"), DIM)]);
            }
            MessageContent::Unsupported { kind } => {
                body.line(vec![(Cow::Owned(format!("(unsupported: {kind})")), DIM)]);
            }
        }
        if !item.reactions.is_empty() {
            body.base = PLAIN;
            body.line(reactions(&item.reactions));
        }
        body.rows
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
        let (spans, _) = fit(&segments, self.width);
        Line::from(
            spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect::<Vec<_>>(),
        )
    }

    /// `timestamp` in the local time zone; `?` if it cannot be shown.
    fn format(&self, timestamp: Timestamp, format: &str) -> String {
        i64::try_from(timestamp.as_millis())
            .ok()
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
            .and_then(|time| {
                strtime::format(format, &time.to_zoned_datetime(self.zone.clone())).ok()
            })
            .unwrap_or_else(|| "?".to_owned())
    }
}

fn rows_isize(rows: &[Line]) -> isize {
    isize::try_from(rows.len()).unwrap_or(isize::MAX)
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

/// `key count` chips; the user's own reactions are bold.
fn reactions(groups: &[ReactionGroup]) -> Vec<Piece<'_>> {
    let mut pieces = Vec::with_capacity(groups.len() * 3);
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            pieces.push((Cow::Borrowed("  "), PLAIN));
        }
        let style = if group.mine { BOLD } else { PLAIN };
        pieces.push((Cow::Borrowed(group.key.as_str()), style));
        pieces.push((Cow::Owned(format!(" {}", group.count)), style));
    }
    pieces
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

/// Builds the rows of a message body: rich text wrapped to the width, after
/// the prefixes of the quotes and lists it is in.
struct Body<'a> {
    width: usize,
    rows: Vec<Line<'a>>,
    /// Under every piece's own style: dim for notices.
    base: Style,
    /// Put before the first line: `* name ` of an emote.
    lead: Vec<Piece<'a>>,
    /// Outermost first.
    prefixes: Vec<Prefix>,
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
        }
    }

    fn rich(&mut self, text: &'a RichText) {
        self.blocks(&text.blocks);
        if !self.lead.is_empty() {
            self.line(Vec::new());
        }
    }

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
            Block::Code { text, .. } => {
                for line in text.split('\n') {
                    self.line(vec![(Cow::Borrowed(line), CODE_BLOCK)]);
                }
            }
            Block::Quote(blocks) => self.nested(Prefix::Quote, blocks),
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
        self.prefixes.push(prefix);
        if blocks.is_empty() {
            self.line(Vec::new());
        }
        self.blocks(blocks);
        self.prefixes.pop();
    }

    /// One line of text, wrapped after the prefixes.
    fn line(&mut self, mut pieces: Vec<Piece<'a>>) {
        if !self.lead.is_empty() {
            pieces.splice(0..0, std::mem::take(&mut self.lead));
        }
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
fn paragraph_lines(inlines: &[Inline]) -> Vec<Vec<Piece<'_>>> {
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
            let part = if spoiler {
                // Blocks of the same width; the message view shows the text.
                // Tabs stay, so they take as many columns as in the text.
                let mut blocks = String::with_capacity(part.len());
                for (index, run) in part.split('\t').enumerate() {
                    if index > 0 {
                        blocks.push('\t');
                    }
                    blocks.extend(std::iter::repeat_n('█', text_width(run)));
                }
                Cow::Owned(blocks)
            } else {
                Cow::Borrowed(part)
            };
            if let Some(line) = lines.last_mut() {
                line.push((part, style));
            }
        }
    }
    lines
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
    use super::*;
    use mxt_core::ids::{EventId, TxnId, UserId};

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

    fn items(entries: &[TimelineEntry]) -> Vec<usize> {
        (0..entries.len())
            .filter(|&index| matches!(entries[index], TimelineEntry::Item(_)))
            .collect()
    }

    fn render<'a>(entries: &'a [TimelineEntry], zone: &'a TimeZone) -> Render<'a> {
        Render {
            entries,
            width: WIDTH,
            height: HEIGHT,
            time_format: DEFAULT_TIME_FORMAT,
            zone,
        }
    }

    /// One frame: the entries laid out as `(index, top row, rows)`, and the
    /// anchor for the next frame.
    fn lay_out(
        entries: &[TimelineEntry],
        cursor: usize,
        follows: bool,
        anchor: Option<&Anchor>,
    ) -> (Vec<(usize, isize, isize)>, Option<Anchor>) {
        let zone = TimeZone::UTC;
        let render = render(entries, &zone);
        let window = render.lay_out(Some(cursor), follows, anchor);
        let placed = (window.first..=window.last())
            .map(|index| {
                let top = window.top_of(index).unwrap();
                (index, top, window.len_of(index).unwrap())
            })
            .collect();
        (placed, render.anchor(&window, Some(cursor)))
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
        let lines = paragraph_lines(&inlines);
        let parts: Vec<&str> = lines[0].iter().map(|(part, _)| part.as_ref()).collect();
        assert_eq!(
            parts,
            ["████", "████", "██████", "█████", "@Alice", ":cat:"]
        );
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
    fn entries_added_above_or_below_leave_the_cursor_item_in_place() {
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

        // New messages.
        let mut appended = entries.clone();
        appended.extend([remote(102, 1), remote(103, 3)]);
        let (placed, _) = lay_out(&appended, cursor, false, anchor.as_ref());
        assert_eq!(place(&placed, cursor), rows);
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
        let rows = render(&entries, &zone).rows(0);
        assert_eq!(rows.len(), HEIGHT);
        // The header and 30 rows of text, of which 22 do not fit.
        assert_eq!(rows[HEIGHT - 1].to_string(), "[+22 lines]");
        // As the cursor item, it fills the pane.
        let (placed, _) = lay_out(&entries, 0, false, None);
        assert_eq!(placed, [(0, 0, 10)]);
    }
}
