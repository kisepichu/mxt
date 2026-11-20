//! The composers (design, "Terminal adapter and the composer", "Mention
//! completion"): an edtui editor per room, created on first focus and kept
//! when switching rooms. The core only knows whether a composer is empty and
//! the `@` token at its cursor, which a composer reports when they change.

use std::collections::HashMap;
use std::io;

use edtui::actions::{LineBreak, MoveForward, OpenSystemEditor, SwitchMode};
use edtui::{EditorState, Index2, Lines, RowIndex};
use mxt_core::action::FocusMode;
use mxt_core::ids::RoomId;
use mxt_core::keys::Key;
use mxt_core::msg::{EditorMode, Msg};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::clipboard::SharedRegister;
use crate::editor::Editor;

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

    /// The mode the core resolves the next key in (see [`Editor::key_mode`]).
    pub(crate) fn key_mode(&self) -> EditorMode {
        self.editor.key_mode()
    }

    /// The search pattern while the editor is in search mode.
    pub(crate) fn search(&self) -> Option<String> {
        self.editor.search()
    }

    /// The text for `Msg::Submit`, lines joined with `\n`.
    pub(crate) fn text(&self) -> String {
        self.editor.state.lines.to_string()
    }

    /// The rows the text takes at `width` columns (see [`Editor::rows`]).
    pub(crate) fn rows(&self, width: usize) -> usize {
        self.editor.rows(width)
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, block: Block, cursor: bool) {
        self.editor.draw(frame, area, block, cursor);
    }

    /// `FocusComposer`. Insert mode starts after the character under the
    /// cursor, like `a`, so the text continues where leaving insert mode
    /// left it; a visual selection is dropped. `Keep` keeps the mode. A key
    /// waiting for the next one is cancelled. The core closed its mention
    /// menu, so the token is reported even when it did not change.
    fn focus(&mut self, mode: FocusMode, msgs: &mut Vec<Msg>) {
        self.editor.cancel_pending();
        let state = &mut self.editor.state;
        if mode == FocusMode::Insert && state.mode != edtui::EditorMode::Insert {
            // edtui keeps the selection when switching to insert mode, and
            // it would stay highlighted while typing.
            state.selection = None;
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

    /// `Paste` (see [`Editor::paste`]), reported once. A paste never opens
    /// the mention menu, not even with an `@` token at its end, and closes
    /// an open one; the next key looks for the token again.
    pub(crate) fn paste(&mut self, text: &str, msgs: &mut Vec<Msg>) {
        self.editor.paste(text);
        self.report_empty(msgs);
        self.report_query(None, msgs);
    }

    /// `LoadComposer`: `text` replaces the text, with the cursor at its end.
    /// The core set its emptiness already.
    pub(crate) fn load(&mut self, text: &str, msgs: &mut Vec<Msg>) {
        self.reset(text);
        self.move_to_end();
        self.empty = text.is_empty();
        self.report(msgs);
    }

    /// The cursor on the last character, or after it in insert mode.
    fn move_to_end(&mut self) {
        let state = &mut self.editor.state;
        let row = state.lines.len().saturating_sub(1);
        let len = state.lines.len_col(row).unwrap_or(0);
        let col = if state.mode == edtui::EditorMode::Insert {
            len
        } else {
            len.saturating_sub(1)
        };
        state.cursor = Index2::new(row, col);
    }

    /// `ClearComposer`. The core set its emptiness and closed its menu.
    pub(crate) fn clear(&mut self) {
        self.reset("");
        self.empty = true;
        self.query = None;
    }

    /// A fresh editor state holding `text`, so undo does not bring back the
    /// sent or replaced text, and no key waiting for the next one. Insert
    /// mode is kept; other modes become normal, which needs no selection or
    /// search.
    fn reset(&mut self, text: &str) {
        let insert = self.editor.state.mode == edtui::EditorMode::Insert;
        let mut state = EditorState::new(Lines::from(text));
        state.set_clipboard(self.register.clone());
        if insert {
            state.mode = edtui::EditorMode::Insert;
        }
        self.editor.state = state;
        self.editor.cancel_pending();
    }

    /// `ReplaceMentionToken`: the token, from its `@` to the cursor, becomes
    /// `text` and a space, with the cursor after the space. Text after the
    /// cursor stays, also when it is glued to the token. The core closed its
    /// menu.
    pub(crate) fn replace_token(&mut self, text: &str, msgs: &mut Vec<Msg>) {
        let state = &mut self.editor.state;
        let Some(line) = state.lines.get_mut(RowIndex::new(state.cursor.row)) else {
            return;
        };
        let col = state.cursor.col.min(line.len());
        let Some(start) = token_start(line, col) else {
            return;
        };
        line.splice(start..col, text.chars().chain([' ']));
        state.cursor.col = start + text.chars().count() + 1;
        self.query = None;
        self.report(msgs);
    }

    /// `ExternalEditor`: asks edtui for the external editor and runs `open`,
    /// which is edtui's `system_editor::open`, with the editor state. After
    /// the editor the cursor is at the end of the text, as after
    /// `LoadComposer`, where edtui leaves it at the start. The edit is
    /// reported also when `open` failed.
    pub(crate) fn edit_externally(
        &mut self,
        msgs: &mut Vec<Msg>,
        open: impl FnOnce(&mut EditorState) -> io::Result<()>,
    ) -> io::Result<()> {
        self.editor.state.execute(OpenSystemEditor);
        let opened = open(&mut self.editor.state);
        if opened.is_ok() {
            self.move_to_end();
        }
        self.report(msgs);
        opened
    }

    /// Reports what changed for the core: whether the composer is empty,
    /// and the `@` token's query.
    fn report(&mut self, msgs: &mut Vec<Msg>) {
        self.report_empty(msgs);
        let query = self.token();
        self.report_query(query, msgs);
    }

    fn report_empty(&mut self, msgs: &mut Vec<Msg>) {
        let empty = self.is_empty();
        if empty != self.empty {
            self.empty = empty;
            msgs.push(Msg::ComposerEdited {
                room: self.room.clone(),
                empty,
            });
        }
    }

    fn report_query(&mut self, query: Option<String>, msgs: &mut Vec<Msg>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::tests::keys;
    use edtui::system_editor;
    use mxt_core::action::{Action, Binding};
    use mxt_core::app::{App, Focus};
    use mxt_core::effect::{Effect, UiEffect};
    use mxt_core::ids::{Timestamp, UserId};
    use mxt_core::keymap::{Keymap, TableId};
    use mxt_core::keys::KeySeq;
    use mxt_core::resolver::TimerId;
    use mxt_core::room::{NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};
    use mxt_core::settings::Settings;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
        // Whitespace is text: `composer.leave` keeps a reply of spaces.
        assert_eq!(type_keys(&mut composer, "<Space>"), [edited(false)]);
        assert_eq!(type_keys(&mut composer, "<BS>"), [edited(true)]);
        let mut msgs = Vec::new();
        composer.insert_newline(&mut msgs);
        // A line break is text.
        assert_eq!(msgs, [edited(false)]);
        assert_eq!(composer.text(), "\n");
    }

    #[test]
    fn a_paste_in_insert_mode_inserts_its_lines_and_opens_no_menu() {
        let mut composer = focused("hi<Space>!<Left>");
        let mut msgs = Vec::new();
        composer.paste("one\ntwo @al", &mut msgs);
        // Reported once; the `@al` the paste ends with opens no menu.
        assert_eq!(msgs, []);
        assert_eq!(composer.text(), "hi one\ntwo @al!");
        assert_eq!(composer.mode(), EditorMode::Insert);
        // Typing goes on after the paste and finds the token again.
        assert_eq!(type_keys(&mut composer, "i"), [query(Some("ali"))]);
        assert_eq!(composer.text(), "hi one\ntwo @ali!");

        // A paste closes an open menu, and an empty composer reports that
        // it has text.
        let mut composer = focused("@al");
        let mut msgs = Vec::new();
        composer.paste("ice", &mut msgs);
        assert_eq!(msgs, [query(None)]);
        let mut composer = focused("");
        let mut msgs = Vec::new();
        composer.paste("\n", &mut msgs);
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
    fn accepting_replaces_the_token_up_to_the_cursor() {
        let mut composer = focused("hi<Space>@alice");
        assert_eq!(
            type_keys(&mut composer, "<Left><Left><Left>"),
            [query(Some("alic")), query(Some("ali")), query(Some("al"))]
        );
        let mut msgs = Vec::new();
        composer.replace_token("@Alice", &mut msgs);
        assert_eq!(msgs, []);
        type_keys(&mut composer, "x");
        assert_eq!(composer.text(), "hi @Alice xice");
    }

    #[test]
    fn accepting_keeps_text_glued_after_the_cursor() {
        for (typed, expected) in [
            // `@al` typed at the start of a Japanese draft.
            (
                "さん、確認お願いします<Esc>0i@al",
                "@Alice xさん、確認お願いします",
            ),
            (",<Space>there<Esc>0ihi<Space>@al", "hi @Alice x, there"),
        ] {
            let mut composer = focused(typed);
            let mut msgs = Vec::new();
            composer.replace_token("@Alice", &mut msgs);
            assert_eq!(msgs, []);
            type_keys(&mut composer, "x");
            assert_eq!(composer.text(), expected);
        }
    }

    #[test]
    fn focus_continues_after_the_text() {
        let mut composer = focused("hello<Esc>");
        composer.focus(FocusMode::Insert, &mut Vec::new());
        type_keys(&mut composer, "!");
        assert_eq!(composer.text(), "hello!");
    }

    #[test]
    fn focus_from_visual_mode_drops_the_selection() {
        let mut composer = focused("hello<Esc>vh");
        assert_eq!(composer.mode(), EditorMode::Visual);
        composer.focus(FocusMode::Insert, &mut Vec::new());
        assert_eq!(composer.mode(), EditorMode::Insert);
        assert!(composer.editor.state.selection.is_none());
    }

    #[test]
    fn focus_and_reset_cancel_a_waiting_key() {
        let mut composer = focused("abc<Esc>0f");
        assert_eq!(composer.key_mode(), EditorMode::AwaitingChar);
        composer.focus(FocusMode::Keep, &mut Vec::new());
        assert_eq!(composer.key_mode(), EditorMode::Normal);
        // In insert mode the first key is typed.
        type_keys(&mut composer, "f");
        composer.focus(FocusMode::Insert, &mut Vec::new());
        type_keys(&mut composer, "z");
        assert_eq!(composer.text(), "azbc");

        // edtui's own `df` is cancelled too, so `<Esc>` leaves insert mode.
        let mut composer = focused("abc<Esc>0df");
        composer.focus(FocusMode::Insert, &mut Vec::new());
        type_keys(&mut composer, "z<Esc>");
        assert_eq!(
            (composer.text().as_str(), composer.mode()),
            ("azbc", EditorMode::Normal)
        );

        // `LoadComposer` while `r` waits: `x` deletes rather than replaces.
        let mut composer = focused("abc<Esc>r");
        composer.load("xyz", &mut Vec::new());
        assert_eq!(composer.key_mode(), EditorMode::Normal);
        type_keys(&mut composer, "x");
        assert_eq!(composer.text(), "xy");
    }

    #[test]
    fn the_external_editor_is_asked_for_and_its_edit_reported() {
        let replace = |text: &'static str, result: io::Result<()>| {
            move |state: &mut EditorState| {
                // edtui's `open` does nothing unless it was asked for.
                assert!(system_editor::is_pending(state));
                state.lines = Lines::from(text);
                state.cursor = Index2::new(0, 0);
                result
            }
        };
        let mut composer = focused("draft");
        let mut msgs = Vec::new();
        composer
            .edit_externally(&mut msgs, replace("first\nsecond", Ok(())))
            .unwrap();
        assert_eq!(msgs, []);
        // The cursor is at the end.
        type_keys(&mut composer, "!");
        assert_eq!(composer.text(), "first\nsecond!");

        // A failure is returned after the edit is reported.
        let failed = replace("", Err(io::Error::other("no editor")));
        let error = composer.edit_externally(&mut msgs, failed).unwrap_err();
        assert_eq!(error.to_string(), "no editor");
        assert_eq!(msgs, [edited(true)]);
    }

    #[test]
    fn rows_wrap_whole_grapheme_clusters_and_the_insert_cursor() {
        let rows = |mut composer: Composer, text: &str, width| {
            composer.load(text, &mut Vec::new());
            composer.rows(width)
        };
        assert_eq!(rows(composer(), "", 4), 1);
        assert_eq!(rows(composer(), "abcd", 4), 1);
        assert_eq!(rows(composer(), "abcde", 4), 2);
        // `う` does not fit after `あい` in five columns.
        assert_eq!(rows(composer(), "あいう", 5), 2);
        assert_eq!(rows(composer(), "a\n\nb", 4), 3);
        // A grapheme cluster takes its own width, not the sum of its
        // characters': a family is two columns, and `👍🏽` moves to the next
        // row whole.
        let family = "👨\u{200d}👩\u{200d}👧";
        assert_eq!(rows(composer(), &family.repeat(2), 4), 1);
        assert_eq!(rows(composer(), "ab👍🏽", 3), 2);
        // The insert cursor after the text needs a column.
        assert_eq!(rows(focused(""), "abcd", 4), 2);
        assert_eq!(rows(focused(""), "abc", 4), 1);
        assert_eq!(rows(focused(""), "あい", 4), 2);
        assert_eq!(rows(focused(""), "あい", 5), 1);
        // Only on its own line.
        assert_eq!(rows(focused(""), "abcd\nab", 4), 2);
        let mut composer = focused("abcd");
        type_keys(&mut composer, "<Left>");
        assert_eq!(composer.rows(4), 1);
    }

    #[test]
    fn edtui_draws_the_insert_cursor_on_the_counted_rows() {
        let families = "👨\u{200d}👩\u{200d}👧".repeat(2);
        let texts = [
            ("abcd", 4),
            ("abc", 4),
            ("あい", 4),
            ("abcdabcd", 4),
            ("ab👍🏽", 4),
            (families.as_str(), 4),
            ("a❤\u{fe0f}", 4),
        ];
        for (text, width) in texts {
            let mut composer = focused("");
            composer.load(text, &mut Vec::new());
            let rows = u16::try_from(composer.rows(width)).unwrap();
            let width = u16::try_from(width).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(width, rows + 1)).unwrap();
            let area = Rect::new(0, 0, width, rows);
            terminal
                .draw(|frame| composer.draw(frame, area, Block::new(), true))
                .unwrap();
            // The cursor after the text is on the last counted row.
            let cursor = composer.editor.state.cursor_screen_position().unwrap();
            assert_eq!(cursor.y, rows - 1, "{text:?} at {width}: {cursor:?}");
        }
    }

    #[test]
    fn the_terminal_cursor_follows_grapheme_clusters() {
        // Each emoji takes two columns, however many characters it has, so
        // the insert cursor (where an input method puts its preedit) after
        // it, before `b`, is on column 3.
        for text in ["a👍🏽b", "a👨\u{200d}👩\u{200d}👧b", "a❤\u{fe0f}b"] {
            let mut composer = focused("");
            composer.load(text, &mut Vec::new());
            type_keys(&mut composer, "<Left>");
            let mut terminal = Terminal::new(TestBackend::new(20, 1)).unwrap();
            terminal
                .draw(|frame| composer.draw(frame, frame.area(), Block::new(), true))
                .unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert_eq!((cursor.x, cursor.y), (3, 0), "{text:?}");
        }
    }

    fn leader() -> KeySeq {
        KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
    }

    /// The app with the default keymap and the unread rooms `!a` and `!b`,
    /// `!a` open and its composer focused, after typing `text` there in
    /// insert mode and `<Esc>`.
    fn composing(text: &str) -> (App, Composers) {
        composing_with(Keymap::new(&leader(), []), text)
    }

    /// [`composing`] with `keymap`.
    fn composing_with(keymap: Keymap, text: &str) -> (App, Composers) {
        let mut app = App::new(keymap, Settings::default(), UserId::new("@me:example.org"));
        for (room, at) in [("!a:example.org", 20), ("!b:example.org", 10)] {
            app.update(Msg::RoomUpserted(RoomSummary {
                id: RoomId::new(room),
                name: RoomName {
                    network: None,
                    room: room.to_owned(),
                },
                kind: RoomKind::Group,
                unread: UnreadCounts {
                    messages: 1,
                    ..UnreadCounts::default()
                },
                mode: NotificationMode::Default,
                latest_activity: Some(Timestamp::from_millis(at)),
            }));
        }
        let mut composers = Composers::new(SharedRegister::default());
        run(&mut app, &mut composers, &format!("<CR>i{text}<Esc>"));
        assert_eq!(
            (app.focus(), app.open_room()),
            (Focus::Composer, Some(&RoomId::new("!a:example.org")))
        );
        (app, composers)
    }

    /// Types `text` as the terminal does: each key goes to the core with
    /// the open room's composer's key mode while the composer has focus,
    /// and the effects go to [`apply`]. The last timer the core started.
    fn run(app: &mut App, composers: &mut Composers, text: &str) -> Option<TimerId> {
        let mut timer = None;
        for key in keys(text) {
            let editor = (app.focus() == Focus::Composer)
                .then(|| app.open_room().and_then(|room| composers.get(room)))
                .flatten()
                .map(Composer::key_mode);
            let effects = app.update(Msg::Key { key, editor });
            timer = apply(app, composers, effects).or(timer);
        }
        timer
    }

    /// Runs `effects` as the terminal does: `FocusComposer`, `ReplayKeys`
    /// and `InsertNewline` go to the composers, and what they report to the
    /// core. The last timer started.
    fn apply(app: &mut App, composers: &mut Composers, effects: Vec<Effect>) -> Option<TimerId> {
        let mut msgs = Vec::new();
        let mut timer = None;
        for effect in effects {
            match effect {
                Effect::Ui(UiEffect::StartTimer { id, .. }) => timer = Some(id),
                Effect::Ui(UiEffect::FocusComposer { room, mode }) => {
                    composers.focus(room, mode, &mut msgs);
                }
                Effect::Ui(UiEffect::ReplayKeys(keys)) => {
                    if let Some(composer) = composers.focused_mut() {
                        composer.replay(&keys, &mut msgs);
                    }
                }
                Effect::Ui(UiEffect::InsertNewline(room)) => {
                    if let Some(composer) = composers.get_mut(&room) {
                        composer.insert_newline(&mut msgs);
                    }
                }
                _ => {}
            }
        }
        for msg in msgs {
            app.update(msg);
        }
        timer
    }

    fn text_of(composers: &Composers) -> String {
        composers
            .get(&RoomId::new("!a:example.org"))
            .unwrap()
            .text()
    }

    #[test]
    fn operators_and_text_objects_never_run_the_default_unread_keys() {
        // `]u` and `[u` are in `normal`, where the composer's normal and
        // visual modes look too. The key after an operator, after a text
        // object's `i` or `a`, or after `g` goes to the editor: it neither
        // waits for a `u` nor opens the next unread room with it.
        let markdown = r#"[link](url)<Space>"quoted""#;
        let unchanged = r#"[link](url) "quoted""#;
        for (typed, expected) in [
            ("0lci[text<Esc>", r#"[text](url) "quoted""#),
            ("0ldi[", r#"[](url) "quoted""#),
            ("0f(ldi(", r#"[link]() "quoted""#),
            ("0lvi[", unchanged),
            ("0lca]", unchanged),
            (r#"0f"lyi""#, unchanged),
            ("0d]", unchanged),
            ("0c[", unchanged),
            ("0y]", unchanged),
            ("0g]", unchanged),
            // The composer keeps the focus.
            ("0g<Esc>", unchanged),
        ] {
            let (mut app, mut composers) = composing(markdown);
            run(&mut app, &mut composers, typed);
            assert_eq!(text_of(&composers), expected, "{typed}");
            assert!(app.pending_keys().is_empty(), "{typed}");
            run(&mut app, &mut composers, "u");
            assert_eq!(
                (app.focus(), app.open_room()),
                (Focus::Composer, Some(&RoomId::new("!a:example.org"))),
                "{typed}"
            );
        }

        // `ci[` ends in insert mode, where `u` is typed; after `di[` it
        // undoes.
        let (mut app, mut composers) = composing(markdown);
        run(&mut app, &mut composers, "0lci[u");
        assert_eq!(text_of(&composers), r#"[u](url) "quoted""#);
        let (mut app, mut composers) = composing(markdown);
        run(&mut app, &mut composers, "0ldi[u");
        assert_eq!(text_of(&composers), unchanged);

        // `]u` itself still opens the next unread room from normal mode.
        run(&mut app, &mut composers, "]u");
        assert_eq!(
            (app.focus(), app.open_room()),
            (Focus::Timeline, Some(&RoomId::new("!b:example.org")))
        );
    }

    #[test]
    fn a_binding_in_normal_that_starts_with_g_runs_in_the_composer() {
        // The keymap sees `g` before edtui does, so `g` waits for the
        // binding's next key.
        let keymap = || {
            let gt = KeySeq::parse("gt", &leader()).unwrap();
            let binding = Binding::single(Action::FocusTimeline);
            Keymap::new(&leader(), [(TableId::Normal, gt, binding)])
        };
        let (mut app, mut composers) = composing_with(keymap(), "one<C-j>two");
        run(&mut app, &mut composers, "gt");
        assert_eq!(app.focus(), Focus::Timeline);

        // `gg` matches no binding: the first `g` goes to edtui, and the
        // second once `timeoutlen` passes.
        let (mut app, mut composers) = composing_with(keymap(), "one<C-j>two");
        let row = |composers: &Composers| {
            let composer = composers.get(&RoomId::new("!a:example.org")).unwrap();
            (composer.editor.state.cursor.row, composer.key_mode())
        };
        let timer = run(&mut app, &mut composers, "gg").unwrap();
        assert_eq!(row(&composers), (1, EditorMode::AwaitingChar));
        let effects = app.update(Msg::Timeout { id: timer });
        apply(&mut app, &mut composers, effects);
        assert_eq!(row(&composers), (0, EditorMode::Normal));
        assert_eq!(app.focus(), Focus::Composer);
    }

    #[test]
    fn keys_typed_quickly_after_gg_or_dd_run_with_a_binding_that_starts_with_g_or_d() {
        // The second `g` or `d` waits for the binding, while the editor,
        // which has the first one, waits for its next key. The second one
        // then ends as on its timeout, before the key typed after it.
        let keymap = |typed: &str| {
            let keys = KeySeq::parse(typed, &leader()).unwrap();
            let binding = Binding::single(Action::FocusTimeline);
            Keymap::new(&leader(), [(TableId::Normal, keys, binding)])
        };
        let row = |composers: &Composers| {
            let composer = composers.get(&RoomId::new("!a:example.org")).unwrap();
            composer.editor.state.cursor.row
        };
        // `gg` keeps the column, as in nvim, so `x` deletes the `e`.
        let (mut app, mut composers) = composing_with(keymap("gt"), "one<C-j>two");
        run(&mut app, &mut composers, "ggx");
        assert_eq!(
            (row(&composers), text_of(&composers)),
            (0, "on\ntwo".into())
        );
        assert!(app.pending_keys().is_empty());

        // `dd` deletes the first line, and `j` moves down.
        let (mut app, mut composers) = composing_with(keymap("dq"), "one<C-j>two<C-j>three");
        run(&mut app, &mut composers, "kkddj");
        assert_eq!(
            (row(&composers), text_of(&composers)),
            (1, "two\nthree".into())
        );
        assert_eq!(app.focus(), Focus::Composer);
    }
}
