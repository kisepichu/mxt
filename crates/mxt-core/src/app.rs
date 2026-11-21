//! The application state and `update`: keys and adapter messages in, effects
//! out, without I/O.

mod command_line;
mod composer;
mod picker;
mod recovery;
mod rooms;
mod search;
#[cfg(test)]
mod tests;
mod timeline;

use std::collections::{HashMap, VecDeque};

use crate::action::{Action, Binding};
use crate::command::{COMMANDS, help_text};
use crate::effect::{Effect, MatrixEffect, RecoveryKey, UiEffect};
use crate::ids::{RoomId, SendId, UserId};
use crate::keymap::{InputContext, KeySink, Keymap, keymap_text};
use crate::keys::Key;
use crate::line::LineEditor;
use crate::msg::{EditorMode, Msg, Pane, SyncState};
use crate::resolver::{Resolved, Resolver, TimerId};
use crate::rooms::RoomList;
use crate::settings::Settings;
use crate::timeline::{ItemRef, TimelineEntry};

pub use command_line::{CommandLine, LineKind};
pub use picker::{Picker, PickerItem, PickerKey, PickerKind};
pub use recovery::SecretPrompt;

use command_line::OpenLine;
use composer::{Composer, MentionMenu, PendingSend};
use picker::OpenPicker;
use search::Direction;
use timeline::Timeline;

/// The largest paste in bytes that goes where text would go: four times the
/// 64 KiB limit on a Matrix event (design, "Input routing").
const PASTE_LIMIT: usize = 256 * 1024;

/// How many status messages `:messages` keeps (design, "Help views").
const STATUS_HISTORY: usize = 200;

/// A paste up to its first line break, for the inputs that hold one line.
fn first_line(text: &str) -> &str {
    text.split_once('\n').map_or(text, |(line, _)| line)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    RoomList,
    Timeline,
    Composer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Error,
}

/// A message of the status line; the next key clears it from the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
}

/// The items a pane showed in its last render, inclusive.
#[derive(Clone, Copy, Debug)]
struct Viewport {
    first: usize,
    last: usize,
}

/// A cursor movement of `cursor.*`, with its count.
enum Motion {
    Down(u32),
    Up(u32),
    /// Half pages of the pane's last viewport.
    HalfPageDown(u32),
    HalfPageUp(u32),
    First,
    Last,
}

/// What the full-screen view shows (design, "Help views").
enum View {
    /// A message's text or image: the item under the cursor when it opened.
    Message(ItemRef),
    /// `:help`, `:map` or `:messages`: text the core generated when it
    /// opened.
    Help,
}

/// An action waiting for `y` in `CONFIRM` mode.
enum Confirm {
    Redact { room: RoomId, item: ItemRef },
}

impl Confirm {
    fn question(&self) -> &'static str {
        match self {
            Confirm::Redact { .. } => "Delete this message? (y/n)",
        }
    }
}

/// The UI state (design, "Domain model"), changed only by [`App::update`].
pub struct App {
    keymap: Keymap,
    settings: Settings,
    /// The user's own ID, for `OwnEvent` and mention ranking.
    me: UserId,
    resolver: Resolver,
    /// The input context of the last key, where the resolver's pending keys
    /// started.
    key_context: InputContext,
    /// The editor mode reported with the last key, for the keys resolved
    /// again after a binding, also after a timeout. `composer.focus` with
    /// insert mode sets it to insert, as the tui adapter enters insert mode
    /// for it. Other mode changes before the next key are unknown to the
    /// core, such as a released `a` in `composer.normal` that starts insert
    /// mode: keys resolved again after it still use the mode reported last.
    editor: Option<EditorMode>,
    rooms: RoomList,
    /// The room list cursor, once a cursor action, a search or opening a room
    /// placed it. Until then, and while the list is empty, it is `None` and
    /// the cursor is on the first room, whichever room that is, so rooms that
    /// arrive in any order at startup leave it at the top.
    room_cursor: Option<usize>,
    open_room: Option<RoomId>,
    /// The open room's timeline; empty while no room is open.
    timeline: Timeline,
    /// The full-screen view, while it is open.
    view: Option<View>,
    focus: Focus,
    sidebar_visible: bool,
    sync_state: SyncState,
    terminal_focused: bool,
    /// The status line's messages, oldest first, at most
    /// [`STATUS_HISTORY`], for `:messages`. The `CONFIRM` question and the
    /// `SECRET` prompt are not among them.
    status_history: VecDeque<StatusMessage>,
    /// Whether the newest of `status_history` is in the status line; the
    /// next key clears it.
    status_shown: bool,
    room_list_viewport: Option<Viewport>,
    /// The open room's, from a render of that room; `None` until then.
    timeline_viewport: Option<Viewport>,
    command_line: Option<OpenLine>,
    /// The pattern `search.next` and `search.prev` repeat.
    last_search: Option<String>,
    /// Per room, created when first used.
    composers: HashMap<RoomId, Composer>,
    /// The submitted drafts whose sends are not answered yet.
    pending_sends: HashMap<SendId, PendingSend>,
    /// The number of the next send's `SendId`.
    next_send: u64,
    mention_menu: Option<MentionMenu>,
    picker: Option<OpenPicker>,
    /// `CONFIRM` mode: the next key answers.
    confirm: Option<Confirm>,
    /// `SECRET` mode: the recovery key typed so far, and its cursor.
    secret: Option<LineEditor<RecoveryKey>>,
}

impl App {
    pub fn new(keymap: Keymap, settings: Settings, me: UserId) -> App {
        let (focus, key_context) = if settings.room_list_visible {
            (Focus::RoomList, InputContext::RoomList)
        } else {
            (Focus::Timeline, InputContext::Timeline)
        };
        App {
            keymap,
            me,
            resolver: Resolver::new(),
            key_context,
            editor: None,
            rooms: RoomList::new(settings.room_sort),
            room_cursor: None,
            open_room: None,
            timeline: Timeline::default(),
            view: None,
            focus,
            sidebar_visible: settings.room_list_visible,
            sync_state: SyncState::default(),
            terminal_focused: true,
            status_history: VecDeque::new(),
            status_shown: false,
            room_list_viewport: None,
            timeline_viewport: None,
            command_line: None,
            last_search: None,
            composers: HashMap::new(),
            pending_sends: HashMap::new(),
            next_send: 0,
            mention_menu: None,
            picker: None,
            confirm: None,
            secret: None,
            settings,
        }
    }

    /// Deterministic and free of I/O.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let mut effects = Vec::new();
        match msg {
            Msg::Key { key, editor } => self.key(key, editor, &mut effects),
            Msg::Paste { text, editor } => self.paste(text, editor, &mut effects),
            Msg::Timeout { id } => self.timeout(id, &mut effects),
            Msg::RoomUpserted(summary) => self.upsert_room(summary),
            Msg::RoomRemoved(id) => self.remove_room(id, &mut effects),
            Msg::SyncState(state) => self.sync_state = state,
            Msg::Timeline { room, diffs } => self.timeline_diffs(room, diffs, &mut effects),
            Msg::PaginationDone {
                room,
                reached_start,
            } => self.pagination_done(room, reached_start, &mut effects),
            Msg::PaginationFailed { room, error } => self.pagination_failed(room, error),
            Msg::Viewport { pane, first, last } => {
                let viewport = Some(Viewport { first, last });
                match pane {
                    Pane::RoomList => self.room_list_viewport = viewport,
                    Pane::Timeline => {
                        self.timeline_viewport = viewport;
                        self.check_receipt(&mut effects);
                    }
                }
            }
            Msg::TerminalFocus(focused) => {
                self.terminal_focused = focused;
                self.check_receipt(&mut effects);
            }
            Msg::EffectFailed { error } => self.set_status(StatusKind::Error, error),
            Msg::SendQueued { id } => self.send_queued(id),
            Msg::SendFailed { id, error } => self.send_failed(id, error, &mut effects),
            Msg::Submit { room, body } => self.submit(room, body, &mut effects),
            Msg::ComposerEdited { room, empty } => self.composer_edited(room, empty),
            Msg::MentionQuery { room, query } => self.mention_query(room, query),
            Msg::MembersLoaded { room, members } => self.members_loaded(room, members),
            Msg::Downloaded { path } => self.downloaded(path, &mut effects),
            Msg::ImageFetched { item, bytes } => self.image_fetched(item, bytes, &mut effects),
            Msg::ImageUnavailable { item, reason } => self.image_unavailable(&item, &reason),
            Msg::Recovered => self.recovered(),
            Msg::ConfigWarnings { warnings } => self.config_warnings(warnings),
        }
        effects
    }

    pub fn rooms(&self) -> &RoomList {
        &self.rooms
    }

    /// Index of the room list cursor in `rooms().rooms()`.
    pub fn room_cursor(&self) -> Option<usize> {
        self.room_cursor
            .or_else(|| (!self.rooms.is_empty()).then_some(0))
    }

    pub fn open_room(&self) -> Option<&RoomId> {
        self.open_room.as_ref()
    }

    /// The open room's entries, oldest first (empty when no room is open).
    pub fn timeline(&self) -> &[TimelineEntry] {
        self.timeline.entries()
    }

    /// Index of the timeline cursor in `timeline()`, always on an `Item`.
    pub fn timeline_cursor(&self) -> Option<usize> {
        self.timeline.cursor()
    }

    /// Whether the timeline follows the newest item.
    pub fn timeline_follows(&self) -> bool {
        self.timeline.follows()
    }

    /// The first and last timeline entries of the open room's last reported
    /// viewport; `None` until a render of that room reported one.
    pub fn timeline_viewport(&self) -> Option<(usize, usize)> {
        self.timeline_viewport
            .map(|viewport| (viewport.first, viewport.last))
    }

    pub fn view_open(&self) -> bool {
        self.view.is_some()
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    pub fn sync_state(&self) -> SyncState {
        self.sync_state
    }

    pub fn terminal_focused(&self) -> bool {
        self.terminal_focused
    }

    pub fn status(&self) -> Option<&StatusMessage> {
        self.status_history.back().filter(|_| self.status_shown)
    }

    /// The question while in `CONFIRM` mode.
    pub fn confirmation(&self) -> Option<&str> {
        self.confirm.as_ref().map(Confirm::question)
    }

    /// Pending keys, for the status line (like nvim's showcmd).
    pub fn pending_keys(&self) -> &[Key] {
        self.resolver.pending()
    }

    /// The pending count prefix, for the status line.
    pub fn pending_count(&self) -> Option<u32> {
        self.resolver.count()
    }

    /// "CONFIRM" while an action asks for confirmation, "SECRET" while the
    /// recovery key is typed, "COMMAND" while a command or search line is
    /// open, "PICKER" while a picker is open, "VIEW" and the mode of its
    /// editor while the message view is open ("VIEW INSERT": keys go to the
    /// editor, `q` included), "NORMAL", or for the composer's editor
    /// "INSERT", "NORMAL", "VISUAL" or "SEARCH". `EditorMode::AwaitingChar`
    /// only comes with keys, and counts as normal mode here.
    pub fn mode_label(&self, editor: Option<EditorMode>) -> &'static str {
        if self.confirm.is_some() {
            return "CONFIRM";
        }
        if self.secret.is_some() {
            return "SECRET";
        }
        if self.command_line.is_some() {
            return "COMMAND";
        }
        if self.picker.is_some() {
            return "PICKER";
        }
        if self.view.is_some() {
            return match editor {
                None => "VIEW",
                Some(EditorMode::Normal | EditorMode::AwaitingChar) => "VIEW NORMAL",
                Some(EditorMode::Insert) => "VIEW INSERT",
                Some(EditorMode::Visual) => "VIEW VISUAL",
                Some(EditorMode::Search) => "VIEW SEARCH",
            };
        }
        match (self.focus, editor) {
            (Focus::Composer, Some(EditorMode::Insert)) => "INSERT",
            (Focus::Composer, Some(EditorMode::Visual)) => "VISUAL",
            (Focus::Composer, Some(EditorMode::Search)) => "SEARCH",
            _ => "NORMAL",
        }
    }

    /// Where `key` goes (design, "Tables and resolution"); `CONFIRM` and
    /// `SECRET` mode take keys before the keymap.
    fn input_context(&self, editor: Option<EditorMode>) -> InputContext {
        if self.command_line.is_some() {
            return InputContext::CommandLine;
        }
        if self.picker.is_some() {
            return InputContext::Picker;
        }
        if self.view.is_some() {
            return match editor {
                None | Some(EditorMode::Normal | EditorMode::Visual) => InputContext::MessageView,
                Some(EditorMode::Insert | EditorMode::Search | EditorMode::AwaitingChar) => {
                    InputContext::EditorPassthrough
                }
            };
        }
        match self.focus {
            Focus::RoomList => InputContext::RoomList,
            Focus::Timeline => InputContext::Timeline,
            Focus::Composer => match editor {
                None | Some(EditorMode::Normal) => InputContext::ComposerNormal,
                Some(EditorMode::Visual) => InputContext::ComposerVisual,
                Some(EditorMode::Insert) if self.mention_menu().is_some() => {
                    InputContext::ComposerCompletion
                }
                Some(EditorMode::Insert) => InputContext::ComposerInsert,
                Some(EditorMode::Search | EditorMode::AwaitingChar) => {
                    InputContext::EditorPassthrough
                }
            },
        }
    }

    fn key(&mut self, key: Key, editor: Option<EditorMode>, effects: &mut Vec<Effect>) {
        // Outside `feed`, so a status set by a binding survives the keys fed
        // again after it.
        self.status_shown = false;
        self.editor = editor;
        // The mention menu belongs to insert mode, and the tui reports the
        // editor mode with every key.
        if editor != Some(EditorMode::Insert) {
            self.cancel_completion();
        }
        self.feed(key, effects);
    }

    /// A paste (design, "Input routing") goes where typed text would go,
    /// without the keymap: to the editor that has the keys, the first line
    /// to the command line, the picker query or, in `SECRET` mode, the
    /// recovery key, at its cursor, and nowhere in the room list and the
    /// timeline. Where the text would go, a paste larger than
    /// [`PASTE_LIMIT`] goes nowhere, and the status line says so. Pending
    /// keys end first, as on their timeout, so they keep their order with
    /// the text; keys pending in another input context end or are dropped
    /// as a key would end or drop them (see [`App::enter_context`]). Like
    /// any key but `y`, a paste cancels a question, and it never answers one
    /// that a binding ending before it asks.
    fn paste(&mut self, text: String, editor: Option<EditorMode>, effects: &mut Vec<Effect>) {
        self.status_shown = false;
        self.editor = editor;
        if editor != Some(EditorMode::Insert) {
            self.cancel_completion();
        }
        if self.confirm.take().is_some() {
            self.resolver.reset();
            return;
        }
        // Keys resolved again after a binding can be pending once more, in
        // the context the binding left; each round ends at least one key.
        let context = loop {
            // `SECRET` mode takes a paste before the keymap, as `feed` gives
            // it keys, also when a binding ending here enters it, as it
            // would take the keys typed after that binding.
            if self.secret.is_some() {
                if !self.paste_too_large(&text) {
                    self.paste_into_secret(first_line(&text));
                }
                return;
            }
            let context = self.input_context(self.editor);
            if !self.enter_context(context, effects) {
                let ended = self.resolver.interrupt(self.keymap.effective(context));
                if ended.is_empty() {
                    break context;
                }
                for next in ended {
                    self.resolved(context.sink(), next, effects);
                }
            }
            if self.confirm.is_some() {
                return;
            }
        };
        match context.sink() {
            KeySink::Drop => {}
            _ if self.paste_too_large(&text) => {}
            KeySink::Editor => effects.push(Effect::Ui(UiEffect::Paste(text))),
            KeySink::Text => {
                let line = first_line(&text);
                if self.command_line.is_some() {
                    self.paste_into_line(line);
                } else {
                    self.paste_into_picker(line);
                }
            }
        }
    }

    /// Whether a paste is larger than [`PASTE_LIMIT`], which the status
    /// line then says.
    fn paste_too_large(&mut self, text: &str) -> bool {
        let too_large = text.len() > PASTE_LIMIT;
        if too_large {
            self.set_status(
                StatusKind::Info,
                format!(
                    "Paste too large ({} KiB; the limit is {} KiB)",
                    text.len().div_ceil(1024),
                    PASTE_LIMIT / 1024
                ),
            );
        }
        too_large
    }

    /// Resolves `key` in the current input context. The keys the resolver
    /// hands back after a binding ran come here again one by one, so when
    /// the binding changed the context, they go to the new context (design,
    /// "Keymap resolver").
    fn feed(&mut self, key: Key, effects: &mut Vec<Effect>) {
        if let Some(confirm) = self.confirm.take() {
            // Keys pending from before the question go with it.
            self.resolver.reset();
            return self.answer(confirm, key, effects);
        }
        if self.secret.is_some() {
            return self.type_secret(key, effects);
        }
        let context = self.input_context(self.editor);
        if self.enter_context(context, effects) {
            // As after keys fed again, a question drops `key`, and a binding
            // that ran may have changed the context again.
            if self.confirm.is_none() {
                self.feed(key, effects);
            }
            return;
        }
        let resolved = self.resolver.feed(
            self.keymap.effective(context),
            context.counts(),
            self.settings.timeoutlen_ms,
            key,
        );
        for next in resolved {
            self.resolved(context.sink(), next, effects);
        }
    }

    /// Makes `context` the input context keys are resolved in. Keys pending
    /// in another context are dropped, with the count, except between two
    /// contexts whose keys go to the editor: there a key released to the
    /// editor changed its mode while they waited, as the first `g` of `gg`
    /// makes edtui wait for its next key while the second waits for a
    /// binding such as `gt`. They end as on their timeout then, in the
    /// context they started in. Whether keys ended.
    fn enter_context(&mut self, context: InputContext, effects: &mut Vec<Effect>) -> bool {
        let started = self.key_context;
        self.key_context = context;
        if started == context {
            return false;
        }
        if started.sink() != KeySink::Editor || context.sink() != KeySink::Editor {
            self.resolver.reset();
            return false;
        }
        let ended = self.resolver.interrupt(self.keymap.effective(started));
        let any = !ended.is_empty();
        for next in ended {
            self.resolved(KeySink::Editor, next, effects);
        }
        any
    }

    /// `y` runs the confirmed action; any other key cancels it.
    fn answer(&mut self, confirm: Confirm, key: Key, effects: &mut Vec<Effect>) {
        if key.text() != Some('y') {
            return;
        }
        match confirm {
            Confirm::Redact { room, item } => {
                effects.push(Effect::Matrix(MatrixEffect::Redact { room, item }));
            }
        }
    }

    /// Resolves the pending keys in the context they started in; the keys
    /// handed back go through `feed` like typed keys.
    fn timeout(&mut self, id: TimerId, effects: &mut Vec<Effect>) {
        let context = self.key_context;
        for next in self.resolver.timeout(self.keymap.effective(context), id) {
            self.resolved(context.sink(), next, effects);
        }
    }

    fn resolved(&mut self, sink: KeySink, resolved: Resolved, effects: &mut Vec<Effect>) {
        match resolved {
            Resolved::Run { binding, count } => self.run_binding(&binding, count, effects),
            Resolved::Release(keys) => self.release(sink, keys, effects),
            Resolved::StartTimer { id, after_ms } => {
                effects.push(Effect::Ui(UiEffect::StartTimer { id, after_ms }));
            }
            Resolved::Refeed(keys) => {
                for key in keys {
                    // Unlike nvim, a question drops the keys left over
                    // instead of taking one as the answer, so keys typed
                    // ahead never confirm a redaction.
                    if self.confirm.is_some() {
                        break;
                    }
                    self.feed(key, effects);
                }
            }
        }
    }

    /// Runs the actions in order, each against the state the previous one left.
    fn run_binding(&mut self, binding: &Binding, count: Option<u32>, effects: &mut Vec<Effect>) {
        for action in binding.actions() {
            let count = if action.takes_count() {
                count.unwrap_or(1)
            } else {
                1
            };
            self.run(action, count, effects);
        }
    }

    fn release(&mut self, sink: KeySink, keys: Vec<Key>, effects: &mut Vec<Effect>) {
        match sink {
            KeySink::Drop => {}
            KeySink::Editor => {
                // `effects` belongs to one feed or timeout, so consecutive
                // releases become one effect.
                if let Some(Effect::Ui(UiEffect::ReplayKeys(replay))) = effects.last_mut() {
                    replay.extend(keys);
                } else {
                    effects.push(Effect::Ui(UiEffect::ReplayKeys(keys)));
                }
            }
            // The command line or the picker query, whichever has the keys.
            KeySink::Text => {
                for key in keys {
                    if self.command_line.is_some() {
                        self.type_into_line(key);
                    } else {
                        self.type_into_picker(key);
                    }
                }
            }
        }
    }

    fn run(&mut self, action: &Action, count: u32, effects: &mut Vec<Effect>) {
        match action {
            Action::CursorDown => self.move_cursor(Motion::Down(count), effects),
            Action::CursorUp => self.move_cursor(Motion::Up(count), effects),
            Action::CursorFirst => self.move_cursor(Motion::First, effects),
            Action::CursorLast => self.move_cursor(Motion::Last, effects),
            Action::CursorHalfPageDown => self.move_cursor(Motion::HalfPageDown(count), effects),
            Action::CursorHalfPageUp => self.move_cursor(Motion::HalfPageUp(count), effects),
            Action::RoomOpen(target) => self.open_target(target, effects),
            Action::RoomOpenSelected => self.open_selected(effects),
            Action::RoomMarkRead => self.mark_read(effects),
            Action::RoomMute => self.set_mute(true, effects),
            Action::RoomUnmute => self.set_mute(false, effects),
            Action::MessageView => self.view_message(effects),
            Action::ViewClose => self.close_view(effects),
            Action::HelpCommands => self.view_help(help_text(COMMANDS), effects),
            Action::HelpKeymap => self.view_help(keymap_text(&self.keymap), effects),
            Action::StatusMessages => self.view_help(self.messages_text(), effects),
            Action::MessageYank => self.yank_message(effects),
            Action::MessageGotoReply => self.goto_reply(effects),
            Action::MessageOpen => self.open_message(effects),
            Action::MessageReply => self.reply(effects),
            Action::MessageEdit => self.edit_message(effects),
            Action::MessageDelete => self.delete_message(),
            Action::MessageReact(emoji) => self.react(*emoji, effects),
            Action::MessageResend => self.resend_message(effects),
            Action::ComposerFocus(mode) => self.focus_composer(*mode, effects),
            Action::ComposerSend => self.composer_effect(UiEffect::RequestSubmit, effects),
            Action::ComposerNewline => self.composer_effect(UiEffect::InsertNewline, effects),
            Action::ComposerExternalEditor => {
                self.composer_effect(UiEffect::ExternalEditor, effects);
            }
            Action::ComposerCancel => self.cancel_draft(effects),
            Action::ComposerLeave => self.leave_composer(effects),
            Action::CompletionNext => self.move_completion(true),
            Action::CompletionPrev => self.move_completion(false),
            Action::CompletionAccept => self.accept_completion(effects),
            Action::CompletionCancel => self.cancel_completion(),
            Action::PickerRooms(filter) => self.pick_room(*filter),
            Action::PickerNext => self.move_picker(true),
            Action::PickerPrev => self.move_picker(false),
            Action::PickerAccept => self.accept_picker(effects),
            Action::PickerCancel => self.cancel_picker(),
            Action::CommandLineOpen => self.open_line(LineKind::Command),
            Action::CommandLineExecute => self.execute_line(effects),
            Action::CommandLineCancel => self.cancel_line(),
            Action::CommandLineComplete => self.complete_line(),
            Action::SearchStart => self.start_search(),
            Action::SearchNext => self.search_again(Direction::Forward, effects),
            Action::SearchPrev => self.search_again(Direction::Backward, effects),
            Action::SidebarToggle => {
                self.sidebar_visible = !self.sidebar_visible;
                if !self.sidebar_visible && self.focus == Focus::RoomList {
                    self.focus = Focus::Timeline;
                }
            }
            Action::FocusRoomList => {
                self.sidebar_visible = true;
                self.focus = Focus::RoomList;
            }
            Action::FocusTimeline => self.focus = Focus::Timeline,
            Action::FileUpload { path } => self.upload(path, effects),
            Action::AccountRecover => self.start_recovery(),
            Action::AppQuit => effects.push(Effect::Ui(UiEffect::Quit)),
            Action::Nop => {}
        }
    }

    /// Moves the cursor of the focused pane; the composer has no cursor of
    /// the core.
    fn move_cursor(&mut self, motion: Motion, effects: &mut Vec<Effect>) {
        match self.focus {
            Focus::RoomList => self.move_room_cursor(motion),
            Focus::Timeline => self.move_timeline_cursor(motion, effects),
            Focus::Composer => {}
        }
    }

    /// Shows the message in the status line and keeps it for `:messages`.
    fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.remember_status(StatusMessage {
            kind,
            text: text.into(),
        });
        self.status_shown = true;
    }

    /// Keeps `message` in the history, dropping the oldest beyond
    /// [`STATUS_HISTORY`]. The status line does not show it, nor the message
    /// it was showing, which is no longer the newest.
    fn remember_status(&mut self, message: StatusMessage) {
        if self.status_history.len() == STATUS_HISTORY {
            self.status_history.pop_front();
        }
        self.status_history.push_back(message);
        self.status_shown = false;
    }

    /// `Msg::ConfigWarnings`: the warnings go into the history first, then
    /// the status line counts them, as an error so that it is not missed.
    fn config_warnings(&mut self, warnings: Vec<String>) {
        let count = warnings.len();
        if count == 0 {
            return;
        }
        for text in warnings {
            self.remember_status(StatusMessage {
                kind: StatusKind::Error,
                text,
            });
        }
        let settings = if count == 1 { "setting" } else { "settings" };
        self.set_status(
            StatusKind::Error,
            format!("Ignored {count} {settings} in the configuration; see :messages"),
        );
    }

    /// The text of `:messages` (design, "Help views"): its first line, then,
    /// when there are messages, an empty line and one line per message,
    /// oldest first.
    fn messages_text(&self) -> String {
        let mut text =
            format!("Status line messages, oldest first; the last {STATUS_HISTORY} are kept.");
        if !self.status_history.is_empty() {
            text.push('\n');
        }
        for message in &self.status_history {
            text.push('\n');
            text.push_str(&message.text);
        }
        text
    }

    fn viewport(&self, pane: Pane) -> Option<Viewport> {
        match pane {
            Pane::RoomList => self.room_list_viewport,
            Pane::Timeline => self.timeline_viewport,
        }
    }

    /// Half of the items the pane showed in its last viewport, at least one.
    /// The timeline's viewport is in entries, of which only items count.
    fn half_page(&self, pane: Pane) -> usize {
        let visible = self.viewport(pane).map_or(1, |viewport| match pane {
            Pane::RoomList => viewport.last.saturating_sub(viewport.first) + 1,
            Pane::Timeline => self
                .timeline()
                .iter()
                .take(viewport.last.saturating_add(1))
                .skip(viewport.first)
                .filter(|entry| matches!(entry, TimelineEntry::Item(_)))
                .count(),
        });
        (visible / 2).max(1)
    }
}
