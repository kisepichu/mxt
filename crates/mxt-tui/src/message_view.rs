//! The full-screen message view (design, "Terminal adapter and the
//! composer"): an edtui scratch editor in vim mode holding the message's
//! text. Its edits are dropped with it.

use crossterm::event::{KeyCode as C, KeyModifiers as M};
use edtui::events::KeyInput;
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView, Lines};
use mxt_core::keys::{Key, KeyCode};
use mxt_core::msg::EditorMode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Clear, Padding};

use crate::clipboard::SharedRegister;

pub(crate) struct MessageView {
    state: EditorState,
    keys: EditorEventHandler,
}

impl MessageView {
    pub(crate) fn new(text: &str, register: SharedRegister) -> MessageView {
        let mut state = EditorState::new(Lines::from(text));
        state.set_clipboard(register);
        MessageView {
            state,
            keys: EditorEventHandler::vim_mode(),
        }
    }

    pub(crate) fn mode(&self) -> EditorMode {
        match self.state.mode {
            edtui::EditorMode::Normal => EditorMode::Normal,
            edtui::EditorMode::Visual => EditorMode::Visual,
            edtui::EditorMode::Insert => EditorMode::Insert,
            edtui::EditorMode::Search => EditorMode::Search,
        }
    }

    /// The keys the core passed on, in order. Keys edtui has no code for
    /// are left out.
    pub(crate) fn replay(&mut self, keys: &[Key]) {
        for &key in keys {
            if let Some(input) = key_input(key, self.state.mode) {
                self.keys.on_key_event(input, &mut self.state);
            }
        }
    }

    /// The search pattern while the editor is in search mode.
    pub(crate) fn search(&self) -> Option<String> {
        (self.state.mode == edtui::EditorMode::Search).then(|| self.state.search_pattern())
    }

    /// Draws the editor over `area`. Outside search mode the terminal cursor
    /// goes to the editor's cursor, where an input method puts its preedit.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        // The terminal cursor marks the cursor; the selection and search
        // matches are reversed.
        let theme = EditorTheme::default()
            .base(Style::new())
            .hide_cursor()
            .selection_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(Block::new().padding(Padding::horizontal(1)))
            .hide_status_line();
        frame.render_widget(
            EditorView::new(&mut self.state).theme(theme).wrap(true),
            area,
        );
        if self.state.mode != edtui::EditorMode::Search
            && let Some(position) = self.state.cursor_screen_position()
        {
            frame.set_cursor_position(position);
        }
    }
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
mod tests {
    use super::*;
    use edtui::EditorMode as Mode;
    use mxt_core::keys::KeySeq;

    fn keys(text: &str) -> Vec<Key> {
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
    fn replayed_keys_move_search_and_yank() {
        let register = SharedRegister::default();
        let mut view = MessageView::new("first line\nSecond line", register.clone());
        view.replay(&keys("G"));
        assert_eq!(view.state.cursor.row, 1);
        view.replay(&keys("gg/Sec"));
        assert_eq!(view.mode(), EditorMode::Search);
        assert_eq!(view.search().as_deref(), Some("Sec"));
        view.replay(&keys("<CR>yy"));
        assert_eq!(view.mode(), EditorMode::Normal);
        assert!(register.take_unsent().is_some());
    }
}
