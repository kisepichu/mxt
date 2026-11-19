//! The terminal: setup and restore, drawing, converting events and
//! executing UI effects.

use std::io::{self, Stdout, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableFocusChange, EnableFocusChange, Event, EventStream};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use edtui::system_editor;
use futures_util::StreamExt as _;
use mxt_core::app::{App, Focus};
use mxt_core::effect::{UiEffect, ViewContent};
use mxt_core::ids::RoomId;
use mxt_core::msg::{EditorMode, Msg, Pane};
use mxt_core::resolver::TimerId;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;

use crate::clipboard::SharedRegister;
use crate::composer::{Composer, Composers};
use crate::editor::Editor;
use crate::input::key_from_crossterm;
use crate::overlay::Overlays;
use crate::timeline_view::{TimeFormat, TimelinePane};
use crate::timers::Timers;
use crate::view::{self, Panes};

type Backend = CrosstermBackend<Stdout>;

/// Whether the terminal is in raw mode and on the alternate screen. Shared by
/// [`Tui::restore`] and the panic hook, so the terminal is restored once: a
/// second restore would move the cursor back over the printed panic.
static ACTIVE: AtomicBool = AtomicBool::new(false);

pub struct Tui {
    terminal: Terminal<Backend>,
    /// Terminal input, created by the first [`Tui::next_event`] after
    /// [`Tui::new`] or the external editor.
    events: Option<EventStream>,
    /// The room list's cursor and scroll offset between frames.
    room_list: ListState,
    /// The room list range last reported with `Msg::Viewport`.
    room_list_viewport: Option<(usize, usize)>,
    timeline: TimelinePane,
    /// The room and the timeline range last reported with `Msg::Viewport`.
    timeline_viewport: Option<(RoomId, (usize, usize))>,
    /// The message view's scratch editor while it is open.
    view: Option<Editor>,
    composers: Composers,
    overlays: Overlays,
    register: SharedRegister,
    timers: Timers<TimerId>,
    should_quit: bool,
}

impl Tui {
    /// Raw mode, alternate screen, focus change reporting, hidden cursor.
    pub fn new() -> io::Result<Tui> {
        crossterm::terminal::enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        let setup = execute!(io::stdout(), EnterAlternateScreen, EnableFocusChange, Hide)
            .and_then(|()| Terminal::new(CrosstermBackend::new(io::stdout())))
            .and_then(|mut terminal| terminal.clear().map(|()| terminal));
        match setup {
            Ok(terminal) => {
                let register = SharedRegister::default();
                Ok(Tui {
                    terminal,
                    events: None,
                    room_list: ListState::default(),
                    room_list_viewport: None,
                    timeline: TimelinePane::new(),
                    timeline_viewport: None,
                    view: None,
                    composers: Composers::new(register.clone()),
                    overlays: Overlays::default(),
                    register,
                    timers: Timers::new(),
                    should_quit: false,
                })
            }
            Err(error) => {
                // The setup error is the one worth reporting.
                let _ = restore_terminal();
                Err(error)
            }
        }
    }

    /// Sets `ui.time_format`. Until then the timeline shows `%H:%M`.
    pub fn set_time_format(&mut self, format: TimeFormat) {
        self.timeline.set_time_format(format);
    }

    /// Sends the last copy to the terminal clipboard if it has not reached
    /// it yet, draws the frame, and returns `Msg::Viewport` for panes whose
    /// visible range changed since the last draw. The timeline's range is
    /// also reported when the open room changed, even if its indices are
    /// the same, because the core forgets the viewport when another room
    /// opens. Fails once the terminal has been restored, which while the UI
    /// runs only the panic hook does, so a panic in any task ends the event
    /// loop instead of drawing onto the main screen.
    pub fn draw(&mut self, app: &App) -> io::Result<Vec<Msg>> {
        if !ACTIVE.load(Ordering::SeqCst) {
            return Err(io::Error::other("the terminal was restored after a panic"));
        }
        if let Some(sequence) = self.register.take_unsent() {
            let backend = self.terminal.backend_mut();
            backend.write_all(sequence.as_bytes())?;
            backend.flush()?;
        }
        let Tui {
            terminal,
            room_list,
            timeline,
            view,
            composers,
            overlays,
            ..
        } = self;
        let composer = app.open_room().and_then(|room| composers.get_mut(room));
        let mut visible = view::Visible::default();
        terminal.draw(|frame| {
            let panes = Panes {
                room_list,
                timeline,
                view: view.as_mut(),
                composer,
                overlays,
            };
            visible = view::draw(frame, app, panes);
        })?;

        let mut msgs = Vec::new();
        if let Some((first, last)) = visible.room_list
            && self.room_list_viewport != visible.room_list
        {
            self.room_list_viewport = visible.room_list;
            msgs.push(Msg::Viewport {
                pane: Pane::RoomList,
                first,
                last,
            });
        }
        if let (Some(range), Some(room)) = (visible.timeline, app.open_room())
            && self
                .timeline_viewport
                .as_ref()
                .is_none_or(|(reported, reported_range)| {
                    reported != room || *reported_range != range
                })
        {
            self.timeline_viewport = Some((room.clone(), range));
            let (first, last) = range;
            msgs.push(Msg::Viewport {
                pane: Pane::Timeline,
                first,
                last,
            });
        }
        Ok(msgs)
    }

    /// The next terminal event, `None` once the input has ended. Safe to
    /// cancel, as in `select!`.
    pub async fn next_event(&mut self) -> Option<io::Result<Event>> {
        self.events
            .get_or_insert_with(EventStream::new)
            .next()
            .await
    }

    /// Converts one terminal event. Key presses and repeats become `Msg::Key`
    /// with the mode of the editor that has the keys: the message view's
    /// while it is open, otherwise the open room's composer's while it has
    /// focus. Focus events become `Msg::TerminalFocus`; resizes produce
    /// nothing (the loop redraws after every event anyway). Mouse and paste
    /// events are not enabled.
    pub fn handle_event(&mut self, app: &App, event: Event) -> Vec<Msg> {
        let msg = match event {
            Event::Key(event) => key_from_crossterm(event).map(|key| Msg::Key {
                key,
                editor: self.editor_mode(app),
            }),
            Event::FocusGained => Some(Msg::TerminalFocus(true)),
            Event::FocusLost => Some(Msg::TerminalFocus(false)),
            Event::Resize(..) | Event::Mouse(_) | Event::Paste(_) => None,
        };
        msg.into_iter().collect()
    }

    fn editor_mode(&self, app: &App) -> Option<EditorMode> {
        if let Some(view) = &self.view {
            return Some(view.mode());
        }
        if app.focus() != Focus::Composer {
            return None;
        }
        let room = app.open_room()?;
        self.composers.get(room).map(Composer::mode)
    }

    /// Executes a UI effect and returns what it reports to the core.
    ///
    /// - `StartTimer` schedules a deadline.
    /// - `ReplayKeys` goes to the message view's editor while it is open,
    ///   otherwise to the composer that last got the focus.
    /// - `OpenView` replaces the message view and `CloseView` drops it with
    ///   its edits. `ShowImage`, `OpenPath` and `OpenUrl` are ignored until
    ///   phase 4 Task 3.
    /// - `Yank` copies into the register, and the next draw sends it to the
    ///   terminal clipboard.
    /// - `FocusComposer` creates the room's composer on first focus.
    ///   `RequestSubmit` answers `Msg::Submit` with the composer's text.
    /// - A composer reports `Msg::ComposerEdited` when edits change whether
    ///   it is empty and, in insert mode, `Msg::MentionQuery` when they
    ///   change the `@` token; `ClearComposer` and `LoadComposer` leave the
    ///   emptiness to the core.
    /// - `ExternalEditor` runs `$VISUAL` or `$EDITOR` on the main screen
    ///   (see [`Tui::edit_externally`]). A failure becomes
    ///   `Msg::EffectFailed`.
    /// - `Quit` sets `should_quit`.
    pub fn execute(&mut self, effect: UiEffect, now: Instant) -> Vec<Msg> {
        let mut msgs = Vec::new();
        match effect {
            UiEffect::StartTimer { id, after_ms } => {
                // A deadline beyond `Instant`'s range never comes.
                if let Some(deadline) = now.checked_add(Duration::from_millis(after_ms)) {
                    self.timers.start(id, deadline);
                }
            }
            UiEffect::ReplayKeys(keys) => {
                if let Some(view) = &mut self.view {
                    keys.into_iter().for_each(|key| view.feed(key));
                } else if let Some(composer) = self.composers.focused_mut() {
                    composer.replay(&keys, &mut msgs);
                }
            }
            // Until phase 4 Task 3 draws images, an image's view shows its
            // name, as the text view did before.
            UiEffect::OpenView(ViewContent::Text(text) | ViewContent::Image { name: text, .. }) => {
                self.view = Some(Editor::new(&text, self.register.clone()));
            }
            // Running the opener and drawing images arrive in phase 4 Task 3.
            UiEffect::ShowImage { .. } | UiEffect::OpenPath(_) | UiEffect::OpenUrl(_) => {}
            UiEffect::CloseView => self.view = None,
            UiEffect::Yank(text) => self.register.copy(text),
            UiEffect::FocusComposer { room, mode } => self.composers.focus(room, mode, &mut msgs),
            UiEffect::RequestSubmit(room) => {
                if let Some(composer) = self.composers.get(&room) {
                    let body = composer.text();
                    msgs.push(Msg::Submit { room, body });
                }
            }
            UiEffect::InsertNewline(room) => {
                if let Some(composer) = self.composers.get_mut(&room) {
                    composer.insert_newline(&mut msgs);
                }
            }
            UiEffect::LoadComposer { room, text } => {
                self.composers.get_or_create(&room).load(&text, &mut msgs);
            }
            UiEffect::ClearComposer(room) => {
                if let Some(composer) = self.composers.get_mut(&room) {
                    composer.clear();
                }
            }
            UiEffect::ReplaceMentionToken { room, text } => {
                if let Some(composer) = self.composers.get_mut(&room) {
                    composer.replace_token(&text, &mut msgs);
                }
            }
            UiEffect::ExternalEditor(room) => self.edit_externally(&room, &mut msgs),
            UiEffect::Quit => self.should_quit = true,
        }
        msgs
    }

    /// Runs the external editor on the room's composer.
    ///
    /// - The event stream is dropped first. While a poll of it is pending,
    ///   as after a timer won the event loop's `select!`, its reader thread
    ///   waits on the terminal and would take keys from the editor. The new
    ///   stream after the editor shares crossterm's event queue, so no event
    ///   is lost.
    /// - edtui leaves raw mode and the alternate screen around the editor
    ///   and clears the terminal after it, so the next draw repaints
    ///   everything. The editor gets the cursor shown and no focus reports.
    /// - A panic in another task while the editor runs restores the
    ///   terminal under it, and edtui enters raw mode and the alternate
    ///   screen again afterwards, so they are left again. The next draw
    ///   then ends the event loop.
    fn edit_externally(&mut self, room: &RoomId, msgs: &mut Vec<Msg>) {
        let Some(composer) = self.composers.get_mut(room) else {
            return;
        };
        self.events = None;
        let terminal = &mut self.terminal;
        // edtui's request is made right before its `open`, which takes it,
        // so a failure here does not leave it pending.
        let edited = execute!(io::stdout(), DisableFocusChange, Show).and_then(|()| {
            let edited =
                composer.edit_externally(msgs, |state| system_editor::open(state, terminal));
            let restored = if ACTIVE.load(Ordering::SeqCst) {
                execute!(io::stdout(), EnableFocusChange, Hide)
            } else {
                crossterm::terminal::disable_raw_mode()
                    .and_then(|()| execute!(io::stdout(), LeaveAlternateScreen))
            };
            edited.and(restored)
        });
        if let Err(error) = edited {
            msgs.push(Msg::EffectFailed {
                error: format!("The external editor failed: {error}"),
            });
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// The earliest pending timer deadline.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.timers.next_deadline()
    }

    /// Removes and returns `Msg::Timeout` for every timer due at `now`.
    pub fn expired(&mut self, now: Instant) -> Vec<Msg> {
        self.timers
            .expired(now)
            .map(|id| Msg::Timeout { id })
            .collect()
    }

    /// Leaves raw mode and the alternate screen. Also run by `Drop`.
    pub fn restore(&mut self) -> io::Result<()> {
        restore_terminal()
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Nothing is left to report the error to.
        let _ = self.restore();
    }
}

/// Restores the terminal before the default hook prints the panic.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The panic is the error worth printing.
        let _ = restore_terminal();
        previous(info);
    }));
}

/// Undoes [`Tui::new`] unless that is done already.
fn restore_terminal() -> io::Result<()> {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    let raw = crossterm::terminal::disable_raw_mode();
    let screen = execute!(io::stdout(), DisableFocusChange, LeaveAlternateScreen, Show);
    raw.and(screen)
}
