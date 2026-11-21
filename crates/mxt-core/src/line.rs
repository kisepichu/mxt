//! The line editor of the `:` command line, the `/` search lines, the
//! pickers' queries and the `SECRET` prompt (design, "Line editing"): a text,
//! a cursor between grapheme clusters, and nvim's command-line editing keys,
//! with words split as the editors' word motions split them.

use std::borrow::BorrowMut;
use std::cmp::Ordering;
use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::keys::{Key, KeyCode, Modifiers};

/// A line of text and a cursor in it, a byte offset on a grapheme cluster
/// boundary. `T` holds the text: a `String`, or the recovery key, which is
/// edited in its own buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEditor<T = String> {
    text: T,
    cursor: usize,
}

/// What a key did to the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChange {
    /// The text changed, and the cursor maybe with it.
    Text,
    /// Only the cursor moved.
    Cursor,
    /// Nothing: a key the line editor does not take, or one with nothing to
    /// do, such as `<Left>` at the start.
    Nothing,
    /// `<BS>` found the text empty, which closes a command or search line.
    BackspaceOnEmpty,
}

/// What a key that the line editor takes does.
enum Op {
    Move(usize),
    Delete(Range<usize>),
}

const NONE: Modifiers = Modifiers {
    ctrl: false,
    alt: false,
    shift: false,
};
const CTRL: Modifiers = Modifiers {
    ctrl: true,
    alt: false,
    shift: false,
};
const SHIFT: Modifiers = Modifiers {
    ctrl: false,
    alt: false,
    shift: true,
};

impl<T: BorrowMut<String>> LineEditor<T> {
    /// `text` with the cursor at its end.
    pub fn new(text: T) -> LineEditor<T> {
        let cursor = text.borrow().len();
        LineEditor { text, cursor }
    }

    pub fn text(&self) -> &str {
        self.text.borrow()
    }

    /// The cursor's byte offset in [`LineEditor::text`], on a grapheme
    /// cluster boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn into_text(self) -> T {
        self.text
    }

    /// Replaces the text in the same buffer and puts the cursor at its end,
    /// as each completion step does.
    pub(crate) fn set_text(&mut self, text: &str) {
        let own = self.text.borrow_mut();
        own.clear();
        own.push_str(text);
        self.cursor = own.len();
    }

    /// Inserts `text` at the cursor, which moves after it, as typing and
    /// pastes do. Whether the text changed.
    pub(crate) fn insert(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.text.borrow_mut().insert_str(self.cursor, text);
        self.cursor = boundary_at_or_after(self.text(), self.cursor + text.len());
        true
    }

    /// Edits the line with `key`, one of the keys of the design's "Line
    /// editing" table; other keys do nothing.
    pub(crate) fn key(&mut self, key: Key) -> LineChange {
        if let Some(c) = key.text() {
            self.insert(c.encode_utf8(&mut [0; 4]));
            return LineChange::Text;
        }
        let (text, at) = (self.text(), self.cursor);
        let op = match (key.code(), key.mods()) {
            (KeyCode::Left, NONE) => Op::Move(previous(text, at)),
            (KeyCode::Right, NONE) => Op::Move(next(text, at)),
            (KeyCode::Home, NONE) | (KeyCode::Char('b'), CTRL) => Op::Move(0),
            (KeyCode::End, NONE) | (KeyCode::Char('e'), CTRL) => Op::Move(text.len()),
            (KeyCode::Left, SHIFT) => Op::Move(word_start(text, at)),
            (KeyCode::Right, SHIFT) => Op::Move(word_end(text, at)),
            (KeyCode::Backspace, NONE) if text.is_empty() => {
                return LineChange::BackspaceOnEmpty;
            }
            (KeyCode::Backspace, NONE) => Op::Delete(previous(text, at)..at),
            (KeyCode::Delete, NONE) => Op::Delete(at..next(text, at)),
            (KeyCode::Char('w'), CTRL) => Op::Delete(word_start(text, at)..at),
            (KeyCode::Char('u'), CTRL) => Op::Delete(0..at),
            _ => return LineChange::Nothing,
        };
        match op {
            Op::Move(to) if to == at => LineChange::Nothing,
            Op::Move(to) => {
                self.cursor = to;
                LineChange::Cursor
            }
            Op::Delete(range) if range.is_empty() => LineChange::Nothing,
            Op::Delete(range) => {
                let start = range.start;
                self.text.borrow_mut().replace_range(range, "");
                self.cursor = boundary_at_or_after(self.text(), start);
                LineChange::Text
            }
        }
    }
}

/// The boundary of the cluster before `at`; `at` at the start.
fn previous(text: &str, at: usize) -> usize {
    text[..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(at, |(start, _)| start)
}

/// The boundary of the cluster after `at`; `at` at the end.
fn next(text: &str, at: usize) -> usize {
    text[at..]
        .graphemes(true)
        .next()
        .map_or(at, |cluster| at + cluster.len())
}

/// Where `<S-Left>` moves from `at`, and where `<C-w>` deletes from: back
/// over blanks, then back over a run of clusters of one class.
fn word_start(text: &str, at: usize) -> usize {
    let mut clusters = text[..at]
        .grapheme_indices(true)
        .rev()
        .skip_while(|&(_, cluster)| class(cluster) == Class::Blank)
        .peekable();
    let Some(&(_, first)) = clusters.peek() else {
        return 0;
    };
    let word = class(first);
    clusters
        .take_while(|&(_, cluster)| class(cluster) == word)
        .last()
        .map_or(at, |(start, _)| start)
}

/// Where `<S-Right>` moves from `at`: forward over blanks, then forward over
/// a run of clusters of one class.
fn word_end(text: &str, at: usize) -> usize {
    let mut clusters = text[at..]
        .grapheme_indices(true)
        .skip_while(|&(_, cluster)| class(cluster) == Class::Blank)
        .peekable();
    let Some(&(_, first)) = clusters.peek() else {
        return text.len();
    };
    let word = class(first);
    clusters
        .take_while(|&(_, cluster)| class(cluster) == word)
        .last()
        .map_or(at, |(start, cluster)| at + start + cluster.len())
}

/// `at` when it is a cluster boundary, else the end of the cluster around
/// it: an edit can join the text on both sides of the cursor into one
/// cluster, as a combining mark typed before another character does.
fn boundary_at_or_after(text: &str, at: usize) -> usize {
    let mut cursor = GraphemeCursor::new(at, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(false) => cursor
            .next_boundary(text, 0)
            .ok()
            .flatten()
            .unwrap_or(text.len()),
        _ => at,
    }
}

/// The class of a grapheme cluster for words: its first character's. A word
/// is a run of clusters of one class other than `Blank`.
fn class(cluster: &str) -> Class {
    cluster.chars().next().map_or(Class::Blank, Class::of)
}

/// The classes of the editors' word motions, vim's character classes as mxt's
/// edtui fork has them (design, "edtui differences from vim").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// Spaces and tabs, the no-break and the ideographic space included.
    Blank,
    /// ASCII letters and digits, `_`, Latin-1 letters, and the letters of
    /// scripts without a class of their own (Greek, Cyrillic, ...).
    Word,
    /// Punctuation and symbols, ASCII, full-width and ideographic alike.
    /// Emoji count as symbols.
    Punctuation,
    Superscript,
    Subscript,
    Braille,
    Hiragana,
    Katakana,
    Kanji,
    Hangul,
}

impl Class {
    /// A copy of `CharacterClass` in `src/actions/motion.rs` of mxt's edtui
    /// fork (`kisepichu/edtui` at the commit the workspace `Cargo.toml`
    /// pins), which follows vim's `utf_class()` (`utf_class_buf()` in vim's
    /// `src/mbyte.c`); `mxt-core` cannot depend on edtui, so the two copies
    /// change together. Vim's classes of the characters above Latin-1:
    /// sorted, non-overlapping ranges. Characters outside them are word
    /// characters.
    ///
    /// The fork's comment on the rows marked `*`, which are not in vim's
    /// table:
    /// - Vim gives emoji a class of their own; here they are symbols. The
    ///   `*` symbol rows cover the emoji outside vim's symbol ranges (whole
    ///   symbol blocks, or single code points).
    /// - Vim treats variation selectors as composing characters, part of the
    ///   character before them. Motions here move by char, so the selectors
    ///   are symbols, which keeps a word together after emoji and symbols,
    ///   and the ideographic selectors are ideographs.
    /// - Vim gives tags the word class; here they are symbols, like the flag
    ///   emoji of a tag sequence.
    ///
    /// The line editor moves by grapheme cluster and classifies a cluster by
    /// its first character, so a variation selector, a skin tone or a tag
    /// after a base character never counts on its own here.
    const RANGES: [(char, char, Class); 78] = [
        ('\u{037e}', '\u{037e}', Class::Punctuation), // Greek question mark
        ('\u{0387}', '\u{0387}', Class::Punctuation), // Greek ano teleia
        ('\u{055a}', '\u{055f}', Class::Punctuation), // Armenian punctuation
        ('\u{0589}', '\u{0589}', Class::Punctuation), // Armenian full stop
        ('\u{05be}', '\u{05be}', Class::Punctuation),
        ('\u{05c0}', '\u{05c0}', Class::Punctuation),
        ('\u{05c3}', '\u{05c3}', Class::Punctuation),
        ('\u{05f3}', '\u{05f4}', Class::Punctuation),
        ('\u{060c}', '\u{060c}', Class::Punctuation),
        ('\u{061b}', '\u{061b}', Class::Punctuation),
        ('\u{061f}', '\u{061f}', Class::Punctuation),
        ('\u{066a}', '\u{066d}', Class::Punctuation),
        ('\u{06d4}', '\u{06d4}', Class::Punctuation),
        ('\u{0700}', '\u{070d}', Class::Punctuation), // Syriac punctuation
        ('\u{0964}', '\u{0965}', Class::Punctuation),
        ('\u{0970}', '\u{0970}', Class::Punctuation),
        ('\u{0df4}', '\u{0df4}', Class::Punctuation),
        ('\u{0e4f}', '\u{0e4f}', Class::Punctuation),
        ('\u{0e5a}', '\u{0e5b}', Class::Punctuation),
        ('\u{0f04}', '\u{0f12}', Class::Punctuation),
        ('\u{0f3a}', '\u{0f3d}', Class::Punctuation),
        ('\u{0f85}', '\u{0f85}', Class::Punctuation),
        ('\u{104a}', '\u{104f}', Class::Punctuation), // Myanmar punctuation
        ('\u{10fb}', '\u{10fb}', Class::Punctuation), // Georgian punctuation
        ('\u{1361}', '\u{1368}', Class::Punctuation), // Ethiopic punctuation
        ('\u{166d}', '\u{166e}', Class::Punctuation), // Canadian Syl. punctuation
        ('\u{1680}', '\u{1680}', Class::Blank),
        ('\u{169b}', '\u{169c}', Class::Punctuation),
        ('\u{16eb}', '\u{16ed}', Class::Punctuation),
        ('\u{1735}', '\u{1736}', Class::Punctuation),
        ('\u{17d4}', '\u{17dc}', Class::Punctuation), // Khmer punctuation
        ('\u{1800}', '\u{180a}', Class::Punctuation), // Mongolian punctuation
        ('\u{2000}', '\u{200b}', Class::Blank),       // spaces
        ('\u{200c}', '\u{2027}', Class::Punctuation), // punctuation and symbols
        ('\u{2028}', '\u{2029}', Class::Blank),
        ('\u{202a}', '\u{202e}', Class::Punctuation), // punctuation and symbols
        ('\u{202f}', '\u{202f}', Class::Blank),
        ('\u{2030}', '\u{205e}', Class::Punctuation), // punctuation and symbols
        ('\u{205f}', '\u{205f}', Class::Blank),
        ('\u{2060}', '\u{206f}', Class::Punctuation), // punctuation and symbols
        ('\u{2070}', '\u{207f}', Class::Superscript),
        ('\u{2080}', '\u{2094}', Class::Subscript),
        ('\u{20a0}', '\u{27ff}', Class::Punctuation), // all kinds of symbols
        ('\u{2800}', '\u{28ff}', Class::Braille),
        ('\u{2900}', '\u{2998}', Class::Punctuation), // arrows, brackets, etc.
        ('\u{29d8}', '\u{29db}', Class::Punctuation),
        ('\u{29fc}', '\u{29fd}', Class::Punctuation),
        ('\u{2b00}', '\u{2bff}', Class::Punctuation), // * misc. symbols and arrows
        ('\u{2e00}', '\u{2e7f}', Class::Punctuation), // supplemental punctuation
        ('\u{3000}', '\u{3000}', Class::Blank),       // ideographic space
        ('\u{3001}', '\u{3020}', Class::Punctuation), // ideographic punctuation
        ('\u{3030}', '\u{3030}', Class::Punctuation),
        ('\u{303d}', '\u{303d}', Class::Punctuation),
        ('\u{3040}', '\u{309f}', Class::Hiragana),
        ('\u{30a0}', '\u{30ff}', Class::Katakana),
        ('\u{3297}', '\u{3297}', Class::Punctuation), // * circled ideograph congratulation
        ('\u{3299}', '\u{3299}', Class::Punctuation), // * circled ideograph secret
        ('\u{3300}', '\u{9fff}', Class::Kanji),
        ('\u{ac00}', '\u{d7a3}', Class::Hangul),
        ('\u{f900}', '\u{faff}', Class::Kanji),
        ('\u{fd3e}', '\u{fd3f}', Class::Punctuation),
        ('\u{fe00}', '\u{fe0f}', Class::Punctuation), // * variation selectors
        ('\u{fe30}', '\u{fe6b}', Class::Punctuation), // punctuation forms
        ('\u{ff00}', '\u{ff0f}', Class::Punctuation), // half/fullwidth ASCII
        ('\u{ff1a}', '\u{ff20}', Class::Punctuation), // half/fullwidth ASCII
        ('\u{ff3b}', '\u{ff40}', Class::Punctuation), // half/fullwidth ASCII
        ('\u{ff5b}', '\u{ff65}', Class::Punctuation), // half/fullwidth ASCII
        ('\u{1d000}', '\u{1d24f}', Class::Punctuation), // musical notation
        ('\u{1d400}', '\u{1d7ff}', Class::Punctuation), // mathematical alphanumeric symbols
        ('\u{1f000}', '\u{1f2ff}', Class::Punctuation), // game pieces; enclosed characters
        ('\u{1f300}', '\u{1f9ff}', Class::Punctuation), // many symbol blocks
        ('\u{1fa70}', '\u{1faff}', Class::Punctuation), // * symbols and pictographs ext. A
        ('\u{20000}', '\u{2a6df}', Class::Kanji),
        ('\u{2a700}', '\u{2b73f}', Class::Kanji),
        ('\u{2b740}', '\u{2b81f}', Class::Kanji),
        ('\u{2f800}', '\u{2fa1f}', Class::Kanji),
        ('\u{e0020}', '\u{e007f}', Class::Punctuation), // * tags
        ('\u{e0100}', '\u{e01ef}', Class::Kanji),       // * ideographic variation selectors
    ];

    fn of(c: char) -> Class {
        if u32::from(c) < 0x100 {
            if c.is_ascii_whitespace() || c == '\u{a0}' {
                return Class::Blank;
            }
            // Underscore and the Latin-1 letters count as word characters
            // (vim's default `iskeyword`, `@,48-57,_,192-255`).
            if c.is_ascii_alphanumeric() || c == '_' || c >= '\u{c0}' {
                return Class::Word;
            }
            return Class::Punctuation;
        }
        Class::RANGES
            .binary_search_by(|&(first, last, _)| {
                if last < c {
                    Ordering::Less
                } else if first > c {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .map_or(Class::Word, |index| Class::RANGES[index].2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeySeq;

    /// Keys in key notation; none for an empty text.
    fn keys(text: &str) -> Vec<Key> {
        if text.is_empty() {
            return Vec::new();
        }
        KeySeq::parse(text, &KeySeq::default())
            .unwrap()
            .keys()
            .to_vec()
    }

    /// The line `text` with the cursor at the end, after `keys` in key
    /// notation; the change the last key made.
    fn edit(text: &str, typed: &str) -> (LineEditor, LineChange) {
        let mut line = LineEditor::new(text.to_owned());
        let mut change = LineChange::Nothing;
        for key in keys(typed) {
            change = line.key(key);
        }
        (line, change)
    }

    /// The text with `|` at the cursor.
    fn shown(line: &LineEditor) -> String {
        let (before, after) = line.text().split_at(line.cursor());
        format!("{before}|{after}")
    }

    fn after(text: &str, keys: &str) -> String {
        shown(&edit(text, keys).0)
    }

    /// The texts `<C-w>` leaves, pressed again until the line is empty.
    fn deleted_words(text: &str) -> Vec<String> {
        let mut line = LineEditor::new(text.to_owned());
        let key = keys("<C-w>")[0];
        let mut left = Vec::new();
        while !line.text().is_empty() {
            assert_eq!(line.key(key), LineChange::Text, "{left:?}");
            left.push(line.text().to_owned());
        }
        left
    }

    const THUMBS: &str = "👍🏽";
    const FAMILY: &str = "👨\u{200d}👩\u{200d}👧";

    #[test]
    fn typing_inserts_at_the_cursor() {
        assert_eq!(after("ac", "<Left>b"), "ab|c");
        // Shift is folded into the character.
        assert_eq!(after("", "<S-a>B"), "AB|");
        assert_eq!(after("日本", "<Left>の"), "日の|本");
        // Not with ctrl or alt.
        assert_eq!(
            edit("a", "<C-z><M-b>"),
            (LineEditor::new("a".into()), LineChange::Nothing)
        );
    }

    #[test]
    fn left_and_right_move_by_grapheme_cluster() {
        for cluster in [THUMBS, FAMILY, "日", "e\u{301}"] {
            let text = format!("a{cluster}b");
            assert_eq!(after(&text, "<Left><Left>"), format!("a|{cluster}b"));
            assert_eq!(
                after(&text, "<Home><Right><Right>"),
                format!("a{cluster}|b")
            );
        }
        let (line, change) = edit("ab", "<Left><Left><Left>");
        assert_eq!((shown(&line), change), ("|ab".into(), LineChange::Nothing));
        let (line, change) = edit("ab", "<Right>");
        assert_eq!((shown(&line), change), ("ab|".into(), LineChange::Nothing));
        assert_eq!(edit("ab", "<Left>").1, LineChange::Cursor);
    }

    #[test]
    fn home_end_and_their_ctrl_keys_move_to_the_ends() {
        for (keys, shown) in [
            ("<Home>", "|日本語"),
            ("<C-b>", "|日本語"),
            ("<Home><End>", "日本語|"),
            ("<C-b><C-e>", "日本語|"),
        ] {
            assert_eq!(after("日本語", keys), shown, "{keys}");
        }
        assert_eq!(edit("日本語", "<End>").1, LineChange::Nothing);
    }

    #[test]
    fn backspace_and_delete_take_one_cluster() {
        for cluster in [THUMBS, FAMILY, "語"] {
            let text = format!("a{cluster}{cluster}b");
            assert_eq!(after(&text, "<Left><BS>"), format!("a{cluster}|b"));
            assert_eq!(after(&text, "<Home><Right><Del>"), format!("a|{cluster}b"));
        }
        assert_eq!(edit("ab", "<BS>").1, LineChange::Text);
        // Nothing at the start or at the end of a line with text.
        let (line, change) = edit("ab", "<Home><BS>");
        assert_eq!((shown(&line), change), ("|ab".into(), LineChange::Nothing));
        let (line, change) = edit("ab", "<Del>");
        assert_eq!((shown(&line), change), ("ab|".into(), LineChange::Nothing));
        // An empty line reports the backspace, and `<Del>` does nothing.
        assert_eq!(edit("", "<BS>").1, LineChange::BackspaceOnEmpty);
        assert_eq!(edit("", "<Del>").1, LineChange::Nothing);
    }

    #[test]
    fn ctrl_w_deletes_words_by_class() {
        assert_eq!(
            deleted_words("日本語のテキスト"),
            ["日本語の", "日本語", ""]
        );
        assert_eq!(
            deleted_words("open foo.bar"),
            ["open foo.", "open foo", "open ", ""]
        );
        // Blanks before the cursor go with the word before them; emoji and
        // full-width punctuation are punctuation, and hangul a class.
        assert_eq!(
            deleted_words(&format!("한국어{THUMBS}！ abc  ")),
            [
                format!("한국어{THUMBS}！ "),
                "한국어".to_owned(),
                String::new()
            ]
        );
        assert_eq!(deleted_words("a\u{3000}テスト"), ["a\u{3000}", ""]);
    }

    #[test]
    fn ctrl_w_and_ctrl_u_delete_before_the_cursor() {
        assert_eq!(after("open foo.bar", "<S-Left><S-Left><C-w>"), "open |.bar");
        assert_eq!(after("open foo.bar", "<Left><Left><Left><C-u>"), "|bar");
        let (line, change) = edit("ab", "<Home><C-u>");
        assert_eq!((shown(&line), change), ("|ab".into(), LineChange::Nothing));
        assert_eq!(edit("ab", "<Home><C-w>").1, LineChange::Nothing);
    }

    #[test]
    fn shift_arrows_move_by_word_across_blanks() {
        let text = "open  foo.bar 日本語のテキスト";
        let stops = [
            "open  foo.bar 日本語のテキスト|",
            "open  foo.bar 日本語の|テキスト",
            "open  foo.bar 日本語|のテキスト",
            "open  foo.bar |日本語のテキスト",
            "open  foo.|bar 日本語のテキスト",
            "open  foo|.bar 日本語のテキスト",
            "open  |foo.bar 日本語のテキスト",
            "|open  foo.bar 日本語のテキスト",
        ];
        let mut keys = String::new();
        for stop in stops {
            assert_eq!(after(text, &keys), stop, "{keys}");
            keys.push_str("<S-Left>");
        }
        assert_eq!(edit(text, &keys).1, LineChange::Nothing);
        let stops = [
            "open|  foo.bar 日本語のテキスト",
            "open  foo|.bar 日本語のテキスト",
            "open  foo.|bar 日本語のテキスト",
            "open  foo.bar| 日本語のテキスト",
            "open  foo.bar 日本語|のテキスト",
            "open  foo.bar 日本語の|テキスト",
            "open  foo.bar 日本語のテキスト|",
        ];
        let mut keys = "<Home>".to_owned();
        for stop in stops {
            keys.push_str("<S-Right>");
            assert_eq!(after(text, &keys), stop, "{keys}");
        }
        // Blanks alone: to the ends.
        assert_eq!(after("a  ", "<S-Left>"), "|a  ");
        assert_eq!(after("  a", "<Home><S-Right>"), "  a|");
        assert_eq!(after("a  ", "<Left><S-Right>"), "a  |");
    }

    #[test]
    fn inserting_and_setting_text_keep_the_cursor_on_a_cluster_boundary() {
        let mut line = LineEditor::new("ab".to_owned());
        line.key(keys("<Left>")[0]);
        assert!(line.insert("日本"));
        assert_eq!(shown(&line), "a日本|b");
        assert!(!line.insert(""));
        // A combining mark typed before a character ends up in its cluster:
        // the cursor goes after the whole cluster.
        let mut line = LineEditor::new("\u{301}".to_owned());
        line.key(keys("<Home>")[0]);
        line.insert("e");
        assert_eq!(shown(&line), "e\u{301}|");
        line.set_text("open ");
        assert_eq!(shown(&line), "open |");
    }

    #[test]
    fn classes_follow_the_editors() {
        for (c, class) in [
            (' ', Class::Blank),
            ('\t', Class::Blank),
            ('\u{a0}', Class::Blank),
            ('\u{3000}', Class::Blank),
            ('a', Class::Word),
            ('_', Class::Word),
            ('é', Class::Word),
            ('ж', Class::Word),
            ('.', Class::Punctuation),
            ('、', Class::Punctuation),
            ('！', Class::Punctuation),
            ('👍', Class::Punctuation),
            ('🫠', Class::Punctuation),
            ('²', Class::Punctuation),
            ('⁴', Class::Superscript),
            ('₂', Class::Subscript),
            ('⠃', Class::Braille),
            ('の', Class::Hiragana),
            ('テ', Class::Katakana),
            ('語', Class::Kanji),
            ('한', Class::Hangul),
        ] {
            assert_eq!(Class::of(c), class, "{c:?}");
        }
        assert!(Class::RANGES.windows(2).all(|pair| pair[0].1 < pair[1].0));
    }
}
