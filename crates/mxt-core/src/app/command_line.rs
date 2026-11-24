//! The `:` command line and the `/` search line: text editing, execution,
//! completion and the histories.

use super::join::join_candidates;
use super::{App, StatusKind};
use crate::action::NotifyMode;
use crate::command::{Argument, COMMANDS, CommandError, parse_command};
use crate::effect::{Effect, StoreEffect};
use crate::emoji;
use crate::fuzzy;
use crate::history::{History, LineHistories};
use crate::keys::Key;
use crate::line::{LineChange, LineEditor};
use crate::msg::Pane;
use crate::room::Invite;
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

/// The open line, and its completion or history browsing, which end with
/// the line.
pub(super) struct OpenLine {
    line: CommandLine,
    cycle: Cycle,
}

/// What the open line cycles through: at most one, since each ends the
/// other.
enum Cycle {
    None,
    Completing(Completion),
    Browsing(Browse),
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

/// `command_line.history_prev` and `command_line.history_next` since the
/// last other change of the line.
struct Browse {
    /// The text and cursor at the first `history_prev`, which come back past
    /// the newest match.
    typed: LineEditor,
    /// The index in the history of the match in the line.
    at: usize,
}

impl Browse {
    /// The text before the cursor at the first `history_prev`: the lines
    /// that start with it match.
    fn filter(&self) -> &str {
        &self.typed.text()[..self.typed.cursor()]
    }
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
            cycle: Cycle::None,
        });
    }

    /// A key released to the line: the line editor's. Editing the text or
    /// moving the cursor ends a completion and the history browsing, and a
    /// backspace on an empty line closes it.
    pub(super) fn type_into_line(&mut self, key: Key) {
        let Some(open) = &mut self.command_line else {
            return;
        };
        match open.line.input.key(key) {
            LineChange::Text | LineChange::Cursor => open.cycle = Cycle::None,
            LineChange::BackspaceOnEmpty => self.command_line = None,
            LineChange::Nothing => {}
        }
    }

    /// Pasted text without line breaks, inserted at the cursor.
    pub(super) fn paste_into_line(&mut self, text: &str) {
        if let Some(open) = &mut self.command_line
            && open.line.input.insert(text)
        {
            open.cycle = Cycle::None;
        }
    }

    pub(super) fn cancel_line(&mut self) {
        self.command_line = None;
    }

    /// Closes the line, puts its text into the line's history, then runs its
    /// command or searches with its text (an empty search text reuses the
    /// last pattern). A command line of blanks only, which runs nothing, is
    /// not recorded, and neither is `:recover` with an argument: that can
    /// only be the recovery key, which never goes to disk.
    pub(super) fn execute_line(&mut self, effects: &mut Vec<Effect>) {
        let Some(open) = self.command_line.take() else {
            return;
        };
        let CommandLine { kind, input } = open.line;
        let text = input.into_text();
        match kind {
            LineKind::Command => {
                let parsed = parse_command(&text);
                if !matches!(
                    parsed,
                    Ok(None) | Err(CommandError::UnexpectedArgument { command: "recover" })
                ) {
                    self.record_line(kind, &text, effects);
                }
                match parsed {
                    Ok(Some(action)) => self.run(&action, 1, effects),
                    Ok(None) => {}
                    Err(error) => self.set_status(StatusKind::Error, error.to_string()),
                }
            }
            LineKind::Search(pane) => {
                self.record_line(kind, &text, effects);
                self.search_line(pane, text, effects);
            }
        }
    }

    /// Records an executed line in its history, as it was typed, and when
    /// that changed the history, saves both.
    fn record_line(&mut self, kind: LineKind, text: &str, effects: &mut Vec<Effect>) {
        let history = match kind {
            LineKind::Command => &mut self.histories.commands,
            LineKind::Search(_) => &mut self.histories.searches,
        };
        if history.record(text) {
            let lines = |history: &History| history.lines().map(str::to_owned).collect();
            effects.push(Effect::Store(StoreEffect::SaveHistory {
                commands: lines(&self.histories.commands),
                searches: lines(&self.histories.searches),
            }));
        }
    }

    /// `Msg::HistoryLoaded`.
    pub(super) fn history_loaded(&mut self, commands: Vec<String>, searches: Vec<String>) {
        self.histories = LineHistories {
            commands: History::from_lines(commands),
            searches: History::from_lines(searches),
        };
    }

    /// `command_line.history_prev`: the first press takes the text before
    /// the cursor as the filter and keeps the text and cursor; each press
    /// puts the next older line of the line's history that starts with the
    /// filter in the line, the cursor at its end. With none left it does
    /// nothing.
    pub(super) fn history_prev(&mut self) {
        let Some(open) = &mut self.command_line else {
            return;
        };
        let history = history_of(&self.histories, open.line.kind);
        let (filter, before) = match &open.cycle {
            Cycle::Browsing(browse) => (browse.filter(), browse.at),
            Cycle::None | Cycle::Completing(_) => {
                let input = &open.line.input;
                (&input.text()[..input.cursor()], history.lines().len())
            }
        };
        let older = history
            .lines()
            .enumerate()
            .take(before)
            .rev()
            .find(|(_, line)| line.starts_with(filter));
        let Some((at, line)) = older else {
            return;
        };
        match &mut open.cycle {
            Cycle::Browsing(browse) => browse.at = at,
            cycle => {
                *cycle = Cycle::Browsing(Browse {
                    typed: open.line.input.clone(),
                    at,
                });
            }
        }
        open.line.input.set_text(line);
    }

    /// `command_line.history_next`: while browsing, the next newer match,
    /// and past the newest the text and cursor from before browsing, which
    /// ends it. Without browsing it does nothing.
    pub(super) fn history_next(&mut self) {
        let Some(open) = &mut self.command_line else {
            return;
        };
        let Cycle::Browsing(browse) = &mut open.cycle else {
            return;
        };
        let history = history_of(&self.histories, open.line.kind);
        let newer = history
            .lines()
            .enumerate()
            .skip(browse.at + 1)
            .find(|(_, line)| line.starts_with(browse.filter()));
        match newer {
            Some((at, line)) => {
                browse.at = at;
                open.line.input.set_text(line);
            }
            None => {
                open.line.input = std::mem::take(&mut browse.typed);
                open.cycle = Cycle::None;
            }
        }
    }

    /// Replaces the whole text, wherever the cursor is, with the next
    /// candidate and puts the cursor at the end; after the last one the
    /// typed text comes back, then the cycle repeats. A single candidate ends
    /// the completion, so the next press completes the new text (`open `
    /// goes on to room names), like vim. It ends the history browsing.
    pub(super) fn complete_line(&mut self) {
        let Some(open) = &mut self.command_line else {
            return;
        };
        if open.line.kind != LineKind::Command {
            open.cycle = Cycle::None;
            return;
        }
        match &mut open.cycle {
            Cycle::Completing(completion) => {
                completion.shown = (completion.shown + 1) % (completion.candidates.len() + 1);
                let shown = completion
                    .candidates
                    .get(completion.shown)
                    .unwrap_or(&completion.typed);
                open.line.input.set_text(shown);
            }
            cycle => {
                let text = open.line.input.text();
                let mut candidates = candidates(text, &self.rooms, &self.invites);
                if candidates.len() <= 1 {
                    if let Some(only) = candidates.pop() {
                        open.line.input.set_text(&only);
                    }
                    *cycle = Cycle::None;
                    return;
                }
                let typed = text.to_owned();
                open.line.input.set_text(&candidates[0]);
                *cycle = Cycle::Completing(Completion {
                    typed,
                    candidates,
                    shown: 0,
                });
            }
        }
    }
}

/// The history of a line of `kind`: searches share one, whichever pane they
/// search.
fn history_of(histories: &LineHistories, kind: LineKind) -> &History {
    match kind {
        LineKind::Command => &histories.commands,
        LineKind::Search(_) => &histories.searches,
    }
}

/// Without whitespace in `text`: the commands that start with it, with a
/// space after those that take an argument. For `open <query>`: the
/// displayed room names ranked against the query, as `open <name>`. For
/// `join <query>`: the pending invites ranked by name (see
/// `join_candidates`). For `react <prefix>`: the shortcodes that start with
/// the prefix, a leading `:` ignored, in alphabetical order, as
/// `react <shortcode>`. For `notify <prefix>`: `all` and `mentions`, those
/// that start with the prefix, in that order, as `notify <mode>`.
fn candidates(text: &str, rooms: &RoomList, invites: &[Invite]) -> Vec<String> {
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
        Some(("join", query)) => join_candidates(query.trim(), invites),
        Some(("react", prefix)) => {
            let prefix = prefix.trim();
            let prefix = prefix.strip_prefix(':').unwrap_or(prefix);
            emoji::shortcodes_starting_with(prefix)
                .iter()
                .map(|shortcode| format!("react {shortcode}"))
                .collect()
        }
        Some(("notify", prefix)) => {
            let prefix = prefix.trim();
            NotifyMode::ALL
                .into_iter()
                .map(NotifyMode::config_name)
                .filter(|mode| mode.starts_with(prefix))
                .map(|mode| format!("notify {mode}"))
                .collect()
        }
        Some(_) => Vec::new(),
    }
}
