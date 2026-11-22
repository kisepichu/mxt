//! The histories of the `:` command line and the `/` search lines (design,
//! "Line editing"), which `mxt` keeps in a file across starts.

use std::collections::VecDeque;

/// How many lines each history keeps.
const LIMIT: usize = 100;

/// Executed lines, oldest first, each at most once, at most [`LIMIT`],
/// none empty and none holding a line break, as the file needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    lines: VecDeque<String>,
}

impl History {
    /// The lines of a file, oldest first: the newest [`LIMIT`], each kept at
    /// its newest place, empty ones and ones with a line break left out.
    pub fn from_lines(lines: Vec<String>) -> History {
        let mut kept = VecDeque::with_capacity(lines.len().min(LIMIT));
        for line in lines.into_iter().rev() {
            if kept.len() == LIMIT {
                break;
            }
            if recordable(&line) && !kept.contains(&line) {
                kept.push_front(line);
            }
        }
        History { lines: kept }
    }

    /// Puts an executed line at the newest place: an equal line moves there,
    /// and beyond [`LIMIT`] the oldest drops. An empty line and one with a
    /// line break, which completion can put in a line with a room or invite
    /// name, are not recorded. Whether the history changed.
    pub fn record(&mut self, line: &str) -> bool {
        if !recordable(line) || self.lines.back().is_some_and(|newest| newest == line) {
            return false;
        }
        let at = self.lines.iter().position(|old| old == line);
        let line = match at.and_then(|at| self.lines.remove(at)) {
            Some(old) => old,
            None => {
                if self.lines.len() == LIMIT {
                    self.lines.pop_front();
                }
                line.to_owned()
            }
        };
        self.lines.push_back(line);
        true
    }

    /// Oldest first.
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.lines.iter().map(String::as_str)
    }
}

/// Not empty, and a single line.
fn recordable(line: &str) -> bool {
    !line.is_empty() && !line.contains('\n')
}

/// The two histories: the `:` command line's, and the `/` search lines',
/// which the room list's and the timeline's share.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineHistories {
    pub commands: History,
    pub searches: History,
}
