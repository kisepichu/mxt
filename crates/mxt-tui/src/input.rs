//! Terminal input: crossterm key events to core keys, and pasted text.

use crossterm::event::{KeyCode as C, KeyEvent, KeyEventKind, KeyModifiers as M};
use mxt_core::keys::{Key, KeyCode, Modifiers};

/// crossterm key event to a core key; `None` for releases and keys the core
/// has no notation for.
pub fn key_from_crossterm(event: KeyEvent) -> Option<Key> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let mut mods = Modifiers {
        ctrl: event.modifiers.contains(M::CONTROL),
        alt: event.modifiers.contains(M::ALT),
        shift: event.modifiers.contains(M::SHIFT),
    };
    let code = match event.code {
        C::Char(c) => KeyCode::Char(c),
        C::Enter => KeyCode::Enter,
        C::Esc => KeyCode::Esc,
        C::Tab => KeyCode::Tab,
        // Some terminals report BackTab without the shift modifier.
        C::BackTab => {
            mods.shift = true;
            KeyCode::Tab
        }
        C::Backspace => KeyCode::Backspace,
        C::Delete => KeyCode::Delete,
        C::Insert => KeyCode::Insert,
        C::Up => KeyCode::Up,
        C::Down => KeyCode::Down,
        C::Left => KeyCode::Left,
        C::Right => KeyCode::Right,
        C::Home => KeyCode::Home,
        C::End => KeyCode::End,
        C::PageUp => KeyCode::PageUp,
        C::PageDown => KeyCode::PageDown,
        // Key notation has `<F1>` to `<F12>`.
        C::F(n @ 1..=12) => KeyCode::F(n),
        _ => return None,
    };
    Some(Key::new(code, mods))
}

/// Pasted text with its line breaks as `\n`: terminals and tmux send them
/// as `\r`, some as `\r\n`.
pub(crate) fn pasted_text(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    let mut unified = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            chars.next_if_eq(&'\n');
            unified.push('\n');
        } else {
            unified.push(c);
        }
    }
    unified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode as C, KeyEvent, KeyEventKind, KeyModifiers as M};
    use mxt_core::keys::{Key, KeySeq};

    fn key(text: &str) -> Key {
        KeySeq::parse(text, &KeySeq::default()).unwrap().keys()[0]
    }
    fn press(code: C, mods: M) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn characters_and_shift() {
        assert_eq!(
            key_from_crossterm(press(C::Char('j'), M::NONE)),
            Some(key("j"))
        );
        assert_eq!(
            key_from_crossterm(press(C::Char('G'), M::SHIFT)),
            Some(key("G"))
        );
        assert_eq!(
            key_from_crossterm(press(C::Char('あ'), M::NONE)),
            Some(key("あ"))
        );
        assert_eq!(
            key_from_crossterm(press(C::Char(' '), M::NONE)),
            Some(key("<Space>"))
        );
    }

    #[test]
    fn control_and_special_keys() {
        assert_eq!(
            key_from_crossterm(press(C::Char('w'), M::CONTROL)),
            Some(key("<C-w>"))
        );
        assert_eq!(
            key_from_crossterm(press(C::Enter, M::NONE)),
            Some(key("<CR>"))
        );
        assert_eq!(
            key_from_crossterm(press(C::BackTab, M::SHIFT)),
            Some(key("<S-Tab>"))
        );
        // Terminals that leave the modifier out still give `<S-Tab>`.
        assert_eq!(
            key_from_crossterm(press(C::BackTab, M::NONE)),
            Some(key("<S-Tab>"))
        );
        assert_eq!(
            key_from_crossterm(press(C::Esc, M::NONE)),
            Some(key("<Esc>"))
        );
        assert_eq!(
            key_from_crossterm(press(C::F(5), M::NONE)),
            Some(key("<F5>"))
        );
        assert_eq!(
            key_from_crossterm(press(C::Char('x'), M::ALT | M::CONTROL)),
            Some(key("<C-A-x>"))
        );
    }

    #[test]
    fn releases_are_ignored() {
        let mut e = press(C::Char('j'), M::NONE);
        e.kind = KeyEventKind::Release;
        assert_eq!(key_from_crossterm(e), None);
    }

    #[test]
    fn pasted_line_breaks_become_newlines() {
        // tmux's `paste-buffer` and most terminals send `\r`.
        assert_eq!(pasted_text("a\rb\r\nc\nd\r".into()), "a\nb\nc\nd\n");
    }
}
