//! The action registry: every action keys and commands can run, built from a
//! name and arguments.

use std::fmt;

use crate::emoji::Emoji;
use crate::rooms::{Pick, RoomFilter, RoomTarget};

/// A neutral argument value; the configuration loader converts TOML values into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    String(String),
    Integer(i64),
    Boolean(bool),
    /// Any other TOML value (a float, an array, a table, a date-time), which
    /// no action takes, so a known argument with it has the wrong type.
    Unsupported,
}

/// The editor mode `composer.focus` leaves the composer in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FocusMode {
    #[default]
    Insert,
    /// The editor's previous mode.
    Keep,
}

impl FocusMode {
    pub const ALL: [FocusMode; 2] = [FocusMode::Insert, FocusMode::Keep];

    /// The value of the `mode` argument: "insert", "keep".
    pub fn config_name(self) -> &'static str {
        match self {
            FocusMode::Insert => "insert",
            FocusMode::Keep => "keep",
        }
    }
}

/// An action with checked arguments. [`Action::name`] is its registry name,
/// `<target>.<verb>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    RoomOpen(RoomTarget),
    RoomOpenSelected,
    RoomMarkRead,
    RoomMute,
    RoomUnmute,
    /// `:join`: an alias, a room ID, a user ID, a matrix.to link or the name
    /// of a pending invite, read when the action runs.
    RoomJoin {
        target: String,
    },
    MessageReply,
    MessageEdit,
    MessageDelete,
    MessageYank,
    /// With an emoji, toggles that reaction; without, the emoji picker.
    MessageReact(Option<Emoji>),
    MessageOpen,
    MessageGotoReply,
    MessageResend,
    MessageView,
    ComposerFocus(FocusMode),
    ComposerSend,
    ComposerNewline,
    ComposerCancel,
    /// Back to the timeline; an empty reply or edit ends first.
    ComposerLeave,
    ComposerExternalEditor,
    CompletionNext,
    CompletionPrev,
    CompletionAccept,
    CompletionCancel,
    PickerRooms(RoomFilter),
    PickerNext,
    PickerPrev,
    PickerAccept,
    PickerCancel,
    CommandLineOpen,
    CommandLineExecute,
    CommandLineCancel,
    CommandLineComplete,
    CursorDown,
    CursorUp,
    CursorFirst,
    CursorLast,
    CursorHalfPageDown,
    CursorHalfPageUp,
    SearchStart,
    SearchNext,
    SearchPrev,
    ViewClose,
    /// `:help`: the command list.
    HelpCommands,
    /// `:map`: the key bindings of each table.
    HelpKeymap,
    /// `:messages`: the status line's recent messages.
    StatusMessages,
    SidebarToggle,
    FocusRoomList,
    FocusTimeline,
    FileUpload {
        path: String,
    },
    AccountRecover,
    AppQuit,
    /// Does nothing; binding it shadows the same keys in fallback tables.
    Nop,
}

impl Action {
    /// Builds a checked action from its registry name and arguments. Every
    /// action takes no arguments except:
    ///
    /// - `room.open`: `filter` (`all`, `unread`, `notify` or `mentions`;
    ///   default `all`), and either `pick` (`first`, `next` or `prev`; default
    ///   `first`) or `query` (a fuzzy room name query, non-empty)
    /// - `picker.rooms`: `filter`, as for `room.open`
    /// - `composer.focus`: `mode` (`insert` or `keep`; default `insert`)
    /// - `file.upload`: `path` (non-empty), required
    /// - `message.react`: `emoji` (an emoji or a shortcode, see
    ///   [`Emoji::parse`]; non-empty)
    /// - `room.join`: `target` (non-empty), required
    ///
    /// Argument names are unique in a configuration table; if one repeats
    /// anyway, its last value counts.
    ///
    /// The checks run in the order of the design's "Layers", and the first
    /// error is returned: the name, the names of the arguments, conflicts
    /// between arguments, then each argument's type and value and a missing
    /// required argument, in the order the action reads them.
    pub fn from_spec(name: &str, args: &[(String, ArgValue)]) -> Result<Action, ActionError> {
        let action = match name {
            "room.open" => {
                return room_open(Args::new("room.open", &["filter", "pick", "query"], args)?);
            }
            "picker.rooms" => {
                let args = Args::new("picker.rooms", &["filter"], args)?;
                return Ok(Action::PickerRooms(args.filter()?));
            }
            "composer.focus" => {
                let args = Args::new("composer.focus", &["mode"], args)?;
                let mode = args.choice("mode", &FocusMode::ALL, FocusMode::config_name)?;
                return Ok(Action::ComposerFocus(mode.unwrap_or_default()));
            }
            "file.upload" => return file_upload(Args::new("file.upload", &["path"], args)?),
            "message.react" => {
                return message_react(Args::new("message.react", &["emoji"], args)?);
            }
            "room.join" => return room_join(Args::new("room.join", &["target"], args)?),
            "room.open_selected" => Action::RoomOpenSelected,
            "room.mark_read" => Action::RoomMarkRead,
            "room.mute" => Action::RoomMute,
            "room.unmute" => Action::RoomUnmute,
            "message.reply" => Action::MessageReply,
            "message.edit" => Action::MessageEdit,
            "message.delete" => Action::MessageDelete,
            "message.yank" => Action::MessageYank,
            "message.open" => Action::MessageOpen,
            "message.goto_reply" => Action::MessageGotoReply,
            "message.resend" => Action::MessageResend,
            "message.view" => Action::MessageView,
            "composer.send" => Action::ComposerSend,
            "composer.newline" => Action::ComposerNewline,
            "composer.cancel" => Action::ComposerCancel,
            "composer.leave" => Action::ComposerLeave,
            "composer.external_editor" => Action::ComposerExternalEditor,
            "completion.next" => Action::CompletionNext,
            "completion.prev" => Action::CompletionPrev,
            "completion.accept" => Action::CompletionAccept,
            "completion.cancel" => Action::CompletionCancel,
            "picker.next" => Action::PickerNext,
            "picker.prev" => Action::PickerPrev,
            "picker.accept" => Action::PickerAccept,
            "picker.cancel" => Action::PickerCancel,
            "command_line.open" => Action::CommandLineOpen,
            "command_line.execute" => Action::CommandLineExecute,
            "command_line.cancel" => Action::CommandLineCancel,
            "command_line.complete" => Action::CommandLineComplete,
            "cursor.down" => Action::CursorDown,
            "cursor.up" => Action::CursorUp,
            "cursor.first" => Action::CursorFirst,
            "cursor.last" => Action::CursorLast,
            "cursor.half_page_down" => Action::CursorHalfPageDown,
            "cursor.half_page_up" => Action::CursorHalfPageUp,
            "search.start" => Action::SearchStart,
            "search.next" => Action::SearchNext,
            "search.prev" => Action::SearchPrev,
            "view.close" => Action::ViewClose,
            "help.commands" => Action::HelpCommands,
            "help.keymap" => Action::HelpKeymap,
            "status.messages" => Action::StatusMessages,
            "sidebar.toggle" => Action::SidebarToggle,
            "focus.room_list" => Action::FocusRoomList,
            "focus.timeline" => Action::FocusTimeline,
            "account.recover" => Action::AccountRecover,
            "app.quit" => Action::AppQuit,
            "nop" => Action::Nop,
            _ => {
                return Err(ActionError::UnknownAction {
                    name: name.to_owned(),
                });
            }
        };
        Args::new(action.name(), &[], args)?;
        Ok(action)
    }

    /// The registry name, for example "room.open".
    pub fn name(&self) -> &'static str {
        match self {
            Action::RoomOpen(_) => "room.open",
            Action::RoomOpenSelected => "room.open_selected",
            Action::RoomMarkRead => "room.mark_read",
            Action::RoomMute => "room.mute",
            Action::RoomUnmute => "room.unmute",
            Action::RoomJoin { .. } => "room.join",
            Action::MessageReply => "message.reply",
            Action::MessageEdit => "message.edit",
            Action::MessageDelete => "message.delete",
            Action::MessageYank => "message.yank",
            Action::MessageReact(_) => "message.react",
            Action::MessageOpen => "message.open",
            Action::MessageGotoReply => "message.goto_reply",
            Action::MessageResend => "message.resend",
            Action::MessageView => "message.view",
            Action::ComposerFocus(_) => "composer.focus",
            Action::ComposerSend => "composer.send",
            Action::ComposerNewline => "composer.newline",
            Action::ComposerCancel => "composer.cancel",
            Action::ComposerLeave => "composer.leave",
            Action::ComposerExternalEditor => "composer.external_editor",
            Action::CompletionNext => "completion.next",
            Action::CompletionPrev => "completion.prev",
            Action::CompletionAccept => "completion.accept",
            Action::CompletionCancel => "completion.cancel",
            Action::PickerRooms(_) => "picker.rooms",
            Action::PickerNext => "picker.next",
            Action::PickerPrev => "picker.prev",
            Action::PickerAccept => "picker.accept",
            Action::PickerCancel => "picker.cancel",
            Action::CommandLineOpen => "command_line.open",
            Action::CommandLineExecute => "command_line.execute",
            Action::CommandLineCancel => "command_line.cancel",
            Action::CommandLineComplete => "command_line.complete",
            Action::CursorDown => "cursor.down",
            Action::CursorUp => "cursor.up",
            Action::CursorFirst => "cursor.first",
            Action::CursorLast => "cursor.last",
            Action::CursorHalfPageDown => "cursor.half_page_down",
            Action::CursorHalfPageUp => "cursor.half_page_up",
            Action::SearchStart => "search.start",
            Action::SearchNext => "search.next",
            Action::SearchPrev => "search.prev",
            Action::ViewClose => "view.close",
            Action::HelpCommands => "help.commands",
            Action::HelpKeymap => "help.keymap",
            Action::StatusMessages => "status.messages",
            Action::SidebarToggle => "sidebar.toggle",
            Action::FocusRoomList => "focus.room_list",
            Action::FocusTimeline => "focus.timeline",
            Action::FileUpload { .. } => "file.upload",
            Action::AccountRecover => "account.recover",
            Action::AppQuit => "app.quit",
            Action::Nop => "nop",
        }
    }

    /// The arguments [`Action::from_spec`] builds this action from, as
    /// names and string values: those at their default are left out, the
    /// others follow in the order `filter`, `pick`, `query`, `mode`, `path`,
    /// `emoji`, `target`. An emoji is the emoji itself, not a shortcode.
    pub fn arguments(&self) -> Vec<(&'static str, &str)> {
        let filter = |filter: RoomFilter| {
            (filter != RoomFilter::default()).then(|| ("filter", filter.config_name()))
        };
        match self {
            Action::RoomOpen(RoomTarget::Pick { filter: f, pick }) => {
                let pick = (*pick != Pick::default()).then(|| ("pick", pick.config_name()));
                filter(*f).into_iter().chain(pick).collect()
            }
            Action::RoomOpen(RoomTarget::Fuzzy { filter: f, query }) => filter(*f)
                .into_iter()
                .chain([("query", query.as_str())])
                .collect(),
            Action::PickerRooms(f) => filter(*f).into_iter().collect(),
            Action::ComposerFocus(mode) if *mode != FocusMode::default() => {
                vec![("mode", mode.config_name())]
            }
            Action::FileUpload { path } => vec![("path", path.as_str())],
            Action::MessageReact(Some(emoji)) => vec![("emoji", emoji.as_str())],
            Action::RoomJoin { target } => vec![("target", target.as_str())],
            _ => Vec::new(),
        }
    }

    /// Movement actions that take a count prefix: cursor.down, cursor.up,
    /// cursor.half_page_down, cursor.half_page_up.
    pub fn takes_count(&self) -> bool {
        matches!(
            self,
            Action::CursorDown
                | Action::CursorUp
                | Action::CursorHalfPageDown
                | Action::CursorHalfPageUp
        )
    }
}

fn room_open(args: Args<'_>) -> Result<Action, ActionError> {
    if args.has("pick") && args.has("query") {
        return Err(ActionError::Conflict {
            action: args.action,
            args: ["pick", "query"],
        });
    }
    let filter = args.filter()?;
    let target = match args.non_empty_string("query")? {
        Some(query) => RoomTarget::Fuzzy {
            filter,
            query: query.to_owned(),
        },
        None => {
            let pick = args.choice("pick", &Pick::ALL, Pick::config_name)?;
            RoomTarget::Pick {
                filter,
                pick: pick.unwrap_or_default(),
            }
        }
    };
    Ok(Action::RoomOpen(target))
}

fn file_upload(args: Args<'_>) -> Result<Action, ActionError> {
    let path = args
        .non_empty_string("path")?
        .ok_or(ActionError::MissingArgument {
            action: args.action,
            arg: "path",
        })?;
    Ok(Action::FileUpload {
        path: path.to_owned(),
    })
}

fn room_join(args: Args<'_>) -> Result<Action, ActionError> {
    let target = args
        .non_empty_string("target")?
        .ok_or(ActionError::MissingArgument {
            action: args.action,
            arg: "target",
        })?;
    Ok(Action::RoomJoin {
        target: target.to_owned(),
    })
}

/// An empty `emoji` is [`ActionError::Empty`] and an unknown one
/// [`ActionError::UnknownEmoji`], so a binding never reacts with something no
/// client shows as an emoji.
fn message_react(args: Args<'_>) -> Result<Action, ActionError> {
    let Some(value) = args.non_empty_string("emoji")? else {
        return Ok(Action::MessageReact(None));
    };
    match Emoji::parse(value) {
        Some(emoji) => Ok(Action::MessageReact(Some(emoji))),
        None => Err(ActionError::UnknownEmoji {
            action: args.action,
            value: value.to_owned(),
        }),
    }
}

/// The arguments of one action, checked against the names it accepts.
struct Args<'a> {
    action: &'static str,
    args: &'a [(String, ArgValue)],
}

impl<'a> Args<'a> {
    /// Rejects arguments that `action` does not accept.
    fn new(
        action: &'static str,
        accepted: &[&str],
        args: &'a [(String, ArgValue)],
    ) -> Result<Self, ActionError> {
        match args
            .iter()
            .find(|(arg, _)| !accepted.contains(&arg.as_str()))
        {
            Some((arg, _)) => Err(ActionError::UnknownArgument {
                action,
                arg: arg.clone(),
            }),
            None => Ok(Args { action, args }),
        }
    }

    fn has(&self, arg: &str) -> bool {
        self.args.iter().any(|(name, _)| name == arg)
    }

    fn string(&self, arg: &'static str) -> Result<Option<&'a str>, ActionError> {
        match self.args.iter().rev().find(|(name, _)| name == arg) {
            None => Ok(None),
            Some((_, ArgValue::String(value))) => Ok(Some(value)),
            Some(_) => Err(ActionError::WrongType {
                action: self.action,
                arg,
                expected: "string",
            }),
        }
    }

    /// A string argument that must not be empty when it is given.
    fn non_empty_string(&self, arg: &'static str) -> Result<Option<&'a str>, ActionError> {
        match self.string(arg)? {
            Some("") => Err(ActionError::Empty {
                action: self.action,
                arg,
            }),
            value => Ok(value),
        }
    }

    /// A string argument with a fixed set of values, each named by `name`.
    fn choice<T: Copy>(
        &self,
        arg: &'static str,
        values: &[T],
        name: fn(T) -> &'static str,
    ) -> Result<Option<T>, ActionError> {
        let Some(value) = self.string(arg)? else {
            return Ok(None);
        };
        match values.iter().copied().find(|known| name(*known) == value) {
            Some(known) => Ok(Some(known)),
            None => Err(ActionError::UnknownValue {
                action: self.action,
                arg,
                value: value.to_owned(),
                allowed: values.iter().copied().map(name).collect(),
            }),
        }
    }

    fn filter(&self) -> Result<RoomFilter, ActionError> {
        let filter = self.choice("filter", &RoomFilter::ALL, RoomFilter::config_name)?;
        Ok(filter.unwrap_or_default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    UnknownAction {
        name: String,
    },
    UnknownArgument {
        action: &'static str,
        arg: String,
    },
    WrongType {
        action: &'static str,
        arg: &'static str,
        expected: &'static str,
    },
    /// A value outside the fixed set of values `arg` takes.
    UnknownValue {
        action: &'static str,
        arg: &'static str,
        value: String,
        allowed: Vec<&'static str>,
    },
    /// An empty string where a value is required.
    Empty {
        action: &'static str,
        arg: &'static str,
    },
    Conflict {
        action: &'static str,
        args: [&'static str; 2],
    },
    MissingArgument {
        action: &'static str,
        arg: &'static str,
    },
    /// An `emoji` that is neither an emoji nor a shortcode.
    UnknownEmoji {
        action: &'static str,
        value: String,
    },
}

impl ActionError {
    /// Whether the error names something this version does not know: an
    /// action, an argument, a value of an argument with fixed values or an
    /// emoji, which a newer version may know. The configuration ignores such
    /// a binding with a warning (design, "Layers").
    pub fn is_unknown_name(&self) -> bool {
        matches!(
            self,
            ActionError::UnknownAction { .. }
                | ActionError::UnknownArgument { .. }
                | ActionError::UnknownValue { .. }
                | ActionError::UnknownEmoji { .. }
        )
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionError::UnknownAction { name } => write!(f, "unknown action \"{name}\""),
            ActionError::UnknownArgument { action, arg } => {
                write!(f, "{action}: unknown argument \"{arg}\"")
            }
            ActionError::WrongType {
                action,
                arg,
                expected,
            } => write!(f, "{action}: \"{arg}\" must be a {expected}"),
            ActionError::UnknownValue {
                action,
                arg,
                value,
                allowed,
            } => {
                write!(f, "{action}: unknown \"{arg}\" value \"{value}\"")?;
                for (index, name) in allowed.iter().enumerate() {
                    let separator = if index == 0 { "; expected one of" } else { "," };
                    write!(f, "{separator} \"{name}\"")?;
                }
                Ok(())
            }
            ActionError::Empty { action, arg } => {
                write!(f, "{action}: \"{arg}\" must not be empty")
            }
            ActionError::Conflict {
                action,
                args: [first, second],
            } => write!(
                f,
                "{action}: \"{first}\" and \"{second}\" cannot be used together"
            ),
            ActionError::MissingArgument { action, arg } => {
                write!(f, "{action}: \"{arg}\" is required")
            }
            ActionError::UnknownEmoji { action, value } => write!(
                f,
                "{action}: unknown emoji \"{value}\"; expected an emoji or a shortcode, \
                 such as \"👍\", \"thumbsup\" or \":+1:\""
            ),
        }
    }
}

impl std::error::Error for ActionError {}

/// One or more actions run in order. Never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding(Vec<Action>);

impl Binding {
    /// `None` for an empty list.
    pub fn new(actions: Vec<Action>) -> Option<Binding> {
        (!actions.is_empty()).then_some(Binding(actions))
    }

    pub fn single(action: Action) -> Binding {
        Binding(vec![action])
    }

    pub fn actions(&self) -> &[Action] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::{Pick, RoomFilter, RoomTarget};

    fn s(k: &str, v: &str) -> (String, ArgValue) {
        (k.to_owned(), ArgValue::String(v.to_owned()))
    }

    #[test]
    fn actions_without_arguments() {
        assert_eq!(
            Action::from_spec("cursor.down", &[]),
            Ok(Action::CursorDown)
        );
        assert_eq!(Action::from_spec("nop", &[]), Ok(Action::Nop));
        assert_eq!(Action::from_spec("app.quit", &[]), Ok(Action::AppQuit));
    }

    #[test]
    fn every_action_name_round_trips() {
        let names = [
            "room.open",
            "room.open_selected",
            "room.mark_read",
            "room.mute",
            "room.unmute",
            "message.reply",
            "message.edit",
            "message.delete",
            "message.yank",
            "message.react",
            "message.open",
            "message.goto_reply",
            "message.resend",
            "message.view",
            "composer.focus",
            "composer.send",
            "composer.newline",
            "composer.cancel",
            "composer.leave",
            "composer.external_editor",
            "completion.next",
            "completion.prev",
            "completion.accept",
            "completion.cancel",
            "picker.rooms",
            "picker.next",
            "picker.prev",
            "picker.accept",
            "picker.cancel",
            "command_line.open",
            "command_line.execute",
            "command_line.cancel",
            "command_line.complete",
            "cursor.down",
            "cursor.up",
            "cursor.first",
            "cursor.last",
            "cursor.half_page_down",
            "cursor.half_page_up",
            "search.start",
            "search.next",
            "search.prev",
            "view.close",
            "help.commands",
            "help.keymap",
            "status.messages",
            "sidebar.toggle",
            "focus.room_list",
            "focus.timeline",
            "account.recover",
            "app.quit",
            "nop",
        ];
        for name in names {
            let action = Action::from_spec(name, &[]).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(action.name(), name);
        }
        let upload = Action::from_spec("file.upload", &[s("path", "/tmp/a b.png")]).unwrap();
        assert_eq!(upload.name(), "file.upload");
        let join = Action::from_spec("room.join", &[s("target", "#mxt:example.org")]).unwrap();
        assert_eq!(join.name(), "room.join");
    }

    /// `arguments()` as the configuration would give them to `from_spec`.
    fn spec(action: &Action) -> Vec<(String, ArgValue)> {
        action
            .arguments()
            .into_iter()
            .map(|(name, value)| s(name, value))
            .collect()
    }

    #[test]
    fn arguments_leave_defaults_out_and_build_the_same_action() {
        let cases = [
            (Action::from_spec("room.open", &[]), vec![]),
            (
                Action::from_spec("room.open", &[s("pick", "prev"), s("filter", "notify")]),
                vec![("filter", "notify"), ("pick", "prev")],
            ),
            (
                Action::from_spec("room.open", &[s("query", "a \"b\""), s("filter", "all")]),
                vec![("query", "a \"b\"")],
            ),
            (
                Action::from_spec("picker.rooms", &[s("filter", "all")]),
                vec![],
            ),
            (
                Action::from_spec("composer.focus", &[s("mode", "insert")]),
                vec![],
            ),
            (
                Action::from_spec("composer.focus", &[s("mode", "keep")]),
                vec![("mode", "keep")],
            ),
            (
                Action::from_spec("file.upload", &[s("path", "/tmp/a b.png")]),
                vec![("path", "/tmp/a b.png")],
            ),
            (Action::from_spec("message.react", &[]), vec![]),
            (
                Action::from_spec("message.react", &[s("emoji", "thumbsup")]),
                vec![("emoji", "👍")],
            ),
            (
                Action::from_spec("room.join", &[s("target", "#mxt:example.org")]),
                vec![("target", "#mxt:example.org")],
            ),
        ];
        for (action, arguments) in cases {
            let action = action.unwrap();
            assert_eq!(action.arguments(), arguments, "{action:?}");
            assert_eq!(
                Action::from_spec(action.name(), &spec(&action)),
                Ok(action.clone())
            );
        }
    }

    #[test]
    fn room_open_arguments() {
        assert_eq!(
            Action::from_spec("room.open", &[s("filter", "unread"), s("pick", "next")]),
            Ok(Action::RoomOpen(RoomTarget::Pick {
                filter: RoomFilter::Unread,
                pick: Pick::Next
            }))
        );
        assert_eq!(
            Action::from_spec("room.open", &[]),
            Ok(Action::RoomOpen(RoomTarget::Pick {
                filter: RoomFilter::All,
                pick: Pick::First
            }))
        );
        assert_eq!(
            Action::from_spec("room.open", &[s("query", "general")]),
            Ok(Action::RoomOpen(RoomTarget::Fuzzy {
                filter: RoomFilter::All,
                query: "general".into()
            }))
        );
    }

    #[test]
    fn pick_and_query_conflict() {
        assert_eq!(
            Action::from_spec("room.open", &[s("pick", "next"), s("query", "x")]),
            Err(ActionError::Conflict {
                action: "room.open",
                args: ["pick", "query"]
            })
        );
    }

    #[test]
    fn rejects_unknown_actions_and_wrong_arguments() {
        assert_eq!(
            Action::from_spec("room.opne", &[]),
            Err(ActionError::UnknownAction {
                name: "room.opne".into()
            })
        );
        assert_eq!(
            Action::from_spec("cursor.down", &[s("filter", "all")]),
            Err(ActionError::UnknownArgument {
                action: "cursor.down",
                arg: "filter".into()
            })
        );
        assert_eq!(
            Action::from_spec("room.open", &[("pick".into(), ArgValue::Integer(1))]),
            Err(ActionError::WrongType {
                action: "room.open",
                arg: "pick",
                expected: "string"
            })
        );
        let unknown_filter = Action::from_spec("room.open", &[s("filter", "unreads")]).unwrap_err();
        assert_eq!(
            unknown_filter.to_string(),
            "room.open: unknown \"filter\" value \"unreads\"; \
             expected one of \"all\", \"unread\", \"notify\", \"mentions\""
        );
        let unknown_mode = Action::from_spec("composer.focus", &[s("mode", "append")]);
        assert_eq!(
            unknown_mode,
            Err(ActionError::UnknownValue {
                action: "composer.focus",
                arg: "mode",
                value: "append".into(),
                allowed: vec!["insert", "keep"]
            })
        );
        assert_eq!(
            Action::from_spec("file.upload", &[]),
            Err(ActionError::MissingArgument {
                action: "file.upload",
                arg: "path"
            })
        );
        assert_eq!(
            Action::from_spec("room.join", &[]),
            Err(ActionError::MissingArgument {
                action: "room.join",
                arg: "target"
            })
        );
    }

    #[test]
    fn empty_strings_are_rejected() {
        let empty_query = Action::from_spec("room.open", &[s("query", "")]).unwrap_err();
        assert_eq!(
            empty_query,
            ActionError::Empty {
                action: "room.open",
                arg: "query"
            }
        );
        assert_eq!(
            empty_query.to_string(),
            "room.open: \"query\" must not be empty"
        );
        assert_eq!(
            Action::from_spec("file.upload", &[s("path", "")]),
            Err(ActionError::Empty {
                action: "file.upload",
                arg: "path"
            })
        );
        assert_eq!(
            Action::from_spec("message.react", &[s("emoji", "")]),
            Err(ActionError::Empty {
                action: "message.react",
                arg: "emoji"
            })
        );
        assert_eq!(
            Action::from_spec("room.join", &[s("target", "")]),
            Err(ActionError::Empty {
                action: "room.join",
                arg: "target"
            })
        );
    }

    #[test]
    fn values_of_other_toml_types_are_checked_like_any_other_value() {
        let unsupported = |name: &str| (name.to_owned(), ArgValue::Unsupported);
        assert_eq!(
            Action::from_spec("message.future", &[unsupported("ids")]),
            Err(ActionError::UnknownAction {
                name: "message.future".into()
            })
        );
        assert_eq!(
            Action::from_spec("room.open", &[unsupported("ratio")]),
            Err(ActionError::UnknownArgument {
                action: "room.open",
                arg: "ratio".into()
            })
        );
        assert_eq!(
            Action::from_spec("room.open", &[unsupported("filter")]),
            Err(ActionError::WrongType {
                action: "room.open",
                arg: "filter",
                expected: "string"
            })
        );
    }

    #[test]
    fn the_first_check_in_the_design_order_decides() {
        // Argument names come before conflicts.
        assert_eq!(
            Action::from_spec(
                "room.open",
                &[s("sort", "name"), s("pick", "next"), s("query", "x")]
            ),
            Err(ActionError::UnknownArgument {
                action: "room.open",
                arg: "sort".into()
            })
        );
        // Conflicts come before values.
        assert_eq!(
            Action::from_spec(
                "room.open",
                &[s("pick", "next"), s("query", "x"), s("filter", "dm")]
            ),
            Err(ActionError::Conflict {
                action: "room.open",
                args: ["pick", "query"]
            })
        );
    }

    #[test]
    fn only_names_a_newer_version_may_know_are_unknown_names() {
        let unknown = [
            ActionError::UnknownAction { name: "x".into() },
            ActionError::UnknownArgument {
                action: "room.open",
                arg: "sort".into(),
            },
            ActionError::UnknownValue {
                action: "room.open",
                arg: "filter",
                value: "dm".into(),
                allowed: vec!["all"],
            },
            ActionError::UnknownEmoji {
                action: "message.react",
                value: "nonsense".into(),
            },
        ];
        for error in unknown {
            assert!(error.is_unknown_name(), "{error:?}");
        }
        let known = [
            ActionError::WrongType {
                action: "room.open",
                arg: "filter",
                expected: "string",
            },
            ActionError::Empty {
                action: "room.open",
                arg: "query",
            },
            ActionError::Conflict {
                action: "room.open",
                args: ["pick", "query"],
            },
            ActionError::MissingArgument {
                action: "file.upload",
                arg: "path",
            },
        ];
        for error in known {
            assert!(!error.is_unknown_name(), "{error:?}");
        }
    }

    #[test]
    fn message_react_takes_an_emoji_or_a_shortcode() {
        let thumbs = Action::MessageReact(Emoji::get("👍"));
        for emoji in ["👍", "thumbsup", ":+1:"] {
            assert_eq!(
                Action::from_spec("message.react", &[s("emoji", emoji)]),
                Ok(thumbs.clone())
            );
        }
        assert_eq!(
            Action::from_spec("message.react", &[]),
            Ok(Action::MessageReact(None))
        );
    }

    #[test]
    fn an_unknown_emoji_is_rejected() {
        for emoji in ["nonsense", ":+1"] {
            let error = Action::from_spec("message.react", &[s("emoji", emoji)]).unwrap_err();
            assert_eq!(
                error,
                ActionError::UnknownEmoji {
                    action: "message.react",
                    value: emoji.into()
                }
            );
            let message = error.to_string();
            assert!(
                message.starts_with(&format!("message.react: unknown emoji \"{emoji}\"; ")),
                "{message}"
            );
        }
    }

    #[test]
    fn composer_focus_mode() {
        assert_eq!(
            Action::from_spec("composer.focus", &[]),
            Ok(Action::ComposerFocus(FocusMode::Insert))
        );
        assert_eq!(
            Action::from_spec("composer.focus", &[s("mode", "keep")]),
            Ok(Action::ComposerFocus(FocusMode::Keep))
        );
    }

    #[test]
    fn picker_filter() {
        assert_eq!(
            Action::from_spec("picker.rooms", &[s("filter", "mentions")]),
            Ok(Action::PickerRooms(RoomFilter::Mentions))
        );
    }

    #[test]
    fn counts_apply_to_line_movement_only() {
        assert!(Action::CursorDown.takes_count());
        assert!(Action::CursorHalfPageUp.takes_count());
        assert!(!Action::CursorFirst.takes_count());
        assert!(!Action::RoomMarkRead.takes_count());
    }

    #[test]
    fn empty_binding_is_rejected() {
        assert_eq!(Binding::new(vec![]), None);
        assert_eq!(
            Binding::new(vec![Action::Nop]).unwrap().actions(),
            &[Action::Nop]
        );
    }
}
