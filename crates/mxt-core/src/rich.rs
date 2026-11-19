//! Rich message text: the blocks and inline spans of a message body. The
//! matrix adapter builds it from HTML bodies, and the terminal adapter maps it
//! to styled lines.

use std::fmt::Write as _;

use crate::ids::UserId;

/// Inline styles; any combination can apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub spoiler: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionTarget {
    User(UserId),
    /// A room ID or alias, as written in the link.
    Room(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Link {
        text: String,
        url: String,
        style: Style,
    },
    /// `spoiler` when any of the mention's text is in a spoiler.
    Mention {
        text: String,
        target: MentionTarget,
        spoiler: bool,
    },
    /// A custom emoticon, shown by its alt text.
    Emoticon {
        alt: String,
        spoiler: bool,
    },
    LineBreak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    /// `text` has no trailing newline: a renderer trims the one that HTML
    /// `<pre><code>` content usually ends with.
    Code {
        language: Option<String>,
        text: String,
    },
    Quote(Vec<Block>),
    /// `start` is the number of the first item of an ordered list.
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RichText {
    pub blocks: Vec<Block>,
}

impl RichText {
    /// One paragraph per line of `text`, line breaks kept.
    pub fn plain(text: &str) -> RichText {
        let blocks = text
            .split('\n')
            .map(|line| {
                Block::Paragraph(if line.is_empty() {
                    Vec::new()
                } else {
                    vec![Inline::Text {
                        text: line.to_owned(),
                        style: Style::default(),
                    }]
                })
            })
            .collect();
        RichText { blocks }
    }

    /// The text as a reader sees it: inline text, link text, mention text,
    /// emoticon alt text; blocks separated by newlines; list items prefixed
    /// with `- ` or `<n>. `; quote lines prefixed with `> `; code blocks as
    /// their text. The later lines of a list item are indented under its
    /// first line.
    pub fn to_plain(&self) -> String {
        let mut plain = Plain::default();
        plain.blocks(&self.blocks);
        plain.out
    }

    /// Mention spans, in order (used to rebuild the mentions of an edit).
    pub fn mentions(&self) -> Vec<(&str, &MentionTarget)> {
        let mut mentions = Vec::new();
        for_each_paragraph(&self.blocks, &mut |inlines| {
            mentions.extend(inlines.iter().filter_map(|inline| match inline {
                Inline::Mention { text, target, .. } => Some((text.as_str(), target)),
                _ => None,
            }));
        });
        mentions
    }

    /// The URLs of link spans and the bare `http://` / `https://` URLs in
    /// text that is not inline code, in order, each once (`message.open`).
    /// Messages without HTML have no link spans, so their URLs are only
    /// bare ones.
    pub fn links(&self) -> Vec<&str> {
        let mut links: Vec<&str> = Vec::new();
        let mut push = |url| {
            if !links.contains(&url) {
                links.push(url);
            }
        };
        for_each_paragraph(&self.blocks, &mut |inlines| {
            for inline in inlines {
                match inline {
                    Inline::Link { url, .. } => push(url.as_str()),
                    Inline::Text { text, style } if !style.code => {
                        text.split_whitespace()
                            .filter_map(bare_url)
                            .for_each(&mut push);
                    }
                    _ => {}
                }
            }
        });
        links
    }
}

/// Whether `url` is an `http://` or `https://` URL (the scheme in any case),
/// the only ones `message.open` opens: a link's target is chosen by its
/// sender, and other schemes reach other programs.
pub fn is_web_url(url: &str) -> bool {
    let scheme = |prefix: &str| {
        url.get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    };
    scheme("http://") || scheme("https://")
}

/// The `http://` or `https://` URL in `word`, without the brackets and quotes
/// around it and the punctuation of the sentence after it, ASCII or
/// full-width, as in `（https://example.org/x）。`.
fn bare_url(word: &str) -> Option<&str> {
    let url = word
        .trim_start_matches([
            '(', '[', '{', '<', '"', '\'', '（', '「', '『', '【', '〈', '《',
        ])
        .trim_end_matches([
            '.', ',', ';', ':', '!', '?', '\'', '"', ')', ']', '}', '>', '。', '、', '，', '．',
            '！', '？', '）', '」', '』', '】', '〉', '》',
        ]);
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    (!rest.is_empty()).then_some(url)
}

/// Calls `f` with the inlines of every paragraph, in order, also inside
/// quotes and lists.
fn for_each_paragraph<'a>(blocks: &'a [Block], f: &mut impl FnMut(&'a [Inline])) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => f(inlines),
            Block::Code { .. } => {}
            Block::Quote(blocks) => for_each_paragraph(blocks, f),
            Block::List { items, .. } => {
                for item in items {
                    for_each_paragraph(item, f);
                }
            }
        }
    }
}

/// Builds [`RichText::to_plain`]: every line starts with the prefixes of the
/// quotes and list items it is in.
#[derive(Default)]
struct Plain {
    out: String,
    /// Outermost first.
    prefixes: Vec<Prefix>,
}

enum Prefix {
    Quote,
    /// A list item: its marker on its first line, as many spaces on the others.
    Item {
        /// `None` in unordered lists.
        number: Option<u64>,
        started: bool,
    },
}

impl Plain {
    fn blocks(&mut self, blocks: &[Block]) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                self.out.push('\n');
            }
            self.block(block);
        }
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Paragraph(inlines) => {
                self.start_line();
                for inline in inlines {
                    match inline {
                        Inline::Text { text, .. }
                        | Inline::Link { text, .. }
                        | Inline::Mention { text, .. }
                        | Inline::Emoticon { alt: text, .. } => self.text(text),
                        Inline::LineBreak => self.text("\n"),
                    }
                }
            }
            Block::Code { text, .. } => {
                self.start_line();
                self.text(text);
            }
            Block::Quote(blocks) => self.nested(Prefix::Quote, blocks),
            Block::List {
                ordered,
                start,
                items,
            } => {
                for (index, item) in (0..).zip(items) {
                    if index > 0 {
                        self.out.push('\n');
                    }
                    let number = ordered.then(|| start.saturating_add(index));
                    let prefix = Prefix::Item {
                        number,
                        started: false,
                    };
                    self.nested(prefix, item);
                }
            }
        }
    }

    fn nested(&mut self, prefix: Prefix, blocks: &[Block]) {
        self.prefixes.push(prefix);
        if blocks.is_empty() {
            self.start_line();
        }
        self.blocks(blocks);
        self.prefixes.pop();
    }

    /// Text inside a line; its line breaks start new lines.
    fn text(&mut self, text: &str) {
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            self.out.push_str(first);
        }
        for line in lines {
            self.out.push('\n');
            self.start_line();
            self.out.push_str(line);
        }
    }

    fn start_line(&mut self) {
        let Plain { out, prefixes } = self;
        for prefix in prefixes {
            match prefix {
                Prefix::Quote => out.push_str("> "),
                Prefix::Item { number, started } => {
                    if *started {
                        // The width of `- ` or `<n>. `.
                        let width =
                            number.map_or(2, |n| n.checked_ilog10().unwrap_or(0) as usize + 3);
                        out.extend(std::iter::repeat_n(' ', width));
                    } else {
                        *started = true;
                        match number {
                            // Writing to a `String` cannot fail.
                            Some(n) => _ = write!(out, "{n}. "),
                            None => out.push_str("- "),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Inline {
        Inline::Text {
            text: text.into(),
            style: Style::default(),
        }
    }
    fn paragraph(s: &str) -> Block {
        Block::Paragraph(vec![text(s)])
    }
    fn rich(blocks: Vec<Block>) -> RichText {
        RichText { blocks }
    }
    fn user(id: &str) -> MentionTarget {
        MentionTarget::User(UserId::new(id))
    }
    fn mention(text: &str, target: MentionTarget) -> Inline {
        Inline::Mention {
            text: text.into(),
            target,
            spoiler: false,
        }
    }

    #[test]
    fn plain_and_to_plain_round_trip_multi_line_text() {
        let text = "first\n\nthird line\n";
        let rich = RichText::plain(text);
        assert_eq!(rich.blocks.len(), 4);
        assert_eq!(rich.to_plain(), text);
    }

    #[test]
    fn to_plain_of_a_paragraph_with_bold_text_a_link_and_a_mention() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let rich = rich(vec![Block::Paragraph(vec![
            text("Hi "),
            Inline::Text {
                text: "there".into(),
                style: bold,
            },
            text(", see "),
            Inline::Link {
                text: "the docs".into(),
                url: "https://example.org/docs".into(),
                style: Style::default(),
            },
            text(" and ask "),
            mention("@Alice", user("@alice:example.org")),
            text(" "),
            Inline::Emoticon {
                alt: ":wave:".into(),
                spoiler: false,
            },
            Inline::LineBreak,
            text("bye"),
        ])]);
        assert_eq!(
            rich.to_plain(),
            "Hi there, see the docs and ask @Alice :wave:\nbye"
        );
    }

    #[test]
    fn to_plain_of_a_quote_containing_a_list() {
        let rich = rich(vec![
            Block::Quote(vec![Block::List {
                ordered: false,
                start: 1,
                items: vec![vec![paragraph("a")], vec![paragraph("b")]],
            }]),
            paragraph("after"),
        ]);
        assert_eq!(rich.to_plain(), "> - a\n> - b\nafter");
    }

    #[test]
    fn to_plain_of_an_ordered_list_starting_at_3() {
        let rich = rich(vec![
            paragraph("Steps:"),
            Block::List {
                ordered: true,
                start: 3,
                // A second paragraph in an item is indented under its text.
                items: vec![vec![paragraph("x")], vec![paragraph("y"), paragraph("z")]],
            },
        ]);
        assert_eq!(rich.to_plain(), "Steps:\n3. x\n4. y\n   z");
    }

    #[test]
    fn to_plain_of_a_code_block() {
        let rich = rich(vec![
            paragraph("Run:"),
            Block::Code {
                language: Some("sh".into()),
                text: "cargo test\ncargo fmt".into(),
            },
        ]);
        assert_eq!(rich.to_plain(), "Run:\ncargo test\ncargo fmt");
    }

    #[test]
    fn mentions_returns_user_and_room_mentions_in_order() {
        let room = MentionTarget::Room("#room:example.org".into());
        let rich = rich(vec![
            Block::Paragraph(vec![
                mention("@Alice", user("@alice:example.org")),
                text(" see "),
            ]),
            Block::Quote(vec![Block::Paragraph(vec![mention(
                "#room:example.org",
                room.clone(),
            )])]),
            Block::List {
                ordered: false,
                start: 1,
                items: vec![vec![Block::Paragraph(vec![mention(
                    "Bob",
                    user("@bob:example.org"),
                )])]],
            },
        ]);
        assert_eq!(
            rich.mentions(),
            vec![
                ("@Alice", &user("@alice:example.org")),
                ("#room:example.org", &room),
                ("Bob", &user("@bob:example.org")),
            ]
        );
    }

    #[test]
    fn links_leave_out_full_width_punctuation_around_bare_urls() {
        let rich = RichText::plain(
            "見て （https://example.org/a）。 https://example.org/b。\n\
             「https://example.org/c」、 『https://example.org/d』！ \
             【https://example.org/e】？ 〈https://example.org/f〉， 《https://example.org/g》．",
        );
        let urls: Vec<String> = "abcdefg"
            .chars()
            .map(|c| format!("https://example.org/{c}"))
            .collect();
        assert_eq!(rich.links(), urls);
    }

    #[test]
    fn only_http_and_https_urls_are_web_urls() {
        for url in [
            "https://example.org/x",
            "http://example.org",
            "HTTPS://example.org",
        ] {
            assert!(is_web_url(url), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "mailto:alice@example.org",
            "javascript:alert(1)",
            "--help",
            "https:",
            "",
        ] {
            assert!(!is_web_url(url), "{url}");
        }
    }
}
