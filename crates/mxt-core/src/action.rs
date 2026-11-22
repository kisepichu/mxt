//! The action registry: every action keys and commands can run, built from a
//! name and arguments.

use std::fmt;
use std::num::NonZeroUsize;

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

/// An argument value of a built action, as [`Action::arguments`] gives it
/// back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgRef<'a> {
    String(&'a str),
    Integer(usize),
}

#[cfg(test)]
impl ArgRef<'_> {
    /// The value the configuration gives [`Action::from_spec`] for it.
    pub(crate) fn to_value(self) -> ArgValue {
        match self {
            ArgRef::String(value) => ArgValue::String(value.to_owned()),
            ArgRef::Integer(value) => ArgValue::Integer(i64::try_from(value).unwrap()),
        }
    }
}

/// What `message.react` toggles on the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactWith {
    /// Nothing yet: the emoji picker opens.
    Picker,
    /// The reaction with this emoji.
    Emoji(Emoji),
    /// The reaction shown at this place under the message, from 1, in the
    /// order of `TimelineItem::reactions`, which the timeline draws.
    Index(NonZeroUsize),
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
    /// Toggles a reaction, or opens the emoji picker.
    MessageReact(ReactWith),
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
    /// The shown timeline's composer goes to normal mode, as edtui's `<Esc>`
    /// leaves insert, visual and search mode.
    ComposerNormal,
    /// In insert mode, the shown timeline's composer deletes the word
    /// before the cursor with the blanks after it, as the line editor's
    /// `<C-w>` does; at the start of a line, the line break.
    ComposerDeleteWordBefore,
    /// Empties the shown timeline's composer as one change that `u` undoes,
    /// keeping its reply or edit.
    ComposerClear,
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
    /// The next older line of the line's history that starts with the text
    /// before the cursor (design, "Line editing").
    CommandLineHistoryPrev,
    /// The next newer match while browsing the history; past the newest,
    /// the text and cursor from before browsing.
    CommandLineHistoryNext,
    CursorDown,
    CursorUp,
    CursorFirst,
    CursorLast,
    CursorHalfPageDown,
    CursorHalfPageUp,
    SearchStart,
    SearchNext,
    SearchPrev,
    /// Hides the search highlight until the next search, as nvim's
    /// `:nohlsearch` does.
    SearchClearHighlight,
    /// To the older entry of the jump list, like nvim's `<C-o>`.
    JumpBack,
    /// To the newer entry of the jump list, like nvim's `<C-i>`.
    JumpForward,
    /// Shows the thread of the message under the timeline cursor in place
    /// of the main timeline.
    ThreadOpen,
    /// Back from the thread to the main timeline.
    ThreadClose,
    ViewClose,
    /// In the view editor's visual mode, back to normal mode, as vim's
    /// `<Esc>`; otherwise [`Action::ViewClose`].
    ViewLeave,
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
    ///   default `all`), and either `pick` (`first`, `next` or `prev`, see
    ///   [`Pick`]: none picks the open room; default `first`) or `query` (a
    ///   fuzzy room name query, non-empty)
    /// - `picker.rooms`: `filter`, as for `room.open`
    /// - `composer.focus`: `mode` (`insert` or `keep`; default `insert`)
    /// - `file.upload`: `path` (non-empty), required
    /// - `message.react`: either `emoji` (an emoji or a shortcode, see
    ///   [`Emoji::parse`]; non-empty) or `index` (a positive integer)
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
                return message_react(Args::new("message.react", &["emoji", "index"], args)?);
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
            "composer.normal" => Action::ComposerNormal,
            "composer.delete_word_before" => Action::ComposerDeleteWordBefore,
            "composer.clear" => Action::ComposerClear,
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
            "command_line.history_prev" => Action::CommandLineHistoryPrev,
            "command_line.history_next" => Action::CommandLineHistoryNext,
            "cursor.down" => Action::CursorDown,
            "cursor.up" => Action::CursorUp,
            "cursor.first" => Action::CursorFirst,
            "cursor.last" => Action::CursorLast,
            "cursor.half_page_down" => Action::CursorHalfPageDown,
            "cursor.half_page_up" => Action::CursorHalfPageUp,
            "search.start" => Action::SearchStart,
            "search.next" => Action::SearchNext,
            "search.prev" => Action::SearchPrev,
            "search.clear_highlight" => Action::SearchClearHighlight,
            "jump.back" => Action::JumpBack,
            "jump.forward" => Action::JumpForward,
            "thread.open" => Action::ThreadOpen,
            "thread.close" => Action::ThreadClose,
            "view.close" => Action::ViewClose,
            "view.leave" => Action::ViewLeave,
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
            Action::ComposerNormal => "composer.normal",
            Action::ComposerDeleteWordBefore => "composer.delete_word_before",
            Action::ComposerClear => "composer.clear",
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
            Action::CommandLineHistoryPrev => "command_line.history_prev",
            Action::CommandLineHistoryNext => "command_line.history_next",
            Action::CursorDown => "cursor.down",
            Action::CursorUp => "cursor.up",
            Action::CursorFirst => "cursor.first",
            Action::CursorLast => "cursor.last",
            Action::CursorHalfPageDown => "cursor.half_page_down",
            Action::CursorHalfPageUp => "cursor.half_page_up",
            Action::SearchStart => "search.start",
            Action::SearchNext => "search.next",
            Action::SearchPrev => "search.prev",
            Action::SearchClearHighlight => "search.clear_highlight",
            Action::JumpBack => "jump.back",
            Action::JumpForward => "jump.forward",
            Action::ThreadOpen => "thread.open",
            Action::ThreadClose => "thread.close",
            Action::ViewClose => "view.close",
            Action::ViewLeave => "view.leave",
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
    /// names and values: those at their default are left out, the others
    /// follow in the order `filter`, `pick`, `query`, `mode`, `path`,
    /// `emoji`, `index`, `target`. An emoji is the emoji itself, not a
    /// shortcode.
    pub fn arguments(&self) -> Vec<(&'static str, ArgRef<'_>)> {
        let filter = |filter: RoomFilter| {
            (filter != RoomFilter::default())
                .then(|| ("filter", ArgRef::String(filter.config_name())))
        };
        match self {
            Action::RoomOpen(RoomTarget::Pick { filter: f, pick }) => {
                let pick = (*pick != Pick::default())
                    .then(|| ("pick", ArgRef::String(pick.config_name())));
                filter(*f).into_iter().chain(pick).collect()
            }
            Action::RoomOpen(RoomTarget::Fuzzy { filter: f, query }) => filter(*f)
                .into_iter()
                .chain([("query", ArgRef::String(query))])
                .collect(),
            Action::PickerRooms(f) => filter(*f).into_iter().collect(),
            Action::ComposerFocus(mode) if *mode != FocusMode::default() => {
                vec![("mode", ArgRef::String(mode.config_name()))]
            }
            Action::FileUpload { path } => vec![("path", ArgRef::String(path))],
            Action::MessageReact(ReactWith::Emoji(emoji)) => {
                vec![("emoji", ArgRef::String(emoji.as_str()))]
            }
            Action::MessageReact(ReactWith::Index(index)) => {
                vec![("index", ArgRef::Integer(index.get()))]
            }
            Action::RoomJoin { target } => vec![("target", ArgRef::String(target))],
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

/// `emoji` and `index` are [`ActionError::Conflict`]. An empty `emoji` is
/// [`ActionError::Empty`] and an unknown one [`ActionError::UnknownEmoji`],
/// so a binding never reacts with something no client shows as an emoji.
fn message_react(args: Args<'_>) -> Result<Action, ActionError> {
    if args.has("emoji") && args.has("index") {
        return Err(ActionError::Conflict {
            action: args.action,
            args: ["emoji", "index"],
        });
    }
    if let Some(index) = args.positive("index")? {
        return Ok(Action::MessageReact(ReactWith::Index(index)));
    }
    let Some(value) = args.non_empty_string("emoji")? else {
        return Ok(Action::MessageReact(ReactWith::Picker));
    };
    match Emoji::parse(value) {
        Some(emoji) => Ok(Action::MessageReact(ReactWith::Emoji(emoji))),
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

    /// An integer argument of 1 or more. Any other value, 0 and negative
    /// integers included, is [`ActionError::WrongType`].
    fn positive(&self, arg: &'static str) -> Result<Option<NonZeroUsize>, ActionError> {
        let value = match self.args.iter().rev().find(|(name, _)| name == arg) {
            None => return Ok(None),
            Some((_, ArgValue::Integer(value))) => {
                usize::try_from(*value).ok().and_then(NonZeroUsize::new)
            }
            Some(_) => None,
        };
        match value {
            Some(value) => Ok(Some(value)),
            None => Err(ActionError::WrongType {
                action: self.action,
                arg,
                expected: "positive integer",
            }),
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
            "composer.normal",
            "composer.delete_word_before",
            "composer.clear",
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
            "command_line.history_prev",
            "command_line.history_next",
            "cursor.down",
            "cursor.up",
            "cursor.first",
            "cursor.last",
            "cursor.half_page_down",
            "cursor.half_page_up",
            "search.start",
            "search.next",
            "search.prev",
            "search.clear_highlight",
            "jump.back",
            "jump.forward",
            "thread.open",
            "thread.close",
            "view.close",
            "view.leave",
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
            .map(|(name, value)| (name.to_owned(), value.to_value()))
            .collect()
    }

    fn int(k: &str, v: i64) -> (String, ArgValue) {
        (k.to_owned(), ArgValue::Integer(v))
    }

    #[test]
    fn arguments_leave_defaults_out_and_build_the_same_action() {
        let text = ArgRef::String;
        let cases = [
            (Action::from_spec("room.open", &[]), vec![]),
            (
                Action::from_spec("room.open", &[s("pick", "prev"), s("filter", "notify")]),
                vec![("filter", text("notify")), ("pick", text("prev"))],
            ),
            (
                Action::from_spec("room.open", &[s("query", "a \"b\""), s("filter", "all")]),
                vec![("query", text("a \"b\""))],
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
                vec![("mode", text("keep"))],
            ),
            (
                Action::from_spec("file.upload", &[s("path", "/tmp/a b.png")]),
                vec![("path", text("/tmp/a b.png"))],
            ),
            (Action::from_spec("message.react", &[]), vec![]),
            (
                Action::from_spec("message.react", &[s("emoji", "thumbsup")]),
                vec![("emoji", text("👍"))],
            ),
            (
                Action::from_spec("message.react", &[int("index", 3)]),
                vec![("index", ArgRef::Integer(3))],
            ),
            (
                Action::from_spec("room.join", &[s("target", "#mxt:example.org")]),
                vec![("target", text("#mxt:example.org"))],
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
        let thumbs = Action::MessageReact(ReactWith::Emoji(Emoji::get("👍").unwrap()));
        for emoji in ["👍", "thumbsup", ":+1:"] {
            assert_eq!(
                Action::from_spec("message.react", &[s("emoji", emoji)]),
                Ok(thumbs.clone())
            );
        }
        assert_eq!(
            Action::from_spec("message.react", &[]),
            Ok(Action::MessageReact(ReactWith::Picker))
        );
    }

    #[test]
    fn message_react_takes_a_positive_index() {
        for n in [1, 2, 12] {
            let index = NonZeroUsize::new(usize::try_from(n).unwrap()).unwrap();
            assert_eq!(
                Action::from_spec("message.react", &[int("index", n)]),
                Ok(Action::MessageReact(ReactWith::Index(index)))
            );
        }
        let not_positive = [int("index", 0), int("index", -1), s("index", "1")];
        for arg in not_positive {
            let error = Action::from_spec("message.react", std::slice::from_ref(&arg)).unwrap_err();
            assert_eq!(
                error,
                ActionError::WrongType {
                    action: "message.react",
                    arg: "index",
                    expected: "positive integer"
                },
                "{arg:?}"
            );
            assert_eq!(
                error.to_string(),
                "message.react: \"index\" must be a positive integer"
            );
        }
    }

    #[test]
    fn index_and_emoji_conflict() {
        // Before the values are read: neither value is valid here.
        let error =
            Action::from_spec("message.react", &[int("index", 0), s("emoji", "")]).unwrap_err();
        assert_eq!(
            error,
            ActionError::Conflict {
                action: "message.react",
                args: ["emoji", "index"]
            }
        );
        assert!(!error.is_unknown_name());
        assert_eq!(
            error.to_string(),
            "message.react: \"emoji\" and \"index\" cannot be used together"
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
