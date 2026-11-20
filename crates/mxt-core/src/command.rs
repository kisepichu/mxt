//! The command registry and the parsing of the `:` command line into actions.

use std::fmt;

use crate::action::Action;
use crate::rooms::{RoomFilter, RoomTarget};

/// What a command takes after its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Argument {
    /// The command rejects an argument.
    None,
    /// Without an argument the command shows this usage, such as
    /// `:open <room>`.
    Required(&'static str),
}

/// A command of the registry.
pub struct Command {
    pub name: &'static str,
    pub argument: Argument,
    /// One line for `:help`.
    pub description: &'static str,
    /// The action for the argument, which is empty for [`Argument::None`]
    /// and never empty for [`Argument::Required`].
    action: fn(&str) -> Result<Action, CommandError>,
}

/// Every command, in the order of the design's Commands table, which is the
/// order of completion and of `:help`.
pub static COMMANDS: &[Command] = &[
    Command {
        name: "open",
        argument: Argument::Required(":open <room>"),
        description: "Open a room by its name",
        action: |room| {
            Ok(Action::RoomOpen(RoomTarget::Fuzzy {
                filter: RoomFilter::All,
                query: room.to_owned(),
            }))
        },
    },
    Command {
        name: "mute",
        argument: Argument::None,
        description: "Mute the room under the room list's cursor, or the open room",
        action: |_| Ok(Action::RoomMute),
    },
    Command {
        name: "unmute",
        argument: Argument::None,
        description: "Unmute the room, so the account's default notifications apply",
        action: |_| Ok(Action::RoomUnmute),
    },
    Command {
        name: "upload",
        argument: Argument::Required(":upload <path>"),
        description: "Send a file to the open room",
        action: |path| {
            Ok(Action::FileUpload {
                path: path.to_owned(),
            })
        },
    },
    Command {
        name: "recover",
        argument: Argument::None,
        description: "Verify this device and fetch room keys with the recovery key",
        action: |_| Ok(Action::AccountRecover),
    },
    Command {
        name: "q",
        argument: Argument::None,
        description: "Quit",
        action: |_| Ok(Action::AppQuit),
    },
];

/// Parses a command line without the leading `:`. `Ok(None)` for a blank line.
/// Everything after the command name, trimmed, is one argument, so
/// `open my room` and paths with spaces need no quoting. Names match
/// exactly.
pub fn parse_command(line: &str) -> Result<Option<Action>, CommandError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let (name, arg) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(name, arg)| (name, arg.trim_start()));
    let Some(command) = COMMANDS.iter().find(|command| command.name == name) else {
        return Err(CommandError::Unknown {
            name: name.to_owned(),
        });
    };
    match command.argument {
        Argument::None if !arg.is_empty() => Err(CommandError::UnexpectedArgument {
            command: command.name,
        }),
        Argument::Required(usage) if arg.is_empty() => Err(CommandError::MissingArgument {
            command: command.name,
            usage,
        }),
        _ => (command.action)(arg).map(Some),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Unknown {
        name: String,
    },
    /// `usage` is the command with a placeholder, such as `:open <room>`.
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
            CommandError::MissingArgument { usage, .. } => write!(f, "usage: {usage}"),
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
        assert_eq!(
            parse_command("open").map_err(|error| error.to_string()),
            Err("usage: :open <room>".to_owned())
        );
        assert_eq!(
            parse_command("mute now").map_err(|error| error.to_string()),
            Err(":mute takes no argument".to_owned())
        );
    }

    #[test]
    fn names_match_exactly() {
        for name in ["h", "o", "qu", "Q"] {
            assert_eq!(
                parse_command(name),
                Err(CommandError::Unknown { name: name.into() })
            );
        }
    }

    #[test]
    fn the_argument_kind_decides_what_a_command_accepts() {
        for command in COMMANDS {
            let name = command.name;
            let with_argument = parse_command(&format!("{name} x"));
            let without = parse_command(name);
            match command.argument {
                Argument::None => {
                    assert_eq!(
                        with_argument,
                        Err(CommandError::UnexpectedArgument { command: name })
                    );
                    assert!(matches!(without, Ok(Some(_))), "{name}");
                }
                Argument::Required(usage) => {
                    assert!(matches!(with_argument, Ok(Some(_))), "{name}");
                    assert_eq!(
                        without,
                        Err(CommandError::MissingArgument {
                            command: name,
                            usage
                        })
                    );
                    assert!(usage.starts_with(&format!(":{name} ")), "{usage}");
                }
            }
        }
    }
}
