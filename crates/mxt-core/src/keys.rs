//! Keys and key sequences in nvim key notation.
//!
//! Keys are normalized when they are built, so the keymap, the configuration
//! and the terminal adapter compare them directly.

use std::borrow::Borrow;
use std::fmt::{self, Write};

/// A key without its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// A function key; key notation has `<F1>` to `<F12>`.
    F(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// A key and its modifiers, normalized by [`Key::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    code: KeyCode,
    mods: Modifiers,
}

impl Key {
    /// Normalizes like nvim: for `Char`, shift is folded into the character
    /// (`a` + shift becomes `A`) and dropped; with ctrl, ASCII letters are
    /// lowercased (`<C-A>` is `<C-a>`).
    pub fn new(code: KeyCode, mut mods: Modifiers) -> Key {
        let code = match code {
            KeyCode::Char(mut c) => {
                if mods.shift {
                    c = shifted(c);
                    mods.shift = false;
                }
                if mods.ctrl {
                    c = c.to_ascii_lowercase();
                }
                KeyCode::Char(c)
            }
            code => code,
        };
        Key { code, mods }
    }

    /// A plain character without modifiers.
    pub fn char(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: Modifiers::default(),
        }
    }

    pub fn code(self) -> KeyCode {
        self.code
    }

    pub fn mods(self) -> Modifiers {
        self.mods
    }

    /// `Some(c)` for a `Char` without ctrl or alt: text a text input should insert.
    pub fn text(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if !self.mods.ctrl && !self.mods.alt => Some(c),
            _ => None,
        }
    }
}

/// `c` typed with shift: its uppercase form when that is one character.
fn shifted(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(upper), None) => upper,
        _ => c,
    }
}

/// Names inside `<...>`, compared case-insensitively, aliases included.
const NAMES: &[(&str, KeyCode)] = &[
    ("CR", KeyCode::Enter),
    ("Enter", KeyCode::Enter),
    ("Return", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Tab", KeyCode::Tab),
    ("BS", KeyCode::Backspace),
    ("Backspace", KeyCode::Backspace),
    ("Del", KeyCode::Delete),
    ("Delete", KeyCode::Delete),
    ("Insert", KeyCode::Insert),
    ("Ins", KeyCode::Insert),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
    ("Space", KeyCode::Char(' ')),
    ("lt", KeyCode::Char('<')),
    ("Bslash", KeyCode::Char('\\')),
    ("Bar", KeyCode::Char('|')),
];

/// The key a name inside `<...>` stands for, `<leader>` excluded.
fn named_key(name: &str) -> Option<KeyCode> {
    if let Some(&(_, code)) = NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(code);
    }
    let number = name.strip_prefix(['F', 'f'])?;
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u8 = number.parse().ok()?;
    (1..=12).contains(&n).then_some(KeyCode::F(n))
}

/// How [`Key`]'s `Display` writes a key code.
enum Label {
    Name(&'static str),
    Char(char),
    F(u8),
}

fn label(code: KeyCode) -> Label {
    Label::Name(match code {
        KeyCode::Char(' ') => "Space",
        KeyCode::Char('<') => "lt",
        KeyCode::Char('\\') => "Bslash",
        KeyCode::Char('|') => "Bar",
        KeyCode::Char(c) => return Label::Char(c),
        KeyCode::F(n) => return Label::F(n),
        KeyCode::Enter => "CR",
        KeyCode::Esc => "Esc",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "BS",
        KeyCode::Delete => "Del",
        KeyCode::Insert => "Insert",
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
    })
}

/// Notation form: `a`, `A`, `<Space>`, `<lt>`, `<CR>`, `<C-x>`, `<A-x>`,
/// `<S-Tab>`, `<C-A-x>`, `<F5>`.
impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = label(self.code);
        if let Label::Char(c) = label
            && self.mods == Modifiers::default()
        {
            return f.write_char(c);
        }
        f.write_char('<')?;
        for (on, prefix) in [
            (self.mods.ctrl, "C-"),
            (self.mods.alt, "A-"),
            (self.mods.shift, "S-"),
        ] {
            if on {
                f.write_str(prefix)?;
            }
        }
        match label {
            Label::Name(name) => f.write_str(name)?,
            Label::Char(c) => f.write_char(c)?,
            Label::F(n) => write!(f, "F{n}")?,
        }
        f.write_char('>')
    }
}

/// A sequence of keys, ordered lexicographically by its keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct KeySeq(Vec<Key>);

impl KeySeq {
    /// Parses nvim key notation; `<leader>` expands to `leader`.
    ///
    /// A character outside `<...>` is that character; a `<` must start a
    /// `<...>` group (`<lt>` is a literal `<`). A group holds modifier
    /// prefixes (`C-`, `A-` or `M-`, `S-`, case-insensitive, in any order)
    /// followed by one character or a key name (case-insensitive), or it is
    /// `<leader>` without modifiers.
    ///
    /// `S-` on a character must leave an uppercase letter (`<S-a>` is `A`).
    /// Otherwise [`Key::new`] would drop the shift, so `<S-1>` and `<C-S-c>`
    /// are errors instead of silently meaning `1` and `<C-c>`.
    pub fn parse(text: &str, leader: &KeySeq) -> Result<KeySeq, KeyParseError> {
        if text.is_empty() {
            return Err(KeyParseError::Empty);
        }
        let mut keys = Vec::new();
        let mut at = 0;
        while let Some(c) = text[at..].chars().next() {
            if c == '<' {
                at += 1 + parse_group(&text[at + 1..], at, leader, &mut keys)?;
            } else {
                keys.push(Key::char(c));
                at += c.len_utf8();
            }
        }
        Ok(KeySeq(keys))
    }

    pub fn from_keys(keys: Vec<Key>) -> KeySeq {
        KeySeq(keys)
    }

    pub fn keys(&self) -> &[Key] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Parses the group of a `<` at byte offset `at` of the text; `body` is the
/// text after that `<`. Returns the length of `body` the group takes,
/// including its `>`.
fn parse_group(
    body: &str,
    at: usize,
    leader: &KeySeq,
    keys: &mut Vec<Key>,
) -> Result<usize, KeyParseError> {
    let close = body.find('>').ok_or(KeyParseError::Unclosed { at })?;
    let mut mods = Modifiers::default();
    let mut rest = body;
    // A modifier is a character and a dash with more text after them, so
    // `<C-->` is ctrl and `-`. Modifiers never take a `>`, so `close` is still
    // the first `>` after them.
    loop {
        let mut chars = rest.chars();
        let (Some(m), Some('-'), Some(_)) = (chars.next(), chars.next(), chars.next()) else {
            break;
        };
        match m.to_ascii_lowercase() {
            'c' => mods.ctrl = true,
            'a' | 'm' => mods.alt = true,
            's' => mods.shift = true,
            _ => return Err(KeyParseError::UnknownModifier { name: m.into() }),
        }
        rest = &rest[m.len_utf8() + 1..];
    }
    let start = body.len() - rest.len();
    let mut chars = rest.chars();
    // One character before a `>` is that character, even `>` itself (`<C->>`).
    let (end, code) = match (chars.next(), chars.next()) {
        (Some(c), Some('>')) => (start + c.len_utf8(), Some(KeyCode::Char(c))),
        _ => (close, named_key(&body[start..close])),
    };
    match code {
        Some(code) => {
            let key = Key::new(code, mods);
            if mods.shift
                && let KeyCode::Char(c) = key.code()
                && !c.is_uppercase()
            {
                return Err(KeyParseError::ShiftIgnored {
                    name: body[..end].to_owned(),
                    key,
                });
            }
            keys.push(key);
        }
        None if body[start..end].eq_ignore_ascii_case("leader") => {
            if mods != Modifiers::default() {
                return Err(KeyParseError::ModifiedLeader);
            }
            if leader.is_empty() {
                return Err(KeyParseError::NoLeader);
            }
            keys.extend_from_slice(leader.keys());
        }
        None => {
            return Err(KeyParseError::UnknownKey {
                name: body[..end].to_owned(),
            });
        }
    }
    Ok(end + 1)
}

/// Lets maps keyed by `KeySeq` be queried with a slice of keys.
impl Borrow<[Key]> for KeySeq {
    fn borrow(&self) -> &[Key] {
        &self.0
    }
}

/// The concatenated notation of the keys.
impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|key| key.fmt(f))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyParseError {
    Empty,
    /// A `<` without a matching `>`. Write `<lt>` for a literal `<`.
    Unclosed {
        at: usize,
    },
    /// The text inside `<...>`, such as `Foo` or `C-F13`.
    UnknownKey {
        name: String,
    },
    UnknownModifier {
        name: String,
    },
    /// `S-` on a character that keeps no shift after normalization: `<S-1>`
    /// is `1`, `<S-Space>` is `<Space>`, `<C-S-c>` is `<C-c>`. `name` is the
    /// text inside `<...>`, `key` the key it would have been.
    ShiftIgnored {
        name: String,
        key: Key,
    },
    /// `<leader>` where no leader is available (inside the leader itself).
    NoLeader,
    /// `<leader>` with modifiers, such as `<C-leader>`.
    ModifiedLeader,
}

impl KeyParseError {
    /// Whether the error names a key or a modifier this version does not
    /// know, which a newer version may know: the configuration ignores such
    /// a binding with a warning (design, "Layers"). Malformed notation is
    /// never one.
    pub fn is_unknown_name(&self) -> bool {
        matches!(
            self,
            KeyParseError::UnknownKey { .. } | KeyParseError::UnknownModifier { .. }
        )
    }
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyParseError::Empty => f.write_str("empty key sequence"),
            KeyParseError::Unclosed { at } => write!(
                f,
                "\"<\" at byte {at} is not closed by \">\"; write <lt> for a literal \"<\""
            ),
            KeyParseError::UnknownKey { name } => write!(f, "unknown key <{name}>"),
            KeyParseError::UnknownModifier { name } => write!(f, "unknown modifier \"{name}-\""),
            KeyParseError::NoLeader => f.write_str("<leader> cannot be used in the leader itself"),
            KeyParseError::ModifiedLeader => f.write_str("<leader> takes no modifiers"),
            KeyParseError::ShiftIgnored { name, key } => {
                write!(f, "<{name}> is the same key as {key}")?;
                match key.code() {
                    KeyCode::Char(c) if key.mods().ctrl && shifted(c) != c => {
                        f.write_str(": with ctrl, letters do not keep their case")
                    }
                    _ => f.write_str(
                        ": shift is kept only as an uppercase letter; write the character \
                         shift types instead, such as ! for shift+1",
                    ),
                }
            }
        }
    }
}

impl std::error::Error for KeyParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> Key {
        Key::new(
            KeyCode::Char(c),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        )
    }
    fn code(code: KeyCode) -> Key {
        Key::new(code, Modifiers::default())
    }
    fn parse(text: &str) -> Result<KeySeq, KeyParseError> {
        KeySeq::parse(text, &KeySeq::from_keys(vec![Key::char(' ')]))
    }
    fn keys(text: &str) -> Vec<Key> {
        parse(text).unwrap().keys().to_vec()
    }

    #[test]
    fn plain_characters() {
        assert_eq!(keys("gg"), vec![Key::char('g'), Key::char('g')]);
        assert_eq!(keys("]u"), vec![Key::char(']'), Key::char('u')]);
        assert_eq!(keys("日本"), vec![Key::char('日'), Key::char('本')]);
    }

    #[test]
    fn special_names_are_case_insensitive() {
        assert_eq!(keys("<CR>"), vec![code(KeyCode::Enter)]);
        assert_eq!(keys("<cr>"), vec![code(KeyCode::Enter)]);
        assert_eq!(
            keys("<Enter><Return>"),
            vec![code(KeyCode::Enter), code(KeyCode::Enter)]
        );
        assert_eq!(
            keys("<Esc><Tab><BS><Del>"),
            vec![
                code(KeyCode::Esc),
                code(KeyCode::Tab),
                code(KeyCode::Backspace),
                code(KeyCode::Delete),
            ]
        );
        assert_eq!(keys("<F12>"), vec![code(KeyCode::F(12))]);
        assert_eq!(
            keys("<Space><lt><Bslash><Bar>"),
            vec![
                Key::char(' '),
                Key::char('<'),
                Key::char('\\'),
                Key::char('|'),
            ]
        );
    }

    #[test]
    fn modifiers() {
        assert_eq!(keys("<C-x><C-e>"), vec![ctrl('x'), ctrl('e')]);
        assert_eq!(keys("<C-X>"), vec![ctrl('x')]);
        assert_eq!(keys("<c-w>h"), vec![ctrl('w'), Key::char('h')]);
        assert_eq!(keys("<C-->"), vec![ctrl('-')]);
        let alt_x = Key::new(
            KeyCode::Char('x'),
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(keys("<A-x>"), vec![alt_x]);
        assert_eq!(keys("<M-x>"), vec![alt_x]);
        let ctrl_alt = Key::new(
            KeyCode::Char('x'),
            Modifiers {
                ctrl: true,
                alt: true,
                shift: false,
            },
        );
        assert_eq!(keys("<C-A-x>"), vec![ctrl_alt]);
        assert_eq!(keys("<A-C-x>"), vec![ctrl_alt]);
        let shift_tab = Key::new(
            KeyCode::Tab,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(keys("<S-Tab>"), vec![shift_tab]);
    }

    #[test]
    fn shift_folds_into_characters() {
        assert_eq!(keys("<S-a>"), vec![Key::char('A')]);
        assert_eq!(keys("<S-A>"), vec![Key::char('A')]);
        let alt_shift_a = Key::new(
            KeyCode::Char('A'),
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(keys("<A-S-a>"), vec![alt_shift_a]);
        let shifted = Key::new(
            KeyCode::Char('A'),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(shifted, Key::char('A'));
    }

    #[test]
    fn a_shift_that_normalization_drops_is_an_error() {
        for text in [
            "<S-1>",
            "<S-Space>",
            "<S-lt>",
            "<C-S-c>",
            "<C-S-A>",
            "<A-S-1>",
        ] {
            assert!(
                matches!(parse(text), Err(KeyParseError::ShiftIgnored { .. })),
                "{text}"
            );
        }
        // The hints name the key it would have been and what to write instead.
        let lost = parse("<S-1>").unwrap_err().to_string();
        assert!(
            lost.contains("same key as 1") && lost.contains('!'),
            "{lost}"
        );
        let ctrl = parse("<C-S-c>").unwrap_err().to_string();
        assert!(ctrl.contains("same key as <C-c>"), "{ctrl}");
    }

    #[test]
    fn leader_expands() {
        let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
        let seq = KeySeq::parse("<leader><leader>e", &comma).unwrap();
        assert_eq!(
            seq.keys(),
            &[Key::char(','), Key::char(','), Key::char('e')]
        );
        assert_eq!(keys("<Leader>m"), vec![Key::char(' '), Key::char('m')]);
        assert_eq!(
            KeySeq::parse("<leader>", &KeySeq::default()),
            Err(KeyParseError::NoLeader)
        );
    }

    #[test]
    fn errors() {
        assert_eq!(parse(""), Err(KeyParseError::Empty));
        assert_eq!(parse("a<C-x"), Err(KeyParseError::Unclosed { at: 1 }));
        assert_eq!(parse("<"), Err(KeyParseError::Unclosed { at: 0 }));
        assert!(matches!(
            parse("<Foo>"),
            Err(KeyParseError::UnknownKey { .. })
        ));
        assert!(matches!(
            parse("<X-a>"),
            Err(KeyParseError::UnknownModifier { .. })
        ));
        assert_eq!(parse("<C-leader>"), Err(KeyParseError::ModifiedLeader));
        assert_eq!(
            KeySeq::parse("<C-leader>", &KeySeq::default()),
            Err(KeyParseError::ModifiedLeader)
        );
        for text in ["<F13>", "<D-s>", "<C-F13>"] {
            assert!(parse(text).unwrap_err().is_unknown_name(), "{text}");
        }
        for text in ["", "<C-x", "<S-1>", "<C-leader>"] {
            assert!(!parse(text).unwrap_err().is_unknown_name(), "{text:?}");
        }
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "gg",
            "<C-x><C-e>",
            "<Space>e",
            "<lt>",
            "<CR>",
            "<S-Tab>",
            "<C-A-x>",
            "<F5>",
            "]u",
        ] {
            let seq = parse(text).unwrap();
            assert_eq!(parse(&seq.to_string()).unwrap(), seq, "{text}");
        }
        assert_eq!(parse("<c-X>").unwrap().to_string(), "<C-x>");
    }

    #[test]
    fn modified_and_named_characters_round_trip() {
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        for key in [
            ctrl('>'),
            ctrl('-'),
            ctrl(' '),
            Key::new(KeyCode::Char('<'), alt),
            Key::char('\\'),
            Key::char('|'),
            Key::char('>'),
        ] {
            assert_eq!(keys(&key.to_string()), vec![key], "{key}");
        }
        assert_eq!(ctrl('>').to_string(), "<C->>");
    }

    #[test]
    fn text_is_only_unmodified_characters() {
        assert_eq!(Key::char('a').text(), Some('a'));
        assert_eq!(Key::char('日').text(), Some('日'));
        assert_eq!(ctrl('a').text(), None);
        assert_eq!(code(KeyCode::Enter).text(), None);
    }
}
