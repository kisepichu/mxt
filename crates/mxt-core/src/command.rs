//! Parsing of the `:` command line into actions.

use std::fmt;

use crate::action::Action;
use crate::rooms::{RoomFilter, RoomTarget};

/// Command names, in the order completion offers them.
pub const COMMANDS: &[&str] = &["open", "mute", "unmute", "upload", "recover", "q"];

/// Whether the command takes an argument (completion appends a space after it).
pub fn takes_argument(command: &str) -> bool {
    matches!(command, "open" | "upload")
}

/// Parses a command line without the leading `:`. `Ok(None)` for a blank line.
/// Everything after the command name, trimmed, is one argument, so
/// `open my room` and paths with spaces need no quoting.
pub fn parse_command(line: &str) -> Result<Option<Action>, CommandError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let (name, arg) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(name, arg)| (name, arg.trim_start()));
    let action = match name {
        "open" => Action::RoomOpen(RoomTarget::Fuzzy {
            filter: RoomFilter::All,
            query: required("open", "open <room>", arg)?,
        }),
        "upload" => Action::FileUpload {
            path: required("upload", "upload <path>", arg)?,
        },
        "mute" => without_argument("mute", arg, Action::RoomMute)?,
        "unmute" => without_argument("unmute", arg, Action::RoomUnmute)?,
        "recover" => without_argument("recover", arg, Action::AccountRecover)?,
        "q" => without_argument("q", arg, Action::AppQuit)?,
        _ => {
            return Err(CommandError::Unknown {
                name: name.to_owned(),
            });
        }
    };
    Ok(Some(action))
}

fn required(command: &'static str, usage: &'static str, arg: &str) -> Result<String, CommandError> {
    if arg.is_empty() {
        return Err(CommandError::MissingArgument { command, usage });
    }
    Ok(arg.to_owned())
}

fn without_argument(
    command: &'static str,
    arg: &str,
    action: Action,
) -> Result<Action, CommandError> {
    if !arg.is_empty() {
        return Err(CommandError::UnexpectedArgument { command });
    }
    Ok(action)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Unknown {
        name: String,
    },
    /// `usage` is the command with a placeholder, such as `open <room>`.
    MissingArgument {
        command: &'static str,
        usage: &'static str,
    },
    UnexpectedArgument {
        command: &'static str,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Unknown { name } => write!(f, "unknown command \":{name}\""),
            CommandError::MissingArgument { usage, .. } => write!(f, "usage: :{usage}"),
            CommandError::UnexpectedArgument { command } => {
                write!(f, ":{command} takes no argument")
            }
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::rooms::{RoomFilter, RoomTarget};

    #[test]
    fn open_takes_the_rest_of_the_line() {
        assert_eq!(
            parse_command("open  my room  "),
            Ok(Some(Action::RoomOpen(RoomTarget::Fuzzy {
                filter: RoomFilter::All,
                query: "my room".into()
            })))
        );
    }

    #[test]
    fn upload_keeps_spaces_in_paths() {
        assert_eq!(
            parse_command("upload /tmp/a b.png"),
            Ok(Some(Action::FileUpload {
                path: "/tmp/a b.png".into()
            }))
        );
    }

    #[test]
    fn simple_commands() {
        assert_eq!(parse_command("q"), Ok(Some(Action::AppQuit)));
        assert_eq!(parse_command(" mute "), Ok(Some(Action::RoomMute)));
        assert_eq!(parse_command("unmute"), Ok(Some(Action::RoomUnmute)));
        assert_eq!(parse_command("recover"), Ok(Some(Action::AccountRecover)));
        assert_eq!(parse_command("   "), Ok(None));
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse_command("quit"),
            Err(CommandError::Unknown {
                name: "quit".into()
            })
        );
        assert!(matches!(
            parse_command("open"),
            Err(CommandError::MissingArgument {
                command: "open",
                ..
            })
        ));
        assert_eq!(
            parse_command("mute now"),
            Err(CommandError::UnexpectedArgument { command: "mute" })
        );
    }

    #[test]
    fn argument_flags() {
        assert!(takes_argument("open"));
        assert!(takes_argument("upload"));
        assert!(!takes_argument("q"));
    }

    #[test]
    fn every_completed_command_parses() {
        for &command in COMMANDS {
            let line = if takes_argument(command) {
                format!("{command} x")
            } else {
                command.to_owned()
            };
            assert!(matches!(parse_command(&line), Ok(Some(_))), "{line}");
        }
    }
}
