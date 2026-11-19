//! An edtui editor in vim mode with the shared register: the composers and
//! the message view's scratch editor (design, "Terminal adapter and the
//! composer").
//!
//! In normal and visual mode mxt handles some keys in place of edtui
//! (design, "edtui differences from vim"): `f`, `F`, `t` and `T` with their
//! character, `;` and `,` repeating them, `r` with its character (normal
//! mode only), and `h`, `l`, `<Left>` and `<Right>` by grapheme cluster.

use std::cmp::Ordering;

use crossterm::event::{KeyCode as C, KeyModifiers as M};
use edtui::actions::{MoveBackward, MoveForward, ReplaceChar, SwitchMode};
use edtui::events::{KeyEventRegister, KeyInput};
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView, Lines, RowIndex};
use mxt_core::keys::{Key, KeyCode, Modifiers};
use mxt_core::msg::EditorMode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::clipboard::SharedRegister;
use crate::view::grapheme_width;

/// The columns edtui draws a tab with (its default).
const TAB_WIDTH: usize = 2;

/// What edtui runs for the keys of [`Command`]. edtui runs a key's action
/// only when the key starts a command, not when it continues one (`d`
/// then `f`), so the switch to search mode tells [`Editor::feed`] which it
/// was. `feed` switches back at once; from normal or visual mode the switch
/// changes nothing else.
const MARK: SwitchMode = SwitchMode(edtui::EditorMode::Search);

pub(crate) struct Editor {
    pub(crate) state: EditorState,
    keys: EditorEventHandler,
    /// A key waiting for the next one, which gets it without the keymap.
    pending: Option<Pending>,
    /// The last `f`, `F`, `t` or `T` with its character, for `;` and `,`.
    last_find: Option<Find>,
}

impl Editor {
    /// In normal mode with the cursor at the start.
    pub(crate) fn new(text: &str, register: SharedRegister) -> Editor {
        let mut state = EditorState::new(Lines::from(text));
        state.set_clipboard(register);
        let mut keys = EditorEventHandler::vim_mode();
        // edtui's `<C-e>` in normal mode asks for the external editor, which
        // only the keymap's `composer.external_editor` opens.
        keys.key_handler
            .remove(&KeyEventRegister::n(vec![KeyInput::ctrl('e')]));
        // Over edtui's `f`, `t`, `h`, `l`, `<Left>` and `<Right>`, so each
        // key has one implementation.
        let candidates = "fFtT;,hlr".chars().map(Key::char).chain(
            [KeyCode::Left, KeyCode::Right].map(|code| Key::new(code, Modifiers::default())),
        );
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
        Editor {
            state,
            keys,
            pending: None,
            last_find: None,
        }
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
    /// key waits for its character, which then goes to the editor without
    /// the keymap (`f:`, `r<Esc>`).
    pub(crate) fn key_mode(&self) -> EditorMode {
        if self.pending.is_some() {
            EditorMode::AwaitingChar
        } else {
            self.mode()
        }
    }

    /// A key the core passed on. Keys edtui has no code for are left out.
    pub(crate) fn feed(&mut self, key: Key) {
        let edtui_char = match self.pending.take() {
            None => false,
            Some(Pending::Edtui) => true,
            Some(Pending::Drop) => return,
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
        };
        let mode = self.state.mode;
        let Some(input) = key_input(key, mode) else {
            return;
        };
        self.keys.on_key_event(input, &mut self.state);
        let Some(command) = Command::of(key, mode) else {
            return;
        };
        if self.state.mode == edtui::EditorMode::Search {
            // `MARK` ran: the key starts a command.
            self.state.mode = mode;
            self.run(command);
        } else if !edtui_char {
            // The key continued another one (`d` then `f`). edtui's `df`,
            // `dt`, `cf` and `ct` now wait for their character, which goes
            // to edtui (so does the one after `yf`, which edtui does not
            // have). edtui has no `dF` or `cT`, so their character is
            // dropped rather than run as a command. With `edtui_char` the
            // key was such a character, and nothing waits.
            self.pending = match command {
                Command::Find {
                    backward: false, ..
                } => Some(Pending::Edtui),
                Command::Find { backward: true, .. } => Some(Pending::Drop),
                _ => None,
            };
        }
    }

    fn run(&mut self, command: Command) {
        match command {
            Command::Find { backward, till } => {
                self.pending = Some(Pending::Find { backward, till });
            }
            Command::Replace => self.pending = Some(Pending::Replace),
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

    /// The search pattern while the editor is in search mode.
    pub(crate) fn search(&self) -> Option<String> {
        (self.state.mode == edtui::EditorMode::Search).then(|| self.state.search_pattern())
    }

    /// The rows the text takes at `width` columns, at least one. Like
    /// edtui's wrapping, a character that does not fit on a row moves to the
    /// next one whole, and an empty line takes a row. In insert mode the
    /// cursor after the end of its line takes a column too: edtui draws it
    /// on the next row when the line's last row is full.
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
    /// `r`, waiting for the character.
    Replace,
}

impl Command {
    /// `key`'s command in normal or visual mode; `r` only in normal mode.
    fn of(key: Key, mode: edtui::EditorMode) -> Option<Command> {
        let visual = match mode {
            edtui::EditorMode::Normal => false,
            edtui::EditorMode::Visual => true,
            edtui::EditorMode::Insert | edtui::EditorMode::Search => return None,
        };
        if key.mods() != Modifiers::default() {
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
            KeyCode::Char('r') if !visual => Command::Replace,
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
    /// `r`.
    Replace,
    /// edtui's `df`, `dt`, `cf` or `ct`: the key goes to edtui.
    Edtui,
    /// `dF`, `cT` and the like, which edtui does not have: the key is
    /// dropped.
    Drop,
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
    let columns = line.iter().map(|&c| match c {
        '\t' => TAB_WIDTH,
        c => grapheme_width(c.encode_utf8(&mut [0; 4])),
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
        let feed =
            |editor: &mut Editor, text| keys(text).into_iter().for_each(|key| editor.feed(key));
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

    fn feed(editor: &mut Editor, text: &str) {
        keys(text).into_iter().for_each(|key| editor.feed(key));
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
        let mut yanked = register.clone();
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
}
