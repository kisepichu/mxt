//! HTML message bodies (`formatted_body` in `org.matrix.custom.html`) to core
//! rich text.

use std::rc::Rc;

use matrix_sdk::ruma::{
    html::{
        ElementData, Html, NodeData, NodeRef,
        matrix::{AnchorUri, MatrixElement, MatrixElementData},
    },
    matrix_uri::MatrixId,
};
use mxt_core::{
    ids::UserId,
    rich::{Block, Inline, MentionTarget, RichText, Style},
};

/// Elements nested deeper than this are dropped, as the SDK's sanitizer drops
/// them (the Matrix specification's limit). This also bounds the recursion.
const MAX_DEPTH: usize = 100;

/// Renders a `formatted_body` (`org.matrix.custom.html`) into `RichText`.
/// Malformed HTML is read the way a browser reads it, so its text is kept.
pub fn render_html(html: &str) -> RichText {
    let html = Html::parse(html);
    let mut blocks = Blocks::default();
    for node in html.children() {
        blocks.node(&node, &Inherited::default(), 0);
    }
    RichText {
        blocks: blocks.finish(),
    }
}

/// What an element passes on to its content.
#[derive(Clone, Default)]
struct Inherited {
    style: Style,
    /// The `<a>` the content is in.
    link: Option<Rc<Link>>,
}

impl Inherited {
    fn styled(&self, change: impl FnOnce(&mut Style)) -> Inherited {
        let mut inherited = self.clone();
        change(&mut inherited.style);
        inherited
    }
}

/// The target of an `<a>`.
enum Link {
    Url(String),
    Mention(MentionTarget),
}

/// Blocks in the making: the finished ones and the paragraph being filled.
#[derive(Default)]
struct Blocks {
    blocks: Vec<Block>,
    line: Line,
}

impl Blocks {
    /// The blocks of `node`'s content.
    fn of(node: &NodeRef, inherited: &Inherited, depth: usize) -> Vec<Block> {
        let mut blocks = Blocks::default();
        blocks.children(node, inherited, depth);
        blocks.finish()
    }

    fn finish(mut self) -> Vec<Block> {
        self.end_paragraph();
        self.blocks
    }

    fn end_paragraph(&mut self) {
        let inlines = std::mem::take(&mut self.line).finish();
        if !inlines.is_empty() {
            self.blocks.push(Block::Paragraph(inlines));
        }
    }

    fn push_block(&mut self, block: Block) {
        self.end_paragraph();
        self.blocks.push(block);
    }

    /// `depth` is the number of elements around `node`.
    fn node(&mut self, node: &NodeRef, inherited: &Inherited, depth: usize) {
        match node.data() {
            NodeData::Text(text) => self.line.text(&text.borrow(), inherited),
            NodeData::Element(element) if depth < MAX_DEPTH => {
                self.element(node, element, inherited, depth);
            }
            _ => {}
        }
    }

    fn children(&mut self, node: &NodeRef, inherited: &Inherited, depth: usize) {
        for child in node.children() {
            self.node(&child, inherited, depth + 1);
        }
    }

    /// `node`'s content as paragraphs of their own.
    fn block(&mut self, node: &NodeRef, inherited: &Inherited, depth: usize) {
        self.end_paragraph();
        self.children(node, inherited, depth);
        self.end_paragraph();
    }

    fn element(
        &mut self,
        node: &NodeRef,
        element: &ElementData,
        inherited: &Inherited,
        depth: usize,
    ) {
        let MatrixElementData {
            element: kind,
            attrs,
        } = element.to_matrix();
        match kind {
            // The reply fallback: the timeline shows the reply preview instead.
            MatrixElement::MatrixReply => {}
            MatrixElement::P | MatrixElement::Div(_) => self.block(node, inherited, depth),
            MatrixElement::H(_) => self.block(node, &inherited.styled(|s| s.bold = true), depth),
            MatrixElement::Blockquote => {
                self.push_block(Block::Quote(Blocks::of(node, inherited, depth)));
            }
            MatrixElement::Ul => self.list(node, false, 1, inherited, depth),
            MatrixElement::Ol(list) => {
                // A start that is not a natural number counts from 1.
                let start = list.start.and_then(|start| u64::try_from(start).ok());
                self.list(node, true, start.unwrap_or(1), inherited, depth);
            }
            MatrixElement::Pre => self.push_block(code_block(node, depth)),
            MatrixElement::Br => self.line.line_break(),
            // Rich text has no tables, rules or disclosures; their parts keep
            // their text on lines of their own, table cells separated by spaces.
            MatrixElement::Hr
            | MatrixElement::Table
            | MatrixElement::Thead
            | MatrixElement::Tbody
            | MatrixElement::Tr
            | MatrixElement::Caption
            | MatrixElement::Li
            | MatrixElement::Details
            | MatrixElement::Summary => self.block(node, inherited, depth),
            MatrixElement::Td | MatrixElement::Th => {
                self.line.space(inherited);
                self.children(node, inherited, depth);
                self.line.space(inherited);
            }
            MatrixElement::Strong | MatrixElement::B => {
                self.children(node, &inherited.styled(|s| s.bold = true), depth);
            }
            MatrixElement::Em | MatrixElement::I => {
                self.children(node, &inherited.styled(|s| s.italic = true), depth);
            }
            MatrixElement::Del | MatrixElement::S => {
                self.children(node, &inherited.styled(|s| s.strike = true), depth);
            }
            // Deprecated, so ruma does not type it.
            MatrixElement::Other(name) if &*name.local == "strike" => {
                self.children(node, &inherited.styled(|s| s.strike = true), depth);
            }
            MatrixElement::Code(_) => {
                self.children(node, &inherited.styled(|s| s.code = true), depth);
            }
            MatrixElement::Span(span) if span.spoiler.is_some() => {
                self.children(node, &inherited.styled(|s| s.spoiler = true), depth);
            }
            MatrixElement::A(anchor) => {
                let href = anchor.href.as_ref().zip(attribute(element, "href"));
                let link = href.map(|(href, raw)| {
                    Rc::new(match mention(href) {
                        Some(target) => Link::Mention(target),
                        None => Link::Url(raw),
                    })
                });
                let inherited = Inherited {
                    link,
                    ..inherited.clone()
                };
                self.children(node, &inherited, depth);
            }
            MatrixElement::Img(image) => {
                let alt = image.alt.as_deref().unwrap_or_default();
                if attrs
                    .iter()
                    .any(|attr| &*attr.name.local == "data-mx-emoticon")
                {
                    self.line.emoticon(alt.to_owned());
                } else {
                    self.line.text(alt, inherited);
                }
            }
            // Unknown elements and the ones without a rich text form (`<u>`,
            // `<sup>`, colored spans) keep their text.
            _ => self.children(node, inherited, depth),
        }
    }

    /// Content between `<li>` items belongs to the item before it.
    fn list(
        &mut self,
        node: &NodeRef,
        ordered: bool,
        start: u64,
        inherited: &Inherited,
        depth: usize,
    ) {
        let mut items: Vec<Blocks> = Vec::new();
        for child in node.children() {
            let is_item = child
                .as_element()
                .is_some_and(|element| &*element.name.local == "li");
            if is_item {
                let mut item = Blocks::default();
                item.children(&child, inherited, depth + 1);
                items.push(item);
            } else if let Some(item) = items.last_mut() {
                item.node(&child, inherited, depth + 1);
            } else if !is_blank(&child) {
                let mut item = Blocks::default();
                item.node(&child, inherited, depth + 1);
                items.push(item);
            }
        }
        let items = items.into_iter().map(Blocks::finish).collect();
        self.push_block(Block::List {
            ordered,
            start,
            items,
        });
    }
}

/// The inline content of the paragraph being filled.
#[derive(Default)]
struct Line {
    inlines: Vec<Inline>,
    /// The `<a>` of the last inline, so the text of one `<a>` stays one inline.
    last_link: Option<Rc<Link>>,
    /// Whitespace waiting for the next word, with what it inherited: a browser
    /// collapses a run of whitespace into one space, and drops it at the start
    /// and the end of a line.
    space: Option<Inherited>,
}

impl Line {
    fn finish(mut self) -> Vec<Inline> {
        // A `<br>` at the end of a paragraph adds no empty line in a browser.
        if matches!(self.inlines.last(), Some(Inline::LineBreak)) {
            self.inlines.pop();
        }
        self.inlines
    }

    fn text(&mut self, text: &str, inherited: &Inherited) {
        for (index, word) in text.split(is_space).enumerate() {
            if index > 0 {
                self.space(inherited);
            }
            if !word.is_empty() {
                self.word(word, inherited);
            }
        }
    }

    fn space(&mut self, inherited: &Inherited) {
        let line_start = matches!(self.inlines.last(), None | Some(Inline::LineBreak));
        if self.space.is_none() && !line_start {
            self.space = Some(inherited.clone());
        }
    }

    fn word(&mut self, word: &str, inherited: &Inherited) {
        self.flush_space(inherited.link.as_ref());
        self.push(word, inherited.style, inherited.link.clone());
    }

    fn emoticon(&mut self, alt: String) {
        self.flush_space(None);
        self.inlines.push(Inline::Emoticon { alt });
        self.last_link = None;
    }

    fn line_break(&mut self) {
        self.space = None;
        self.inlines.push(Inline::LineBreak);
        self.last_link = None;
    }

    /// Writes the waiting space before content in `next_link`. A space in an
    /// `<a>` belongs to it only between two of its words, so a mention's text
    /// never ends with a space.
    fn flush_space(&mut self, next_link: Option<&Rc<Link>>) {
        if let Some(space) = self.space.take() {
            let link = space
                .link
                .filter(|link| next_link.is_some_and(|next| Rc::ptr_eq(link, next)));
            self.push(" ", space.style, link);
        }
    }

    /// Appends to the last inline when it has the same style and `<a>`.
    fn push(&mut self, text: &str, style: Style, link: Option<Rc<Link>>) {
        let same_link = match (&self.last_link, &link) {
            (None, None) => true,
            (Some(last), Some(link)) => Rc::ptr_eq(last, link),
            _ => false,
        };
        if same_link {
            match self.inlines.last_mut() {
                Some(
                    Inline::Text {
                        text: last,
                        style: last_style,
                    }
                    | Inline::Link {
                        text: last,
                        style: last_style,
                        ..
                    },
                ) if *last_style == style => {
                    last.push_str(text);
                    return;
                }
                // Mentions have no style; all of the `<a>`'s text is the mention.
                Some(Inline::Mention { text: last, .. }) => {
                    last.push_str(text);
                    return;
                }
                _ => {}
            }
        }
        let text = text.to_owned();
        self.inlines.push(match link.as_deref() {
            None => Inline::Text { text, style },
            Some(Link::Url(url)) => Inline::Link {
                text,
                url: url.clone(),
                style,
            },
            Some(Link::Mention(target)) => Inline::Mention {
                text,
                target: target.clone(),
            },
        });
        self.last_link = link;
    }
}

/// A `<pre>` as a code block: its text as written, with the language of the
/// `<code>` inside it.
fn code_block(pre: &NodeRef, depth: usize) -> Block {
    let language = pre
        .children()
        .find_map(|child| match child.as_element()?.to_matrix().element {
            MatrixElement::Code(code) => code.language.map(|language| language.to_string()),
            _ => None,
        });
    let mut text = String::new();
    raw_text(pre, depth, &mut text);
    // The newline before `</code></pre>` ends the last line; it is not an
    // empty line.
    if text.ends_with('\n') {
        text.pop();
    }
    Block::Code { language, text }
}

/// The text in `node` as written, with `<br>` as a newline.
fn raw_text(node: &NodeRef, depth: usize, out: &mut String) {
    for child in node.children() {
        match child.data() {
            NodeData::Text(text) => out.push_str(&text.borrow()),
            NodeData::Element(element) if depth + 1 < MAX_DEPTH => {
                if &*element.name.local == "br" {
                    out.push('\n');
                } else {
                    raw_text(&child, depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

/// The user or room a `matrix.to` or `matrix:` link points to; event links
/// are plain links.
fn mention(href: &AnchorUri) -> Option<MentionTarget> {
    let id = match href {
        AnchorUri::Matrix(uri) => uri.id(),
        AnchorUri::MatrixTo(uri) => uri.id(),
        _ => return None,
    };
    match id {
        MatrixId::User(user) => Some(MentionTarget::User(UserId::new(user.as_str()))),
        MatrixId::Room(room) => Some(MentionTarget::Room(room.to_string())),
        MatrixId::RoomAlias(alias) => Some(MentionTarget::Room(alias.to_string())),
        _ => None,
    }
}

fn attribute(element: &ElementData, name: &str) -> Option<String> {
    element
        .attrs
        .borrow()
        .iter()
        .find(|attr| &*attr.name.local == name)
        .map(|attr| attr.value.to_string())
}

/// Whitespace text, or a comment.
fn is_blank(node: &NodeRef) -> bool {
    match node.data() {
        NodeData::Text(text) => text.borrow().chars().all(is_space),
        NodeData::Element(_) => false,
        _ => true,
    }
}

/// HTML's whitespace; other spaces, such as U+00A0, are kept.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str, change: impl FnOnce(&mut Style)) -> Inline {
        let mut style = Style::default();
        change(&mut style);
        Inline::Text {
            text: text.into(),
            style,
        }
    }
    fn text(text: &str) -> Inline {
        styled(text, |_| {})
    }
    fn bold(text: &str) -> Inline {
        styled(text, |s| s.bold = true)
    }
    fn italic(text: &str) -> Inline {
        styled(text, |s| s.italic = true)
    }
    fn paragraph(inlines: Vec<Inline>) -> Block {
        Block::Paragraph(inlines)
    }
    fn plain(s: &str) -> Block {
        paragraph(vec![text(s)])
    }
    fn mention(text: &str, target: MentionTarget) -> Inline {
        Inline::Mention {
            text: text.into(),
            target,
        }
    }
    fn user(id: &str) -> MentionTarget {
        MentionTarget::User(UserId::new(id))
    }
    fn link(text: &str, url: &str, style: Style) -> Inline {
        Inline::Link {
            text: text.into(),
            url: url.into(),
            style,
        }
    }
    fn blocks(html: &str) -> Vec<Block> {
        render_html(html).blocks
    }

    #[test]
    fn p_and_div_are_paragraphs() {
        assert_eq!(
            blocks("<p>one</p><div>two</div>three"),
            vec![plain("one"), plain("two"), plain("three")]
        );
    }

    #[test]
    fn br_is_a_line_break() {
        assert_eq!(
            blocks("one<br>two<br/>three<br>"),
            vec![paragraph(vec![
                text("one"),
                Inline::LineBreak,
                text("two"),
                Inline::LineBreak,
                text("three"),
            ])]
        );
    }

    #[test]
    fn strong_and_b_are_bold_em_and_i_italic() {
        let both = styled("both", |s| {
            s.bold = true;
            s.italic = true;
        });
        assert_eq!(
            blocks("<strong>a</strong> <b>b <i>both</i></b> <em>c</em> <i>d</i>"),
            vec![paragraph(vec![
                bold("a"),
                text(" "),
                bold("b "),
                both,
                text(" "),
                italic("c"),
                text(" "),
                italic("d"),
            ])]
        );
    }

    #[test]
    fn del_s_and_strike_are_strikethrough() {
        assert_eq!(
            blocks("<del>a</del><s>b</s><strike>c</strike>"),
            vec![paragraph(vec![styled("abc", |s| s.strike = true)])]
        );
    }

    #[test]
    fn code_is_inline_code() {
        assert_eq!(
            blocks("run <code>cargo test</code>"),
            vec![paragraph(vec![
                text("run "),
                styled("cargo test", |s| s.code = true),
            ])]
        );
    }

    #[test]
    fn spans_with_data_mx_spoiler_are_spoilers() {
        let spoiler = |t| styled(t, |s| s.spoiler = true);
        assert_eq!(
            blocks(
                "<span data-mx-spoiler>secret</span> and \
                 <span data-mx-spoiler=\"plot\">twist</span><span data-mx-color=\"#ff0000\">red</span>"
            ),
            vec![paragraph(vec![
                spoiler("secret"),
                text(" and "),
                spoiler("twist"),
                text("red"),
            ])]
        );
    }

    #[test]
    fn pre_code_is_a_code_block_with_its_language() {
        assert_eq!(
            blocks(
                "<pre><code class=\"language-rust\">fn main() {\n    println!(\"hi\");\n}\n</code></pre>\
                 <pre><code>a  b\n\nc\n</code></pre>"
            ),
            vec![
                Block::Code {
                    language: Some("rust".into()),
                    text: "fn main() {\n    println!(\"hi\");\n}".into(),
                },
                Block::Code {
                    language: None,
                    text: "a  b\n\nc".into(),
                },
            ]
        );
    }

    #[test]
    fn blockquote_is_a_quote() {
        assert_eq!(
            blocks("<blockquote><p>quoted <b>text</b></p></blockquote><p>reply</p>"),
            vec![
                Block::Quote(vec![paragraph(vec![text("quoted "), bold("text")])]),
                plain("reply"),
            ]
        );
    }

    #[test]
    fn ul_and_ol_are_lists_and_ol_starts_at_its_start() {
        assert_eq!(
            blocks(
                "<ul>\n<li>a</li>\n<li>b<ul><li>c</li></ul></li>\n</ul>\
                 <ol start=\"3\">\n<li>x</li>\n<li><p>y</p></li>\n</ol><ol><li>z</li></ol>"
            ),
            vec![
                Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![
                        vec![plain("a")],
                        vec![
                            plain("b"),
                            Block::List {
                                ordered: false,
                                start: 1,
                                items: vec![vec![plain("c")]],
                            },
                        ],
                    ],
                },
                Block::List {
                    ordered: true,
                    start: 3,
                    items: vec![vec![plain("x")], vec![plain("y")]],
                },
                Block::List {
                    ordered: true,
                    start: 1,
                    items: vec![vec![plain("z")]],
                },
            ]
        );
    }

    #[test]
    fn matrix_to_links_to_users_and_rooms_are_mentions() {
        assert_eq!(
            blocks(
                "<a href=\"https://matrix.to/#/@alice:example.org\">Alice  Smith</a> in \
                 <a href=\"https://matrix.to/#/#room:example.org\">#room:example.org</a> and \
                 <a href=\"https://matrix.to/#/%21abc%3Aexample.org\">a <b>room</b></a>"
            ),
            vec![paragraph(vec![
                mention("Alice Smith", user("@alice:example.org")),
                text(" in "),
                mention(
                    "#room:example.org",
                    MentionTarget::Room("#room:example.org".into())
                ),
                text(" and "),
                mention("a room", MentionTarget::Room("!abc:example.org".into())),
            ])]
        );
    }

    #[test]
    fn other_links_are_links() {
        let event = "https://matrix.to/#/!abc:example.org/$event?via=example.org";
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        assert_eq!(
            blocks(&format!(
                "see <a href=\"https://example.org/docs\">the <b>docs</b> </a>or \
                 <a href=\"{event}\">this message</a>"
            )),
            vec![paragraph(vec![
                text("see "),
                link("the ", "https://example.org/docs", Style::default()),
                link("docs", "https://example.org/docs", bold),
                text(" or "),
                link("this message", event, Style::default()),
            ])]
        );
    }

    #[test]
    fn emoticon_images_are_their_alt_text() {
        assert_eq!(
            blocks(
                "hi <img data-mx-emoticon src=\"mxc://example.org/abc\" alt=\":wave:\" height=\"32\"> \
                 <img src=\"mxc://example.org/def\" alt=\"a cat\">"
            ),
            vec![paragraph(vec![
                text("hi "),
                Inline::Emoticon {
                    alt: ":wave:".into()
                },
                text(" a cat"),
            ])]
        );
    }

    #[test]
    fn headings_are_bold_paragraphs() {
        let bold_italic = styled("x", |s| {
            s.bold = true;
            s.italic = true;
        });
        assert_eq!(
            blocks("<h1>Title</h1><h3>Sub <i>x</i></h3>text"),
            vec![
                paragraph(vec![bold("Title")]),
                paragraph(vec![bold("Sub "), bold_italic]),
                plain("text"),
            ]
        );
    }

    #[test]
    fn unknown_tags_keep_their_text() {
        assert_eq!(
            blocks(
                "<u>under</u> x<sup>2</sup> <custom-tag>kept</custom-tag>\
                 <table><tr><td>a</td><td>b</td></tr></table>"
            ),
            vec![plain("under x2 kept"), plain("a b")]
        );
    }

    #[test]
    fn whitespace_collapses_outside_pre() {
        assert_eq!(
            blocks("<p>  a \n\t b  </p>\n\n<p>c <b> d</b>\u{a0}e</p>"),
            vec![
                plain("a b"),
                paragraph(vec![text("c "), bold("d"), text("\u{a0}e")]),
            ]
        );
    }

    #[test]
    fn discord_bridge_body() {
        assert_eq!(
            blocks("<p>hello <a href=\"https://matrix.to/#/@discord_1:example.org\">Alice</a></p>"),
            vec![paragraph(vec![
                text("hello "),
                mention("Alice", user("@discord_1:example.org")),
            ])]
        );
    }

    #[test]
    fn reply_fallback_is_dropped() {
        assert_eq!(
            blocks(
                "<mx-reply><blockquote>\
                 <a href=\"https://matrix.to/#/!room:example.org/$event\">In reply to</a> \
                 <a href=\"https://matrix.to/#/@bob:example.org\">@bob:example.org</a>\
                 <br>original</blockquote></mx-reply>answer"
            ),
            vec![plain("answer")]
        );
    }

    #[test]
    fn malformed_html_still_yields_its_text() {
        let rich =
            render_html("<p>unclosed <b>bold<div>block</p> tail &amp; <i>more</b><ul><li>item");
        let plain = rich.to_plain();
        assert_eq!(
            plain.split_whitespace().collect::<Vec<_>>(),
            [
                "unclosed", "bold", "block", "tail", "&", "more", "-", "item"
            ]
        );
    }
}
