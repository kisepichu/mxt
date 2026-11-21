//! The `:` command line and the `/` search line: text editing, execution and
//! completion.

use super::{App, StatusKind};
use crate::command::{Argument, COMMANDS, parse_command};
use crate::effect::Effect;
use crate::emoji;
use crate::fuzzy;
use crate::keys::Key;
use crate::line::{LineChange, LineEditor};
use crate::msg::Pane;
use crate::rooms::RoomList;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// `:`, a command.
    Command,
    /// `/`, a search in the pane.
    Search(Pane),
}

/// The open command line, rendered as `:` or `/` followed by its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine {
    pub kind: LineKind,
    /// The text after the prompt, and its cursor.
    pub input: LineEditor,
}

/// The open line and its completion, which ends with the line.
pub(super) struct OpenLine {
    line: CommandLine,
    completion: Option<Completion>,
}

/// The candidates of `command_line.complete` since the last edit or cursor
/// move.
struct Completion {
    /// The text before the first press.
    typed: String,
    /// At least two; a single candidate ends the completion at once.
    candidates: Vec<String>,
    /// The candidate in the text; `candidates.len()` stands for `typed`.
    shown: usize,
}

impl App {
    /// The open command line, rendered as `:` or `/` followed by its text.
    pub fn command_line(&self) -> Option<&CommandLine> {
        self.command_line.as_ref().map(|open| &open.line)
    }

    pub(super) fn open_line(&mut self, kind: LineKind) {
        self.command_line = Some(OpenLine {
            line: CommandLine {
                kind,
                input: LineEditor::default(),
            },
            completion: None,
        });
    }

    /// A key released to the line: the line editor's. Editing the text or
    /// moving the cursor ends a completion, and a backspace on an empty line
    /// closes it.
    pub(super) fn type_into_line(&mut self, key: Key) {
        let Some(open) = &mut self.command_line else {
            return;
        };
        match open.line.input.key(key) {
            LineChange::Text | LineChange::Cursor => open.completion = None,
            LineChange::BackspaceOnEmpty => self.command_line = None,
            LineChange::Nothing => {}
        }
    }

    /// Pasted text without line breaks, inserted at the cursor.
    pub(super) fn paste_into_line(&mut self, text: &str) {
        if let Some(open) = &mut self.command_line
            && open.line.input.insert(text)
        {
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
        let CommandLine { kind, input } = open.line;
        let text = input.into_text();
        match kind {
            LineKind::Command => match parse_command(&text) {
                Ok(Some(action)) => self.run(&action, 1, effects),
                Ok(None) => {}
                Err(error) => self.set_status(StatusKind::Error, error.to_string()),
            },
            LineKind::Search(pane) => self.search_line(pane, text, effects),
        }
    }

    /// Replaces the whole text, wherever the cursor is, with the next
    /// candidate and puts the cursor at the end; after the last one the
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
                let text = open.line.input.text();
                let mut candidates = candidates(text, &self.rooms);
                if candidates.len() <= 1 {
                    if let Some(only) = candidates.pop() {
                        open.line.input.set_text(&only);
                    }
                    return;
                }
                open.completion.insert(Completion {
                    typed: text.to_owned(),
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
        open.line.input.set_text(shown);
    }
}

/// Without whitespace in `text`: the commands that start with it, with a
/// space after those that take an argument. For `open <query>`: the
/// displayed room names ranked against the query, as `open <name>`. For
/// `react <prefix>`: the shortcodes that start with the prefix, a leading
/// `:` ignored, in alphabetical order, as `react <shortcode>`.
fn candidates(text: &str, rooms: &RoomList) -> Vec<String> {
    match text.split_once(char::is_whitespace) {
        None => COMMANDS
            .iter()
            .filter(|command| command.name.starts_with(text))
            .map(|command| match command.argument {
                Argument::None => command.name.to_owned(),
                Argument::Required(_) | Argument::Optional(_) => format!("{} ", command.name),
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
        Some(("react", prefix)) => {
            let prefix = prefix.trim();
            let prefix = prefix.strip_prefix(':').unwrap_or(prefix);
            emoji::shortcodes_starting_with(prefix)
                .iter()
                .map(|shortcode| format!("react {shortcode}"))
                .collect()
        }
        Some(_) => Vec::new(),
    }
}
