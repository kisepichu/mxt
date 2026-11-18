//! The application state and `update`: keys and adapter messages in, effects
//! out, without I/O.

mod command_line;
mod rooms;
mod search;
#[cfg(test)]
mod tests;

use crate::action::{Action, Binding};
use crate::effect::{Effect, UiEffect};
use crate::ids::RoomId;
use crate::keymap::{InputContext, KeySink, Keymap};
use crate::keys::Key;
use crate::msg::{EditorMode, Msg, Pane, SyncState};
use crate::resolver::{Resolved, Resolver, TimerId};
use crate::rooms::RoomList;
use crate::settings::Settings;

pub use command_line::{CommandLine, LineKind};

use command_line::OpenLine;
use rooms::Motion;
use search::Direction;

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

/// The UI state (design, "Domain model"), changed only by [`App::update`].
pub struct App {
    keymap: Keymap,
    settings: Settings,
    resolver: Resolver,
    /// The input context of the last key, where the resolver's pending keys
    /// started.
    key_context: InputContext,
    rooms: RoomList,
    /// The room list cursor, once a cursor action, a search or opening a room
    /// placed it. Until then, and while the list is empty, it is `None` and
    /// the cursor is on the first room, whichever room that is, so rooms that
    /// arrive in any order at startup leave it at the top.
    room_cursor: Option<usize>,
    open_room: Option<RoomId>,
    focus: Focus,
    sidebar_visible: bool,
    sync_state: SyncState,
    terminal_focused: bool,
    status: Option<StatusMessage>,
    room_list_viewport: Option<Viewport>,
    timeline_viewport: Option<Viewport>,
    command_line: Option<OpenLine>,
    /// The pattern `search.next` and `search.prev` repeat.
    last_search: Option<String>,
}

impl App {
    pub fn new(keymap: Keymap, settings: Settings) -> App {
        let (focus, key_context) = if settings.room_list_visible {
            (Focus::RoomList, InputContext::RoomList)
        } else {
            (Focus::Timeline, InputContext::Timeline)
        };
        App {
            keymap,
            resolver: Resolver::new(),
            key_context,
            rooms: RoomList::new(settings.room_sort),
            room_cursor: None,
            open_room: None,
            focus,
            sidebar_visible: settings.room_list_visible,
            sync_state: SyncState::default(),
            terminal_focused: true,
            status: None,
            room_list_viewport: None,
            timeline_viewport: None,
            command_line: None,
            last_search: None,
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
            Msg::Viewport { pane, first, last } => {
                let viewport = Some(Viewport { first, last });
                match pane {
                    Pane::RoomList => self.room_list_viewport = viewport,
                    Pane::Timeline => self.timeline_viewport = viewport,
                }
            }
            Msg::TerminalFocus(focused) => self.terminal_focused = focused,
            Msg::EffectFailed { error } => self.set_status(StatusKind::Error, error),
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

    /// Pending keys, for the status line (like nvim's showcmd).
    pub fn pending_keys(&self) -> &[Key] {
        self.resolver.pending()
    }

    /// The pending count prefix, for the status line.
    pub fn pending_count(&self) -> Option<u32> {
        self.resolver.count()
    }

    /// "COMMAND" while a command or search line is open, "NORMAL", or for
    /// the composer's editor "INSERT", "NORMAL", "VISUAL" or "SEARCH".
    pub fn mode_label(&self, editor: Option<EditorMode>) -> &'static str {
        if self.command_line.is_some() {
            return "COMMAND";
        }
        match (self.focus, editor) {
            (Focus::Composer, Some(EditorMode::Insert)) => "INSERT",
            (Focus::Composer, Some(EditorMode::Visual)) => "VISUAL",
            (Focus::Composer, Some(EditorMode::Search)) => "SEARCH",
            _ => "NORMAL",
        }
    }

    /// Stands in for `composer.focus` (phase 3) in tests.
    #[cfg(test)]
    pub(crate) fn focus_composer_for_test(&mut self) {
        if self.open_room.is_some() {
            self.focus = Focus::Composer;
        }
    }

    /// Where `key` goes (design, "Tables and resolution").
    fn input_context(&self, editor: Option<EditorMode>) -> InputContext {
        if self.command_line.is_some() {
            return InputContext::CommandLine;
        }
        match self.focus {
            Focus::RoomList => InputContext::RoomList,
            Focus::Timeline => InputContext::Timeline,
            Focus::Composer => match editor {
                None | Some(EditorMode::Normal) => InputContext::ComposerNormal,
                Some(EditorMode::Visual) => InputContext::ComposerVisual,
                Some(EditorMode::Insert) => InputContext::ComposerInsert,
                Some(EditorMode::Search) => InputContext::EditorPassthrough,
            },
        }
    }

    fn key(&mut self, key: Key, editor: Option<EditorMode>, effects: &mut Vec<Effect>) {
        // Outside `feed`, so a status set by a binding that `key` ended
        // survives when `key` is fed again.
        self.status = None;
        self.feed(key, editor, effects);
    }

    /// Resolves `key` in the current input context. When a binding that ran
    /// before `key` was resolved (the pending keys `key` ended) changes the
    /// context, the rest of the outputs belong to the old context, so `key`
    /// is resolved afresh in the new one (design, "Keymap resolver").
    fn feed(&mut self, key: Key, editor: Option<EditorMode>, effects: &mut Vec<Effect>) {
        let context = self.input_context(editor);
        if context != self.key_context {
            self.resolver.reset();
            self.key_context = context;
        }
        let map = self.keymap.effective(context);
        let timeoutlen_ms = self.settings.timeoutlen_ms;
        let mut resolved = self
            .resolver
            .feed(map, context.counts(), timeoutlen_ms, key)
            .into_iter();
        while let Some(next) = resolved.next() {
            let ran = matches!(next, Resolved::Run { .. });
            self.resolved(context.sink(), next, effects);
            // `key` was not part of the run when outputs follow it, or when
            // `key` started a count after it (a run takes the count).
            let key_left = resolved.len() > 0 || self.resolver.count().is_some();
            if ran && key_left && self.input_context(editor) != context {
                // The reset in the new context drops the old outputs and the
                // count. Nothing is pending then, so this does not come back.
                return self.feed(key, editor, effects);
            }
        }
    }

    /// Resolves the pending keys in the context they started in.
    fn timeout(&mut self, id: TimerId, effects: &mut Vec<Effect>) {
        let context = self.key_context;
        if let Some(resolved) = self.resolver.timeout(self.keymap.effective(context), id) {
            self.resolved(context.sink(), resolved, effects);
        }
    }

    fn resolved(&mut self, sink: KeySink, resolved: Resolved, effects: &mut Vec<Effect>) {
        match resolved {
            Resolved::Run { binding, count } => self.run_binding(&binding, count, effects),
            Resolved::Release(keys) => self.release(sink, keys, effects),
            Resolved::StartTimer { id, after_ms } => {
                effects.push(Effect::Ui(UiEffect::StartTimer { id, after_ms }));
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
            // The command line; the picker query arrives with the picker.
            KeySink::Text => {
                for key in keys {
                    self.type_into_line(key);
                }
            }
        }
    }

    fn run(&mut self, action: &Action, count: u32, effects: &mut Vec<Effect>) {
        match action {
            Action::CursorDown => self.move_cursor(Motion::Down(count)),
            Action::CursorUp => self.move_cursor(Motion::Up(count)),
            Action::CursorFirst => self.move_cursor(Motion::First),
            Action::CursorLast => self.move_cursor(Motion::Last),
            Action::CursorHalfPageDown => self.move_cursor(Motion::HalfPageDown(count)),
            Action::CursorHalfPageUp => self.move_cursor(Motion::HalfPageUp(count)),
            Action::RoomOpen(target) => self.open_target(target, effects),
            Action::RoomOpenSelected => self.open_selected(effects),
            Action::RoomMarkRead => self.mark_read(effects),
            Action::CommandLineOpen => self.open_line(LineKind::Command),
            Action::CommandLineExecute => self.execute_line(effects),
            Action::CommandLineCancel => self.cancel_line(),
            Action::CommandLineComplete => self.complete_line(),
            Action::SearchStart => self.start_search(),
            Action::SearchNext => self.search_again(Direction::Forward),
            Action::SearchPrev => self.search_again(Direction::Backward),
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
            Action::AppQuit => effects.push(Effect::Ui(UiEffect::Quit)),
            Action::Nop => {}
            // The phase that implements an action replaces this.
            _ => self.set_status(
                StatusKind::Info,
                format!("{} is not available yet", action.name()),
            ),
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
}
