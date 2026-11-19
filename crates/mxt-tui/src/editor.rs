//! An edtui editor in vim mode with the shared register: the composers and
//! the message view's scratch editor (design, "Terminal adapter and the
//! composer").

use crossterm::event::{KeyCode as C, KeyModifiers as M};
use edtui::events::{KeyEventRegister, KeyInput};
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView, Lines};
use mxt_core::keys::{Key, KeyCode};
use mxt_core::msg::EditorMode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;

use crate::clipboard::SharedRegister;
use crate::view::grapheme_width;

/// The columns edtui draws a tab with (its default).
const TAB_WIDTH: usize = 2;

pub(crate) struct Editor {
    pub(crate) state: EditorState,
    keys: EditorEventHandler,
}

impl Editor {
    /// In normal mode with the cursor at the start.
    pub(crate) fn new(text: &str, register: SharedRegister) -> Editor {
        let mut state = EditorState::new(Lines::from(text));
        state.set_clipboard(register);
        let mut keys = EditorEventHandler::vim_mode();
        // edtui's `<C-e>` in normal mode asks for the external editor, which
        // only the keymap's `composer.external_editor` opens.
        keys.key_handler
            .remove(&KeyEventRegister::n(vec![KeyInput::ctrl('e')]));
        Editor { state, keys }
    }

    pub(crate) fn mode(&self) -> EditorMode {
        match self.state.mode {
            edtui::EditorMode::Normal => EditorMode::Normal,
            edtui::EditorMode::Visual => EditorMode::Visual,
            edtui::EditorMode::Insert => EditorMode::Insert,
            edtui::EditorMode::Search => EditorMode::Search,
        }
    }

    /// A key the core passed on. Keys edtui has no code for are left out.
    pub(crate) fn feed(&mut self, key: Key) {
        if let Some(input) = key_input(key, self.state.mode) {
            self.keys.on_key_event(input, &mut self.state);
        }
    }

    /// The search pattern while the editor is in search mode.
    pub(crate) fn search(&self) -> Option<String> {
        (self.state.mode == edtui::EditorMode::Search).then(|| self.state.search_pattern())
    }

    /// The rows the text takes at `width` columns, at least one. Like
    /// edtui's wrapping, a character that does not fit on a row moves to the
    /// next one whole, and an empty line takes a row. In insert mode the
    /// cursor after the end of its line takes a column too: edtui draws it
    /// on the next row when the line's last row is full.
    pub(crate) fn rows(&self, width: usize) -> usize {
        let state = &self.state;
        let insert = state.mode == edtui::EditorMode::Insert;
        let rows: usize = state
            .lines
            .iter_row()
            .enumerate()
            .map(|(row, line)| {
                let cursor_after =
                    insert && row == state.cursor.row && state.cursor.col >= line.len();
                wrapped_rows(line, width, cursor_after)
            })
            .sum();
        rows.max(1)
    }

    /// Draws the editor in `block` over `area`, wrapping its lines. With
    /// `cursor`, the terminal cursor goes to the editor's cursor outside
    /// search mode, where an input method puts its preedit; the bottom line
    /// has it in search mode.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, block: Block, cursor: bool) {
        // The terminal cursor marks the cursor; the selection and search
        // matches are reversed.
        let theme = EditorTheme::default()
            .base(Style::new())
            .hide_cursor()
            .selection_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(block)
            .hide_status_line();
        frame.render_widget(
            EditorView::new(&mut self.state).theme(theme).wrap(true),
            area,
        );
        if cursor
            && self.state.mode != edtui::EditorMode::Search
            && let Some(position) = self.state.cursor_screen_position()
        {
            frame.set_cursor_position(position);
        }
    }
}

/// The rows `line` takes at `width` columns; with `cursor_after`, the cursor
/// after its end takes a column as well.
fn wrapped_rows(line: &[char], width: usize, cursor_after: bool) -> usize {
    let columns = line.iter().map(|&c| match c {
        '\t' => TAB_WIDTH,
        c => grapheme_width(c.encode_utf8(&mut [0; 4])),
    });
    let mut rows = 1;
    let mut used = 0;
    for columns in columns.chain(cursor_after.then_some(1)) {
        if used > 0 && used + columns > width {
            rows += 1;
            used = 0;
        }
        used += columns;
    }
    rows
}

/// A core key as edtui's input in `mode`.
///
/// The core folds shift into characters, while edtui binds capital letters
/// with shift (`G`); in search mode edtui only takes characters without
/// modifiers, so there capitals stay without shift.
fn key_input(key: Key, mode: edtui::EditorMode) -> Option<KeyInput> {
    let mods = key.mods();
    let mut shift = mods.shift;
    // Only codes edtui converts: its conversion panics on the others.
    let code = match key.code() {
        KeyCode::Char(c) => {
            shift |= c.is_uppercase() && mode != edtui::EditorMode::Search;
            C::Char(c)
        }
        KeyCode::Enter => C::Enter,
        KeyCode::Esc => C::Esc,
        KeyCode::Tab => C::Tab,
        KeyCode::Backspace => C::Backspace,
        KeyCode::Delete => C::Delete,
        KeyCode::Up => C::Up,
        KeyCode::Down => C::Down,
        KeyCode::Left => C::Left,
        KeyCode::Right => C::Right,
        KeyCode::Home => C::Home,
        KeyCode::End => C::End,
        KeyCode::PageUp => C::PageUp,
        KeyCode::PageDown => C::PageDown,
        KeyCode::Insert | KeyCode::F(_) => return None,
    };
    let mut modifiers = M::NONE;
    modifiers.set(M::CONTROL, mods.ctrl);
    modifiers.set(M::ALT, mods.alt);
    modifiers.set(M::SHIFT, shift);
    Some(KeyInput::with_modifiers(code, modifiers))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use edtui::EditorMode as Mode;
    use mxt_core::keys::KeySeq;

    pub(crate) fn keys(text: &str) -> Vec<Key> {
        KeySeq::parse(text, &KeySeq::default())
            .unwrap()
            .keys()
            .to_vec()
    }

    fn input(text: &str, mode: Mode) -> Option<KeyInput> {
        key_input(keys(text)[0], mode)
    }

    #[test]
    fn capitals_get_shift_except_in_search() {
        assert_eq!(input("G", Mode::Normal), Some(KeyInput::shift('G')));
        assert_eq!(input("G", Mode::Search), Some(KeyInput::new('G')));
        assert_eq!(input("g", Mode::Normal), Some(KeyInput::new('g')));
        // edtui binds symbols without shift.
        assert_eq!(input("$", Mode::Normal), Some(KeyInput::new('$')));
    }

    #[test]
    fn modifiers_and_special_keys() {
        assert_eq!(input("<C-r>", Mode::Normal), Some(KeyInput::ctrl('r')));
        assert_eq!(input("<M-x>", Mode::Normal), Some(KeyInput::alt('x')));
        assert_eq!(input("<Esc>", Mode::Insert), Some(KeyInput::new(C::Esc)));
        assert_eq!(
            input("<S-Tab>", Mode::Insert),
            Some(KeyInput::shift(C::Tab))
        );
        assert_eq!(input("<F1>", Mode::Normal), None);
        assert_eq!(input("<Insert>", Mode::Normal), None);
    }

    #[test]
    fn fed_keys_move_search_and_yank() {
        let register = SharedRegister::default();
        let mut editor = Editor::new("first line\nSecond line", register.clone());
        let feed =
            |editor: &mut Editor, text| keys(text).into_iter().for_each(|key| editor.feed(key));
        feed(&mut editor, "G");
        assert_eq!(editor.state.cursor.row, 1);
        feed(&mut editor, "gg/Sec");
        assert_eq!(editor.mode(), EditorMode::Search);
        assert_eq!(editor.search().as_deref(), Some("Sec"));
        feed(&mut editor, "<CR>yy");
        assert_eq!(editor.mode(), EditorMode::Normal);
        assert!(register.take_unsent().is_some());
    }
}
