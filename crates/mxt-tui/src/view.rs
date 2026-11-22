//! Layout and drawing, from the core state (design, "Layout", "Room names",
//! "Unread badges"). Colors come only from the 16-color palette.

use std::ops::RangeInclusive;

use mxt_core::app::{App, CommandLine, Focus, LineKind, StatusKind};
use mxt_core::effect::ViewContent;
use mxt_core::msg::{EditorMode, SyncState};
use mxt_core::room::{Badge, RoomName, RoomSummary};
use mxt_core::timeline::{Draft, ItemRef, TimelineEntry, TimelineItem};
use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::line::HORIZONTAL;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Padding};
use unicode_segmentation::UnicodeSegmentation;

use crate::clipboard::SharedRegister;
use crate::composer::Composer;
use crate::editor::Editor;
use crate::image_view::Images;
use crate::overlay::{self, Overlays};
use crate::previews::Previews;
use crate::timeline_view::{MAX_SHOWN_NEWER, TimelinePane, file_size, newer_items};

/// Terminals narrower than this give the room list a third of their width,
/// whatever `ui.room_list.width` says.
const NARROW_WIDTH: u16 = 90;
/// The timeline header and the composer line up with the entries, which
/// leave two columns for the cursor bar and one on the right.
const ENTRIES_PADDING: Padding = Padding {
    left: 2,
    right: 1,
    top: 0,
    bottom: 0,
};

pub(crate) const PLAIN: Style = Style::new();
pub(crate) const DIM: Style = Style::new().add_modifier(Modifier::DIM);
pub(crate) const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
pub(crate) const ERROR: Style = Style::new().fg(Color::Red);

/// Between the groups of the status line.
const GAP: &str = "  ";
const ELLIPSIS: &str = "…";
/// What the composer's top rule is drawn of: the block's border.
const RULE: &str = HORIZONTAL;
/// Between the network part and the room part of a displayed name.
const SEPARATOR: &str = " / ";
/// A name keeps at least this many columns where the layout can make room:
/// a room list row leaves out the mode label first, and a bridged name leaves
/// out its network part, already shortened to `…`.
const MIN_NAME_WIDTH: usize = 6;
/// Before the network part on a row of its own.
const NETWORK_INDENT: &str = "  ";
/// Each character of the recovery key in `SECRET` mode.
const MASK: &str = "*";
/// A thread's title row without its root.
const THREAD: &str = "Thread";
/// Before the root's sender on a thread's title row.
const THREAD_ROOT: &str = "Thread · ";

/// `ui.room_list.width` and `ui.room_list.network` (design, "Room list").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoomListLayout {
    pub width: RoomListWidth,
    pub network: NetworkPlacement,
}

/// A checked `ui.room_list.width`: the room list's columns, its border
/// included. Terminals narrower than 90 columns give the list a third of
/// their width instead, and it never takes more than half of the width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomListWidth(u16);

impl RoomListWidth {
    /// The columns `ui.room_list.width` accepts.
    pub const RANGE: RangeInclusive<u16> = 12..=80;

    /// `None` outside `RANGE`, so the configuration loader reports the
    /// value before the UI starts.
    pub fn new(columns: i64) -> Option<RoomListWidth> {
        u16::try_from(columns)
            .ok()
            .filter(|columns| RoomListWidth::RANGE.contains(columns))
            .map(RoomListWidth)
    }

    pub fn columns(self) -> u16 {
        self.0
    }
}

/// 30 columns.
impl Default for RoomListWidth {
    fn default() -> RoomListWidth {
        RoomListWidth(30)
    }
}

/// Where the room list shows a room's network part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkPlacement {
    /// `network / room` on one row, the network part shortened first.
    #[default]
    Inline,
    /// The room part on one row, and the network part on a second row,
    /// dim, indented and cut at its end. A room without a network part
    /// keeps one row.
    Below,
}

/// The panes' state between frames.
pub(crate) struct Panes<'a> {
    pub(crate) room_list: &'a mut ListState,
    pub(crate) room_list_layout: RoomListLayout,
    pub(crate) timeline: &'a mut TimelinePane,
    /// The message view while it is open.
    pub(crate) view: Option<&'a mut View>,
    /// The decoded images, if there is a picker for the terminal's graphics
    /// protocol.
    pub(crate) images: Option<&'a mut Images>,
    /// The timeline's previews, once `Tui::enable_previews` turned them on.
    pub(crate) previews: Option<&'a mut Previews>,
    /// The shown timeline's composer, once it had focus.
    pub(crate) composer: Option<&'a mut Composer>,
    pub(crate) overlays: &'a mut Overlays,
}

/// The full-screen message view (design, "Terminal adapter and the
/// composer"): a scratch editor holding the message's text, or an image's
/// details with the image below them.
pub(crate) struct View {
    pub(crate) editor: Editor,
    /// The item of an image message.
    image: Option<ItemRef>,
}

impl View {
    /// An image's details are its name, then its size and dimensions.
    pub(crate) fn new(content: ViewContent, register: SharedRegister) -> View {
        let (text, image) = match content {
            ViewContent::Text(text) => (text, None),
            ViewContent::Image {
                item,
                name,
                size,
                dimensions,
            } => {
                let details: Vec<String> = size
                    .map(file_size)
                    .into_iter()
                    .chain(dimensions.map(|(width, height)| format!("{width}x{height}")))
                    .collect();
                let text = if details.is_empty() {
                    name
                } else {
                    format!("{name}\n{}", details.join(GAP))
                };
                (text, Some(item))
            }
        };
        View {
            editor: Editor::new(&text, register),
            image,
        }
    }

    /// Whether the view is an image's.
    pub(crate) fn has_image(&self) -> bool {
        self.image.is_some()
    }

    /// Over `area`: the editor, and for an image the image a row below
    /// the details, once `images` keeps it.
    fn draw(&mut self, frame: &mut Frame, area: Rect, images: Option<&mut Images>) {
        let block = Block::new().padding(Padding::horizontal(1));
        let Some(item) = &self.image else {
            self.editor.draw(frame, area, block, true);
            return;
        };
        let rows = self.editor.rows(usize::from(block.inner(area).width));
        let [details, _, image] = Layout::vertical([
            Constraint::Length(u16::try_from(rows).unwrap_or(u16::MAX)),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        if let Some(images) = images {
            images.draw(frame, block.inner(image), item);
        }
        self.editor.draw(frame, details, block, true);
    }
}

/// The first and last visible items of the panes, inclusive; `None` for a
/// pane that shows none or is not drawn.
#[derive(Default)]
pub(crate) struct Visible {
    pub(crate) room_list: Option<(usize, usize)>,
    pub(crate) timeline: Option<(usize, usize)>,
}

/// Draws the frame. The message view covers everything but the bottom line,
/// and the picker is drawn over the panes. The terminal cursor goes to the
/// focused editor, the picker's query or the bottom line's prompt, whichever
/// is drawn last. In `SECRET` mode the bottom line is the masked recovery
/// key prompt, drawn from the key's length and cursor in grapheme clusters
/// only. The overlays' areas are known before the timeline is drawn, so that
/// it leaves out the previews they would cover.
pub(crate) fn draw(frame: &mut Frame, app: &App, panes: Panes) -> Visible {
    let [main, bottom] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let picker_area = app.picker().map(|_| overlay::picker_area(main));
    let mut visible = Visible::default();
    let mut composer = panes.composer;
    let focused = app.focus() == Focus::Composer && composer.is_some();
    let (search, view_mode) = match panes.view {
        Some(view) => {
            frame.render_widget(Clear, main);
            view.draw(frame, main, panes.images);
            (view.editor.search(), Some(view.editor.mode()))
        }
        None => {
            let right = if app.sidebar_visible() {
                let layout = panes.room_list_layout;
                let width = if main.width < NARROW_WIDTH {
                    main.width / 3
                } else {
                    layout.width.columns().min(main.width / 2)
                };
                let [list, right] =
                    Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)])
                        .areas(main);
                visible.room_list =
                    draw_room_list(frame, list, app, panes.room_list, layout.network);
                right
            } else {
                main
            };
            if app.open_room().is_some() {
                let height = composer_height(composer.as_deref(), right);
                let [timeline, composer_area] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(height)])
                        .areas(right);
                let menu = app.mention_menu();
                let menu_area = menu.and_then(|(members, _)| {
                    overlay::mention_menu_area(composer_area, timeline, members.len())
                });
                let covered: Vec<Rect> = picker_area.into_iter().chain(menu_area).collect();
                visible.timeline = draw_timeline(
                    frame,
                    timeline,
                    app,
                    panes.timeline,
                    panes.previews,
                    &covered,
                );
                let newer = visible
                    .timeline
                    .map_or(0, |(_, last)| newer_items(app.timeline(), last));
                draw_composer(
                    frame,
                    composer_area,
                    app,
                    composer.as_deref_mut(),
                    focused,
                    newer,
                );
                if let Some((members, selected)) = menu {
                    panes.overlays.draw_mention_menu(
                        frame,
                        composer_area,
                        timeline,
                        members,
                        selected,
                    );
                }
            } else {
                let covered: Vec<Rect> = picker_area.into_iter().collect();
                visible.timeline =
                    draw_timeline(frame, right, app, panes.timeline, panes.previews, &covered);
            }
            let search = composer
                .as_deref()
                .filter(|_| focused)
                .and_then(Composer::search);
            (search, None)
        }
    };
    if let Some(picker) = app.picker() {
        panes.overlays.draw_picker(frame, main, picker);
    }
    // The message view's editor while it is open, else the focused composer's.
    let editor = view_mode.or_else(|| composer.filter(|_| focused).map(|composer| composer.mode()));
    match (app.secret_prompt(), app.command_line(), search) {
        (Some(secret), _, _) => {
            let masked = MASK.repeat(secret.clusters);
            let cursor = secret.cursor * MASK.len();
            draw_prompt_line(frame, bottom, secret.prompt, &masked, cursor);
        }
        (None, Some(line), _) => draw_command_line(frame, bottom, line),
        // edtui's own search in the editors: the cursor after the pattern.
        (None, None, Some(pattern)) => {
            draw_prompt_line(frame, bottom, "/", &pattern, pattern.len());
        }
        (None, None, None) => draw_status_line(frame, bottom, app, editor),
    }
    visible
}

/// The list scrolls so the cursor stays visible. The range it returns is in
/// rooms, and a room can take two rows: ratatui draws the rooms from the
/// offset it leaves in `state` whose rows fit whole, once `scroll_offset`
/// has set where a downward scroll starts.
fn draw_room_list(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &mut ListState,
    network: NetworkPlacement,
) -> Option<(usize, usize)> {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .padding(Padding::right(1));
    let inner = block.inner(area);
    let width = usize::from(inner.width);
    let open = app.open_room();
    let rooms = app.rooms().rooms();
    let items: Vec<ListItem> = rooms
        .iter()
        .map(|room| room_item(room, open == Some(&room.id), width, network))
        .collect();
    let cursor = match app.focus() {
        Focus::RoomList => Modifier::REVERSED,
        Focus::Timeline | Focus::Composer => Modifier::UNDERLINED,
    };
    let list = List::new(items).block(block).highlight_style(cursor);
    let height = usize::from(inner.height);
    let selected = app.room_cursor();
    state.select(selected);
    if let Some(selected) = selected {
        *state.offset_mut() = scroll_offset(rooms, network, selected, state.offset(), height);
    }
    frame.render_stateful_widget(list, area, state);

    let first = state.offset();
    let shown = rooms
        .iter()
        .skip(first)
        .scan(0, |rows, room| {
            *rows += room_height(room, network);
            (*rows <= height).then_some(())
        })
        .count();
    (shown > 0).then(|| (first, first + shown - 1))
}

/// The list's offset for a frame with the cursor on room `selected`.
/// Scrolling down, ratatui drops rooms from the top until the rows down to
/// the cursor's room fit, and then draws no room after the cursor, which
/// leaves a row blank when a dropped room had two rows. Starting at the
/// first room from which the cursor's room still fits, it fills the rows
/// after the cursor too. An offset after the cursor stays: scrolling up,
/// ratatui starts at the cursor's room and fills the rows after it.
fn scroll_offset(
    rooms: &[RoomSummary],
    network: NetworkPlacement,
    selected: usize,
    offset: usize,
    height: usize,
) -> usize {
    let Some(down_to) = rooms.get(..=selected) else {
        return offset;
    };
    let mut rows = 0;
    let first = down_to
        .iter()
        .rposition(|room| {
            rows += room_height(room, network);
            rows > height
        })
        .map_or(0, |dropped| dropped + 1);
    // A cursor's room taller than the list is not drawn; it stays first.
    offset.max(first.min(selected))
}

/// A room's rows: the row of `room_row`, then with
/// `NetworkPlacement::Below` the network part's row.
fn room_item(
    room: &RoomSummary,
    is_open: bool,
    width: usize,
    network: NetworkPlacement,
) -> ListItem<'_> {
    let row = room_row(room, is_open, width, network);
    match network_below(room, network) {
        Some(part) => ListItem::new(vec![row, network_row(part, width)]),
        None => ListItem::new(row),
    }
}

/// The rows `room_item` gives the room.
fn room_height(room: &RoomSummary, network: NetworkPlacement) -> usize {
    1 + usize::from(network_below(room, network).is_some())
}

/// The network part that goes on a row of its own.
fn network_below(room: &RoomSummary, network: NetworkPlacement) -> Option<&str> {
    match network {
        NetworkPlacement::Inline => None,
        NetworkPlacement::Below => room.name.network.as_deref(),
    }
}

/// The network part, dim, after `NETWORK_INDENT`, cut at its end.
fn network_row(network: &str, width: usize) -> Line<'_> {
    Line::from(fit(&[(NETWORK_INDENT, PLAIN), (network, DIM)], width).0)
}

/// The displayed name, cut to fit, and right-aligned the badge and the
/// notification mode label. With `NetworkPlacement::Below` the name is the
/// room part alone. The label is left out before the name would get fewer
/// than `MIN_NAME_WIDTH` columns: the badge is the signal to act on. The
/// right part never takes more than the row.
fn room_row(
    room: &RoomSummary,
    is_open: bool,
    width: usize,
    network: NetworkPlacement,
) -> Line<'_> {
    let inline = network == NetworkPlacement::Inline;
    let mut right = Vec::with_capacity(3);
    let mut right_width =
        Badge::from_counts(&room.unread).map_or(0, |badge| push_badge(&mut right, badge, width));
    if let Some(label) = room.mode.label() {
        let space = usize::from(!right.is_empty());
        let with_label = right_width + space + text_width(label);
        // One column stays between the name and the right part.
        let name_room = width.checked_sub(with_label + 1);
        let name_columns = if inline {
            name_width(&room.name)
        } else {
            text_width(&room.name.room)
        };
        let wanted = name_columns.min(MIN_NAME_WIDTH);
        if name_room.is_some_and(|columns| columns >= wanted) {
            if space > 0 {
                right.push(Span::raw(" "));
            }
            right.push(Span::styled(label, DIM));
            right_width = with_label;
        }
    }
    let gap = usize::from(right_width > 0);

    let room_style = if is_open { BOLD } else { PLAIN };
    let name_room = width.saturating_sub(right_width + gap);
    let (mut spans, used) = if inline {
        fit_name(&room.name, room_style, name_room)
    } else {
        fit(&[(room.name.room.as_str(), room_style)], name_room)
    };
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + right_width)),
    ));
    spans.extend(right);
    Line::from(spans)
}

/// Pushes the badge in its style (design, "Unread badges") onto `spans` and
/// returns its columns. It is cut only when `width` is narrower than the
/// badge.
pub(crate) fn push_badge(spans: &mut Vec<Span<'_>>, badge: Badge, width: usize) -> usize {
    let text = badge.text();
    let style = badge_style(badge);
    let badge_width = text_width(&text);
    if badge_width <= width {
        spans.push(Span::styled(text, style));
        return badge_width;
    }
    let (cut, used) = fit(&[(text.as_str(), style)], width);
    spans.extend(
        cut.into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style)),
    );
    used
}

fn badge_style(badge: Badge) -> Style {
    match badge {
        Badge::Mentions(_) => BOLD.fg(Color::Red),
        Badge::Notifications(_) => BOLD,
        Badge::Messages(_) => DIM,
        Badge::Marked => Style::new().fg(Color::Yellow),
    }
}

/// The displayed name, `network / room`, cut to `width` columns. The network
/// part is dim, so a room whose own name is `Discord / Boss` can be told apart
/// from a bridged room.
///
/// A bridged name loses the end of its network part first, down to `…`, so
/// the room part that tells rooms apart stays; only then is the room part
/// cut. When `… / ` would leave the room part fewer than `MIN_NAME_WIDTH`
/// columns, the room part is shown alone.
fn fit_name(name: &RoomName, room_style: Style, width: usize) -> (Vec<Span<'_>>, usize) {
    let room = (name.room.as_str(), room_style);
    let Some(network) = name.network.as_deref() else {
        return fit(&[room], width);
    };
    let separator_width = text_width(SEPARATOR);
    let rest_width = separator_width + text_width(&name.room);
    if let Some(network_width) = width.checked_sub(rest_width).filter(|&w| w > 0) {
        let (mut spans, used) = fit(&[(network, DIM)], network_width);
        spans.extend([
            Span::styled(SEPARATOR, DIM),
            Span::styled(room.0, room_style),
        ]);
        return (spans, used + rest_width);
    }
    if width < text_width(ELLIPSIS) + separator_width + MIN_NAME_WIDTH {
        return fit(&[room], width);
    }
    fit(&[(ELLIPSIS, DIM), (SEPARATOR, DIM), room], width)
}

/// The columns of the whole displayed name.
fn name_width(name: &RoomName) -> usize {
    let network = name.network.as_deref();
    let prefix = network.map_or(0, |network| text_width(network) + text_width(SEPARATOR));
    prefix + text_width(&name.room)
}

/// The open room's displayed name as a header, a thread's title row while
/// a thread is shown in place of the timeline, and the shown timeline's
/// entries below, with their previews except under `covered`.
fn draw_timeline(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    pane: &mut TimelinePane,
    previews: Option<&mut Previews>,
    covered: &[Rect],
) -> Option<(usize, usize)> {
    let block = Block::new().padding(ENTRIES_PADDING);
    let row = |top: u16| Rect {
        y: area.y + top,
        height: area.height.saturating_sub(top).min(1),
        ..block.inner(area)
    };
    let header = row(0);
    let Some(room) = app.open_room().and_then(|id| app.rooms().get(id)) else {
        frame.render_widget(Line::styled("No room open", DIM), header);
        return None;
    };
    let name = Line::from(fit_name(&room.name, BOLD, usize::from(header.width)).0);
    frame.render_widget(name, header);
    let mut top = header.height;
    if let Some(thread) = app.thread() {
        let title = row(top);
        draw_thread_title(frame, title, thread.item);
        top += title.height;
    }
    let entries = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    pane.draw(frame, entries, app, previews, covered)
}

/// A thread's title row (design, "Threads"): `Thread · ` dim, the root's
/// sender bold, and the first line of the root's text as `yy` copies it,
/// cut with `…`; `Thread` alone without the root's item.
fn draw_thread_title(frame: &mut Frame, area: Rect, root: Option<&TimelineItem>) {
    let root = root.map(|root| (root, root.content.plain_text()));
    let segments = match &root {
        Some((root, text)) => vec![
            (THREAD_ROOT, DIM),
            (root.sender_name.as_str(), BOLD),
            (": ", PLAIN),
            (text.lines().next().unwrap_or_default(), PLAIN),
        ],
        None => vec![(THREAD, DIM)],
    };
    let line = Line::from(fit(&segments, usize::from(area.width)).0);
    frame.render_widget(line, area);
}

/// The composer's height in `pane`: a rule, and the rows of its text, from
/// one up to a third of the pane.
fn composer_height(composer: Option<&Composer>, pane: Rect) -> u16 {
    let width = pane
        .width
        .saturating_sub(ENTRIES_PADDING.left + ENTRIES_PADDING.right);
    let rows = composer.map_or(1, |composer| composer.rows(usize::from(width)));
    let most = usize::from(pane.height / 3).max(1);
    u16::try_from(rows.min(most)).unwrap_or(1) + 1
}

/// The shown timeline's composer: a rule naming a reply or an edit and
/// ending in the hint for the `newer` items below the timeline pane, then
/// the text. A timeline whose composer never had focus shows an empty one.
fn draw_composer(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    composer: Option<&mut Composer>,
    focused: bool,
    newer: usize,
) {
    let mut block = Block::new()
        .borders(Borders::TOP)
        .border_style(DIM)
        .padding(ENTRIES_PADDING);
    // The columns the label takes from the rule's left end.
    let mut label_width = 0;
    if let Some(label) = draft_label(app) {
        let label = format!(" {label} ");
        label_width = text_width(RULE) + text_width(&label);
        block = block.title(Line::from(vec![Span::styled(RULE, DIM), Span::raw(label)]));
    }
    match composer {
        Some(composer) => composer.draw(frame, area, block, focused),
        None => frame.render_widget(block, area),
    }
    draw_newer_hint(frame, area, label_width, newer);
}

/// The newer-messages hint (design, "Timeline") over the right end of the
/// rule, `area`'s first row: ` ↓ N newer ` and a `─`, dim, with `99+` for
/// counts above 99, after the label's `label_width` columns and at least
/// one `─`. Where that does not fit, ` ↓ N ` and the `─`; else nothing, so
/// the label is never cut for it.
fn draw_newer_hint(frame: &mut Frame, area: Rect, label_width: usize, newer: usize) {
    if newer == 0 || area.height == 0 {
        return;
    }
    let count = newer.min(MAX_SHOWN_NEWER);
    let plus = if newer > MAX_SHOWN_NEWER { "+" } else { "" };
    let room = usize::from(area.width).saturating_sub(label_width + text_width(RULE));
    let mut hint = format!(" ↓ {count}{plus} newer {RULE}");
    if text_width(&hint) > room {
        hint = format!(" ↓ {count}{plus} {RULE}");
        if text_width(&hint) > room {
            return;
        }
    }
    let width = u16::try_from(text_width(&hint)).unwrap_or(area.width);
    let hint_area = Rect {
        x: area.right() - width,
        width,
        height: 1,
        ..area
    };
    frame.render_widget(Line::styled(hint, DIM), hint_area);
}

/// What the shown timeline's draft is, unless it is a new message.
fn draft_label(app: &App) -> Option<String> {
    match app.draft(&app.shown_timeline()?)? {
        Draft::New => None,
        Draft::Reply { to } => {
            let sender = app.timeline().iter().find_map(|entry| match entry {
                TimelineEntry::Item(item) if matches!(&item.id, ItemRef::Remote(id) if id == to) => {
                    Some(item.sender_name.as_str())
                }
                _ => None,
            });
            Some(sender.map_or_else(|| "Reply".to_owned(), |name| format!("Reply to {name}")))
        }
        Draft::Edit { .. } => Some("Edit".to_owned()),
    }
}

/// The mode (with the mode of the message view's editor, or of the
/// composer's while it has focus), the pending count and keys, the open
/// room, the confirmation question or the status message, and right-aligned
/// the sync state. When they do not fit, the room name is cut first, so the
/// question or the status message stays readable.
fn draw_status_line(frame: &mut Frame, area: Rect, app: &App, editor: Option<EditorMode>) {
    let mode = format!(" {} ", app.mode_label(editor));
    let mut pending = app
        .pending_count()
        .map(|n| n.to_string())
        .unwrap_or_default();
    for key in app.pending_keys() {
        pending.push_str(&key.to_string());
    }
    let mut head = vec![(mode.as_str(), Style::new().add_modifier(Modifier::REVERSED))];
    if !pending.is_empty() {
        head.extend([(GAP, PLAIN), (pending.as_str(), PLAIN)]);
    }
    let question = app.confirmation();
    let status = match (question.as_deref(), app.status()) {
        (Some(question), _) => Some([(GAP, PLAIN), (question, BOLD)]),
        (None, Some(status)) => {
            let style = match status.kind {
                StatusKind::Info => PLAIN,
                StatusKind::Error => ERROR,
            };
            Some([(GAP, PLAIN), (status.text.as_str(), style)])
        }
        (None, None) => None,
    };

    let sync = sync_label(app.sync_state());
    let width = usize::from(area.width);
    // One column stays between the left part and the sync state.
    let available = width.saturating_sub(text_width(sync) + 1);
    let (mut spans, mut used) = fit(&head, available);
    if let Some(room) = app.open_room().and_then(|id| app.rooms().get(id)) {
        let status_width = status.as_ref().map_or(0, |status| segments_width(status));
        let room_width = available.saturating_sub(used + status_width);
        let gap_width = text_width(GAP);
        if room_width > gap_width {
            let (room, room_used) = fit_name(&room.name, PLAIN, room_width - gap_width);
            spans.push(Span::raw(GAP));
            spans.extend(room);
            used += gap_width + room_used;
        }
    }
    if let Some(status) = &status {
        let (status, status_used) = fit(status, available.saturating_sub(used));
        spans.extend(status);
        used += status_used;
    }
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + text_width(sync))),
    ));
    spans.push(Span::raw(sync));
    frame.render_widget(Line::from(spans), area);
}

fn sync_label(state: SyncState) -> &'static str {
    match state {
        SyncState::Starting => "starting",
        SyncState::Synced => "synced",
        SyncState::Offline => "offline",
        SyncState::Retrying => "retrying",
    }
}

/// `:` or `/` and the text, with the terminal cursor at the line's cursor.
fn draw_command_line(frame: &mut Frame, area: Rect, line: &CommandLine) {
    let prompt = match line.kind {
        LineKind::Command => ":",
        LineKind::Search(_) => "/",
    };
    draw_prompt_line(frame, area, prompt, line.input.text(), line.input.cursor());
}

/// The prompt and the text, with the terminal cursor at `cursor`, a byte
/// offset in `text` on a grapheme cluster boundary: on the first column of
/// the cluster after it, or after the text at its end (design, "Line
/// editing"). A text wider than the line loses whole clusters at its start,
/// as few as it takes for the cluster after the cursor (a column at the end)
/// to fit, so with the cursor at the end, the end of the text stays visible;
/// the line's end cuts off the rest.
pub(crate) fn draw_prompt_line(
    frame: &mut Frame,
    area: Rect,
    prompt: &str,
    text: &str,
    cursor: usize,
) {
    let prompt_columns = text_width(prompt);
    let (before, after) = text.split_at(cursor);
    let under_cursor = after.graphemes(true).next().map_or(0, grapheme_width);
    let room = usize::from(area.width).saturating_sub(prompt_columns + under_cursor.max(1));
    let mut start = 0;
    let mut before_columns = text_width(before);
    for (at, grapheme) in before.grapheme_indices(true) {
        if before_columns <= room {
            break;
        }
        start = at + grapheme.len();
        before_columns -= grapheme_width(grapheme);
    }
    frame.render_widget(
        Line::from(vec![Span::raw(prompt), Span::raw(&text[start..])]),
        area,
    );
    let column = u16::try_from(prompt_columns + before_columns).unwrap_or(u16::MAX);
    let x = area.x.saturating_add(column);
    frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
}

/// Spans of the segments cut to `width` columns at grapheme cluster
/// boundaries, ending with `…` when cut, and the columns they take. Empty
/// segments are left out.
pub(crate) fn fit<'a>(segments: &[(&'a str, Style)], width: usize) -> (Vec<Span<'a>>, usize) {
    let mut spans = Vec::with_capacity(segments.len() + 1);
    let total = segments_width(segments);
    if total <= width {
        spans.extend(
            segments
                .iter()
                .filter(|(text, _)| !text.is_empty())
                .map(|&(text, style)| Span::styled(text, style)),
        );
        return (spans, total);
    }
    let ellipsis = text_width(ELLIPSIS);
    let budget = width.saturating_sub(ellipsis);
    let mut used = 0;
    for &(text, style) in segments {
        for (at, grapheme) in text.grapheme_indices(true) {
            let w = grapheme_width(grapheme);
            if used + w > budget {
                if at > 0 {
                    spans.push(Span::styled(&text[..at], style));
                }
                if width >= ellipsis {
                    spans.push(Span::styled(ELLIPSIS, style));
                    used += ellipsis;
                }
                return (spans, used);
            }
            used += w;
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, style));
        }
    }
    (spans, used)
}

fn segments_width(segments: &[(&str, Style)]) -> usize {
    segments.iter().map(|(text, _)| text_width(text)).sum()
}

/// The columns ratatui gives the text: it draws each segment, a span, per
/// grapheme cluster.
pub(crate) fn text_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// The columns ratatui gives a grapheme cluster. A cluster with a control
/// character takes none: ratatui leaves it out.
pub(crate) fn grapheme_width(grapheme: &str) -> usize {
    if grapheme.contains(char::is_control) {
        0
    } else {
        usize::from(grapheme.cell_width())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::clipboard::SharedRegister;
    use crate::editor::tests::keys;
    use mxt_core::effect::{Effect, UiEffect};
    use mxt_core::ids::{EventId, RoomId, TimelineId, Timestamp, UserId};
    use mxt_core::keymap::Keymap;
    use mxt_core::keys::KeySeq;
    use mxt_core::msg::Msg;
    use mxt_core::rich::RichText;
    use mxt_core::room::{NotificationMode, RoomKind, UnreadCounts};
    use mxt_core::settings::Settings;
    use mxt_core::timeline::{MessageContent, TimelineDiff, TimelineItem};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    fn no_rooms() -> App {
        let leader = KeySeq::parse("<Space>", &KeySeq::default()).unwrap();
        App::new(
            Keymap::new(&leader, []),
            Settings::default(),
            UserId::new("@me:example.org"),
        )
    }

    /// The app with the rooms `!a` and `!b`, `!a` first, and the room list
    /// focused.
    pub(crate) fn app() -> App {
        let mut app = no_rooms();
        for (id, at) in [("!a:example.org", 20), ("!b:example.org", 10)] {
            app.update(upsert(id, at));
        }
        app
    }

    /// The group `id` in the room list, last active at `at` milliseconds.
    pub(crate) fn upsert(id: &str, at: u64) -> Msg {
        Msg::RoomUpserted(RoomSummary {
            id: RoomId::new(id),
            name: name(None, id),
            kind: RoomKind::Group,
            unread: UnreadCounts::default(),
            mode: NotificationMode::Default,
            latest_activity: Some(Timestamp::from_millis(at)),
            last_read: None,
        })
    }

    /// The app with `rooms` listed in this order, as `!0:example.org` and
    /// on, and the room list focused.
    fn app_with(rooms: Vec<RoomSummary>) -> App {
        let mut app = no_rooms();
        for (n, room) in (0..).zip(rooms) {
            app.update(Msg::RoomUpserted(RoomSummary {
                id: RoomId::new(format!("!{n}:example.org")),
                // The newest first.
                latest_activity: Some(Timestamp::from_millis(1000 - n)),
                ..room
            }));
        }
        app
    }

    /// The keys in key notation, as the core resolves them.
    pub(crate) fn press(app: &mut App, text: &str) -> Vec<Effect> {
        keys(text)
            .into_iter()
            .flat_map(|key| app.update(Msg::Key { key, editor: None }))
            .collect()
    }

    /// The one-line message with the event `$<n>` and the body `message <n>`.
    pub(crate) fn message(n: u32) -> TimelineEntry {
        TimelineEntry::Item(TimelineItem {
            id: ItemRef::Remote(EventId::new(format!("${n}:example.org"))),
            sender: UserId::new("@alice:example.org"),
            sender_name: "Alice".to_owned(),
            timestamp: Timestamp::from_millis(0),
            content: MessageContent::Text {
                body: format!("message {n}"),
                rich: RichText::plain(&format!("message {n}")),
            },
            reply: None,
            reactions: Vec::new(),
            edited: false,
            send_state: None,
            thread_summary: None,
        })
    }

    /// `room`'s timeline as one-line messages with the events `$<n>`.
    pub(crate) fn reset(room: &str, events: std::ops::Range<u32>) -> Msg {
        Msg::Timeline {
            timeline: TimelineId::main(RoomId::new(room)),
            diffs: vec![TimelineDiff::Reset(events.map(message).collect())],
        }
    }

    /// A terminal and the panes' state between its frames.
    pub(crate) struct Screen {
        pub(crate) terminal: Terminal<TestBackend>,
        room_list: ListState,
        room_list_layout: RoomListLayout,
        timeline: TimelinePane,
        overlays: Overlays,
        pub(crate) previews: Option<Previews>,
        /// Whether the last frame must be drawn again on a cleared screen
        /// (see `Previews::finish_frame`).
        pub(crate) again: bool,
    }

    impl Screen {
        pub(crate) fn new() -> Screen {
            Screen::sized(80, RoomListLayout::default())
        }

        /// A terminal `width` columns wide and 12 rows high.
        pub(crate) fn sized(width: u16, room_list_layout: RoomListLayout) -> Screen {
            Screen::of(width, 12, room_list_layout)
        }

        /// A terminal of `width` by `height`.
        pub(crate) fn of(width: u16, height: u16, room_list_layout: RoomListLayout) -> Screen {
            Screen {
                terminal: Terminal::new(TestBackend::new(width, height)).unwrap(),
                room_list: ListState::default(),
                room_list_layout,
                timeline: TimelinePane::new(),
                overlays: Overlays::default(),
                previews: None,
                again: false,
            }
        }

        /// One frame, with the message view while it is open.
        pub(crate) fn draw(&mut self, app: &App, view: Option<&mut View>) -> Visible {
            let mut visible = Visible::default();
            if let Some(previews) = &mut self.previews {
                previews.begin_frame(app);
            }
            self.terminal
                .draw(|frame| {
                    let panes = Panes {
                        room_list: &mut self.room_list,
                        room_list_layout: self.room_list_layout,
                        timeline: &mut self.timeline,
                        view,
                        images: None,
                        previews: self.previews.as_mut(),
                        composer: None,
                        overlays: &mut self.overlays,
                    };
                    visible = draw(frame, app, panes);
                })
                .unwrap();
            self.again = self.previews.as_mut().is_some_and(Previews::finish_frame);
            visible
        }

        /// Clears the screen, as `Tui::draw` does before drawing a frame
        /// again.
        pub(crate) fn repaint(&mut self) {
            crate::terminal::repaint(&mut self.terminal).unwrap();
            if let Some(previews) = &mut self.previews {
                previews.screen_cleared();
            }
        }

        /// Row `y` as drawn, without the cells wide characters cover.
        fn row(&self, y: u16) -> String {
            let buffer = self.terminal.backend().buffer();
            let mut line = String::new();
            let mut x = buffer.area.left();
            while x < buffer.area.right() {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += u16::try_from(grapheme_width(symbol).max(1)).unwrap();
            }
            line
        }

        fn bottom_line(&self) -> String {
            self.row(self.terminal.backend().buffer().area.bottom() - 1)
        }

        /// The row of the composer's top rule: over its one row of text
        /// and the bottom line.
        pub(crate) fn composer_rule(&self) -> String {
            self.row(self.terminal.backend().buffer().area.bottom() - 3)
        }

        /// The bottom line without trailing spaces, and the terminal
        /// cursor's column.
        fn prompt(&mut self, app: &App) -> (String, u16) {
            self.draw(app, None);
            let cursor = self.terminal.get_cursor_position().unwrap();
            assert_eq!(cursor.y, 11);
            (self.bottom_line().trim_end().to_owned(), cursor.x)
        }

        pub(crate) fn cell(&self, x: u16, y: u16) -> &Cell {
            &self.terminal.backend().buffer()[(x, y)]
        }

        /// The first `rows` rows of a 30-column room list, without trailing
        /// spaces.
        fn list_rows(&self, rows: u16) -> Vec<String> {
            (0..rows)
                .map(|y| {
                    let row: String = (0..LIST_COLUMNS)
                        .map(|x| self.cell(x, y).symbol())
                        .collect();
                    row.trim_end().to_owned()
                })
                .collect()
        }

        /// The column of the room list's border.
        fn border(&self) -> Option<u16> {
            let width = self.terminal.backend().buffer().area.width;
            (0..width).find(|&x| self.cell(x, 0).symbol() == "│")
        }
    }

    #[test]
    fn the_status_line_shows_the_message_view_editor_mode() {
        let mut app = app();
        press(&mut app, "<CR>");
        app.update(reset("!a:example.org", 1..4));
        press(&mut app, "<CR>");
        assert!(app.view_open());
        let mut screen = Screen::new();
        let mut view = View::new(
            ViewContent::Text("message 3".to_owned()),
            SharedRegister::default(),
        );
        screen.draw(&app, Some(&mut view));
        assert!(
            screen.bottom_line().starts_with(" VIEW NORMAL "),
            "{:?}",
            screen.bottom_line()
        );
        // In insert mode `q` is text, not `view.close`.
        keys("i")
            .into_iter()
            .for_each(|key| view.editor.feed(key, &mut Vec::new()));
        screen.draw(&app, Some(&mut view));
        assert!(
            screen.bottom_line().starts_with(" VIEW INSERT "),
            "{:?}",
            screen.bottom_line()
        );
    }

    /// Types `text` in the open message view as the terminal does: each key
    /// goes to the core with the view editor's key mode, and `ReplayKeys`
    /// to the editor. Whether the core closed the view.
    fn type_in_view(app: &mut App, view: &mut View, text: &str) -> bool {
        let mut closed = false;
        for key in keys(text) {
            let editor = Some(view.editor.key_mode());
            for effect in app.update(Msg::Key { key, editor }) {
                match effect {
                    Effect::Ui(UiEffect::ReplayKeys(keys)) => {
                        keys.into_iter()
                            .for_each(|key| view.editor.feed(key, &mut Vec::new()));
                    }
                    Effect::Ui(UiEffect::CloseView) => closed = true,
                    _ => {}
                }
            }
        }
        closed
    }

    #[test]
    fn the_key_after_g_in_the_message_view_goes_to_its_editor() {
        // edtui drops `gq` and `g<Esc>`; the next `q` is `view.close`.
        for typed in ["gq", "g<Esc>"] {
            let mut app = app();
            press(&mut app, "<CR>");
            app.update(reset("!a:example.org", 1..4));
            press(&mut app, "<CR>");
            let mut view = View::new(
                ViewContent::Text("message 3".to_owned()),
                SharedRegister::default(),
            );
            assert!(!type_in_view(&mut app, &mut view, typed), "{typed}");
            assert!(app.view_open(), "{typed}");
            assert!(type_in_view(&mut app, &mut view, "q"), "{typed}");
            assert!(!app.view_open(), "{typed}");
        }
    }

    #[test]
    fn black_hole_yy_in_the_message_view_keeps_the_register() {
        let mut app = app();
        press(&mut app, "<CR>");
        app.update(reset("!a:example.org", 1..4));
        press(&mut app, "<CR>");
        let register = SharedRegister::default();
        let text = ViewContent::Text("message 3".to_owned());
        let mut view = View::new(text, register.clone());
        type_in_view(&mut app, &mut view, r#""_yy"#);
        assert_eq!(register.take_unsent(), None);
        type_in_view(&mut app, &mut view, "yy");
        assert!(register.take_unsent().is_some());
    }

    #[test]
    fn the_recovery_key_is_drawn_masked_and_only_while_it_is_typed() {
        let mut app = app();
        let mut screen = Screen::new();
        press(&mut app, ":recover<CR>s3cr<Space>t");
        screen.draw(&app, None);
        assert_eq!(screen.bottom_line().trim_end(), "Recovery key: ******");
        let buffer = screen.terminal.backend().buffer();
        let drawn: String = buffer.content().iter().map(Cell::symbol).collect();
        assert!(!drawn.contains("s3cr"), "{drawn}");
        assert_eq!(
            screen.terminal.get_cursor_position().unwrap(),
            Position::new(20, 11)
        );

        press(&mut app, "<Esc>");
        screen.draw(&app, None);
        assert!(
            screen.bottom_line().starts_with(" NORMAL "),
            "{:?}",
            screen.bottom_line()
        );
    }

    #[test]
    fn the_terminal_cursor_sits_on_the_line_cursor() {
        let mut app = app();
        let mut screen = Screen::new();
        press(&mut app, ":ab日本語<Left><Left>");
        assert_eq!(screen.prompt(&app), (":ab日本語".into(), 5));
        press(&mut app, "<Home>");
        assert_eq!(screen.prompt(&app), (":ab日本語".into(), 1));
        press(&mut app, "<End>");
        assert_eq!(screen.prompt(&app), (":ab日本語".into(), 9));
        press(&mut app, "<Esc>:recover<CR>abc<Left><Left>");
        assert_eq!(screen.prompt(&app), ("Recovery key: ***".into(), 15));
    }

    #[test]
    fn the_terminal_cursor_sits_on_the_picker_query_cursor() {
        let mut app = app();
        let mut screen = Screen::new();
        press(&mut app, "<Space><Space>a日本<Home>");
        screen.draw(&app, None);
        assert!(screen.row(1).contains("│> a日本 "), "{:?}", screen.row(1));
        // The query starts in the cell after the picker's `> `, on the row
        // under its top border.
        let width = screen.terminal.backend().buffer().area.width;
        let start = (1..width)
            .find(|&x| screen.cell(x - 1, 1).symbol() == ">" && screen.cell(x, 1).symbol() == " ")
            .unwrap()
            + 1;
        let cursor = screen.terminal.get_cursor_position().unwrap();
        assert_eq!(cursor, Position::new(start, 1));
        press(&mut app, "<End>");
        screen.draw(&app, None);
        // `a日本` takes five columns.
        let cursor = screen.terminal.get_cursor_position().unwrap();
        assert_eq!(cursor, Position::new(start + 5, 1));
    }

    #[test]
    fn a_line_wider_than_the_screen_loses_as_much_of_its_start_as_the_cursor_needs() {
        let mut app = app();
        let mut screen = Screen::sized(10, RoomListLayout::default());
        // 16 columns. At the end, the end of the text and a column for the
        // cursor fit.
        press(&mut app, ":日本語のテキスト");
        assert_eq!(screen.prompt(&app), (":テキスト".into(), 9));
        // Before `ス`, both of its columns fit; `ト` is cut off.
        press(&mut app, "<Left><Left>");
        assert_eq!(screen.prompt(&app), (":のテキス".into(), 7));
        press(&mut app, "<Home>");
        assert_eq!(screen.prompt(&app), (":日本語の".into(), 1));
    }

    const FAMILY: &str = "👨\u{200d}👩\u{200d}👧";

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    /// The drawn text and the columns `fit` reports.
    fn fit_plain(segment: &str, width: usize) -> (String, usize) {
        let (spans, used) = fit(&[(segment, PLAIN)], width);
        (text(&spans), used)
    }

    fn name(network: Option<&str>, room: &str) -> RoomName {
        RoomName {
            network: network.map(str::to_owned),
            room: room.to_owned(),
        }
    }

    fn fit_name_text(name: &RoomName, width: usize) -> String {
        let (spans, used) = fit_name(name, PLAIN, width);
        let drawn = text(&spans);
        assert_eq!(used, text_width(&drawn), "{drawn:?}");
        drawn
    }

    fn summary(name: RoomName, unread: UnreadCounts, mode: NotificationMode) -> RoomSummary {
        RoomSummary {
            id: RoomId::new("!room:example.org"),
            name,
            kind: RoomKind::Group,
            unread,
            mode,
            latest_activity: None,
            last_read: None,
        }
    }

    fn mentions(mentions: u64) -> UnreadCounts {
        UnreadCounts {
            mentions,
            ..UnreadCounts::default()
        }
    }

    fn row_text(room: &RoomSummary, width: usize, network: NetworkPlacement) -> String {
        let line = room_row(room, false, width, network);
        let drawn = text(&line.spans);
        assert_eq!(text_width(&drawn), width, "{drawn:?}");
        drawn
    }

    #[test]
    fn emoji_with_vs16_takes_two_columns() {
        // U+2764 U+FE0F: one column per char, two as a cluster.
        assert_eq!(fit_plain("❤️ab", 4), ("❤️ab".to_owned(), 4));
        assert_eq!(fit_plain("❤️abc", 4), ("❤️a…".to_owned(), 4));
    }

    #[test]
    fn zwj_sequence_is_one_cluster() {
        // Six columns per char, two as a cluster.
        let name = format!("a{FAMILY}b");
        assert_eq!(fit_plain(&name, 4), (name.clone(), 4));
        // The cut falls between clusters, never inside the sequence.
        let name = format!("a{FAMILY}{FAMILY}");
        assert_eq!(fit_plain(&name, 4), (format!("a{FAMILY}…"), 4));
    }

    #[test]
    fn wide_character_at_the_cut_is_left_out_whole() {
        assert_eq!(fit_plain("日本語", 4), ("日…".to_owned(), 3));
        assert_eq!(fit_plain("日本語", 1), ("…".to_owned(), 1));
    }

    #[test]
    fn width_zero_draws_nothing() {
        assert_eq!(fit_plain("abc", 0), (String::new(), 0));
        assert_eq!(fit_plain("", 0), (String::new(), 0));
    }

    #[test]
    fn control_characters_take_no_column() {
        assert_eq!(fit_plain("a\u{7}b", 2), ("a\u{7}b".to_owned(), 2));
    }

    #[test]
    fn bridged_name_shortens_the_network_part_first() {
        let bridged = name(Some("Discord"), "plain-room");
        assert_eq!(fit_name_text(&bridged, 20), "Discord / plain-room");
        assert_eq!(fit_name_text(&bridged, 18), "Disc… / plain-room");
        assert_eq!(fit_name_text(&bridged, 14), "… / plain-room");
        assert_eq!(fit_name_text(&bridged, 13), "… / plain-ro…");
        assert_eq!(fit_name_text(&bridged, 10), "… / plain…");
        // `… / ` would leave the room part fewer than six columns.
        assert_eq!(fit_name_text(&bridged, 9), "plain-ro…");
        assert_eq!(fit_name_text(&name(None, "plain-room"), 5), "plai…");
    }

    #[test]
    fn narrow_row_drops_the_label_before_the_name() {
        let room = summary(
            name(Some("Discord"), "plain-room"),
            mentions(3),
            NotificationMode::Mute,
        );
        assert_eq!(
            row_text(&room, 40, NetworkPlacement::Inline),
            "Discord / plain-room            @3 Muted"
        );
        assert_eq!(
            row_text(&room, 14, NetworkPlacement::Inline),
            "… / plain-… @3"
        );
        // A short name keeps the label while it fits whole.
        let short = summary(name(None, "ops"), mentions(3), NotificationMode::Mute);
        assert_eq!(
            row_text(&short, 13, NetworkPlacement::Inline),
            "ops  @3 Muted"
        );
    }

    #[test]
    fn below_the_label_depends_on_the_room_part_alone() {
        let room = summary(
            name(Some("Zulip"), "x"),
            UnreadCounts::default(),
            NotificationMode::Mute,
        );
        // Inline the label would leave `Zulip / x` four columns.
        assert_eq!(row_text(&room, 10, NetworkPlacement::Inline), "Zulip / x ");
        assert_eq!(row_text(&room, 10, NetworkPlacement::Below), "x    Muted");
    }

    #[test]
    fn right_part_never_exceeds_the_row() {
        let room = summary(
            name(None, "plain-room"),
            mentions(100),
            NotificationMode::Mute,
        );
        assert_eq!(row_text(&room, 4, NetworkPlacement::Inline), "@99+");
        assert_eq!(row_text(&room, 3, NetworkPlacement::Inline), "@9…");
        assert_eq!(row_text(&room, 0, NetworkPlacement::Inline), "");
    }

    /// The columns of a 30-column room list's rows: the border and the
    /// padding take two.
    const LIST_COLUMNS: u16 = 28;

    fn layout(width: u16, network: NetworkPlacement) -> RoomListLayout {
        RoomListLayout {
            width: RoomListWidth::new(width.into()).unwrap(),
            network,
        }
    }

    /// Listed in this order: a Discord DM, a muted Discord room with
    /// mentions, a room without a network, and a room on a network with a
    /// long name.
    fn bridged() -> App {
        let none = UnreadCounts::default;
        app_with(vec![
            RoomSummary {
                kind: RoomKind::Direct,
                ..summary(
                    name(Some("Discord"), "Alice"),
                    none(),
                    NotificationMode::Default,
                )
            },
            summary(
                name(Some("Discord"), "general"),
                mentions(3),
                NotificationMode::Mute,
            ),
            summary(name(None, "ops"), none(), NotificationMode::Default),
            summary(
                name(Some("Zulip workspace with a long name"), "x"),
                none(),
                NotificationMode::Default,
            ),
        ])
    }

    #[test]
    fn the_room_list_takes_the_set_width_up_to_half() {
        let app = app();
        // The terminal's width, the setting, and the list's width.
        for (terminal, setting, list) in [
            (100, 40, 40),
            (100, 12, 12),
            (100, 80, 50),
            (90, 60, 45),
            // Narrower than 90 columns: a third, whatever the setting.
            (89, 12, 29),
            (80, 80, 26),
        ] {
            let mut screen = Screen::sized(terminal, layout(setting, NetworkPlacement::Inline));
            screen.draw(&app, None);
            assert_eq!(screen.border(), Some(list - 1), "{terminal} {setting}");
        }
    }

    #[test]
    fn the_network_part_goes_inline_or_on_the_row_below() {
        let app = bridged();
        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Inline));
        screen.draw(&app, None);
        assert_eq!(
            screen.list_rows(5),
            [
                "Discord / Alice",
                "Discord / general   @3 Muted",
                "ops",
                "Zulip workspace with a … / x",
                "",
            ]
        );

        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Below));
        screen.draw(&app, None);
        assert_eq!(
            screen.list_rows(8),
            [
                "Alice",
                "  Discord",
                "general             @3 Muted",
                "  Discord",
                "ops",
                "x",
                "  Zulip workspace with a lo…",
                "",
            ]
        );
    }

    #[test]
    fn below_the_open_room_part_is_bold_and_the_network_part_dim() {
        let mut app = bridged();
        press(&mut app, "j<CR>");
        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Below));
        screen.draw(&app, None);
        assert!(screen.cell(0, 2).modifier.contains(Modifier::BOLD));
        let network = screen.cell(2, 3).modifier;
        assert!(network.contains(Modifier::DIM) && !network.contains(Modifier::BOLD));
    }

    #[test]
    fn the_cursor_covers_both_rows_of_a_room() {
        let mut app = bridged();
        press(&mut app, "j");
        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Below));
        screen.draw(&app, None);
        // `general`'s rows, and not the rooms' around them.
        for (y, cursor) in [(1, false), (2, true), (3, true), (4, false)] {
            for x in 0..LIST_COLUMNS {
                let reversed = screen.cell(x, y).modifier.contains(Modifier::REVERSED);
                assert_eq!(reversed, cursor, "{x} {y}");
            }
        }
    }

    #[test]
    fn the_room_list_range_counts_rooms_not_rows() {
        // Two rows, then one, and so on: 12 rows for 8 rooms, in a list of
        // 11 rows.
        let rooms = (0..8)
            .map(|n| {
                let network = (n % 2 == 0).then_some("Discord");
                let room = format!("room {n}");
                summary(
                    name(network, &room),
                    UnreadCounts::default(),
                    NotificationMode::Default,
                )
            })
            .collect();
        let mut app = app_with(rooms);
        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Below));
        assert_eq!(screen.draw(&app, None).room_list, Some((0, 6)));
        press(&mut app, "G");
        assert_eq!(screen.draw(&app, None).room_list, Some((1, 7)));

        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Inline));
        assert_eq!(screen.draw(&app, None).room_list, Some((0, 7)));
    }

    #[test]
    fn scrolling_down_past_a_two_row_room_fills_the_last_row() {
        // `room 0` takes two rows and the 11 rooms after it one each, in a
        // list of 11 rows.
        let rooms = (0..12)
            .map(|n| {
                let network = (n == 0).then_some("Discord");
                let room = format!("room {n}");
                summary(
                    name(network, &room),
                    UnreadCounts::default(),
                    NotificationMode::Default,
                )
            })
            .collect();
        let mut app = app_with(rooms);
        let mut screen = Screen::sized(100, layout(30, NetworkPlacement::Below));
        press(&mut app, &"j".repeat(9));
        assert_eq!(screen.draw(&app, None).room_list, Some((0, 9)));
        // `room 10` scrolls `room 0` out, and leaves a row for `room 11`.
        press(&mut app, "j");
        assert_eq!(screen.draw(&app, None).room_list, Some((1, 11)));
        let rows = screen.list_rows(11);
        assert_eq!([rows[0].as_str(), &rows[10]], ["room 1", "room 11"]);
    }

    /// The thread of `!a` whose root is `$<root>`.
    fn thread_of_a(root: u32) -> TimelineId {
        TimelineId {
            room: RoomId::new("!a:example.org"),
            thread: Some(EventId::new(format!("${root}:example.org"))),
        }
    }

    /// `message(n)`, sent by Bob.
    fn from_bob(n: u32) -> TimelineEntry {
        let mut entry = message(n);
        if let TimelineEntry::Item(item) = &mut entry {
            item.sender = UserId::new("@bob:example.org");
            item.sender_name = "Bob".to_owned();
        }
        entry
    }

    /// Every row as drawn, without trailing spaces.
    fn screen_rows(screen: &Screen) -> Vec<String> {
        let height = screen.terminal.backend().buffer().area.height;
        (0..height)
            .map(|y| screen.row(y).trim_end().to_owned())
            .collect()
    }

    #[test]
    fn a_thread_shows_its_title_row_under_the_rooms_name() {
        let mut app = app();
        press(&mut app, "<CR>");
        app.update(reset("!a:example.org", 1..4));
        press(&mut app, "kt");
        let mut screen = Screen::new();
        screen.draw(&app, None);
        let rows = screen_rows(&screen);
        assert!(rows[0].ends_with("!a:example.org"), "{rows:#?}");
        assert!(
            rows[1].ends_with("  Thread · Alice: message 2"),
            "{rows:#?}"
        );
        let title = rows[1].find("Thread").unwrap();
        let style = |offset: usize| {
            let x = u16::try_from(rows[1][..title + offset].chars().count()).unwrap();
            screen.cell(x, 1).modifier
        };
        assert!(style(0).contains(Modifier::DIM));
        assert!(style("Thread · ".len()).contains(Modifier::BOLD));

        // Cut with `…` on a narrow pane.
        press(&mut app, "<Space>e");
        let mut narrow = Screen::sized(20, RoomListLayout::default());
        narrow.draw(&app, None);
        assert_eq!(screen_rows(&narrow)[1].trim(), "Thread · Alice: …");

        // Neither the thread nor the timeline holds the root.
        app.update(reset("!a:example.org", 3..4));
        app.update(Msg::Timeline {
            timeline: thread_of_a(2),
            diffs: vec![TimelineDiff::Reset(vec![message(10)])],
        });
        narrow.draw(&app, None);
        assert_eq!(screen_rows(&narrow)[1].trim(), "Thread");
    }

    #[test]
    fn a_threads_root_shows_its_row_without_the_unread_mark() {
        let mut app = app();
        press(&mut app, "<CR>");
        let mut root = message(1);
        if let TimelineEntry::Item(item) = &mut root {
            item.timestamp = Timestamp::from_millis(1_000);
            item.thread_summary = Some(mxt_core::timeline::ThreadSummary {
                replies: std::num::NonZeroU32::new(1).unwrap(),
                latest: Some(Timestamp::from_millis(5_000)),
            });
        }
        let mut newest = message(2);
        if let TimelineEntry::Item(item) = &mut newest {
            item.timestamp = Timestamp::from_millis(2_000);
        }
        app.update(Msg::Timeline {
            timeline: TimelineId::main(RoomId::new("!a:example.org")),
            diffs: vec![TimelineDiff::Reset(vec![root.clone(), newest])],
        });
        let mut screen = Screen::new();
        let (first, last) = screen.draw(&app, None).timeline.unwrap();
        // The receipt goes on message 2; the SDK counts the reply after it
        // again.
        app.update(Msg::Viewport {
            pane: mxt_core::msg::Pane::Timeline,
            first,
            last,
        });
        app.update(Msg::RoomUpserted(RoomSummary {
            id: RoomId::new("!a:example.org"),
            name: name(None, "!a:example.org"),
            kind: RoomKind::Group,
            unread: UnreadCounts {
                messages: 1,
                ..UnreadCounts::default()
            },
            mode: NotificationMode::Default,
            latest_activity: Some(Timestamp::from_millis(5_000)),
            last_read: None,
        }));
        let summary_row = |screen: &Screen| {
            let rows = screen_rows(screen);
            rows.into_iter().find(|row| row.contains("💬 1")).unwrap()
        };
        screen.draw(&app, None);
        assert!(summary_row(&screen).ends_with(" · unread"));

        press(&mut app, "kt");
        app.update(Msg::Timeline {
            timeline: thread_of_a(1),
            diffs: vec![TimelineDiff::Reset(vec![root])],
        });
        screen.draw(&app, None);
        assert!(!summary_row(&screen).contains("unread"));
    }

    #[test]
    fn a_thread_shows_its_own_composer_draft() {
        let mut app = app();
        press(&mut app, "<CR>");
        app.update(reset("!a:example.org", 1..4));
        let mut screen = Screen::new();
        // A reply to message 2 in the room's composer, then back to the
        // timeline.
        press(&mut app, "kr<C-w>k");
        screen.draw(&app, None);
        assert!(screen.composer_rule().contains("Reply to Alice"));

        press(&mut app, "t");
        app.update(Msg::Timeline {
            timeline: thread_of_a(2),
            diffs: vec![TimelineDiff::Reset(vec![message(2), from_bob(10)])],
        });
        screen.draw(&app, None);
        assert!(!screen.composer_rule().contains("Reply"));
        press(&mut app, "r<C-w>k");
        screen.draw(&app, None);
        assert!(screen.composer_rule().contains("Reply to Bob"));

        press(&mut app, "q");
        screen.draw(&app, None);
        assert!(screen.composer_rule().contains("Reply to Alice"));
    }
}
