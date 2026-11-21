//! `account.recover`: the recovery key typed in `SECRET` mode (design,
//! "Modes"), and the matrix adapter's answer.

use unicode_segmentation::UnicodeSegmentation;

use super::{App, StatusKind};
use crate::effect::{Effect, MatrixEffect, RecoveryKey};
use crate::keys::{Key, KeyCode, Modifiers};
use crate::line::LineEditor;

/// What the bottom line shows before the masked key.
const PROMPT: &str = "Recovery key: ";

/// The `SECRET` line: the prompt, and of the key only its length and the
/// cursor, both in grapheme clusters, for one `*` per cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretPrompt {
    pub prompt: &'static str,
    pub clusters: usize,
    /// The number of clusters before the cursor.
    pub cursor: usize,
}

impl App {
    /// In `SECRET` mode: the prompt and the key's length and cursor, for the
    /// masked input. The key itself leaves the core only in
    /// `MatrixEffect::Recover`. A binding can also ask a question, which
    /// `feed` answers first, so the prompt waits until it is answered.
    pub fn secret_prompt(&self) -> Option<SecretPrompt> {
        if self.confirm.is_some() {
            return None;
        }
        let secret = self.secret.as_ref()?;
        let (before, after) = secret.text().split_at(secret.cursor());
        let cursor = before.graphemes(true).count();
        Some(SecretPrompt {
            prompt: PROMPT,
            clusters: cursor + after.graphemes(true).count(),
            cursor,
        })
    }

    /// `account.recover` enters `SECRET` mode; keys pending from before go.
    pub(super) fn start_recovery(&mut self) {
        self.resolver.reset();
        // Room for a recovery key (59 characters) or a long passphrase, so
        // typing and editing it reallocate nothing and leave no partial
        // copies of it in freed memory.
        let key = RecoveryKey::from(String::with_capacity(128));
        self.secret = Some(LineEditor::new(key));
    }

    /// A key in `SECRET` mode, which bypasses the keymap: `<CR>` submits
    /// (a blank key skips recovery, as at startup), `<Esc>` cancels, and the
    /// line editor's keys edit the key. Either way the core keeps nothing of
    /// it: a submitted key moves into the effect.
    pub(super) fn type_secret(&mut self, key: Key, effects: &mut Vec<Effect>) {
        let plain = key.mods() == Modifiers::default();
        match key.code() {
            KeyCode::Enter if plain => {
                if let Some(secret) = self
                    .secret
                    .take()
                    .map(LineEditor::into_text)
                    .filter(|secret| !secret.is_blank())
                {
                    self.set_status(StatusKind::Info, "Recovering…");
                    effects.push(Effect::Matrix(MatrixEffect::Recover(secret)));
                }
            }
            KeyCode::Esc if plain => self.secret = None,
            _ => {
                if let Some(secret) = &mut self.secret {
                    // A backspace on an empty key keeps the prompt open.
                    secret.key(key);
                }
            }
        }
    }

    /// Pasted text without line breaks, inserted into the key at the cursor
    /// like typed text, so it never submits.
    pub(super) fn paste_into_secret(&mut self, text: &str) {
        if let Some(secret) = &mut self.secret {
            secret.insert(text);
        }
    }

    /// `Msg::Recovered`.
    pub(super) fn recovered(&mut self) {
        self.set_status(StatusKind::Info, "Recovery complete");
    }
}
