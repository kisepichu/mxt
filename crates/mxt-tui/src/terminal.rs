//! The terminal: setup and restore, drawing, converting events and
//! executing UI effects.

use std::io::{self, Stdout, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, Event,
    EventStream,
};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use edtui::system_editor;
use futures_util::StreamExt as _;
use mxt_core::app::{App, Focus};
use mxt_core::effect::UiEffect;
use mxt_core::ids::TimelineId;
use mxt_core::msg::{EditorMode, Msg, Pane};
use mxt_core::resolver::TimerId;
use mxt_core::rich::is_web_url;
use mxt_core::settings::ImagePreview;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;

use crate::clipboard::SharedRegister;
use crate::composer::{Composer, Composers};
use crate::image_view::Images;
use crate::input::{key_from_crossterm, pasted_text};
use crate::opener::Opener;
use crate::overlay::Overlays;
use crate::previews::{ImageRows, Limits, Previews};
use crate::timeline_view::{TimeFormat, TimelinePane};
use crate::timers::Timers;
use crate::view::{self, Panes, RoomListLayout, View};

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
    /// `ui.room_list.width` and `ui.room_list.network`.
    room_list_layout: RoomListLayout,
    /// The room list range last reported with `Msg::Viewport`.
    room_list_viewport: Option<(usize, usize)>,
    timeline: TimelinePane,
    /// The message view while it is open.
    view: Option<View>,
    /// Whether the next draw clears the screen first and writes every cell
    /// (see [`Tui::close_view`]).
    repaint: bool,
    /// `None` with `ui.image_preview = "off"`, where the terminal is never
    /// asked for its graphics protocol.
    images: Option<Images>,
    /// The timeline's previews, once [`Tui::enable_previews`] turned them
    /// on.
    previews: Option<Previews>,
    opener: Opener,
    composers: Composers,
    overlays: Overlays,
    register: SharedRegister,
    timers: Timers<TimerId>,
    should_quit: bool,
}

impl Tui {
    /// Raw mode, alternate screen, focus change reporting, bracketed paste,
    /// hidden cursor, and unless `image_preview` is `off` the graphics
    /// protocol for images. `opener` is `files.opener`.
    pub fn new(opener: Vec<String>, image_preview: ImagePreview) -> io::Result<Tui> {
        crossterm::terminal::enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        let setup = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableFocusChange,
            EnableBracketedPaste,
            Hide
        )
        .and_then(|()| {
            // The terminal answers the queries on its input, which the
            // event stream does not read yet. When the answers do not
            // come in time, as in a tmux pane that is not visible,
            // ratatui-image's thread may still take the next input (see
            // `Images::query`), so nothing below asks the terminal:
            // ratatui's `clear` would ask for the cursor position. The
            // clear wipes what a terminal prints for a query it does not
            // know, and the new terminal's buffers are as blank as the
            // screen. Without previews there is no query, and
            // ratatui-image does not turn on tmux's `allow-passthrough`.
            let images = match image_preview {
                ImagePreview::Auto => Some(Images::query(None)?),
                ImagePreview::Forced(protocol) => Some(Images::query(Some(protocol))?),
                ImagePreview::Off => None,
            };
            execute!(io::stdout(), Clear(ClearType::All))?;
            let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
            Ok((terminal, images))
        });
        match setup {
            Ok((terminal, images)) => {
                let register = SharedRegister::default();
                Ok(Tui {
                    terminal,
                    events: None,
                    room_list: ListState::default(),
                    room_list_layout: RoomListLayout::default(),
                    room_list_viewport: None,
                    timeline: TimelinePane::new(),
                    view: None,
                    repaint: false,
                    images,
                    previews: None,
                    opener: Opener::new(opener),
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

    /// Sets `ui.room_list.width` and `ui.room_list.network`. Until then the
    /// list is 30 columns wide and shows network parts inline.
    pub fn set_room_list_layout(&mut self, layout: RoomListLayout) {
        self.room_list_layout = layout;
    }

    /// Turns on previews in the timeline (design, "Images in the
    /// timeline"), at most `rows` rows tall, in the message view's graphics
    /// protocol; nothing without one (`ui.image_preview = "off"`). A worker
    /// thread decodes and encodes them and calls `wake` after each, so that
    /// the event loop draws a frame. Fails, leaving previews off, when the
    /// thread cannot start.
    pub fn enable_previews(
        &mut self,
        rows: ImageRows,
        wake: Box<dyn Fn() + Send>,
    ) -> io::Result<()> {
        let Some(images) = &self.images else {
            return Ok(());
        };
        let previews = Previews::new(rows, images.preview_encoder(), Limits::DEFAULT, wake)?;
        self.previews = Some(previews);
        Ok(())
    }

    /// Sends the last copy to the terminal clipboard if it has not reached
    /// it yet, takes the previews the worker finished, draws the frame (on
    /// a cleared screen after an image that tmux stores, see
    /// `Tui::close_view`), and returns `Msg::Viewport` for the room list when
    /// its visible range changed since the last draw, for the timeline when
    /// its visible range is not the one the core has (see
    /// `timeline_report`), and `Msg::PreviewsWanted` when the list of
    /// previews to fetch changed. A frame that leaves out or moves a tmux
    /// sixel preview is drawn without tmux's sixel, then again on a cleared
    /// screen with it (see `Previews::finish_frame`), so that each is sent
    /// once. An image that failed to encode while drawing becomes
    /// `Msg::EffectFailed`, once per image. Fails once the terminal has
    /// been restored, which while the UI runs only the panic hook does, so a
    /// panic in any task ends the event loop instead of drawing onto the
    /// main screen.
    pub fn draw(&mut self, app: &App) -> io::Result<Vec<Msg>> {
        if !ACTIVE.load(Ordering::SeqCst) {
            return Err(io::Error::other("the terminal was restored after a panic"));
        }
        if let Some(sequence) = self.register.take_unsent() {
            let backend = self.terminal.backend_mut();
            backend.write_all(sequence.as_bytes())?;
            backend.flush()?;
        }
        if let Some(previews) = &mut self.previews {
            previews.take_done();
        }
        if std::mem::take(&mut self.repaint) {
            self.repaint_screen()?;
        }
        let mut visible = self.draw_frame(app)?;
        if self.previews.as_mut().is_some_and(Previews::finish_frame) {
            self.repaint_screen()?;
            visible = self.draw_frame(app)?;
            // On the cleared screen no placement can go.
            if let Some(previews) = &mut self.previews {
                previews.finish_frame();
            }
        }

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
        msgs.extend(timeline_report(app, visible.timeline));
        msgs.extend(self.previews.as_mut().and_then(Previews::take_report));
        if let Some(error) = self.images.as_mut().and_then(Images::take_failure) {
            msgs.push(cannot_show(error));
        }
        Ok(msgs)
    }

    /// One frame of `app`.
    fn draw_frame(&mut self, app: &App) -> io::Result<view::Visible> {
        let Tui {
            terminal,
            room_list,
            room_list_layout,
            timeline,
            view,
            images,
            previews,
            composers,
            overlays,
            ..
        } = self;
        if let Some(previews) = previews {
            previews.begin_frame();
        }
        let composer = app
            .shown_timeline()
            .and_then(|shown| composers.get_mut(&shown));
        let mut visible = view::Visible::default();
        terminal.draw(|frame| {
            let panes = Panes {
                room_list,
                room_list_layout: *room_list_layout,
                timeline,
                view: view.as_mut(),
                images: images.as_mut(),
                previews: previews.as_mut(),
                composer,
                overlays,
            };
            visible = view::draw(frame, app, panes);
        })?;
        Ok(visible)
    }

    /// Clears the screen, which drops every image, and makes the next draw
    /// write every cell (see [`repaint`]).
    fn repaint_screen(&mut self) -> io::Result<()> {
        repaint(&mut self.terminal)?;
        if let Some(previews) = &mut self.previews {
            previews.screen_cleared();
        }
        Ok(())
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
    /// and pastes `Msg::Paste` (with `\n` line breaks; an empty paste is
    /// dropped), both with the key mode (see `Editor::key_mode`) of the
    /// editor that has the keys: the message view's while it is open,
    /// otherwise the shown timeline's composer's while it has focus. Focus
    /// events become `Msg::TerminalFocus`. A resize produces nothing (the
    /// loop redraws after every event anyway) but takes tmux's cell size
    /// again for images (see [`Images::resized`]), the previews' too. Mouse
    /// events are not enabled.
    pub fn handle_event(&mut self, app: &App, event: Event) -> Vec<Msg> {
        let msg = match event {
            Event::Key(event) => key_from_crossterm(event).map(|key| Msg::Key {
                key,
                editor: self.editor_mode(app),
            }),
            Event::Paste(text) if text.is_empty() => None,
            Event::Paste(text) => Some(Msg::Paste {
                text: pasted_text(text),
                editor: self.editor_mode(app),
            }),
            Event::FocusGained => Some(Msg::TerminalFocus(true)),
            Event::FocusLost => Some(Msg::TerminalFocus(false)),
            Event::Resize(..) => {
                if let Some(images) = &mut self.images {
                    images.resized();
                    if let Some(previews) = &mut self.previews {
                        previews.set_encoder(images.preview_encoder());
                    }
                }
                None
            }
            Event::Mouse(_) => None,
        };
        msg.into_iter().collect()
    }

    fn editor_mode(&self, app: &App) -> Option<EditorMode> {
        if let Some(view) = &self.view {
            return Some(view.editor.key_mode());
        }
        if app.focus() != Focus::Composer {
            return None;
        }
        let shown = app.shown_timeline()?;
        self.composers.get(&shown).map(Composer::key_mode)
    }

    /// Executes a UI effect and returns what it reports to the core.
    ///
    /// - `StartTimer` schedules a deadline.
    /// - `ReplayKeys` and `Paste` go to the message view's editor while it
    ///   is open, otherwise to the composer that last got the focus.
    /// - `OpenView` replaces the message view and `CloseView` drops it with
    ///   its edits. `ShowImage` decodes the image for the view (see
    ///   [`Images::insert`]); a failure becomes `Msg::EffectFailed`. Without
    ///   previews it does nothing (the core does not fetch images then).
    /// - `ShowPreview` hands the image to the previews' worker, and
    ///   `PreviewFailed` leaves the item its text line; both do nothing
    ///   without previews.
    /// - `OpenPath` and `OpenUrl` run `files.opener` (see
    ///   [`Opener::open`]), `OpenUrl` only for `http://` and `https://`
    ///   URLs. A refusal or a failure to start it becomes `Msg::EffectFailed`.
    /// - `Yank` copies into the register, and the next draw sends it to the
    ///   terminal clipboard.
    /// - `FocusComposer` creates the timeline's composer on first focus.
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
                    keys.into_iter().for_each(|key| view.editor.feed(key));
                } else if let Some(composer) = self.composers.focused_mut() {
                    composer.replay(&keys, &mut msgs);
                }
            }
            UiEffect::Paste(text) => {
                if let Some(view) = &mut self.view {
                    view.editor.paste(&text);
                } else if let Some(composer) = self.composers.focused_mut() {
                    composer.paste(&text, &mut msgs);
                }
            }
            UiEffect::OpenView(content) => {
                self.close_view();
                self.view = Some(View::new(content, self.register.clone()));
            }
            UiEffect::ShowImage { item, bytes } => {
                let Some(images) = &mut self.images else {
                    return msgs;
                };
                let shown = match self.terminal.size() {
                    Ok(screen) => images
                        .insert(item, bytes.as_bytes(), screen)
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                };
                if let Err(error) = shown {
                    msgs.push(cannot_show(error));
                }
            }
            UiEffect::ShowPreview { item, bytes } => {
                if let Some(previews) = &mut self.previews {
                    previews.show(item, bytes);
                }
            }
            UiEffect::PreviewFailed(item) => {
                if let Some(previews) = &mut self.previews {
                    previews.fail(item);
                }
            }
            UiEffect::OpenPath(path) => msgs.extend(open(&self.opener, &path)),
            UiEffect::OpenUrl(url) if is_web_url(&url) => msgs.extend(open(&self.opener, &url)),
            UiEffect::OpenUrl(_) => msgs.push(Msg::EffectFailed {
                error: "Not opening the link: only http and https links are opened".to_owned(),
            }),
            UiEffect::CloseView => self.close_view(),
            UiEffect::Yank(text) => self.register.copy(text),
            UiEffect::FocusComposer { timeline, mode } => {
                self.composers.focus(timeline, mode, &mut msgs);
            }
            UiEffect::RequestSubmit(timeline) => {
                if let Some(composer) = self.composers.get(&timeline) {
                    let body = composer.text();
                    msgs.push(Msg::Submit { timeline, body });
                }
            }
            UiEffect::InsertNewline(timeline) => {
                if let Some(composer) = self.composers.get_mut(&timeline) {
                    composer.insert_newline(&mut msgs);
                }
            }
            UiEffect::LoadComposer { timeline, text } => {
                self.composers
                    .get_or_create(&timeline)
                    .load(&text, &mut msgs);
            }
            UiEffect::ClearComposer(timeline) => {
                if let Some(composer) = self.composers.get_mut(&timeline) {
                    composer.clear();
                }
            }
            UiEffect::ReplaceMentionToken { timeline, text } => {
                if let Some(composer) = self.composers.get_mut(&timeline) {
                    composer.replace_token(&text, &mut msgs);
                }
            }
            UiEffect::ExternalEditor(timeline) => self.edit_externally(&timeline, &mut msgs),
            UiEffect::Quit => self.should_quit = true,
        }
        msgs
    }

    /// Drops the message view. tmux drops a sixel image it stores once
    /// ASCII text or an erase is written over it, but the frame after the
    /// view may write only other characters there, such as CJK text, and
    /// tmux would draw the image again on its next redraw of the pane. So
    /// after an image that tmux stores, the next draw clears the screen,
    /// which drops every image.
    fn close_view(&mut self) {
        let image = self.view.take().is_some_and(|view| view.has_image());
        self.repaint |= image && self.images.as_ref().is_some_and(Images::stored_by_tmux);
    }

    /// Runs the external editor on the timeline's composer.
    ///
    /// - The event stream is dropped first. While a poll of it is pending,
    ///   as after a timer won the event loop's `select!`, its reader thread
    ///   waits on the terminal and would take keys from the editor. The new
    ///   stream after the editor shares crossterm's event queue, so no event
    ///   is lost.
    /// - edtui leaves raw mode and the alternate screen around the editor
    ///   and then clears the terminal, ignoring a failure. Its clear asks for
    ///   the cursor position, and the first query after the stream is
    ///   dropped fails at once (the drop leaves a wake-up pending, which the
    ///   query's poll reports as its timeout), so the clear is skipped and
    ///   ratatui keeps the frame from before. [`repaint`] clears the screen
    ///   and forgets that frame, so the next draw writes every cell. The
    ///   editor gets the cursor shown, and neither focus reports nor
    ///   bracketed paste unless it turns them on itself.
    /// - A panic in another task while the editor runs restores the
    ///   terminal under it, and edtui enters raw mode and the alternate
    ///   screen again afterwards, so they are left again. The next draw
    ///   then ends the event loop.
    fn edit_externally(&mut self, timeline: &TimelineId, msgs: &mut Vec<Msg>) {
        let Some(composer) = self.composers.get_mut(timeline) else {
            return;
        };
        self.events = None;
        let terminal = &mut self.terminal;
        let previews = &mut self.previews;
        // edtui's request is made right before its `open`, which takes it,
        // so a failure here does not leave it pending.
        let edited = execute!(
            io::stdout(),
            DisableFocusChange,
            DisableBracketedPaste,
            Show
        )
        .and_then(|()| {
            let edited =
                composer.edit_externally(msgs, |state| system_editor::open(state, terminal));
            let restored = if ACTIVE.load(Ordering::SeqCst) {
                execute!(io::stdout(), EnableFocusChange, EnableBracketedPaste, Hide)
                    .and_then(|()| repaint(terminal))
                    .inspect(|()| {
                        if let Some(previews) = previews {
                            previews.screen_cleared();
                        }
                    })
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

    /// Leaves raw mode, the alternate screen and the terminal modes
    /// [`Tui::new`] turned on. Also run by `Drop`.
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

/// Runs `files.opener` on `target`. A failure is the opener's, so its text,
/// which the event loop also logs, leaves out the link or file name, which
/// come from a message.
fn open(opener: &Opener, target: &str) -> Option<Msg> {
    let error = opener.open(target).err()?;
    Some(Msg::EffectFailed {
        error: format!("Cannot start files.opener: {error}"),
    })
}

/// The status for an image that could not be decoded or encoded.
fn cannot_show(error: impl std::fmt::Display) -> Msg {
    Msg::EffectFailed {
        error: format!("Cannot show the image: {error}"),
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
    let screen = execute!(
        io::stdout(),
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
    raw.and(screen)
}

/// Clears the screen and forgets the last frame, so the next draw writes
/// every cell. `Terminal::clear` would ask for the cursor position; a
/// resize of the full-screen viewport to the terminal's size does the same
/// without asking, and also catches a resize while the screen was away.
pub(crate) fn repaint<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
) -> Result<(), B::Error> {
    let size = terminal.size()?;
    terminal.resize(size.into())
}

/// `Msg::Viewport` for the timeline range a frame drew (`drawn`) when the
/// core has another one. The core forgets the range when another room
/// opens, so the opened room's range is reported even when its indices are
/// the ones reported before, also after a quick return to a room (A, B,
/// then A again before B was drawn with entries).
fn timeline_report(app: &App, drawn: Option<(usize, usize)>) -> Option<Msg> {
    let (first, last) = drawn.filter(|&range| app.timeline_viewport() != Some(range))?;
    Some(Msg::Viewport {
        pane: Pane::Timeline,
        first,
        last,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::{Screen, app, press, reset};
    use mxt_core::effect::{Effect, MatrixEffect};
    use mxt_core::ids::{EventId, RoomId};
    use ratatui::backend::Backend as _;

    /// Draws a frame and gives the core the timeline report it makes.
    fn frame(app: &mut App, screen: &mut Screen) -> Vec<Effect> {
        let visible = screen.draw(app, None);
        timeline_report(app, visible.timeline)
            .map(|msg| app.update(msg))
            .unwrap_or_default()
    }

    fn receipt(room: &str, event: u32) -> Effect {
        Effect::Matrix(MatrixEffect::SendReceipt {
            room: RoomId::new(room),
            event: EventId::new(format!("${event}:example.org")),
        })
    }

    #[test]
    fn the_opened_room_range_is_reported_even_when_its_indices_are_the_same() {
        let mut app = app();
        let mut screen = Screen::new();
        press(&mut app, "<CR>");
        app.update(reset("!a:example.org", 1..4));
        assert_eq!(frame(&mut app, &mut screen), [receipt("!a:example.org", 3)]);
        assert_eq!(app.timeline_viewport(), Some((0, 2)));
        // Reported once.
        let visible = screen.draw(&app, None);
        assert_eq!(timeline_report(&app, visible.timeline), None);

        // `!b` shows the same range.
        press(&mut app, "<C-w>hj<CR>");
        app.update(reset("!b:example.org", 11..14));
        assert_eq!(
            frame(&mut app, &mut screen),
            [receipt("!b:example.org", 13)]
        );

        // To `!a` and back to `!b` before `!a`'s timeline arrives: the frame
        // between shows no entries.
        press(&mut app, "<C-w>hk<CR>");
        assert_eq!(frame(&mut app, &mut screen), []);
        press(&mut app, "<C-w>hj<CR>");
        app.update(reset("!b:example.org", 11..14));
        assert_eq!(
            frame(&mut app, &mut screen),
            [receipt("!b:example.org", 13)]
        );
        assert_eq!(app.timeline_viewport(), Some((0, 2)));
    }

    #[test]
    fn the_frame_after_the_external_editor_writes_the_whole_screen() {
        let mut app = app();
        press(&mut app, "<CR>");
        app.update(reset("!a:example.org", 1..4));
        let mut screen = Screen::new();
        screen.draw(&app, None);
        let frame = screen.terminal.backend().buffer().clone();

        // Back from the editor, the alternate screen is blank.
        screen.terminal.backend_mut().clear().unwrap();
        repaint(&mut screen.terminal).unwrap();
        screen.draw(&app, None);
        assert_eq!(screen.terminal.backend().buffer(), &frame);
    }

    #[test]
    fn failures_to_open_name_neither_the_link_nor_the_file() {
        let missing = Opener::new(vec!["/nonexistent/mxt-opener".into()]);
        for target in [
            "https://example.org/plans",
            "/home/alice/Downloads/plans.pdf",
        ] {
            match open(&missing, target) {
                Some(Msg::EffectFailed { error }) => {
                    assert!(error.starts_with("Cannot start files.opener: "), "{error}");
                    assert!(!error.contains("plans"), "{error}");
                }
                other => panic!("not a failure: {other:?}"),
            }
        }
    }
}
