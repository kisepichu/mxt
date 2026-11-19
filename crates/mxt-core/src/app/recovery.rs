//! `account.recover`: the recovery key typed in `SECRET` mode (design,
//! "Modes"), and the matrix adapter's answer.

use super::command_line::Edit;
use super::{App, StatusKind};
use crate::effect::{Effect, MatrixEffect, RecoveryKey};
use crate::keys::{Key, KeyCode, Modifiers};

/// What the bottom line shows before the masked key.
const PROMPT: &str = "Recovery key: ";

impl App {
    /// In `SECRET` mode: the prompt and the number of characters typed, for
    /// the masked input. The key itself leaves the core only in
    /// `MatrixEffect::Recover`.
    pub fn secret_prompt(&self) -> Option<(&'static str, usize)> {
        let typed = self.secret.as_ref()?.expose().chars().count();
        Some((PROMPT, typed))
    }

    /// `account.recover` enters `SECRET` mode; keys pending from before go.
    pub(super) fn start_recovery(&mut self) {
        self.resolver.reset();
        self.secret = Some(RecoveryKey::default());
    }

    /// A key in `SECRET` mode, which bypasses the keymap: `<CR>` submits
    /// (a blank key skips recovery, as at startup), `<Esc>` cancels, and the
    /// text editing keys of the command line edit the key. Either way the
    /// core keeps nothing of it: a submitted key moves into the effect.
    pub(super) fn type_secret(&mut self, key: Key, effects: &mut Vec<Effect>) {
        let plain = key.mods() == Modifiers::default();
        match key.code() {
            KeyCode::Enter if plain => {
                if let Some(secret) = self.secret.take().filter(|secret| !secret.is_blank()) {
                    self.set_status(StatusKind::Info, "Recovering…");
                    effects.push(Effect::Matrix(MatrixEffect::Recover(secret)));
                }
            }
            KeyCode::Esc if plain => self.secret = None,
            _ => {
                if let (Some(secret), Some(edit)) = (&mut self.secret, Edit::of(key)) {
                    // A backspace on an empty key keeps the prompt open.
                    edit.apply(secret.text_mut());
                }
            }
        }
    }

    /// `Msg::Recovered`.
    pub(super) fn recovered(&mut self) {
        self.set_status(StatusKind::Info, "Recovery complete");
    }
}
