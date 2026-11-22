//! The clipboard (design, "Clipboard and terminal focus"): a copy goes to the
//! terminal clipboard with OSC 52 and to the register that every editor state
//! and timeline yanks share; pastes read the register. Each editor copies
//! through its own handle, which can drop its copies for the black hole
//! register `"_` (design, "Input routing").

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use edtui::clipboard::ClipboardTrait;

#[derive(Default)]
struct Register {
    /// The text of the last copy.
    text: String,
    /// Whether `text` has yet to reach the terminal clipboard.
    unsent: bool,
}

/// A handle to the one register; clones share it.
#[derive(Clone, Default)]
pub(crate) struct SharedRegister(Rc<RefCell<Register>>);

impl SharedRegister {
    pub(crate) fn copy(&self, text: String) {
        let mut register = self.0.borrow_mut();
        register.text = text;
        register.unsent = true;
    }

    /// The OSC 52 sequence that puts the last copy on the terminal clipboard,
    /// once per copy. Copies made before it is taken are replaced by later
    /// ones, as the clipboard itself would replace them.
    pub(crate) fn take_unsent(&self) -> Option<String> {
        let mut register = self.0.borrow_mut();
        std::mem::take(&mut register.unsent).then(|| osc52(&register.text))
    }

    /// A handle for one editor, with its own black hole flag, off.
    pub(crate) fn for_editor(&self) -> EditorRegister {
        EditorRegister {
            register: self.clone(),
            black_hole: Rc::default(),
        }
    }
}

/// An editor's handle to the shared register, which its editor states get
/// clones of. While the black hole flag is set, the editor's copies go
/// nowhere; pastes read the register either way. The flag is the handle's,
/// so it never drops another editor's copies.
#[derive(Clone)]
pub(crate) struct EditorRegister {
    register: SharedRegister,
    black_hole: Rc<Cell<bool>>,
}

impl EditorRegister {
    pub(crate) fn black_hole(&self) -> bool {
        self.black_hole.get()
    }

    pub(crate) fn set_black_hole(&self, on: bool) {
        self.black_hole.set(on);
    }
}

impl ClipboardTrait for EditorRegister {
    fn set_text(&mut self, text: String) {
        if !self.black_hole() {
            self.register.copy(text);
        }
    }

    fn get_text(&mut self) -> String {
        self.register.0.borrow().text.clone()
    }
}

/// OSC 52 "set the clipboard selection" with the text in base64, ended with
/// ST.
fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", STANDARD.encode(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_encodes_utf8_in_base64() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x1b\\");
        // UTF-8 E6 97 A5 E6 9C AC.
        assert_eq!(osc52("日本"), "\x1b]52;c;5pel5pys\x1b\\");
        assert_eq!(osc52(""), "\x1b]52;c;\x1b\\");
    }

    #[test]
    fn editor_copies_reach_yanks_and_the_terminal_once() {
        let register = SharedRegister::default();
        let mut editor = register.for_editor();
        editor.set_text("first".to_owned());
        register.copy("second".to_owned());
        assert_eq!(editor.get_text(), "second");
        assert_eq!(register.take_unsent(), Some(osc52("second")));
        assert_eq!(register.take_unsent(), None);
        // A paste does not send the text again.
        assert_eq!(editor.get_text(), "second");
        assert_eq!(register.take_unsent(), None);
    }
}
