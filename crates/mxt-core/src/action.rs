//! The action registry: every action keys and commands can run, built from a
//! name and arguments.

use std::fmt;

use crate::rooms::{Pick, RoomFilter, RoomTarget};

/// A neutral argument value; the configuration loader converts TOML values into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

/// The editor mode `composer.focus` leaves the composer in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FocusMode {
    #[default]
    Insert,
    /// The editor's previous mode.
    Keep,
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
    MessageReply,
    MessageEdit,
    MessageDelete,
    MessageYank,
    MessageReact,
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
    ///
    /// Argument names are unique in a configuration table; if one repeats
    /// anyway, its last value counts.
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
                let mode = args.choice("mode", &MODE_NAMES, MODES)?;
                return Ok(Action::ComposerFocus(mode.unwrap_or_default()));
            }
            "file.upload" => return file_upload(Args::new("file.upload", &["path"], args)?),
            "room.open_selected" => Action::RoomOpenSelected,
            "room.mark_read" => Action::RoomMarkRead,
            "room.mute" => Action::RoomMute,
            "room.unmute" => Action::RoomUnmute,
            "message.reply" => Action::MessageReply,
            "message.edit" => Action::MessageEdit,
            "message.delete" => Action::MessageDelete,
            "message.yank" => Action::MessageYank,
            "message.react" => Action::MessageReact,
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
            Action::MessageReply => "message.reply",
            Action::MessageEdit => "message.edit",
            Action::MessageDelete => "message.delete",
            Action::MessageYank => "message.yank",
            Action::MessageReact => "message.react",
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
    /// others follow in the order `filter`, `pick`, `query`, `mode`, `path`.
    pub fn arguments(&self) -> Vec<(&'static str, &str)> {
        let filter = |filter: RoomFilter| {
            (filter != RoomFilter::default())
                .then(|| ("filter", name_of(&FILTER_NAMES, &FILTERS, filter)))
        };
        match self {
            Action::RoomOpen(RoomTarget::Pick { filter: f, pick }) => {
                let pick = (*pick != Pick::default())
                    .then(|| ("pick", name_of(&PICK_NAMES, &PICKS, *pick)));
                filter(*f).into_iter().chain(pick).collect()
            }
            Action::RoomOpen(RoomTarget::Fuzzy { filter: f, query }) => filter(*f)
                .into_iter()
                .chain([("query", query.as_str())])
                .collect(),
            Action::PickerRooms(f) => filter(*f).into_iter().collect(),
            Action::ComposerFocus(mode) if *mode != FocusMode::default() => {
                vec![("mode", name_of(&MODE_NAMES, &MODES, *mode))]
            }
            Action::FileUpload { path } => vec![("path", path.as_str())],
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

/// The values of `filter`, `pick` and `mode`: `*_NAMES[i]` stands for the
/// value at `i`.
const FILTER_NAMES: [&str; 4] = ["all", "unread", "notify", "mentions"];
const FILTERS: [RoomFilter; 4] = [
    RoomFilter::All,
    RoomFilter::Unread,
    RoomFilter::Notify,
    RoomFilter::Mentions,
];
const PICK_NAMES: [&str; 3] = ["first", "next", "prev"];
const PICKS: [Pick; 3] = [Pick::First, Pick::Next, Pick::Prev];
const MODE_NAMES: [&str; 2] = ["insert", "keep"];
const MODES: [FocusMode; 2] = [FocusMode::Insert, FocusMode::Keep];

/// The name of `value`, the inverse of [`Args::choice`].
///
/// # Panics
///
/// If `values` lacks `value`; the tables above list every value.
fn name_of<T: PartialEq, const N: usize>(
    names: &[&'static str; N],
    values: &[T; N],
    value: T,
) -> &'static str {
    let index = values.iter().position(|known| *known == value);
    names[index.expect("the value tables list every value")]
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
            let pick = args.choice("pick", &PICK_NAMES, PICKS)?;
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
            Some("") => Err(ActionError::InvalidValue {
                action: self.action,
                arg,
                value: String::new(),
                allowed: &[],
            }),
            value => Ok(value),
        }
    }

    /// A string argument with a fixed set of values: `names[i]` stands for
    /// `values[i]`.
    fn choice<T: Copy, const N: usize>(
        &self,
        arg: &'static str,
        names: &'static [&'static str; N],
        values: [T; N],
    ) -> Result<Option<T>, ActionError> {
        let Some(value) = self.string(arg)? else {
            return Ok(None);
        };
        match names.iter().position(|name| *name == value) {
            Some(index) => Ok(Some(values[index])),
            None => Err(ActionError::InvalidValue {
                action: self.action,
                arg,
                value: value.to_owned(),
                allowed: names,
            }),
        }
    }

    fn filter(&self) -> Result<RoomFilter, ActionError> {
        let filter = self.choice("filter", &FILTER_NAMES, FILTERS)?;
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
    /// `allowed` is empty when any non-empty value is allowed; `value` is
    /// then the empty string.
    InvalidValue {
        action: &'static str,
        arg: &'static str,
        value: String,
        allowed: &'static [&'static str],
    },
    Conflict {
        action: &'static str,
        args: [&'static str; 2],
    },
    MissingArgument {
        action: &'static str,
        arg: &'static str,
    },
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
            ActionError::InvalidValue {
                action,
                arg,
                value,
                allowed,
            } => {
                if allowed.is_empty() {
                    return write!(f, "{action}: \"{arg}\" must not be empty");
                }
                write!(f, "{action}: invalid \"{arg}\" value \"{value}\"")?;
                for (index, name) in allowed.iter().enumerate() {
                    let separator = if index == 0 { "; expected one of" } else { "," };
                    write!(f, "{separator} \"{name}\"")?;
                }
                Ok(())
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
        assert!(matches!(
            Action::from_spec("room.open", &[s("filter", "unreads")]),
            Err(ActionError::InvalidValue {
                action: "room.open",
                arg: "filter",
                ..
            })
        ));
        assert_eq!(
            Action::from_spec("file.upload", &[]),
            Err(ActionError::MissingArgument {
                action: "file.upload",
                arg: "path"
            })
        );
    }

    #[test]
    fn empty_strings_are_rejected() {
        let empty_query = Action::from_spec("room.open", &[s("query", "")]).unwrap_err();
        assert_eq!(
            empty_query,
            ActionError::InvalidValue {
                action: "room.open",
                arg: "query",
                value: String::new(),
                allowed: &[]
            }
        );
        assert!(
            empty_query.to_string().contains("must not be empty"),
            "{empty_query}"
        );
        assert!(matches!(
            Action::from_spec("file.upload", &[s("path", "")]),
            Err(ActionError::InvalidValue { arg: "path", .. })
        ));
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
