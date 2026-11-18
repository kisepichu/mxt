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
    Mention {
        text: String,
        target: MentionTarget,
    },
    /// A custom emoticon, shown by its alt text.
    Emoticon {
        alt: String,
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

    /// Mention spans, in order (used in phase 3 to rebuild mentions for edits).
    pub fn mentions(&self) -> Vec<(&str, &MentionTarget)> {
        let mut mentions = Vec::new();
        collect_mentions(&self.blocks, &mut mentions);
        mentions
    }
}

fn collect_mentions<'a>(blocks: &'a [Block], mentions: &mut Vec<(&'a str, &'a MentionTarget)>) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                mentions.extend(inlines.iter().filter_map(|inline| match inline {
                    Inline::Mention { text, target } => Some((text.as_str(), target)),
                    _ => None,
                }));
            }
            Block::Code { .. } => {}
            Block::Quote(blocks) => collect_mentions(blocks, mentions),
            Block::List { items, .. } => {
                for item in items {
                    collect_mentions(item, mentions);
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
                        | Inline::Emoticon { alt: text } => self.text(text),
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
}
