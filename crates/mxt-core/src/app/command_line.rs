//! The `:` command line and the `/` search line: text editing, execution and
//! completion.

use super::{App, StatusKind};
use crate::command::{Argument, COMMANDS, parse_command};
use crate::effect::Effect;
use crate::fuzzy;
use crate::keys::{Key, KeyCode, Modifiers};
use crate::msg::Pane;
use crate::rooms::RoomList;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// `:`, a command.
    Command,
    /// `/`, a search in the pane.
    Search(Pane),
}

/// The open command line, rendered as `:` or `/` followed by `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine {
    pub kind: LineKind,
    pub text: String,
}

/// The open line and its completion, which ends with the line.
pub(super) struct OpenLine {
    line: CommandLine,
    completion: Option<Completion>,
}

/// The candidates of `command_line.complete` since the last edit.
struct Completion {
    /// The text before the first press.
    typed: String,
    /// At least two; a single candidate ends the completion at once.
    candidates: Vec<String>,
    /// The candidate in the text; `candidates.len()` stands for `typed`.
    shown: usize,
}

/// An edit of a text the core owns: the line text or the picker query.
pub(super) enum Edit {
    Insert(char),
    /// `<BS>`: delete the last character.
    Backspace,
    /// `<C-u>`
    Clear,
    /// `<C-w>`
    DeleteWord,
}

impl Edit {
    pub(super) fn of(key: Key) -> Option<Edit> {
        if let Some(c) = key.text() {
            return Some(Edit::Insert(c));
        }
        const NONE: Modifiers = Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
        };
        const CTRL: Modifiers = Modifiers {
            ctrl: true,
            alt: false,
            shift: false,
        };
        match (key.code(), key.mods()) {
            (KeyCode::Backspace, NONE) => Some(Edit::Backspace),
            (KeyCode::Char('u'), CTRL) => Some(Edit::Clear),
            (KeyCode::Char('w'), CTRL) => Some(Edit::DeleteWord),
            _ => None,
        }
    }

    /// Applies the edit; `false` when a backspace finds `text` empty.
    pub(super) fn apply(self, text: &mut String) -> bool {
        match self {
            Edit::Insert(c) => text.push(c),
            Edit::Backspace => return text.pop().is_some(),
            Edit::Clear => text.clear(),
            // Trailing whitespace, then the word before it.
            Edit::DeleteWord => {
                let word_end = text.trim_end().len();
                let kept = text[..word_end].trim_end_matches(|c: char| !c.is_whitespace());
                text.truncate(kept.len());
            }
        }
        true
    }
}

impl App {
    /// The open command line, rendered as `:` or `/` followed by `text`.
    pub fn command_line(&self) -> Option<&CommandLine> {
        self.command_line.as_ref().map(|open| &open.line)
    }

    pub(super) fn open_line(&mut self, kind: LineKind) {
        self.command_line = Some(OpenLine {
            line: CommandLine {
                kind,
                text: String::new(),
            },
            completion: None,
        });
    }

    /// A key released to the line: edits the text, and a backspace on an
    /// empty line closes it; other keys are ignored.
    pub(super) fn type_into_line(&mut self, key: Key) {
        let (Some(open), Some(edit)) = (&mut self.command_line, Edit::of(key)) else {
            return;
        };
        if !edit.apply(&mut open.line.text) {
            self.command_line = None;
            return;
        }
        open.completion = None;
    }

    /// Pasted text without line breaks, added to the line's text.
    pub(super) fn paste_into_line(&mut self, text: &str) {
        if let Some(open) = &mut self.command_line
            && !text.is_empty()
        {
            open.line.text.push_str(text);
            open.completion = None;
        }
    }

    pub(super) fn cancel_line(&mut self) {
        self.command_line = None;
    }

    /// Closes the line, then runs its command or searches with its text (an
    /// empty search text reuses the last pattern).
    pub(super) fn execute_line(&mut self, effects: &mut Vec<Effect>) {
        let Some(open) = self.command_line.take() else {
            return;
        };
        let CommandLine { kind, text } = open.line;
        match kind {
            LineKind::Command => match parse_command(&text) {
                Ok(Some(action)) => self.run(&action, 1, effects),
                Ok(None) => {}
                Err(error) => self.set_status(StatusKind::Error, error.to_string()),
            },
            LineKind::Search(pane) => self.search_line(pane, text, effects),
        }
    }

    /// Replaces the text with the next candidate; after the last one the
    /// typed text comes back, then the cycle repeats. A single candidate ends
    /// the completion, so the next press completes the new text (`open `
    /// goes on to room names), like vim.
    pub(super) fn complete_line(&mut self) {
        let Some(open) = &mut self.command_line else {
            return;
        };
        if open.line.kind != LineKind::Command {
            return;
        }
        let completion = match &mut open.completion {
            Some(completion) => completion,
            None => {
                let mut candidates = candidates(&open.line.text, &self.rooms);
                if candidates.len() <= 1 {
                    if let Some(only) = candidates.pop() {
                        open.line.text = only;
                    }
                    return;
                }
                open.completion.insert(Completion {
                    typed: open.line.text.clone(),
                    shown: candidates.len(),
                    candidates,
                })
            }
        };
        completion.shown = (completion.shown + 1) % (completion.candidates.len() + 1);
        let shown = completion
            .candidates
            .get(completion.shown)
            .unwrap_or(&completion.typed);
        open.line.text.clone_from(shown);
    }
}

/// Without whitespace in `text`: the commands that start with it, with a
/// space after those that take an argument. For `open <query>`: the
/// displayed room names ranked against the query, as `open <name>`.
fn candidates(text: &str, rooms: &RoomList) -> Vec<String> {
    match text.split_once(char::is_whitespace) {
        None => COMMANDS
            .iter()
            .filter(|command| command.name.starts_with(text))
            .map(|command| match command.argument {
                Argument::None => command.name.to_owned(),
                Argument::Required(_) => format!("{} ", command.name),
            })
            .collect(),
        Some(("open", query)) => {
            let names: Vec<String> = rooms
                .rooms()
                .iter()
                .map(|room| room.name.to_string())
                .collect();
            fuzzy::rank(query.trim(), &names)
                .into_iter()
                .map(|index| format!("open {}", names[index]))
                .collect()
        }
        Some(_) => Vec::new(),
    }
}
