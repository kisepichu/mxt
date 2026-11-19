//! The composers (design, "Terminal adapter and the composer", "Mention
//! completion"): an edtui editor per room, created on first focus and kept
//! when switching rooms. The core only knows whether a composer is empty and
//! the `@` token at its cursor, which a composer reports when they change.

use std::collections::HashMap;

use edtui::actions::{LineBreak, MoveForward, OpenSystemEditor, SwitchMode};
use edtui::{EditorState, Index2, Lines, RowIndex, system_editor};
use mxt_core::action::FocusMode;
use mxt_core::ids::RoomId;
use mxt_core::keys::Key;
use mxt_core::msg::{EditorMode, Msg};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::clipboard::SharedRegister;
use crate::editor::Editor;
use crate::view::grapheme_width;

/// The columns edtui draws a tab with (its default).
const TAB_WIDTH: usize = 2;

pub(crate) struct Composers {
    register: SharedRegister,
    rooms: HashMap<RoomId, Composer>,
    /// The room of the last `FocusComposer`, whose editor `ReplayKeys` feeds.
    focused: Option<RoomId>,
}

impl Composers {
    pub(crate) fn new(register: SharedRegister) -> Composers {
        Composers {
            register,
            rooms: HashMap::new(),
            focused: None,
        }
    }

    pub(crate) fn get(&self, room: &RoomId) -> Option<&Composer> {
        self.rooms.get(room)
    }

    pub(crate) fn get_mut(&mut self, room: &RoomId) -> Option<&mut Composer> {
        self.rooms.get_mut(room)
    }

    /// The room's composer, created in normal mode if it does not exist.
    pub(crate) fn get_or_create(&mut self, room: &RoomId) -> &mut Composer {
        self.rooms
            .entry(room.clone())
            .or_insert_with(|| Composer::new(room.clone(), self.register.clone()))
    }

    /// `FocusComposer`: the room's composer gets the keys from now on.
    pub(crate) fn focus(&mut self, room: RoomId, mode: FocusMode, msgs: &mut Vec<Msg>) {
        self.get_or_create(&room).focus(mode, msgs);
        self.focused = Some(room);
    }

    /// The composer that last got the focus.
    pub(crate) fn focused_mut(&mut self) -> Option<&mut Composer> {
        self.rooms.get_mut(self.focused.as_ref()?)
    }
}

pub(crate) struct Composer {
    room: RoomId,
    editor: Editor,
    register: SharedRegister,
    /// Whether the composer is empty as the core knows it: as last reported,
    /// or as `ClearComposer` and `LoadComposer` left it.
    empty: bool,
    /// The `@` token's query as the core knows it.
    query: Option<String>,
}

impl Composer {
    fn new(room: RoomId, register: SharedRegister) -> Composer {
        Composer {
            room,
            editor: Editor::new("", register.clone()),
            register,
            empty: true,
            query: None,
        }
    }

    pub(crate) fn mode(&self) -> EditorMode {
        self.editor.mode()
    }

    /// The search pattern while the editor is in search mode.
    pub(crate) fn search(&self) -> Option<String> {
        self.editor.search()
    }

    /// The text for `Msg::Submit`, lines joined with `\n`.
    pub(crate) fn text(&self) -> String {
        self.editor.state.lines.to_string()
    }

    /// The rows the text takes at `width` columns, at least one. Like
    /// edtui's wrapping, a character that does not fit on a row moves to the
    /// next one whole, and an empty line takes a row.
    pub(crate) fn rows(&self, width: usize) -> usize {
        let rows: usize = self
            .editor
            .state
            .lines
            .iter_row()
            .map(|line| wrapped_rows(line, width))
            .sum();
        rows.max(1)
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, block: Block, cursor: bool) {
        self.editor.draw(frame, area, block, cursor);
    }

    /// `FocusComposer`. Insert mode starts after the character under the
    /// cursor, like `a`, so the text continues where leaving insert mode
    /// left it; `Keep` keeps the mode. The core closed its mention menu, so
    /// the token is reported even when it did not change.
    fn focus(&mut self, mode: FocusMode, msgs: &mut Vec<Msg>) {
        let state = &mut self.editor.state;
        if mode == FocusMode::Insert && state.mode != edtui::EditorMode::Insert {
            state.execute(SwitchMode(edtui::EditorMode::Insert));
            state.execute(MoveForward(1));
        }
        self.query = self.token();
        msgs.push(Msg::MentionQuery {
            room: self.room.clone(),
            query: self.query.clone(),
        });
    }

    /// `ReplayKeys`, reporting after each key.
    pub(crate) fn replay(&mut self, keys: &[Key], msgs: &mut Vec<Msg>) {
        for &key in keys {
            self.editor.feed(key);
            self.report(msgs);
        }
    }

    /// `InsertNewline`: a line break at the cursor.
    pub(crate) fn insert_newline(&mut self, msgs: &mut Vec<Msg>) {
        self.editor.state.execute(LineBreak(1));
        self.report(msgs);
    }

    /// `LoadComposer`: `text` replaces the text, with the cursor at its end.
    /// The core set its emptiness already.
    pub(crate) fn load(&mut self, text: &str, msgs: &mut Vec<Msg>) {
        self.reset(text);
        let state = &mut self.editor.state;
        let row = state.lines.len().saturating_sub(1);
        let len = state.lines.len_col(row).unwrap_or(0);
        let col = if state.mode == edtui::EditorMode::Insert {
            len
        } else {
            len.saturating_sub(1)
        };
        state.cursor = Index2::new(row, col);
        self.empty = text.is_empty();
        self.report(msgs);
    }

    /// `ClearComposer`. The core set its emptiness and closed its menu.
    pub(crate) fn clear(&mut self) {
        self.reset("");
        self.empty = true;
        self.query = None;
    }

    /// A fresh editor state holding `text`, so undo does not bring back the
    /// sent or replaced text. Insert mode is kept; other modes become normal,
    /// which needs no selection or search.
    fn reset(&mut self, text: &str) {
        let insert = self.editor.state.mode == edtui::EditorMode::Insert;
        let mut state = EditorState::new(Lines::from(text));
        state.set_clipboard(self.register.clone());
        if insert {
            state.mode = edtui::EditorMode::Insert;
        }
        self.editor.state = state;
    }

    /// `ReplaceMentionToken`: the token at the cursor and the rest of its
    /// word become `text` and a space, with the cursor after the space. The
    /// core closed its menu.
    pub(crate) fn replace_token(&mut self, text: &str, msgs: &mut Vec<Msg>) {
        let state = &mut self.editor.state;
        let Some(line) = state.lines.get_mut(RowIndex::new(state.cursor.row)) else {
            return;
        };
        let col = state.cursor.col.min(line.len());
        let Some(start) = token_start(line, col) else {
            return;
        };
        let end = col
            + line[col..]
                .iter()
                .take_while(|c| !c.is_whitespace())
                .count();
        line.splice(start..end, text.chars().chain([' ']));
        state.cursor.col = start + text.chars().count() + 1;
        self.query = None;
        self.report(msgs);
    }

    /// Asks edtui to open the external editor; see [`Composer::edit_externally`].
    pub(crate) fn request_external_editor(&mut self) {
        self.editor.state.execute(OpenSystemEditor);
    }

    /// Runs `open` with the editor state if the external editor was asked
    /// for, by [`Composer::request_external_editor`] or edtui's own `<C-e>`,
    /// then reports the edit.
    pub(crate) fn edit_externally<E>(
        &mut self,
        msgs: &mut Vec<Msg>,
        open: impl FnOnce(&mut EditorState) -> Result<(), E>,
    ) -> Result<(), E> {
        if !system_editor::is_pending(&self.editor.state) {
            return Ok(());
        }
        let opened = open(&mut self.editor.state);
        self.report(msgs);
        opened
    }

    /// Reports what changed for the core: whether the composer is empty,
    /// and the `@` token's query.
    fn report(&mut self, msgs: &mut Vec<Msg>) {
        let empty = self.is_empty();
        if empty != self.empty {
            self.empty = empty;
            msgs.push(Msg::ComposerEdited {
                room: self.room.clone(),
                empty,
            });
        }
        let query = self.token();
        if query != self.query {
            self.query.clone_from(&query);
            msgs.push(Msg::MentionQuery {
                room: self.room.clone(),
                query,
            });
        }
    }

    fn is_empty(&self) -> bool {
        let lines = &self.editor.state.lines;
        lines.len() <= 1 && lines.is_empty_row(0) != Some(false)
    }

    /// In insert mode, the text after `@` of the token that ends at the
    /// cursor.
    fn token(&self) -> Option<String> {
        let state = &self.editor.state;
        if state.mode != edtui::EditorMode::Insert {
            return None;
        }
        let line = state.lines.get(RowIndex::new(state.cursor.row))?;
        let col = state.cursor.col.min(line.len());
        token_start(line, col).map(|start| line[start + 1..col].iter().collect())
    }
}

/// Where the `@` token that ends at `col` starts (design, "Mention
/// completion"): an `@` at the start of the line or after whitespace,
/// followed by non-whitespace up to `col`.
fn token_start(line: &[char], col: usize) -> Option<usize> {
    let before = &line[..col];
    let start = before
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |at| at + 1);
    (before.get(start) == Some(&'@')).then_some(start)
}

fn wrapped_rows(line: &[char], width: usize) -> usize {
    let mut rows = 1;
    let mut used = 0;
    for &c in line {
        let columns = match c {
            '\t' => TAB_WIDTH,
            c => grapheme_width(c.encode_utf8(&mut [0; 4])),
        };
        if used > 0 && used + columns > width {
            rows += 1;
            used = 0;
        }
        used += columns;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::tests::keys;

    fn composer() -> Composer {
        Composer::new(RoomId::new("!room:example.org"), SharedRegister::default())
    }

    /// A composer that got the focus in insert mode, then the keys in `text`
    /// (key notation, so a space is `<Space>`).
    fn focused(text: &str) -> Composer {
        let mut composer = composer();
        composer.focus(FocusMode::Insert, &mut Vec::new());
        if !text.is_empty() {
            type_keys(&mut composer, text);
        }
        composer
    }

    fn type_keys(composer: &mut Composer, text: &str) -> Vec<Msg> {
        let mut msgs = Vec::new();
        composer.replay(&keys(text), &mut msgs);
        msgs
    }

    fn edited(empty: bool) -> Msg {
        Msg::ComposerEdited {
            room: RoomId::new("!room:example.org"),
            empty,
        }
    }

    fn query(query: Option<&str>) -> Msg {
        Msg::MentionQuery {
            room: RoomId::new("!room:example.org"),
            query: query.map(str::to_owned),
        }
    }

    #[test]
    fn emptiness_is_reported_when_it_changes() {
        let mut composer = focused("");
        assert_eq!(type_keys(&mut composer, "ab"), [edited(false)]);
        assert_eq!(type_keys(&mut composer, "<BS>"), []);
        assert_eq!(type_keys(&mut composer, "<BS>"), [edited(true)]);
        let mut msgs = Vec::new();
        composer.insert_newline(&mut msgs);
        // A line break is text.
        assert_eq!(msgs, [edited(false)]);
        assert_eq!(composer.text(), "\n");
    }

    #[test]
    fn clear_and_load_leave_emptiness_to_the_core() {
        let mut composer = focused("hi");
        composer.clear();
        assert_eq!(composer.text(), "");
        assert_eq!(type_keys(&mut composer, "x"), [edited(false)]);

        composer.clear();
        let mut msgs = Vec::new();
        composer.load("first\nsecond", &mut msgs);
        assert_eq!(msgs, []);
        // Insert mode stays, with the cursor at the end.
        assert_eq!(type_keys(&mut composer, "!"), []);
        assert_eq!(composer.text(), "first\nsecond!");
        assert_eq!(type_keys(&mut composer, "<Esc>dddd"), [edited(true)]);
    }

    #[test]
    fn the_mention_token_starts_after_whitespace() {
        let mut composer = focused("");
        assert_eq!(
            type_keys(&mut composer, "hi<Space>@"),
            [edited(false), query(Some(""))]
        );
        assert_eq!(
            type_keys(&mut composer, "日本"),
            [query(Some("日")), query(Some("日本"))]
        );
        // Whitespace ends it.
        assert_eq!(type_keys(&mut composer, "<Space>"), [query(None)]);
        // An `@` inside a word is not a token.
        assert_eq!(type_keys(&mut composer, "a@b"), []);
        // A line start counts as whitespace.
        composer.insert_newline(&mut Vec::new());
        assert_eq!(
            type_keys(&mut composer, "@b"),
            [query(Some("")), query(Some("b"))]
        );
        // Only insert mode has a token.
        assert_eq!(type_keys(&mut composer, "<Esc>"), [query(None)]);
        // The core closes its menu on focus, so focus reports the token
        // again; `Keep` keeps normal mode, which has none.
        let mut msgs = Vec::new();
        composer.focus(FocusMode::Keep, &mut msgs);
        assert_eq!(msgs, [query(None)]);
        assert_eq!(composer.mode(), EditorMode::Normal);
        msgs.clear();
        composer.focus(FocusMode::Insert, &mut msgs);
        assert_eq!(msgs, [query(Some("b"))]);
    }

    #[test]
    fn accepting_replaces_the_token_and_the_rest_of_its_word() {
        let mut composer = focused("hi<Space>@alice");
        assert_eq!(
            type_keys(&mut composer, "<Left><Left><Left>"),
            [query(Some("alic")), query(Some("ali")), query(Some("al"))]
        );
        let mut msgs = Vec::new();
        composer.replace_token("@Alice", &mut msgs);
        assert_eq!(msgs, []);
        type_keys(&mut composer, "x");
        assert_eq!(composer.text(), "hi @Alice x");
    }

    #[test]
    fn focus_continues_after_the_text() {
        let mut composer = focused("hello<Esc>");
        composer.focus(FocusMode::Insert, &mut Vec::new());
        type_keys(&mut composer, "!");
        assert_eq!(composer.text(), "hello!");
    }

    #[test]
    fn rows_wrap_whole_characters() {
        let rows = |text: &str, width| {
            let mut composer = composer();
            composer.load(text, &mut Vec::new());
            composer.rows(width)
        };
        assert_eq!(rows("", 4), 1);
        assert_eq!(rows("abcd", 4), 1);
        assert_eq!(rows("abcde", 4), 2);
        // `う` does not fit after `あい` in five columns.
        assert_eq!(rows("あいう", 5), 2);
        assert_eq!(rows("a\n\nb", 4), 3);
    }
}
