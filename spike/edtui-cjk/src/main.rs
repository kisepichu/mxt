//! Throwaway phase 0 check: does edtui place the cursor correctly with wide
//! (CJK) characters and committed input method text?
//!
//! `edtui-cjk --out PATH` runs an edtui editor in vim mode on the whole
//! screen except the last line, which shows edtui's mode, the cursor and the
//! display column the cursor should have. The terminal cursor is put where
//! edtui says the cursor is, as `mxt-tui` will do for the composer (the input
//! method draws its preedit there).
//!
//! After every frame the program sets the terminal title to
//! `edtui-cjk:<keys>` and then writes the editor state as JSON to `PATH`, so
//! `check.sh` can wait for a frame instead of sleeping. `<C-q>` quits.

use std::io;
use std::path::{Path, PathBuf};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::SetTitle;
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView, RowIndex, system_editor};
use ratatui::DefaultTerminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthChar;

fn main() -> io::Result<()> {
    let out = parse_args()?;
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &out);
    ratatui::restore();
    result
}

fn parse_args() -> io::Result<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--out" => Ok(PathBuf::from(path)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: edtui-cjk --out PATH",
        )),
    }
}

fn run(terminal: &mut DefaultTerminal, out: &Path) -> io::Result<()> {
    let mut state = EditorState::default();
    let mut handler = EditorEventHandler::vim_mode();
    // Number of key events handled so far; `check.sh` waits for it.
    let mut keys: u64 = 0;

    // Start from a blank screen even if a previous program left cells behind.
    terminal.clear()?;
    loop {
        // The symbol ratatui drew where the cursor is, to compare with the
        // character the cursor is on.
        let mut cell = None;
        terminal.draw(|frame| {
            let [editor, status] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
            frame.render_widget(
                EditorView::new(&mut state)
                    .theme(EditorTheme::default().hide_status_line())
                    .wrap(true),
                editor,
            );
            // edtui only styles the cursor cell; the terminal cursor is ours.
            if let Some(position) = state.cursor_screen_position() {
                frame.set_cursor_position(position);
                cell = frame
                    .buffer_mut()
                    .cell(position)
                    .map(|c| c.symbol().to_owned());
            }
            frame.render_widget(Paragraph::new(status_line(&state, keys)), status);
        })?;
        // The title comes after the frame's cursor movement in the output, so
        // once tmux shows it, tmux has processed the whole frame.
        crossterm::execute!(io::stdout(), SetTitle(format!("edtui-cjk:{keys}")))?;
        write_state(out, &state, keys, cell.as_deref())?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(());
            }
            handler.on_key_event(key, &mut state);
            keys += 1;
            if system_editor::is_pending(&state) {
                system_editor::open(&mut state, terminal)?;
            }
        }
    }
}

/// The sum of the `unicode-width` widths of the characters before the cursor
/// on its line: where the cursor must be drawn when the line does not wrap.
fn display_col(state: &EditorState) -> usize {
    state
        .lines
        .get(RowIndex::new(state.cursor.row))
        .map_or(0, |line| {
            line.iter()
                .take(state.cursor.col)
                .map(|c| c.width().unwrap_or(0))
                .sum()
        })
}

fn status_line(state: &EditorState, keys: u64) -> String {
    format!(
        "{} ({}, {}) display {} | keys {keys}",
        state.mode.name().to_uppercase(),
        state.cursor.row,
        state.cursor.col,
        display_col(state),
    )
}

fn write_state(out: &Path, state: &EditorState, keys: u64, cell: Option<&str>) -> io::Result<()> {
    let lines: Vec<String> = state
        .lines
        .iter_row()
        .map(|line| line.iter().collect())
        .collect();
    let screen = state.cursor_screen_position().map(|p| [p.x, p.y]);
    let char_under_cursor = state
        .lines
        .get(RowIndex::new(state.cursor.row))
        .and_then(|line| line.get(state.cursor.col));
    let value = serde_json::json!({
        "mode": state.mode.name(),
        "lines": lines,
        "row": state.cursor.row,
        "col": state.cursor.col,
        "display_col": display_col(state),
        "screen": screen,
        "char": char_under_cursor,
        "cell": cell,
        "keys": keys,
    });
    // Write and rename, so a reader never sees a half-written file.
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, value.to_string())?;
    std::fs::rename(&tmp, out)
}
