//! An edtui editor in vim mode with the shared register: the composers and
//! the message view's scratch editor (design, "Terminal adapter and the
//! composer").
//!
//! In normal and visual mode mxt handles some keys in place of edtui
//! (design, "edtui differences from vim"): `f`, `F`, `t` and `T` with their
//! character, `;` and `,` repeating them, `r` with its character (in visual
//! mode, which has no `r` yet, the character is dropped), and `h`, `l`,
//! `<Left>` and `<Right>` by grapheme cluster. As in nvim, the key after an
//! operator (`d`, `c`, `y`), a text object's `i` or `g` goes to edtui
//! without the keymap: nvim's operator-pending mode sees no normal-mode
//! mappings, and nvim reads the key after `g` without mappings too (design,
//! "Input routing").
//!
//! In normal and insert mode, with no key waiting, mxt handles `<Up>` on
//! the first line and `<Down>` on the last, where edtui's `MoveUp` and
//! `MoveDown` stop: they go to the start and the end of the text, as in a
//! text field (design, "Terminal adapter and the composer").
//!
//! In normal and visual mode `"` names a register, as in vim, and only the
//! black hole register `"_` exists: the next command writes neither the
//! register nor the terminal clipboard. Other names are refused with
//! `Msg::EditorRefused` (design, "Input routing").

use std::cmp::Ordering;

use crossterm::event::{KeyCode as C, KeyModifiers as M};
use edtui::actions::{
    AppendCharToSearch, InsertChar, MoveBackward, MoveForward, ReplaceChar, SwitchMode,
};
use edtui::events::{KeyEventRegister, KeyInput};
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView, Index2, Lines, RowIndex};
use mxt_core::keys::{Key, KeyCode, Modifiers};
use mxt_core::msg::{EditorMode, Msg};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::clipboard::{EditorRegister, SharedRegister};
use crate::view::grapheme_width;

/// The columns edtui draws a tab with (its default).
const TAB_WIDTH: usize = 2;

/// The status line error for a register other than `"_`.
const ONLY_BLACK_HOLE: &str = "Only the black hole register \"_ is supported";

/// What edtui runs for the keys of [`Command`]. The switch to search mode
/// tells [`Editor::feed_key`] that the key started a command rather than
/// continued one (`d` then `f`); `feed_key` switches back at once. This
/// relies on two things edtui 0.11.8 does, to check again when edtui
/// changes:
/// - edtui runs a key's action only when the key completes a sequence, so
///   a key that continues one never runs `MARK`
///   (`operators_keep_edtui_df_and_dt_and_drop_d_capital_f`);
/// - from normal or visual mode, the switch to search mode changes nothing
///   but the mode: the selection, the search pattern and `.` stay
///   (`finds_extend_a_visual_selection`).
const MARK: SwitchMode = SwitchMode(edtui::EditorMode::Search);

pub(crate) struct Editor {
    pub(crate) state: EditorState,
    keys: EditorEventHandler,
    /// The handle every editor state of this editor copies through. Its
    /// black hole flag, set by `"_`, is the one record of that state.
    register: EditorRegister,
    /// A key waiting for the next one, which gets it without the keymap. It
    /// counts only in normal and visual mode, where it was typed.
    pending: Option<Pending>,
    /// The last `f`, `F`, `t` or `T` with its character, for `;` and `,`.
    last_find: Option<Find>,
}

impl Editor {
    /// In normal mode with the cursor at the start.
    pub(crate) fn new(text: &str, register: SharedRegister) -> Editor {
        let register = register.for_editor();
        Editor {
            state: state_holding(text, register.clone()),
            keys: key_handler(),
            register,
            pending: None,
            last_find: None,
        }
    }

    /// A new editor state holding `text`, in normal mode with the cursor at
    /// the start, which copies through this editor's handle.
    pub(crate) fn fresh_state(&self, text: &str) -> EditorState {
        state_holding(text, self.register.clone())
    }

    pub(crate) fn mode(&self) -> EditorMode {
        match self.state.mode {
            edtui::EditorMode::Normal => EditorMode::Normal,
            edtui::EditorMode::Visual => EditorMode::Visual,
            edtui::EditorMode::Insert => EditorMode::Insert,
            edtui::EditorMode::Search => EditorMode::Search,
        }
    }

    /// The mode the core resolves the next key in: `AwaitingChar` while a
    /// key waits for the next one, which then goes to the editor without
    /// the keymap (`f:`, `r<Esc>`, `d]`, `ci[`, `g]`, `"_`), and after `"_`
    /// until its command ends (`"_dd`).
    pub(crate) fn key_mode(&self) -> EditorMode {
        let black_hole = self.register.black_hole() && self.normal_or_visual();
        if black_hole || self.waiting().is_some() {
            EditorMode::AwaitingChar
        } else {
            self.mode()
        }
    }

    fn waiting(&self) -> Option<Pending> {
        self.pending.filter(|_| self.normal_or_visual())
    }

    fn normal_or_visual(&self) -> bool {
        matches!(
            self.state.mode,
            edtui::EditorMode::Normal | edtui::EditorMode::Visual
        )
    }

    /// Cancels a key waiting for the next one, mxt's (`f`, `r`, `"`) or
    /// edtui's (`d`, `ci`, `df`), and a `"_` waiting for its command, for a
    /// change of the editor's mode or text without a key (`FocusComposer`,
    /// `LoadComposer`, a paste): the key must not take one typed later, nor
    /// `"_` drop a later copy.
    pub(crate) fn cancel_pending(&mut self) {
        self.pending = None;
        self.register.set_black_hole(false);
        // edtui cannot cancel its keys from outside; a new handler has
        // nothing waiting.
        self.keys = key_handler();
    }

    /// `composer.normal`: normal mode as edtui's own `<Esc>` gives it, so
    /// from insert mode the cursor moves one column left and `.` repeats
    /// the insert, a visual selection is dropped and a search stopped.
    /// Normal mode stays as it is. A key waiting for the next one is
    /// cancelled.
    pub(crate) fn enter_normal_mode(&mut self) {
        self.cancel_pending();
        if self.state.mode != edtui::EditorMode::Normal {
            self.keys
                .on_key_event(KeyInput::new(C::Esc), &mut self.state);
        }
    }

    /// A key the core passed on (see [`Editor::feed_key`]); a register
    /// other than `"_` reports `Msg::EditorRefused` to `msgs`.
    ///
    /// After `"_` the editor's copies go nowhere until its command ends,
    /// that is until a key leaves nothing waiting, whichever way the key
    /// was handled: an edtui command, also one that writes nothing (`"_j`),
    /// mxt's `f`, `t` or `r` with its character, a dropped key, a key edtui
    /// has no code for (`"_<F1>`), `<Up>` or `<Down>` at an edge. The `_`
    /// that sets the flag leaves it for the next key. `.` is a command of
    /// its own: after `"_dd` it writes, and `"_.` does not.
    pub(crate) fn feed(&mut self, key: Key, msgs: &mut Vec<Msg>) {
        let armed = self.register.black_hole();
        self.feed_key(key, msgs);
        if armed && self.waiting().is_none() {
            self.register.set_black_hole(false);
        }
    }

    /// One key. Keys edtui has no code for are left out.
    fn feed_key(&mut self, key: Key, msgs: &mut Vec<Msg>) {
        let waiting = self.waiting();
        self.pending = None;
        if waiting.is_none() && self.move_to_edge(key) {
            return;
        }
        match waiting {
            None | Some(Pending::Operator(_) | Pending::Object | Pending::G) => {}
            Some(Pending::Char { till }) => {
                // edtui takes the character of any character key, and
                // cancels on the other keys, `<Tab>` among them.
                if let KeyCode::Char(target) = key.code() {
                    self.last_find = Some(Find {
                        target,
                        backward: false,
                        till,
                    });
                }
            }
            Some(Pending::Drop) => return,
            Some(Pending::Register) => {
                // vim's register names are characters; `"<Esc>` cancels.
                match key.text() {
                    Some('_') => self.register.set_black_hole(true),
                    Some(_) => msgs.push(Msg::EditorRefused(ONLY_BLACK_HOLE.to_owned())),
                    None => {}
                }
                return;
            }
            Some(Pending::Find { backward, till }) => {
                if let Some(target) = char_arg(key) {
                    let find = Find {
                        target,
                        backward,
                        till,
                    };
                    self.last_find = Some(find);
                    self.find(find, false);
                }
                return;
            }
            Some(Pending::Replace) => {
                if let Some(c) = char_arg(key) {
                    self.state.execute(ReplaceChar(c));
                }
                return;
            }
        }
        let mode = self.state.mode;
        let Some(input) = key_input(key, mode) else {
            if waiting.is_some() {
                // edtui does not get the key, so it would keep waiting.
                self.cancel_pending();
            }
            return;
        };
        self.keys.on_key_event(input, &mut self.state);
        if let Some(command) = Command::of(key, mode)
            && self.state.mode == edtui::EditorMode::Search
        {
            // `MARK` ran: the key starts a command.
            self.state.mode = mode;
            self.run(command);
        } else {
            self.pending = edtui_waits(waiting, key, mode);
        }
    }

    /// A paste (design, "Input routing"): `text`, with `\n` line breaks,
    /// goes in as text, never as keys. Insert mode inserts it at the
    /// cursor, and `.` repeats it with the rest of the insert, as with typed
    /// text. Normal mode puts it after the cursor like `p` with a
    /// characterwise register, and visual mode in place of the selection,
    /// both as `a`, the text and `<Esc>` would: one undo step, back in
    /// normal mode with the cursor on the last pasted character. Neither
    /// touches the register. Search mode takes the text up to the first
    /// line break. A key waiting for the next one is cancelled.
    pub(crate) fn paste(&mut self, text: &str) {
        self.cancel_pending();
        match self.state.mode {
            edtui::EditorMode::Insert => insert_text(&mut self.state, text),
            edtui::EditorMode::Normal => {
                // The switch from normal or visual mode saves the text for
                // undo.
                self.state.execute(SwitchMode(edtui::EditorMode::Insert));
                let col = self.state.cursor.col;
                if let Some(col) = self.line().map(|line| step(line, col, true, true)) {
                    self.state.cursor.col = col;
                }
                insert_text(&mut self.state, text);
                self.state.execute(SwitchMode(edtui::EditorMode::Normal));
            }
            edtui::EditorMode::Visual => {
                let state = &mut self.state;
                let selection = state.selection.take();
                state.execute(SwitchMode(edtui::EditorMode::Insert));
                if let Some(selection) = selection {
                    let start = selection.start();
                    let col = if selection.line_mode { 0 } else { start.col };
                    // Unlike vim's `"-`, the replaced text goes to no
                    // register: the register is the clipboard's.
                    let _replaced = selection.extract_from(&mut state.lines);
                    state.cursor = Index2::new(start.row, col);
                }
                insert_text(state, text);
                state.execute(SwitchMode(edtui::EditorMode::Normal));
            }
            edtui::EditorMode::Search => {
                let line = text.split('\n').next().unwrap_or_default();
                for c in line.chars() {
                    self.state.execute(AppendCharToSearch(c));
                }
            }
        }
    }

    fn run(&mut self, command: Command) {
        match command {
            Command::Find { backward, till } => {
                self.pending = Some(Pending::Find { backward, till });
            }
            // Visual `r` (vim replaces every selected character) is not
            // implemented; its character is dropped rather than run as a
            // command.
            Command::Replace if self.state.mode == edtui::EditorMode::Visual => {
                self.pending = Some(Pending::Drop);
            }
            Command::Replace => self.pending = Some(Pending::Replace),
            // Named again, the register replaces the one before, as in vim:
            // a `"_` before this `"` no longer counts.
            Command::Register => {
                self.register.set_black_hole(false);
                self.pending = Some(Pending::Register);
            }
            Command::Repeat { reverse } => {
                if let Some(find) = self.last_find {
                    let backward = find.backward != reverse;
                    self.find(Find { backward, ..find }, true);
                }
            }
            Command::Step { forward } => {
                let past_end = self.state.mode == edtui::EditorMode::Visual;
                let col = self.state.cursor.col;
                if let Some(col) = self.line().map(|line| step(line, col, forward, past_end)) {
                    self.move_to(col);
                }
            }
        }
    }

    /// Moves to the match of `find` on the cursor's line, if there is one;
    /// `repeat` for `;` and `,`.
    fn find(&mut self, find: Find, repeat: bool) {
        let col = self.state.cursor.col;
        if let Some(col) = self
            .line()
            .and_then(|line| find_col(line, col, find, repeat))
        {
            self.move_to(col);
        }
    }

    fn line(&self) -> Option<&[char]> {
        self.state
            .lines
            .get(RowIndex::new(self.state.cursor.row))
            .map(Vec::as_slice)
    }

    /// Moves the cursor to `col` on its line with edtui's motions, which
    /// keep a visual selection up to date.
    fn move_to(&mut self, col: usize) {
        let from = self.state.cursor.col;
        match col.cmp(&from) {
            Ordering::Greater => self.state.execute(MoveForward(col - from)),
            Ordering::Less => self.state.execute(MoveBackward(from - col)),
            Ordering::Equal => {}
        }
    }

    /// `<Up>` on the first line goes to the start of the text, and
    /// `<Down>` on the last line to its end: after the last character in
    /// insert mode, and in normal mode on the start of the last grapheme
    /// cluster, so the cursor never lands inside one (on `👍🏽`'s first
    /// character, not on its skin tone modifier). Lines are the text's, not
    /// the rows a wrapped line takes on the screen, as for edtui's
    /// `MoveUp` and `MoveDown`. Whether `key` was such a key; other keys,
    /// and these keys in visual and search mode, are left to edtui.
    fn move_to_edge(&mut self, key: Key) -> bool {
        let state = &mut self.state;
        let insert = match state.mode {
            edtui::EditorMode::Insert => true,
            edtui::EditorMode::Normal => false,
            edtui::EditorMode::Visual | edtui::EditorMode::Search => return false,
        };
        if key.mods() != Modifiers::default() {
            return false;
        }
        let last = state.lines.len().saturating_sub(1);
        state.cursor = match key.code() {
            KeyCode::Up if state.cursor.row == 0 => Index2::new(0, 0),
            KeyCode::Down if state.cursor.row >= last => {
                let line = state
                    .lines
                    .get(RowIndex::new(last))
                    .map_or(&[][..], Vec::as_slice);
                let col = if insert {
                    line.len()
                } else {
                    cluster_starts(line).last().copied().unwrap_or(0)
                };
                Index2::new(last, col)
            }
            _ => return false,
        };
        true
    }

    /// The search pattern while the editor is in search mode.
    pub(crate) fn search(&self) -> Option<String> {
        (self.state.mode == edtui::EditorMode::Search).then(|| self.state.search_pattern())
    }

    /// The rows the text takes at `width` columns, at least one. Like
    /// edtui's wrapping, a grapheme cluster that does not fit on a row moves
    /// to the next one whole, and an empty line takes a row. In insert mode
    /// the cursor after the end of its line takes a column too: edtui draws
    /// it on the next row when the line's last row is full.
    pub(crate) fn rows(&self, width: usize) -> usize {
        let state = &self.state;
        let insert = state.mode == edtui::EditorMode::Insert;
        let rows: usize = state
            .lines
            .iter_row()
            .enumerate()
            .map(|(row, line)| {
                let cursor_after =
                    insert && row == state.cursor.row && state.cursor.col >= line.len();
                wrapped_rows(line, width, cursor_after)
            })
            .sum();
        rows.max(1)
    }

    /// Draws the editor in `block` over `area`, wrapping its lines. With
    /// `cursor`, the terminal cursor goes to the editor's cursor outside
    /// search mode, where an input method puts its preedit; the bottom line
    /// has it in search mode.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, block: Block, cursor: bool) {
        // The terminal cursor marks the cursor; the selection and search
        // matches are reversed.
        let theme = EditorTheme::default()
            .base(Style::new())
            .hide_cursor()
            .selection_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(block)
            .hide_status_line();
        frame.render_widget(
            EditorView::new(&mut self.state).theme(theme).wrap(true),
            area,
        );
        if cursor
            && self.state.mode != edtui::EditorMode::Search
            && let Some(position) = self.state.cursor_screen_position()
        {
            frame.set_cursor_position(position);
        }
    }
}

/// A new editor state holding `text` that copies through `register`.
fn state_holding(text: &str, register: EditorRegister) -> EditorState {
    let mut state = EditorState::new(Lines::from(text));
    state.set_clipboard(register);
    state
}

/// Inserts `text` at the cursor, a `\n` starting a new line, with the
/// cursor after it, as typing it in insert mode would: with edtui's
/// `InsertChar`, which records the text for `.` in an insert session. The
/// text after the cursor is set aside meanwhile, so each character goes at
/// the end of the text instead of moving the rest along, which a paste of
/// up to 256 KiB would do once per character.
fn insert_text(state: &mut EditorState, text: &str) {
    if text.is_empty() {
        // `merge` would join the cursor's line to the line above.
        return;
    }
    let row = state.cursor.row;
    let col = state
        .lines
        .len_col(row)
        .map_or(0, |len| state.cursor.col.min(len));
    state.cursor = Index2::new(row, col);
    // With `col` 0 the cursor's whole line goes, and the first character
    // starts a new one.
    let mut rest = state.lines.split_off(state.cursor);
    for c in text.chars() {
        state.execute(InsertChar(c));
    }
    state.lines.merge(&mut rest);
}

/// The keys mxt handles in place of edtui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    /// `f`, `F` (`backward`), `t` and `T` (`till`), waiting for the
    /// character.
    Find { backward: bool, till: bool },
    /// `;`, and `,` (`reverse`).
    Repeat { reverse: bool },
    /// `h` and `<Left>`, `l` and `<Right>` (`forward`).
    Step { forward: bool },
    /// `r`, waiting for the character (in visual mode, to drop it).
    Replace,
    /// `"`, waiting for the register's name. edtui binds no key sequence
    /// that starts with `"` (it has `ci"`, `di"` and visual `i"`), so `"`
    /// did nothing before, and `_` moved to the first non-blank.
    Register,
}

impl Command {
    /// `key`'s command in normal or visual mode.
    fn of(key: Key, mode: edtui::EditorMode) -> Option<Command> {
        let normal_or_visual =
            matches!(mode, edtui::EditorMode::Normal | edtui::EditorMode::Visual);
        if !normal_or_visual || key.mods() != Modifiers::default() {
            return None;
        }
        let command = match key.code() {
            KeyCode::Char('f') => Command::Find {
                backward: false,
                till: false,
            },
            KeyCode::Char('F') => Command::Find {
                backward: true,
                till: false,
            },
            KeyCode::Char('t') => Command::Find {
                backward: false,
                till: true,
            },
            KeyCode::Char('T') => Command::Find {
                backward: true,
                till: true,
            },
            KeyCode::Char(';') => Command::Repeat { reverse: false },
            KeyCode::Char(',') => Command::Repeat { reverse: true },
            KeyCode::Char('h') | KeyCode::Left => Command::Step { forward: false },
            KeyCode::Char('l') | KeyCode::Right => Command::Step { forward: true },
            KeyCode::Char('r') => Command::Replace,
            KeyCode::Char('"') => Command::Register,
            _ => return None,
        };
        Some(command)
    }
}

/// A key waiting for the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// `f`, `F`, `t` or `T`.
    Find { backward: bool, till: bool },
    /// `r` in normal mode.
    Replace,
    /// edtui's operator `d`, `c` or `y` in normal mode. As in vim's
    /// operator-pending mode, the key goes to edtui: `dw`, `yy`, and
    /// `d<Esc>`, which cancels.
    Operator(char),
    /// A text object's `i` after `d` or `c`, or in visual mode: the object
    /// goes to edtui (`ci[`, `vi"`).
    Object,
    /// edtui's `df`, `dt` (`till`), `cf` or `ct`: the character goes to
    /// edtui.
    Char { till: bool },
    /// `g`, which edtui holds for `gg`. As in nvim, the key goes to edtui:
    /// `gg`, and `g<Esc>`, which cancels; edtui drops any other key with
    /// the `g` (`g]`, `gx`).
    G,
    /// `dF`, `yf`, `gt`, `ca`, `yi`, visual `a`, visual `r` and the like,
    /// which edtui does not have: the key is dropped.
    Drop,
    /// `"`: `_` names the black hole register, another character is
    /// refused, and any other key cancels.
    Register,
}

/// What waits after `key`, which edtui got in `mode` while `waiting` waited
/// for it, when the key started no command of mxt's; the key after it goes
/// to edtui without the keymap. This follows edtui 0.11.8's key sequences,
/// to check again when edtui changes: `g` starts only `gg`, `y` only `yy`;
/// `d` and `c` start `df`, `dt`, `cf` and `ct`, which take a character, and
/// `di` and `ci`, which take a text object, as visual `i` does. edtui has no
/// `a` objects, no `yi` and no other find after a key (`dF`, `yf`, `gt`). It
/// dropped those keys, and the key after them is dropped too rather than
/// run as a command. A text object edtui does not know (`cif`) drops its
/// keys, and the next key runs.
fn edtui_waits(waiting: Option<Pending>, key: Key, mode: edtui::EditorMode) -> Option<Pending> {
    use edtui::EditorMode::{Normal, Visual};
    let KeyCode::Char(c) = key.code() else {
        return None;
    };
    if key.mods() != Modifiers::default() {
        return None;
    }
    match (waiting, mode, c) {
        (None, Normal | Visual, 'g') => Some(Pending::G),
        (None, Normal, 'd' | 'c' | 'y') => Some(Pending::Operator(c)),
        (None, Visual, 'i') | (Some(Pending::Operator('d' | 'c')), _, 'i') => Some(Pending::Object),
        (Some(Pending::Operator('d' | 'c')), _, 'f' | 't') => {
            Some(Pending::Char { till: c == 't' })
        }
        (None, Visual, 'a')
        | (Some(Pending::Operator(_)), _, 'i' | 'a' | 'f' | 'F' | 't' | 'T')
        | (Some(Pending::G), _, 'f' | 'F' | 't' | 'T') => Some(Pending::Drop),
        _ => None,
    }
}

/// edtui's vim keys with mxt's keys over them.
fn key_handler() -> EditorEventHandler {
    let mut keys = EditorEventHandler::vim_mode();
    // edtui's `<C-e>` in normal mode asks for the external editor, which
    // only the keymap's `composer.external_editor` opens.
    keys.key_handler
        .remove(&KeyEventRegister::n(vec![KeyInput::ctrl('e')]));
    // Over edtui's `f`, `t`, `h`, `l`, `<Left>` and `<Right>`, so each key
    // has one implementation, and `"`, which edtui does not bind alone.
    let candidates = "fFtT;,hlr\""
        .chars()
        .map(Key::char)
        .chain([KeyCode::Left, KeyCode::Right].map(|code| Key::new(code, Modifiers::default())));
    for key in candidates {
        for mode in [edtui::EditorMode::Normal, edtui::EditorMode::Visual] {
            if Command::of(key, mode).is_some()
                && let Some(input) = key_input(key, mode)
            {
                keys.key_handler
                    .insert(KeyEventRegister::new(vec![input], mode), MARK);
            }
        }
    }
    keys
}

/// An `f`, `F`, `t` or `T` with its character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Find {
    target: char,
    backward: bool,
    till: bool,
}

/// The character a key gives `f`, `t` or `r`: a character without ctrl or
/// alt, or `<Tab>`. Other keys, `<Esc>` among them, cancel.
fn char_arg(key: Key) -> Option<char> {
    match key.code() {
        KeyCode::Tab if key.mods() == Modifiers::default() => Some('\t'),
        _ => key.text(),
    }
}

/// The columns where the grapheme clusters of `line` start.
fn cluster_starts(line: &[char]) -> Vec<usize> {
    let text: String = line.iter().collect();
    let mut col = 0;
    text.graphemes(true)
        .map(|cluster| {
            let start = col;
            col += cluster.chars().count();
            start
        })
        .collect()
}

/// Where `h` (`l` with `forward`) moves from `col` on `line`: to the start
/// of the grapheme cluster before (after) the one under the cursor, which
/// may sit inside a cluster after edtui's motions (`$`). With `past_end`
/// (visual mode), the cursor can go one column past the end, as edtui lets
/// it.
fn step(line: &[char], col: usize, forward: bool, past_end: bool) -> usize {
    let mut stops = cluster_starts(line);
    if past_end {
        stops.push(line.len());
    }
    let Some(at) = stops.partition_point(|&stop| stop <= col).checked_sub(1) else {
        return col;
    };
    if forward {
        stops.get(at + 1).copied().unwrap_or(col)
    } else {
        stops[at.saturating_sub(1)]
    }
}

/// Where `find` moves the cursor from `col` on `line`, if its character is
/// there. Like vim, the search goes by grapheme cluster and matches a
/// cluster's first character. With `repeat` (`;` and `,`), `t` and `T` skip
/// the cluster next to the cursor, so they do not stay before the match
/// they stopped at (vim without `cpo-;`).
fn find_col(line: &[char], col: usize, find: Find, repeat: bool) -> Option<usize> {
    let starts = cluster_starts(line);
    // The cluster under the cursor; one after the last when the cursor is
    // past the end (visual mode).
    let at = if col < line.len() {
        starts.partition_point(|&start| start <= col) - 1
    } else {
        starts.len()
    };
    let skip = usize::from(repeat && find.till);
    let is_target = |&index: &usize| line[starts[index]] == find.target;
    if find.backward {
        let hit = (0..at.saturating_sub(skip)).rev().find(is_target)?;
        // `T` right before a cursor past the end stays there.
        Some(if find.till {
            starts.get(hit + 1).copied().unwrap_or(col)
        } else {
            starts[hit]
        })
    } else {
        let hit = (at + 1 + skip..starts.len()).find(is_target)?;
        Some(if find.till {
            starts[hit - 1]
        } else {
            starts[hit]
        })
    }
}

/// The rows `line` takes at `width` columns; with `cursor_after`, the cursor
/// after its end takes a column as well.
fn wrapped_rows(line: &[char], width: usize, cursor_after: bool) -> usize {
    let text: String = line.iter().collect();
    let columns = text.graphemes(true).map(|cluster| match cluster {
        "\t" => TAB_WIDTH,
        cluster => grapheme_width(cluster),
    });
    let mut rows = 1;
    let mut used = 0;
    for columns in columns.chain(cursor_after.then_some(1)) {
        if used > 0 && used + columns > width {
            rows += 1;
            used = 0;
        }
        used += columns;
    }
    rows
}

/// A core key as edtui's input in `mode`.
///
/// The core folds shift into characters, while edtui binds capital letters
/// with shift (`G`); in search mode edtui only takes characters without
/// modifiers, so there capitals stay without shift.
fn key_input(key: Key, mode: edtui::EditorMode) -> Option<KeyInput> {
    let mods = key.mods();
    let mut shift = mods.shift;
    // Only codes edtui converts: its conversion panics on the others.
    let code = match key.code() {
        KeyCode::Char(c) => {
            shift |= c.is_uppercase() && mode != edtui::EditorMode::Search;
            C::Char(c)
        }
        KeyCode::Enter => C::Enter,
        KeyCode::Esc => C::Esc,
        KeyCode::Tab => C::Tab,
        KeyCode::Backspace => C::Backspace,
        KeyCode::Delete => C::Delete,
        KeyCode::Up => C::Up,
        KeyCode::Down => C::Down,
        KeyCode::Left => C::Left,
        KeyCode::Right => C::Right,
        KeyCode::Home => C::Home,
        KeyCode::End => C::End,
        KeyCode::PageUp => C::PageUp,
        KeyCode::PageDown => C::PageDown,
        KeyCode::Insert | KeyCode::F(_) => return None,
    };
    let mut modifiers = M::NONE;
    modifiers.set(M::CONTROL, mods.ctrl);
    modifiers.set(M::ALT, mods.alt);
    modifiers.set(M::SHIFT, shift);
    Some(KeyInput::with_modifiers(code, modifiers))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use edtui::clipboard::ClipboardTrait as _;
    use edtui::{EditorMode as Mode, Index2};
    use mxt_core::keys::KeySeq;

    pub(crate) fn keys(text: &str) -> Vec<Key> {
        KeySeq::parse(text, &KeySeq::default())
            .unwrap()
            .keys()
            .to_vec()
    }

    fn input(text: &str, mode: Mode) -> Option<KeyInput> {
        key_input(keys(text)[0], mode)
    }

    #[test]
    fn capitals_get_shift_except_in_search() {
        assert_eq!(input("G", Mode::Normal), Some(KeyInput::shift('G')));
        assert_eq!(input("G", Mode::Search), Some(KeyInput::new('G')));
        assert_eq!(input("g", Mode::Normal), Some(KeyInput::new('g')));
        // edtui binds symbols without shift.
        assert_eq!(input("$", Mode::Normal), Some(KeyInput::new('$')));
    }

    #[test]
    fn modifiers_and_special_keys() {
        assert_eq!(input("<C-r>", Mode::Normal), Some(KeyInput::ctrl('r')));
        assert_eq!(input("<M-x>", Mode::Normal), Some(KeyInput::alt('x')));
        assert_eq!(input("<Esc>", Mode::Insert), Some(KeyInput::new(C::Esc)));
        assert_eq!(
            input("<S-Tab>", Mode::Insert),
            Some(KeyInput::shift(C::Tab))
        );
        assert_eq!(input("<F1>", Mode::Normal), None);
        assert_eq!(input("<Insert>", Mode::Normal), None);
    }

    #[test]
    fn fed_keys_move_search_and_yank() {
        let register = SharedRegister::default();
        let mut editor = Editor::new("first line\nSecond line", register.clone());
        feed(&mut editor, "G");
        assert_eq!(editor.state.cursor.row, 1);
        feed(&mut editor, "gg/Sec");
        assert_eq!(editor.mode(), EditorMode::Search);
        assert_eq!(editor.search().as_deref(), Some("Sec"));
        feed(&mut editor, "<CR>yy");
        assert_eq!(editor.mode(), EditorMode::Normal);
        assert!(register.take_unsent().is_some());
    }

    fn holding(text: &str) -> Editor {
        Editor::new(text, SharedRegister::default())
    }

    /// Feeds the keys in `text`; what the editor reported.
    fn feed(editor: &mut Editor, text: &str) -> Vec<Msg> {
        let mut msgs = Vec::new();
        keys(text)
            .into_iter()
            .for_each(|key| editor.feed(key, &mut msgs));
        msgs
    }

    /// The cursor's column after each of `steps`.
    fn cols(editor: &mut Editor, steps: &[&str]) -> Vec<usize> {
        steps
            .iter()
            .map(|step| {
                feed(editor, step);
                editor.state.cursor.col
            })
            .collect()
    }

    fn text(editor: &Editor) -> String {
        editor.state.lines.to_string()
    }

    #[test]
    fn f_and_capital_f_find_kanji_and_semicolon_and_comma_repeat() {
        // a b c 日 本 語 、 本 d e f
        let mut editor = holding("abc日本語、本def");
        let steps = ["f本", ";", ";", ",", "$", "F本", ";", ","];
        assert_eq!(cols(&mut editor, &steps), [4, 7, 7, 4, 10, 7, 4, 7]);
    }

    #[test]
    fn t_and_capital_t_stop_next_to_a_full_width_comma_and_repeat_past_it() {
        // は い 、 そ う で す 、 ね
        let mut editor = holding("はい、そうです、ね");
        // `;` after `t、` moves on to the next comma instead of staying.
        let steps = ["t、", ";", ";", ","];
        assert_eq!(cols(&mut editor, &steps), [1, 6, 6, 3]);
        // `T、` right after a comma stays; `;` goes on to the one before.
        let steps = ["$", "T、", ";", ","];
        assert_eq!(cols(&mut editor, &steps), [8, 8, 3, 6]);
    }

    #[test]
    fn finds_stay_on_the_line_and_esc_cancels() {
        let mut editor = holding("日本\n語本");
        feed(&mut editor, "f語");
        assert_eq!(editor.state.cursor, Index2::new(0, 0));
        feed(&mut editor, "f本j0");
        assert_eq!(editor.state.cursor, Index2::new(1, 0));
        feed(&mut editor, "f");
        assert_eq!(editor.key_mode(), EditorMode::AwaitingChar);
        // `<Esc>` cancels the `f` and keeps the last find for `;`.
        feed(&mut editor, "<Esc>");
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        assert_eq!(editor.state.cursor, Index2::new(1, 0));
        feed(&mut editor, ";");
        assert_eq!(editor.state.cursor, Index2::new(1, 1));
    }

    #[test]
    fn finds_skip_whole_grapheme_clusters() {
        // 👍 🏽 、 x 👍 🏽 、: the emoji is two characters, and `t` and `T`
        // stop on its first, not inside it.
        let mut editor = holding("👍🏽、x👍🏽、");
        let steps = ["t、", "$", "T👍", "F👍", ";"];
        assert_eq!(cols(&mut editor, &steps), [0, 6, 6, 4, 0]);
    }

    #[test]
    fn finds_extend_a_visual_selection() {
        let register = SharedRegister::default();
        let mut editor = Editor::new("abc日本語", register.clone());
        let mut yanked = register.for_editor();
        feed(&mut editor, "vf本y");
        assert_eq!(yanked.get_text(), "abc日本");
        // `Tc` from the end.
        feed(&mut editor, "$vT");
        assert_eq!(editor.key_mode(), EditorMode::AwaitingChar);
        feed(&mut editor, "c");
        assert_eq!(editor.mode(), EditorMode::Visual);
        feed(&mut editor, "y");
        assert_eq!(yanked.get_text(), "日本語");
    }

    #[test]
    fn operators_keep_edtui_df_and_dt_and_drop_d_capital_f() {
        let mut editor = holding("日本、語");
        feed(&mut editor, "dt、");
        assert_eq!(text(&editor), "、語");

        // edtui has no `dF`: nothing happens, and the `.` meant for it does
        // not repeat the `x`.
        let mut editor = holding("a.b.c");
        feed(&mut editor, "x$");
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        feed(&mut editor, "dF");
        assert_eq!(editor.key_mode(), EditorMode::AwaitingChar);
        feed(&mut editor, ".");
        assert_eq!(
            (text(&editor).as_str(), editor.state.cursor.col),
            (".b.c", 3)
        );
        feed(&mut editor, "x");
        assert_eq!(text(&editor), ".b.");

        // `F` as `df`'s character, and the key after it runs.
        let mut editor = holding("aFbFc");
        feed(&mut editor, "dfFx");
        assert_eq!(text(&editor), "Fc");
        // `f` after a finished `dd` is a find again.
        let mut editor = holding("x\nab");
        feed(&mut editor, "ddfb");
        assert_eq!((text(&editor).as_str(), editor.state.cursor.col), ("ab", 1));

        // `;` after edtui's `df.` repeats `f.`.
        let mut editor = holding("a.b.c.d");
        feed(&mut editor, "df.;");
        assert_eq!(
            (text(&editor).as_str(), editor.state.cursor.col),
            ("b.c.d", 1)
        );
        // A key edtui does not get cancels its `df`, so the next key runs.
        let mut editor = holding("axb");
        feed(&mut editor, "df<F1>");
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        feed(&mut editor, "x");
        assert_eq!(text(&editor), "xb");
    }

    #[test]
    fn the_character_after_a_find_edtui_drops_is_dropped() {
        // edtui has no `yf`, `yt`, `gf` or visual `r`. Their character is
        // not run as a command: `.` repeating the `x`, `x`, `u`.
        for (typed, expected) in [
            ("xyf.", "bcd"),
            ("yfx", "abcd"),
            ("xytu", "bcd"),
            ("gfx", "abcd"),
            ("lvlrx", "abcd"),
        ] {
            let mut editor = holding("abcd");
            feed(&mut editor, typed);
            assert_eq!(text(&editor), expected, "{typed}");
            assert_ne!(editor.key_mode(), EditorMode::AwaitingChar, "{typed}");
        }
        // The key after it runs.
        let mut editor = holding("abcd");
        feed(&mut editor, "yf.x");
        assert_eq!(text(&editor), "bcd");
    }

    #[test]
    fn the_key_after_an_operator_goes_to_edtui() {
        // As in vim's operator-pending mode, the key after `d`, `c` or `y`
        // skips the keymap: edtui runs its command or drops both keys, and
        // the key after them runs.
        for operator in ["d", "c", "y"] {
            let mut editor = holding("one two");
            feed(&mut editor, operator);
            assert_eq!(editor.key_mode(), EditorMode::AwaitingChar, "{operator}");
            feed(&mut editor, "]");
            assert_eq!(editor.key_mode(), EditorMode::Normal, "{operator}");
            feed(&mut editor, "x");
            assert_eq!(text(&editor), "ne two", "{operator}");
        }
        for (typed, expected) in [("dw", "two"), ("wdiW", "one "), ("d<Esc>x", "ne two")] {
            let mut editor = holding("one two");
            feed(&mut editor, typed);
            assert_eq!(
                (text(&editor).as_str(), editor.key_mode()),
                (expected, EditorMode::Normal),
                "{typed}"
            );
        }
    }

    #[test]
    fn the_key_after_g_goes_to_edtui() {
        // As in nvim, which reads the key after `g` without mappings, in
        // normal and visual mode.
        for typed in ["g", "vg"] {
            let mut editor = holding("one two");
            feed(&mut editor, typed);
            assert_eq!(editor.key_mode(), EditorMode::AwaitingChar, "{typed}");
        }
        let mut editor = holding("one\ntwo");
        feed(&mut editor, "jgg");
        assert_eq!(
            (editor.state.cursor.row, editor.key_mode()),
            (0, EditorMode::Normal)
        );
        // edtui's only `g` command is `gg`: it drops any other key with the
        // `g`, `<Esc>` among them, and the key after them runs.
        for typed in ["g]", "g<Esc>"] {
            let mut editor = holding("one two");
            feed(&mut editor, typed);
            assert_eq!(
                (text(&editor).as_str(), editor.key_mode()),
                ("one two", EditorMode::Normal),
                "{typed}"
            );
            feed(&mut editor, "x");
            assert_eq!(text(&editor), "ne two", "{typed}");
        }
    }

    #[test]
    fn text_objects_take_their_key_without_the_keymap() {
        let markdown = r#"[link](url) "quoted""#;
        let mut editor = holding(markdown);
        feed(&mut editor, "lc");
        assert_eq!(editor.key_mode(), EditorMode::AwaitingChar);
        feed(&mut editor, "i");
        assert_eq!(editor.key_mode(), EditorMode::AwaitingChar);
        feed(&mut editor, "[");
        assert_eq!(editor.key_mode(), EditorMode::Insert);
        feed(&mut editor, "text<Esc>");
        assert_eq!(text(&editor), r#"[text](url) "quoted""#);

        for (typed, expected) in [
            ("ldi[", r#"[](url) "quoted""#),
            ("f(ldi(", r#"[link]() "quoted""#),
            (r#"f"lci"x<Esc>"#, r#"[link](url) "x""#),
            ("lvi]d", r#"[](url) "quoted""#),
            // edtui has no `a` objects and no `yi`: the object is dropped,
            // and the key after it runs.
            ("lca]x", r#"[ink](url) "quoted""#),
            ("f(lda(x", r#"[link](rl) "quoted""#),
            (r#"f"lyi"x"#, r#"[link](url) "uoted""#),
            ("lva]x", r#"[ink](url) "quoted""#),
            // An object edtui does not know drops its keys, and `x` runs.
            ("ldifx", r#"[ink](url) "quoted""#),
        ] {
            let mut editor = holding(markdown);
            feed(&mut editor, typed);
            assert_eq!(
                (text(&editor).as_str(), editor.key_mode()),
                (expected, EditorMode::Normal),
                "{typed}"
            );
        }
    }

    #[test]
    fn v_and_capital_v_switch_the_selection_kind_from_the_same_anchor() {
        let (mut editor, register) = with_register("one two\nthree four\nfive");
        let mut yanked = register.for_editor();
        // `V` on `two`'s `t`, `j`, then `v`: characters from that `t` to the
        // cursor.
        feed(&mut editor, "wVjv");
        let selection = editor.state.selection.clone().unwrap();
        assert!(!selection.line_mode);
        assert_eq!(
            (selection.start(), selection.end()),
            (Index2::new(0, 4), Index2::new(1, 4))
        );
        // `v` again keeps the selection, and visual mode.
        feed(&mut editor, "v");
        assert_eq!(editor.state.selection, Some(selection));
        assert_eq!(editor.mode(), EditorMode::Visual);
        feed(&mut editor, "y");
        assert_eq!(yanked.get_text(), "two\nthree");

        // `v` on the same `t`, `j`, then `V`: both lines whole.
        feed(&mut editor, "gg0wvjV");
        let selection = editor.state.selection.clone().unwrap();
        assert!(selection.line_mode);
        assert_eq!(
            (selection.start(), selection.end()),
            (Index2::new(0, 0), Index2::new(1, 9))
        );
        feed(&mut editor, "V");
        assert_eq!(editor.state.selection, Some(selection));
        assert_eq!(editor.mode(), EditorMode::Visual);
    }

    #[test]
    fn a_waiting_key_counts_only_in_normal_and_visual_mode() {
        // The mode changes without a key while `f` or `r` waits: in insert
        // mode the next key is typed.
        for waiting in ["f", "r"] {
            let mut editor = holding("abc");
            feed(&mut editor, waiting);
            editor.state.execute(SwitchMode(Mode::Insert));
            assert_eq!(editor.key_mode(), EditorMode::Insert, "{waiting}");
            feed(&mut editor, "z");
            assert_eq!(text(&editor), "zabc", "{waiting}");
        }
    }

    #[test]
    fn entering_normal_mode_from_insert_mode_is_esc() {
        // The cursor moves onto the `o`, and `.` repeats the insert there.
        let mut editor = holding("");
        feed(&mut editor, "ihello");
        editor.enter_normal_mode();
        assert_eq!(
            (editor.mode(), editor.state.cursor.col),
            (EditorMode::Normal, 4)
        );
        feed(&mut editor, ".");
        assert_eq!(text(&editor), "hellhelloo");
    }

    #[test]
    fn entering_normal_mode_drops_a_selection_and_stops_a_search() {
        let mut editor = holding("one two");
        feed(&mut editor, "wvl");
        editor.enter_normal_mode();
        assert_eq!(editor.mode(), EditorMode::Normal);
        assert!(editor.state.selection.is_none());

        // As `<Esc>` does, the cursor goes back to where the search started.
        feed(&mut editor, "0/two");
        editor.enter_normal_mode();
        assert_eq!(
            (editor.mode(), editor.state.cursor.col),
            (EditorMode::Normal, 0)
        );
        assert_eq!(editor.state.search_pattern(), "");
    }

    #[test]
    fn entering_normal_mode_keeps_normal_mode_and_cancels_a_waiting_key() {
        // The cursor stays on the last character.
        let mut editor = holding("hello");
        feed(&mut editor, "$");
        editor.enter_normal_mode();
        assert_eq!(
            (editor.mode(), editor.state.cursor.col),
            (EditorMode::Normal, 4)
        );
        // edtui's `d` no longer waits, so `x` deletes.
        feed(&mut editor, "0d");
        editor.enter_normal_mode();
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        feed(&mut editor, "x");
        assert_eq!(text(&editor), "ello");
    }

    #[test]
    fn r_replaces_the_character_under_the_cursor_and_undoes() {
        let mut editor = holding("日本語");
        feed(&mut editor, "lr");
        assert_eq!(editor.key_mode(), EditorMode::AwaitingChar);
        feed(&mut editor, "漢");
        assert_eq!(
            (text(&editor).as_str(), editor.state.cursor.col),
            ("日漢語", 1)
        );
        feed(&mut editor, "u");
        assert_eq!(text(&editor), "日本語");
        // `r<Esc>` replaces nothing, and the next key runs.
        feed(&mut editor, "0lr<Esc>");
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        assert_eq!(text(&editor), "日本語");
        feed(&mut editor, "x");
        assert_eq!(text(&editor), "日語");
    }

    #[test]
    fn h_and_l_step_over_grapheme_clusters() {
        // a 👍🏽 b 👨‍👩‍👧 c ❤️ d, starting at columns 0 1 3 4 9 10 12.
        let mut editor = holding("a👍🏽b👨\u{200d}👩\u{200d}👧c❤\u{fe0f}d");
        let steps = ["l", "l", "<Right>", "l", "l", "l", "l"];
        assert_eq!(cols(&mut editor, &steps), [1, 3, 4, 9, 10, 12, 12]);
        let steps = ["h", "h", "<Left>", "h", "h", "h", "h"];
        assert_eq!(cols(&mut editor, &steps), [10, 9, 4, 3, 1, 0, 0]);
        // In visual mode too.
        assert_eq!(cols(&mut editor, &["vl", "l", "h"]), [1, 3, 1]);

        // `$` puts the cursor on the skin tone modifier; `h` leaves the
        // whole emoji.
        let mut editor = holding("ok👍🏽");
        assert_eq!(cols(&mut editor, &["$", "h"]), [3, 1]);
    }

    #[test]
    fn word_motions_use_vim_character_classes() {
        // As in vim, kanji, hiragana and katakana are separate classes:
        // 日本語 の テキスト start at columns 0, 3 and 4.
        let mut editor = holding("日本語のテキスト");
        let steps = ["w", "w", "b", "b", "e", "e", "e"];
        assert_eq!(cols(&mut editor, &steps), [3, 4, 3, 0, 2, 3, 7]);
        feed(&mut editor, "0dw");
        assert_eq!(text(&editor), "のテキスト");

        // As in vim, `b` from `」` stops at `日`, where the bracket's class
        // ends the word, and only the next `b` goes to the bracket.
        let mut editor = holding("「日本」");
        assert_eq!(cols(&mut editor, &["w", "w", "b", "b"]), [1, 3, 1, 0]);
    }

    /// The cursor after `typed`, starting at `(row, col)`.
    fn after(editor: &mut Editor, (row, col): (usize, usize), typed: &str) -> Index2 {
        editor.state.cursor = Index2::new(row, col);
        feed(editor, typed);
        editor.state.cursor
    }

    #[test]
    fn up_on_the_first_line_and_down_on_the_last_go_to_the_start_and_the_end() {
        // Insert mode: after the last character.
        let mut editor = holding("ab\ncd");
        feed(&mut editor, "i");
        assert_eq!(after(&mut editor, (0, 1), "<Up>"), Index2::new(0, 0));
        assert_eq!(after(&mut editor, (1, 0), "<Down>"), Index2::new(1, 2));
        // Normal mode: on the last character.
        let mut editor = holding("ab\ncd");
        assert_eq!(after(&mut editor, (0, 1), "<Up>"), Index2::new(0, 0));
        assert_eq!(after(&mut editor, (1, 0), "<Down>"), Index2::new(1, 1));
        // `a👍🏽` is `a`, `👍` and the skin tone modifier. Normal mode stops
        // on the emoji's first character, not inside the cluster; insert
        // mode goes after it.
        let mut editor = holding("ab\na👍🏽");
        assert_eq!(after(&mut editor, (1, 0), "<Down>"), Index2::new(1, 1));
        feed(&mut editor, "i");
        assert_eq!(after(&mut editor, (1, 0), "<Down>"), Index2::new(1, 3));
        // One line is the first and the last.
        let mut editor = holding("one");
        assert_eq!(after(&mut editor, (0, 1), "<Down>"), Index2::new(0, 2));
        assert_eq!(after(&mut editor, (0, 1), "<Up>"), Index2::new(0, 0));
    }

    #[test]
    fn up_and_down_between_the_edges_move_a_line() {
        for insert in [false, true] {
            let mut editor = holding("one\ntwo\nthree");
            if insert {
                feed(&mut editor, "i");
            }
            assert_eq!(
                after(&mut editor, (1, 1), "<Up>"),
                Index2::new(0, 1),
                "{insert}"
            );
            assert_eq!(
                after(&mut editor, (1, 1), "<Down>"),
                Index2::new(2, 1),
                "{insert}"
            );
        }
    }

    #[test]
    fn up_and_down_in_an_empty_editor_stay() {
        for typed in ["<Up>", "<Down>", "i<Up>", "i<Down>"] {
            let mut editor = holding("");
            feed(&mut editor, typed);
            assert_eq!(editor.state.cursor, Index2::new(0, 0), "{typed}");
            assert_eq!(text(&editor), "", "{typed}");
        }
    }

    #[test]
    fn up_from_the_second_row_of_a_wrapped_first_line_goes_to_the_start() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // At four columns `abcdef` takes two rows, `abcd` and `ef`; the
        // cursor on `f` is on the second.
        for insert in [false, true] {
            let mut editor = holding("abcdef\ngh");
            if insert {
                feed(&mut editor, "i");
            }
            editor.state.cursor = Index2::new(0, 5);
            let mut terminal = Terminal::new(TestBackend::new(4, 3)).unwrap();
            let mut screen_row = |editor: &mut Editor| {
                terminal
                    .draw(|frame| editor.draw(frame, frame.area(), Block::new(), true))
                    .unwrap();
                editor.state.cursor_screen_position().unwrap().y
            };
            assert_eq!(screen_row(&mut editor), 1, "{insert}");
            feed(&mut editor, "<Up>");
            assert_eq!(editor.state.cursor, Index2::new(0, 0), "{insert}");
            assert_eq!(screen_row(&mut editor), 0, "{insert}");
        }
    }

    #[test]
    fn visual_mode_k_j_and_a_waiting_key_leave_up_and_down_to_edtui() {
        // edtui's visual `<Up>` and `<Down>` stop at the first and the last
        // line. The last line is longer, so normal mode's `<Down>` would go
        // to `(1, 2)`.
        let mut editor = holding("ab\ncde");
        feed(&mut editor, "lv<Up>");
        assert_eq!(editor.state.cursor, Index2::new(0, 1));
        feed(&mut editor, "j<Down>");
        assert_eq!(editor.state.cursor, Index2::new(1, 1));
        // `k` and `j` stay vim's.
        let mut editor = holding("ab\ncd");
        assert_eq!(after(&mut editor, (0, 1), "k"), Index2::new(0, 1));
        assert_eq!(after(&mut editor, (1, 0), "j"), Index2::new(1, 0));
        // The key after `g` goes to edtui, which drops it with the `g`.
        assert_eq!(after(&mut editor, (0, 1), "g<Up>"), Index2::new(0, 1));
    }

    /// An editor holding `text`, and its register holding `before`, which
    /// has gone to the terminal clipboard.
    fn with_register(text: &str) -> (Editor, SharedRegister) {
        let register = SharedRegister::default();
        register.copy("before".to_owned());
        register.take_unsent();
        (Editor::new(text, register.clone()), register)
    }

    /// The register's text, and whether a copy waits for the terminal
    /// clipboard.
    fn copied(register: &SharedRegister) -> (String, bool) {
        let text = register.for_editor().get_text();
        (text, register.take_unsent().is_some())
    }

    fn before() -> (String, bool) {
        ("before".to_owned(), false)
    }

    #[test]
    fn commands_after_the_black_hole_register_change_the_text_and_write_nothing() {
        for (typed, plain) in [
            (r#""_dd"#, "dd"),
            (r#"w"_diw"#, "wdiw"),
            (r#""_cwX<Esc>"#, "cwX<Esc>"),
            (r#"w"_D"#, "wD"),
            (r#""_yy"#, "yy"),
            // edtui's `x` writes no register.
            (r#""_x"#, "x"),
            (r#"ve"_d"#, "ved"),
            (r#"ve"_cX<Esc>"#, "vecX<Esc>"),
        ] {
            let (mut editor, register) = with_register("one two\nthree");
            feed(&mut editor, plain);
            let expected = text(&editor);
            assert_eq!(copied(&register).1, plain != "x", "{plain}");

            let (mut editor, register) = with_register("one two\nthree");
            assert_eq!(feed(&mut editor, typed), [], "{typed}");
            assert_eq!(text(&editor), expected, "{typed}");
            assert_eq!(copied(&register), before(), "{typed}");
            assert_eq!(editor.key_mode(), EditorMode::Normal, "{typed}");
        }
    }

    #[test]
    fn the_black_hole_register_ends_with_its_command_however_the_key_ends() {
        // After `"_`: a motion, mxt's find and replace with their
        // character, a key edtui has no code for, and `<Up>` on the first
        // line.
        for command in ["j", "fe", "rx", "<F1>", "<Up>"] {
            let (mut editor, register) = with_register("one\ntwo\nthree");
            feed(&mut editor, r#""_"#);
            assert_eq!(editor.key_mode(), EditorMode::AwaitingChar, "{command}");
            feed(&mut editor, command);
            assert_eq!(editor.key_mode(), EditorMode::Normal, "{command}");
            feed(&mut editor, "dd");
            assert!(copied(&register).1, "{command}");
        }
        // A paste ends it too.
        let (mut editor, register) = with_register("one\ntwo");
        feed(&mut editor, r#""_"#);
        editor.paste("x");
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        feed(&mut editor, "dd");
        assert_eq!(copied(&register), ("\noxne".to_owned(), true));
    }

    #[test]
    fn dot_after_black_hole_dd_writes_and_black_hole_dot_does_not() {
        let (mut editor, register) = with_register("one\ntwo\nthree\nfour");
        feed(&mut editor, r#""_dd"#);
        feed(&mut editor, r#""_."#);
        assert_eq!(text(&editor), "three\nfour");
        assert_eq!(copied(&register), before());
        feed(&mut editor, ".");
        assert_eq!(text(&editor), "four");
        assert_eq!(copied(&register), ("\nthree".to_owned(), true));
    }

    #[test]
    fn other_registers_are_refused_and_change_nothing() {
        let refused = || vec![Msg::EditorRefused(ONLY_BLACK_HOLE.to_owned())];
        let (mut editor, register) = with_register("one\ntwo");
        // Once, and the `a` appends nothing.
        assert_eq!(feed(&mut editor, r#""a"#), refused());
        assert_eq!(
            (text(&editor).as_str(), editor.key_mode()),
            ("one\ntwo", EditorMode::Normal)
        );
        feed(&mut editor, "dd");
        assert_eq!(copied(&register), ("\none".to_owned(), true));
        // `<Esc>` cancels silently, and a register named again replaces
        // `"_`, as in vim.
        assert_eq!(feed(&mut editor, r#""<Esc>"#), []);
        assert_eq!(feed(&mut editor, r#""_"+"#), refused());
        assert_eq!(editor.key_mode(), EditorMode::Normal);
        feed(&mut editor, "yy");
        assert!(copied(&register).1);
    }

    #[test]
    fn the_black_hole_register_keeps_another_editors_copies() {
        let register = SharedRegister::default();
        let mut composer = Editor::new("one", register.clone());
        let mut view = Editor::new("two", register.clone());
        feed(&mut composer, r#""_d"#);
        feed(&mut view, "yy");
        let yanked = copied(&register);
        assert!(yanked.1);
        feed(&mut composer, "d");
        assert_eq!(text(&composer), "");
        assert_eq!(copied(&register), (yanked.0, false));
    }

    #[test]
    fn a_paste_in_normal_mode_goes_after_the_cursor_as_one_undo_step() {
        let register = SharedRegister::default();
        register.copy("yanked".to_owned());
        register.take_unsent();
        // The cursor on `👍🏽`: the text goes after the whole cluster.
        let mut editor = Editor::new("a👍🏽b", register.clone());
        feed(&mut editor, "l");
        editor.paste("x\ny");
        assert_eq!(text(&editor), "a👍🏽x\nyb");
        assert_eq!(editor.mode(), EditorMode::Normal);
        assert_eq!(editor.state.cursor, Index2::new(1, 0));
        feed(&mut editor, "u");
        assert_eq!(text(&editor), "a👍🏽b");
        // The register still holds the yank, and nothing goes to the
        // clipboard.
        assert_eq!(register.take_unsent(), None);
        feed(&mut editor, "0p");
        assert_eq!(text(&editor), "ayanked👍🏽b");

        // In an empty editor, and with a key waiting, which the paste
        // cancels.
        let mut editor = holding("");
        feed(&mut editor, "f");
        editor.paste("hi");
        assert_eq!(
            (text(&editor).as_str(), editor.key_mode()),
            ("hi", EditorMode::Normal)
        );
    }

    #[test]
    fn a_paste_in_visual_mode_replaces_the_selection() {
        let mut editor = holding("hello world");
        feed(&mut editor, "ve");
        editor.paste("bye\nall");
        assert_eq!(text(&editor), "bye\nall world");
        assert_eq!(editor.mode(), EditorMode::Normal);
        assert!(editor.state.selection.is_none());
        feed(&mut editor, "u");
        assert_eq!(text(&editor), "hello world");
    }

    #[test]
    fn a_paste_in_linewise_visual_mode_replaces_the_lines() {
        // As in neovim, the text goes before the line after them.
        let mut editor = holding("a\nb\nc");
        feed(&mut editor, "jV");
        editor.paste("X\nY");
        assert_eq!(text(&editor), "a\nX\nYc");
        assert_eq!(editor.mode(), EditorMode::Normal);
        assert_eq!(editor.state.cursor, Index2::new(2, 0));
        feed(&mut editor, "u");
        assert_eq!(text(&editor), "a\nb\nc");
        // On the last line there is no line after.
        let mut editor = holding("a\nb");
        feed(&mut editor, "jV");
        editor.paste("X");
        assert_eq!(text(&editor), "a\nX");
    }

    #[test]
    fn a_paste_in_insert_mode_keeps_the_text_around_it() {
        let mut editor = holding("one\ntwo\nthree");
        feed(&mut editor, "jli");
        editor.paste("A\nB");
        assert_eq!(text(&editor), "one\ntA\nBwo\nthree");
        assert_eq!(editor.state.cursor, Index2::new(2, 1));
        // At the start of a line, and an empty paste, which joins nothing.
        let mut editor = holding("one\ntwo");
        feed(&mut editor, "ji");
        editor.paste("");
        editor.paste("A\n");
        assert_eq!(text(&editor), "one\nA\ntwo");
        assert_eq!(editor.state.cursor, Index2::new(2, 0));
    }

    #[test]
    fn dot_repeats_an_insert_with_a_paste_in_it() {
        // As neovim's paste does, line breaks included.
        for (pasted, repeated) in [("XY", "aXYaXYbb"), ("X\nY", "aX\nYaX\nYbb")] {
            let mut editor = holding("");
            feed(&mut editor, "ia");
            editor.paste(pasted);
            feed(&mut editor, "b<Esc>.");
            assert_eq!(text(&editor), repeated);
        }
    }

    #[test]
    fn a_paste_in_search_mode_searches_for_its_first_line() {
        let mut editor = holding("one\ntwo");
        feed(&mut editor, "/");
        editor.paste("tw\nignored");
        assert_eq!(editor.search().as_deref(), Some("tw"));
        assert_eq!(text(&editor), "one\ntwo");
    }
}
