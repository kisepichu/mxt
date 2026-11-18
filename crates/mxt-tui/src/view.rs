//! Layout and drawing, from the core state (design, "Layout", "Room names",
//! "Unread badges"). Colors come only from the 16-color palette.

use mxt_core::app::{App, CommandLine, Focus, LineKind, StatusKind};
use mxt_core::msg::SyncState;
use mxt_core::room::{Badge, RoomName, RoomSummary};
use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding};
use unicode_segmentation::UnicodeSegmentation;

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
/// Between the network part and the room part of a displayed name.
const SEPARATOR: &str = " / ";
/// A name keeps at least this many columns where the layout can make room:
/// a room list row leaves out the mode label first, and a bridged name leaves
/// out its network part, already shortened to `…`.
const MIN_NAME_WIDTH: usize = 6;

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
/// notification mode label. The label is left out before the name would get
/// fewer than `MIN_NAME_WIDTH` columns: the badge is the signal to act on. The
/// right part never takes more than the row.
fn room_row(room: &RoomSummary, is_open: bool, width: usize) -> Line<'_> {
    let mut right = Vec::with_capacity(3);
    let mut right_width = 0;
    if let Some(badge) = Badge::from_counts(&room.unread) {
        let text = badge.text();
        let style = badge_style(badge);
        right_width = text_width(&text);
        if right_width <= width {
            right.push(Span::styled(text, style));
        } else {
            // Only a row narrower than the badge gets here.
            let (spans, used) = fit(&[(text.as_str(), style)], width);
            let owned = spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style));
            right.extend(owned);
            right_width = used;
        }
    }
    if let Some(label) = room.mode.label() {
        let space = usize::from(!right.is_empty());
        let with_label = right_width + space + text_width(label);
        // One column stays between the name and the right part.
        let name_room = width.checked_sub(with_label + 1);
        let wanted = name_width(&room.name).min(MIN_NAME_WIDTH);
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
    let (mut spans, used) = fit_name(&room.name, room_style, name_room);
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

/// Until phase 2: the open room's displayed name as a header.
fn draw_timeline(frame: &mut Frame, area: Rect, app: &App) {
    let inner = Block::new().padding(Padding::horizontal(1)).inner(area);
    let header = Rect {
        height: inner.height.min(1),
        ..inner
    };
    let line = match app.open_room().and_then(|id| app.rooms().get(id)) {
        Some(room) => Line::from(fit_name(&room.name, BOLD, usize::from(header.width)).0),
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
    let mut text_columns = text_width(text);
    for (at, grapheme) in line.text.grapheme_indices(true) {
        if text_columns <= room {
            break;
        }
        text = &line.text[at + grapheme.len()..];
        text_columns -= grapheme_width(grapheme);
    }
    frame.render_widget(Line::from(vec![Span::raw(prompt), Span::raw(text)]), area);
    let column = u16::try_from(1 + text_columns).unwrap_or(u16::MAX);
    let x = area.x.saturating_add(column);
    frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
}

/// Spans of the segments cut to `width` columns at grapheme cluster
/// boundaries, ending with `…` when cut, and the columns they take. Empty
/// segments are left out.
fn fit<'a>(segments: &[(&'a str, Style)], width: usize) -> (Vec<Span<'a>>, usize) {
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
fn text_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// The columns ratatui gives a grapheme cluster. A cluster with a control
/// character takes none: ratatui leaves it out.
fn grapheme_width(grapheme: &str) -> usize {
    if grapheme.contains(char::is_control) {
        0
    } else {
        usize::from(grapheme.cell_width())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxt_core::ids::RoomId;
    use mxt_core::room::{NotificationMode, RoomKind, UnreadCounts};

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
        }
    }

    fn mentions(mentions: u64) -> UnreadCounts {
        UnreadCounts {
            mentions,
            ..UnreadCounts::default()
        }
    }

    fn row_text(room: &RoomSummary, width: usize) -> String {
        let line = room_row(room, false, width);
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
            NotificationMode::MentionsAndKeywords,
        );
        assert_eq!(
            row_text(&room, 40),
            "Discord / plain-room    @3 Mentions only"
        );
        assert_eq!(row_text(&room, 22), "Disco… / plain-room @3");
        // A short name keeps the label while it fits whole.
        let short = summary(name(None, "ops"), mentions(3), NotificationMode::Mute);
        assert_eq!(row_text(&short, 13), "ops  @3 Muted");
    }

    #[test]
    fn right_part_never_exceeds_the_row() {
        let room = summary(
            name(None, "plain-room"),
            mentions(100),
            NotificationMode::Mute,
        );
        assert_eq!(row_text(&room, 4), "@99+");
        assert_eq!(row_text(&room, 3), "@9…");
        assert_eq!(row_text(&room, 0), "");
    }
}
