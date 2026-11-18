//! Layout and drawing, from the core state (design, "Layout", "Room names",
//! "Unread badges"). Colors come only from the 16-color palette.

use mxt_core::app::{App, CommandLine, Focus, LineKind, StatusKind};
use mxt_core::msg::SyncState;
use mxt_core::room::{Badge, RoomName, RoomSummary};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The room list width, its border included.
const ROOM_LIST_WIDTH: u16 = 30;
/// Terminals narrower than this give the room list a third of their width.
const NARROW_WIDTH: u16 = 90;

const PLAIN: Style = Style::new();
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const ERROR: Style = Style::new().fg(Color::Red);

/// Between the groups of the status line.
const GAP: &str = "  ";
const ELLIPSIS: &str = "…";

/// Draws the frame. Returns the first and last visible room list indices,
/// inclusive, when the room list shows any room.
pub(crate) fn draw(
    frame: &mut Frame,
    app: &App,
    room_list: &mut ListState,
) -> Option<(usize, usize)> {
    let [main, bottom] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let mut visible = None;
    let timeline = if app.sidebar_visible() {
        let width = if main.width < NARROW_WIDTH {
            main.width / 3
        } else {
            ROOM_LIST_WIDTH
        };
        let [list, timeline] =
            Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(main);
        visible = draw_room_list(frame, list, app, room_list);
        timeline
    } else {
        main
    };
    draw_timeline(frame, timeline, app);
    match app.command_line() {
        Some(line) => draw_command_line(frame, bottom, line),
        None => draw_status_line(frame, bottom, app),
    }
    visible
}

/// The list scrolls so the cursor stays visible.
fn draw_room_list(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &mut ListState,
) -> Option<(usize, usize)> {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .padding(Padding::right(1));
    let inner = block.inner(area);
    let width = usize::from(inner.width);
    let open = app.open_room();
    let rows: Vec<ListItem> = app
        .rooms()
        .rooms()
        .iter()
        .map(|room| ListItem::new(room_row(room, open == Some(&room.id), width)))
        .collect();
    let len = rows.len();
    let cursor = match app.focus() {
        Focus::RoomList => Modifier::REVERSED,
        Focus::Timeline | Focus::Composer => Modifier::UNDERLINED,
    };
    let list = List::new(rows).block(block).highlight_style(cursor);
    state.select(app.room_cursor());
    frame.render_stateful_widget(list, area, state);

    let height = usize::from(inner.height);
    (len > 0 && height > 0).then(|| {
        let first = state.offset().min(len - 1);
        (first, (first + height).min(len) - 1)
    })
}

/// The displayed name, cut to fit, and right-aligned the badge and the
/// notification mode label.
fn room_row(room: &RoomSummary, is_open: bool, width: usize) -> Line<'_> {
    let badge = Badge::from_counts(&room.unread);
    let label = room.mode.label();
    let mut right = Vec::with_capacity(3);
    if let Some(badge) = badge {
        right.push(Span::styled(badge.text(), badge_style(badge)));
    }
    if let Some(label) = label {
        if badge.is_some() {
            right.push(Span::raw(" "));
        }
        right.push(Span::styled(label, DIM));
    }
    let right_width: usize = right.iter().map(Span::width).sum();
    let gap = usize::from(right_width > 0);

    let room_style = if is_open { BOLD } else { PLAIN };
    let segments = name_segments(&room.name, room_style);
    let (mut spans, used) = fit(&segments, width.saturating_sub(right_width + gap));
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + right_width)),
    ));
    spans.extend(right);
    Line::from(spans)
}

fn badge_style(badge: Badge) -> Style {
    match badge {
        Badge::Mentions(_) => BOLD.fg(Color::Red),
        Badge::Notifications(_) => BOLD,
        Badge::Messages(_) => DIM,
        Badge::Marked => Style::new().fg(Color::Yellow),
    }
}

/// `network / room`, with the network part dim, so a room whose own name is
/// `Discord / Boss` can be told apart from a bridged room.
fn name_segments(name: &RoomName, room_style: Style) -> [(&str, Style); 3] {
    let (network, separator) = match &name.network {
        Some(network) => (network.as_str(), " / "),
        None => ("", ""),
    };
    [(network, DIM), (separator, DIM), (&name.room, room_style)]
}

/// Until phase 2: the open room's displayed name as a header.
fn draw_timeline(frame: &mut Frame, area: Rect, app: &App) {
    let inner = Block::new().padding(Padding::horizontal(1)).inner(area);
    let header = Rect {
        height: inner.height.min(1),
        ..inner
    };
    let line = match app.open_room().and_then(|id| app.rooms().get(id)) {
        Some(room) => {
            let segments = name_segments(&room.name, BOLD);
            Line::from(fit(&segments, usize::from(header.width)).0)
        }
        None => Line::styled("No room open", DIM),
    };
    frame.render_widget(line, header);
}

/// The mode, the pending count and keys, the open room, the status message,
/// and right-aligned the sync state. When they do not fit, the room name is
/// cut first, so the status message stays readable.
fn draw_status_line(frame: &mut Frame, area: Rect, app: &App) {
    let mode = format!(" {} ", app.mode_label(None));
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
    let status = app.status().map(|status| {
        let style = match status.kind {
            StatusKind::Info => PLAIN,
            StatusKind::Error => ERROR,
        };
        [(GAP, PLAIN), (status.text.as_str(), style)]
    });

    let sync = sync_label(app.sync_state());
    let width = usize::from(area.width);
    // One column stays between the left part and the sync state.
    let available = width.saturating_sub(sync.width() + 1);
    let (mut spans, mut used) = fit(&head, available);
    if let Some(room) = app.open_room().and_then(|id| app.rooms().get(id)) {
        let [network, separator, name] = name_segments(&room.name, PLAIN);
        let status_width = status.as_ref().map_or(0, |status| segments_width(status));
        let room_width = available.saturating_sub(used + status_width);
        if room_width > GAP.len() {
            let (room, room_used) = fit(&[(GAP, PLAIN), network, separator, name], room_width);
            spans.extend(room);
            used += room_used;
        }
    }
    if let Some(status) = &status {
        let (status, status_used) = fit(status, available.saturating_sub(used));
        spans.extend(status);
        used += status_used;
    }
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + sync.width())),
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

/// `:` or `/` and the text, with the terminal cursor after the text. A text
/// wider than the line loses its start, so its end and the cursor stay
/// visible.
fn draw_command_line(frame: &mut Frame, area: Rect, line: &CommandLine) {
    let prompt = match line.kind {
        LineKind::Command => ":",
        LineKind::Search(_) => "/",
    };
    // The prompt and the cursor take a column each.
    let room = usize::from(area.width).saturating_sub(2);
    let mut text = line.text.as_str();
    let mut text_width: usize = text.chars().map(char_width).sum();
    for c in line.text.chars() {
        if text_width <= room {
            break;
        }
        text = &text[c.len_utf8()..];
        text_width -= char_width(c);
    }
    frame.render_widget(Line::from(vec![Span::raw(prompt), Span::raw(text)]), area);
    let column = u16::try_from(1 + text_width).unwrap_or(u16::MAX);
    let x = area.x.saturating_add(column);
    frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
}

/// Spans of the segments cut to `width` display columns, ending with `…` when
/// cut, and the columns they take. Empty segments are left out.
fn fit<'a>(segments: &[(&'a str, Style)], width: usize) -> (Vec<Span<'a>>, usize) {
    let mut spans = Vec::with_capacity(segments.len());
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
    // One column is left for the ellipsis.
    let budget = width.saturating_sub(1);
    let mut used = 0;
    for &(text, style) in segments {
        for (at, c) in text.char_indices() {
            let w = char_width(c);
            if used + w > budget {
                if at > 0 {
                    spans.push(Span::styled(&text[..at], style));
                }
                if width > 0 {
                    spans.push(Span::styled(ELLIPSIS, style));
                    used += 1;
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
    segments
        .iter()
        .flat_map(|(text, _)| text.chars())
        .map(char_width)
        .sum()
}

/// Control characters take no column: ratatui does not draw them.
fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}
