//! The command registry and the parsing of the `:` command line into actions.

use std::borrow::Cow;
use std::fmt::{self, Write as _};

use crate::action::Action;
use crate::emoji::Emoji;
use crate::rooms::{RoomFilter, RoomTarget};

/// What a command takes after its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Argument {
    /// The command rejects an argument.
    None,
    /// Without an argument the command shows this usage, such as
    /// `:open <room>`.
    Required(&'static str),
    /// The argument may be left out; `:help` shows this usage, such as
    /// `:react [<emoji>]`.
    Optional(&'static str),
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

impl Command {
    /// `:open <room>`, or `:mute` for a command without an argument.
    pub fn usage(&self) -> Cow<'static, str> {
        match self.argument {
            Argument::None => Cow::Owned(format!(":{}", self.name)),
            Argument::Required(usage) | Argument::Optional(usage) => Cow::Borrowed(usage),
        }
    }
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
        name: "react",
        argument: Argument::Optional(":react [<emoji>]"),
        description: "Toggle a reaction on the message under the timeline's cursor, or pick one",
        action: |emoji| {
            if emoji.is_empty() {
                return Ok(Action::MessageReact(None));
            }
            match Emoji::parse(emoji) {
                Some(emoji) => Ok(Action::MessageReact(Some(emoji))),
                None => Err(CommandError::UnknownEmoji {
                    emoji: emoji.to_owned(),
                }),
            }
        },
    },
    Command {
        name: "recover",
        argument: Argument::None,
        description: "Verify this device and fetch room keys with the recovery key",
        action: |_| Ok(Action::AccountRecover),
    },
    Command {
        name: "help",
        argument: Argument::None,
        description: "List the commands",
        action: |_| Ok(Action::HelpCommands),
    },
    Command {
        name: "map",
        argument: Argument::None,
        description: "List the key bindings of each keymap table",
        action: |_| Ok(Action::HelpKeymap),
    },
    Command {
        name: "messages",
        argument: Argument::None,
        description: "List the status line's recent messages",
        action: |_| Ok(Action::StatusMessages),
    },
    Command {
        name: "q",
        argument: Argument::None,
        description: "Quit",
        action: |_| Ok(Action::AppQuit),
    },
];

/// The first line of `:help` (design, "Help views").
const HELP_FIRST_LINE: &str =
    "mxt is a Matrix client with nvim-like keys; docs/keys.md describes every key and action.";

/// The text of `:help`: [`HELP_FIRST_LINE`], an empty line, then a line per
/// command in order with its usage, padded with spaces to the longest usage
/// plus two, and its description.
pub fn help_text(commands: &[Command]) -> String {
    let usages: Vec<Cow<'static, str>> = commands.iter().map(Command::usage).collect();
    let width = usages.iter().map(|usage| usage.chars().count()).max();
    let width = width.unwrap_or(0) + 2;
    let mut text = format!("{HELP_FIRST_LINE}\n");
    for (command, usage) in commands.iter().zip(&usages) {
        _ = write!(text, "\n{usage:<width$}{}", command.description);
    }
    text
}

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
    /// `:react`'s argument as typed, which names no emoji.
    UnknownEmoji {
        emoji: String,
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
            CommandError::UnknownEmoji { emoji } => write!(f, "unknown emoji \"{emoji}\""),
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
        assert_eq!(
            parse_command("react :+1").map_err(|error| error.to_string()),
            Err("unknown emoji \":+1\"".to_owned())
        );
        assert_eq!(
            parse_command("messages x").map_err(|error| error.to_string()),
            Err(":messages takes no argument".to_owned())
        );
    }

    #[test]
    fn react_takes_an_emoji_or_a_shortcode_or_nothing() {
        let thumbs = Ok(Some(Action::MessageReact(Emoji::get("👍"))));
        assert_eq!(parse_command("react 👍"), thumbs);
        assert_eq!(parse_command("react  thumbsup "), thumbs);
        assert_eq!(parse_command("react :+1:"), thumbs);
        assert_eq!(parse_command("react"), Ok(Some(Action::MessageReact(None))));
        assert_eq!(
            parse_command("react nonsense"),
            Err(CommandError::UnknownEmoji {
                emoji: "nonsense".into()
            })
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
                Argument::Optional(usage) => {
                    assert!(matches!(without, Ok(Some(_))), "{name}");
                    assert!(usage.starts_with(&format!(":{name} [")), "{usage}");
                }
            }
        }
    }

    #[test]
    fn help_lists_every_command_after_its_first_line() {
        let text = help_text(COMMANDS);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[..2],
            [
                "mxt is a Matrix client with nvim-like keys; docs/keys.md describes every key and action.",
                ""
            ]
        );
        let entries = &lines[2..];
        let names: Vec<&str> = entries
            .iter()
            .map(|line| line.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                ":open",
                ":mute",
                ":unmute",
                ":upload",
                ":react",
                ":recover",
                ":help",
                ":map",
                ":messages",
                ":q"
            ]
        );
        // The descriptions start two columns after the longest usage.
        let longest = COMMANDS
            .iter()
            .map(|command| command.usage().len())
            .max()
            .unwrap();
        for (line, command) in entries.iter().zip(COMMANDS) {
            let (usage, description) = line.split_at(longest + 2);
            assert_eq!(usage.trim_end(), command.usage(), "{line}");
            assert_eq!(description, command.description, "{line}");
        }
        let open = format!("{:<1$}Open a room by its name", ":open <room>", longest + 2);
        assert_eq!(entries[0], open);
    }
}
