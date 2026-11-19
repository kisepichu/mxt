//! The application state and `update`: keys and adapter messages in, effects
//! out, without I/O.

mod command_line;
mod composer;
mod picker;
mod rooms;
mod search;
#[cfg(test)]
mod tests;
mod timeline;

use std::collections::HashMap;

use crate::action::{Action, Binding};
use crate::effect::{Effect, MatrixEffect, UiEffect};
use crate::ids::{RoomId, UserId};
use crate::keymap::{InputContext, KeySink, Keymap};
use crate::keys::Key;
use crate::msg::{EditorMode, Msg, Pane, SyncState};
use crate::resolver::{Resolved, Resolver, TimerId};
use crate::rooms::RoomList;
use crate::settings::Settings;
use crate::timeline::{ItemRef, TimelineEntry};

pub use command_line::{CommandLine, LineKind};
pub use picker::{Picker, PickerItem, PickerKey, PickerKind};

use command_line::OpenLine;
use composer::{Composer, MentionMenu};
use picker::OpenPicker;
use search::Direction;
use timeline::Timeline;

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

/// The message in the status line; the next key clears it.
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
    /// The item the full-screen message view shows, while it is open.
    view: Option<ItemRef>,
    focus: Focus,
    sidebar_visible: bool,
    sync_state: SyncState,
    terminal_focused: bool,
    status: Option<StatusMessage>,
    room_list_viewport: Option<Viewport>,
    /// The open room's, from a render of that room; `None` until then.
    timeline_viewport: Option<Viewport>,
    command_line: Option<OpenLine>,
    /// The pattern `search.next` and `search.prev` repeat.
    last_search: Option<String>,
    /// Per room, created when first used.
    composers: HashMap<RoomId, Composer>,
    mention_menu: Option<MentionMenu>,
    picker: Option<OpenPicker>,
    /// `CONFIRM` mode: the next key answers.
    confirm: Option<Confirm>,
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
            status: None,
            room_list_viewport: None,
            timeline_viewport: None,
            command_line: None,
            last_search: None,
            composers: HashMap::new(),
            mention_menu: None,
            picker: None,
            confirm: None,
            settings,
        }
    }

    /// Deterministic and free of I/O.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let mut effects = Vec::new();
        match msg {
            Msg::Key { key, editor } => self.key(key, editor, &mut effects),
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
            Msg::Submit { room, body } => self.submit(room, body, &mut effects),
            Msg::ComposerEdited { room, empty } => self.composer_edited(room, empty),
            Msg::MentionQuery { room, query } => self.mention_query(room, query),
            Msg::MembersLoaded { room, members } => self.members_loaded(room, members),
            Msg::Downloaded { path } => self.downloaded(path, &mut effects),
            Msg::ImageFetched { item, bytes } => self.image_fetched(item, bytes, &mut effects),
            Msg::ImageUnavailable { item, reason } => self.image_unavailable(&item, &reason),
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
        self.status.as_ref()
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

    /// "CONFIRM" while an action asks for confirmation, "COMMAND" while a
    /// command or search line is open, "PICKER" while a picker is open,
    /// "VIEW" and the mode of its editor while the message view is open
    /// ("VIEW INSERT": keys go to the editor, `q` included), "NORMAL", or
    /// for the composer's editor "INSERT", "NORMAL", "VISUAL" or "SEARCH".
    pub fn mode_label(&self, editor: Option<EditorMode>) -> &'static str {
        if self.confirm.is_some() {
            return "CONFIRM";
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
                Some(EditorMode::Normal) => "VIEW NORMAL",
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

    /// Where `key` goes (design, "Tables and resolution"); `CONFIRM` mode
    /// takes keys before the keymap.
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
                Some(EditorMode::Insert | EditorMode::Search) => InputContext::EditorPassthrough,
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
                Some(EditorMode::Search) => InputContext::EditorPassthrough,
            },
        }
    }

    fn key(&mut self, key: Key, editor: Option<EditorMode>, effects: &mut Vec<Effect>) {
        // Outside `feed`, so a status set by a binding survives the keys fed
        // again after it.
        self.status = None;
        self.editor = editor;
        // The mention menu belongs to insert mode, and the tui reports the
        // editor mode with every key.
        if editor != Some(EditorMode::Insert) {
            self.cancel_completion();
        }
        self.feed(key, effects);
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
        let context = self.input_context(self.editor);
        if context != self.key_context {
            self.resolver.reset();
            self.key_context = context;
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
            Action::MessageYank => self.yank_message(effects),
            Action::MessageGotoReply => self.goto_reply(effects),
            Action::MessageOpen => self.open_message(effects),
            Action::MessageReply => self.reply(effects),
            Action::MessageEdit => self.edit_message(effects),
            Action::MessageDelete => self.delete_message(),
            Action::MessageReact => self.react(),
            Action::MessageResend => self.resend_message(effects),
            Action::ComposerFocus(mode) => self.focus_composer(*mode, effects),
            Action::ComposerSend => self.composer_effect(UiEffect::RequestSubmit, effects),
            Action::ComposerNewline => self.composer_effect(UiEffect::InsertNewline, effects),
            Action::ComposerExternalEditor => {
                self.composer_effect(UiEffect::ExternalEditor, effects);
            }
            Action::ComposerCancel => self.cancel_draft(effects),
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
            Action::AppQuit => effects.push(Effect::Ui(UiEffect::Quit)),
            Action::Nop => {}
            // The phase that implements the action replaces this.
            Action::AccountRecover => self.set_status(
                StatusKind::Info,
                format!("{} is not available yet", action.name()),
            ),
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

    fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            kind,
            text: text.into(),
        });
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
