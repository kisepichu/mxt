//! The `:` command line and the `/` search line: text editing, execution and
//! completion.

use super::{App, StatusKind};
use crate::command::{COMMANDS, parse_command, takes_argument};
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
    /// Never empty.
    candidates: Vec<String>,
    /// The candidate in the text; `candidates.len()` stands for `typed`.
    shown: usize,
}

/// An edit of the line text.
enum Edit {
    Insert(char),
    /// `<BS>`: delete the last character, or close an empty line.
    Backspace,
    /// `<C-u>`
    Clear,
    /// `<C-w>`
    DeleteWord,
}

impl Edit {
    fn of(key: Key) -> Option<Edit> {
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

    /// A key released to the line: edits the text; other keys are ignored.
    pub(super) fn type_into_line(&mut self, key: Key) {
        let (Some(open), Some(edit)) = (&mut self.command_line, Edit::of(key)) else {
            return;
        };
        let text = &mut open.line.text;
        match edit {
            Edit::Insert(c) => text.push(c),
            Edit::Backspace => {
                if text.pop().is_none() {
                    self.command_line = None;
                    return;
                }
            }
            Edit::Clear => text.clear(),
            // Trailing whitespace, then the word before it.
            Edit::DeleteWord => {
                let word_end = text.trim_end().len();
                let kept = text[..word_end].trim_end_matches(|c: char| !c.is_whitespace());
                text.truncate(kept.len());
            }
        }
        open.completion = None;
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
            LineKind::Search(pane) => self.search_line(pane, text),
        }
    }

    /// Replaces the text with the next candidate; after the last one the
    /// typed text comes back, then the cycle repeats.
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
                let candidates = candidates(&open.line.text, &self.rooms);
                if candidates.is_empty() {
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
            .filter(|command| command.starts_with(text))
            .map(|command| {
                if takes_argument(command) {
                    format!("{command} ")
                } else {
                    (*command).to_owned()
                }
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
