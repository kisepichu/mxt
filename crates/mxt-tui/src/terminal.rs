//! The terminal: setup and restore, drawing, converting events and
//! executing UI effects.

use std::io::{self, Stdout, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableFocusChange, EnableFocusChange, Event};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use mxt_core::app::App;
use mxt_core::effect::{UiEffect, ViewContent};
use mxt_core::msg::{Msg, Pane};
use mxt_core::resolver::TimerId;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;

use crate::clipboard::SharedRegister;
use crate::input::key_from_crossterm;
use crate::message_view::MessageView;
use crate::timeline_view::{TimeFormat, TimelinePane};
use crate::timers::Timers;
use crate::view::{self, Panes};

/// Whether the terminal is in raw mode and on the alternate screen. Shared by
/// [`Tui::restore`] and the panic hook, so the terminal is restored once: a
/// second restore would move the cursor back over the printed panic.
static ACTIVE: AtomicBool = AtomicBool::new(false);

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// The room list's cursor and scroll offset between frames.
    room_list: ListState,
    /// The room list range last reported with `Msg::Viewport`.
    room_list_viewport: Option<(usize, usize)>,
    timeline: TimelinePane,
    /// The timeline range last reported with `Msg::Viewport`.
    timeline_viewport: Option<(usize, usize)>,
    /// The message view while it is open.
    view: Option<MessageView>,
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
            Ok(terminal) => Ok(Tui {
                terminal,
                room_list: ListState::default(),
                room_list_viewport: None,
                timeline: TimelinePane::new(),
                timeline_viewport: None,
                view: None,
                register: SharedRegister::default(),
                timers: Timers::new(),
                should_quit: false,
            }),
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
    /// visible range changed since the last draw. Fails once the terminal
    /// has been restored, which while the UI runs only the panic hook does,
    /// so a panic in any task ends the event loop instead of drawing onto
    /// the main screen.
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
            ..
        } = self;
        let mut visible = view::Visible::default();
        terminal.draw(|frame| {
            let panes = Panes {
                room_list,
                timeline,
                view: view.as_mut(),
            };
            visible = view::draw(frame, app, panes);
        })?;

        let mut msgs = Vec::new();
        for (pane, range, reported) in [
            (
                Pane::RoomList,
                visible.room_list,
                &mut self.room_list_viewport,
            ),
            (
                Pane::Timeline,
                visible.timeline,
                &mut self.timeline_viewport,
            ),
        ] {
            if let Some((first, last)) = range
                && *reported != range
            {
                *reported = range;
                msgs.push(Msg::Viewport { pane, first, last });
            }
        }
        Ok(msgs)
    }

    /// Converts one terminal event. Key presses and repeats become `Msg::Key`
    /// with the message view's editor mode while it is open (`editor: None`
    /// otherwise until the composer exists); focus events become
    /// `Msg::TerminalFocus`; resizes produce nothing (the loop redraws after
    /// every event anyway). Mouse and paste events are not enabled.
    pub fn handle_event(&mut self, event: Event) -> Vec<Msg> {
        let msg = match event {
            Event::Key(event) => key_from_crossterm(event).map(|key| Msg::Key {
                key,
                editor: self.view.as_ref().map(MessageView::mode),
            }),
            Event::FocusGained => Some(Msg::TerminalFocus(true)),
            Event::FocusLost => Some(Msg::TerminalFocus(false)),
            Event::Resize(..) | Event::Mouse(_) | Event::Paste(_) => None,
        };
        msg.into_iter().collect()
    }

    /// Executes a UI effect. `StartTimer` schedules a deadline; `ReplayKeys`
    /// goes to the message view's editor while it is open (the composer does
    /// not exist yet, so it is dropped otherwise); `OpenView` replaces the
    /// message view and `CloseView` drops it with its edits; `Yank` copies
    /// into the register, and the next draw sends it to the terminal
    /// clipboard; `Quit` sets `should_quit`. The composer effects are
    /// dropped until the composer exists.
    pub fn execute(&mut self, effect: UiEffect, now: Instant) {
        match effect {
            UiEffect::StartTimer { id, after_ms } => {
                // A deadline beyond `Instant`'s range never comes.
                if let Some(deadline) = now.checked_add(Duration::from_millis(after_ms)) {
                    self.timers.start(id, deadline);
                }
            }
            UiEffect::ReplayKeys(keys) => {
                if let Some(view) = &mut self.view {
                    view.replay(&keys);
                }
            }
            UiEffect::OpenView(ViewContent::Text(text)) => {
                self.view = Some(MessageView::new(&text, self.register.clone()));
            }
            UiEffect::CloseView => self.view = None,
            UiEffect::Yank(text) => self.register.copy(text),
            // The composer arrives in phase 3 Task 4.
            UiEffect::FocusComposer { .. }
            | UiEffect::RequestSubmit(_)
            | UiEffect::InsertNewline(_)
            | UiEffect::LoadComposer { .. }
            | UiEffect::ClearComposer(_)
            | UiEffect::ReplaceMentionToken { .. }
            | UiEffect::ExternalEditor(_) => {}
            UiEffect::Quit => self.should_quit = true,
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
