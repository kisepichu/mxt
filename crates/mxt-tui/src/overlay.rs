//! Overlays over the panes (design, "Layout", "Mention completion"): the
//! mention menu above the composer and the emoji picker.

use mxt_core::app::{Picker, PickerKind};
use mxt_core::mention::Member;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear};

use crate::view::{DIM, PLAIN, draw_prompt_line, fit};

/// The most candidates the mention menu shows at once.
const MENU_ROWS: u16 = 8;
const MENU_WIDTH: u16 = 50;
const PICKER_WIDTH: u16 = 48;
const PICKER_HEIGHT: u16 = 20;
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);
/// Between a member's display name and user ID.
const GAP: &str = "  ";

/// The lists' scroll offsets between frames.
#[derive(Default)]
pub(crate) struct Overlays {
    menu: usize,
    picker: usize,
}

impl Overlays {
    /// The candidates in a box whose bottom border sits on the row above
    /// `composer`, its text in line with the composer's, inside `bounds`.
    pub(crate) fn draw_mention_menu(
        &mut self,
        frame: &mut Frame,
        composer: Rect,
        bounds: Rect,
        members: &[Member],
        selected: usize,
    ) {
        let above = composer.y.saturating_sub(bounds.y);
        let len = u16::try_from(members.len()).unwrap_or(u16::MAX);
        let rows = len.min(MENU_ROWS).min(above.saturating_sub(2));
        if rows == 0 {
            return;
        }
        let area = Rect {
            x: composer.x + 1,
            y: composer.y - rows - 2,
            width: composer.width.saturating_sub(2).min(MENU_WIDTH),
            height: rows + 2,
        };
        let block = Block::bordered().border_style(DIM);
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        let rows = usize::from(rows);
        self.menu = scroll(self.menu, selected, members.len(), rows);
        let width = usize::from(inner.width);
        for (y, (index, member)) in
            (inner.y..).zip(members.iter().enumerate().skip(self.menu).take(rows))
        {
            let user = member.user.as_str();
            let segments = match member
                .display_name
                .as_deref()
                .filter(|name| !name.is_empty())
            {
                Some(name) => vec![(name, PLAIN), (GAP, PLAIN), (user, DIM)],
                None => vec![(user, PLAIN)],
            };
            let line = Line::from(fit(&segments, width).0);
            let line = if index == selected {
                line.style(SELECTED)
            } else {
                line
            };
            frame.render_widget(
                line,
                Rect {
                    y,
                    height: 1,
                    ..inner
                },
            );
        }
    }

    /// A centered box over `bounds`: the query line with the terminal
    /// cursor, then the items.
    pub(crate) fn draw_picker(&mut self, frame: &mut Frame, bounds: Rect, picker: &Picker) {
        let area = bounds.centered(
            Constraint::Length(bounds.width.min(PICKER_WIDTH)),
            Constraint::Length(bounds.height.min(PICKER_HEIGHT)),
        );
        let title = match picker.kind {
            PickerKind::Emoji { .. } => " Reaction ",
            PickerKind::Rooms { .. } => " Room ",
            PickerKind::Links => " Link ",
        };
        let block = Block::bordered().border_style(DIM).title(title);
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        let [query, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        if query.height == 0 {
            return;
        }
        draw_prompt_line(frame, query, "> ", &picker.query);
        if picker.items.is_empty() {
            frame.render_widget(Line::styled(picker.empty_text(), DIM), list);
            return;
        }
        let rows = usize::from(list.height);
        self.picker = scroll(self.picker, picker.selected, picker.items.len(), rows);
        let width = usize::from(list.width);
        for (y, (index, item)) in
            (list.y..).zip(picker.items.iter().enumerate().skip(self.picker).take(rows))
        {
            let line = Line::from(fit(&[(item.label.as_str(), PLAIN)], width).0);
            let line = if index == picker.selected {
                line.style(SELECTED)
            } else {
                line
            };
            frame.render_widget(
                line,
                Rect {
                    y,
                    height: 1,
                    ..list
                },
            );
        }
    }
}

/// The first of `len` items shown in `rows` rows: `offset`, moved as little
/// as keeps `selected` visible.
fn scroll(offset: usize, selected: usize, len: usize, rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    let offset = offset.min(len.saturating_sub(rows));
    if selected < offset {
        selected
    } else if selected >= offset + rows {
        selected + 1 - rows
    } else {
        offset
    }
}
