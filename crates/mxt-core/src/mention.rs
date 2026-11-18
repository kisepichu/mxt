//! Mentions: rendering the composer text into the bodies of a message, and
//! ranking the candidates of mention completion (design, "Matrix adapter"
//! (Sending) and "Mention completion").

use std::cmp::Reverse;
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::fuzzy;
use crate::ids::UserId;
use crate::rich::{MentionTarget, RichText};

/// A mention inserted by mention completion: its text in the composer and
/// the user it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionRef {
    pub user: UserId,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedBody {
    /// The editor text unchanged.
    pub plain: String,
    /// Markdown for the HTML body: remaining mention texts linked.
    pub markdown: String,
    /// The users whose mention text is still present, in first-occurrence
    /// order, without duplicates.
    pub user_ids: Vec<UserId>,
}

/// Design, "Matrix adapter" (Sending). Every occurrence of a mention's text
/// outside inline code and code blocks becomes a Markdown link to
/// `https://matrix.to/#/<user ID>` with the text escaped. An occurrence
/// starts at the start of the text or after whitespace, and ends at the end
/// of the text or before a character that is not a letter or digit, so
/// `@Bob` does not match inside `@Bobby`. Longer texts are matched first, so
/// `@Alice` inside `@Alice Smith` does not match when both are mentions.
pub fn render_mentions(text: &str, mentions: &[MentionRef]) -> RenderedBody {
    let code = code_ranges(text);
    let mut longest_first: Vec<&MentionRef> = mentions
        .iter()
        .filter(|mention| !mention.text.is_empty())
        .collect();
    longest_first.sort_by_key(|mention| Reverse(mention.text.len()));
    let mut links: Vec<(Range<usize>, &UserId)> = Vec::new();
    for mention in longest_first {
        for (start, found) in text.match_indices(mention.text.as_str()) {
            let range = start..start + found.len();
            let taken = |other: &Range<usize>| overlap(other, &range);
            if stands_alone(text, &range)
                && !code.iter().any(taken)
                && !links.iter().any(|(link, _)| taken(link))
            {
                links.push((range, &mention.user));
            }
        }
    }
    links.sort_by_key(|(range, _)| range.start);

    let mut markdown = String::with_capacity(text.len());
    let mut user_ids: Vec<UserId> = Vec::new();
    let mut copied = 0;
    for (range, user) in links {
        markdown.push_str(&text[copied..range.start]);
        markdown.push('[');
        escape_markdown(&text[range.clone()], &mut markdown);
        markdown.push_str("](https://matrix.to/#/");
        markdown.push_str(user.as_str());
        markdown.push(')');
        copied = range.end;
        if !user_ids.contains(user) {
            user_ids.push(user.clone());
        }
    }
    markdown.push_str(&text[copied..]);
    RenderedBody {
        plain: text.to_owned(),
        markdown,
        user_ids,
    }
}

/// The byte ranges of inline code (with its backticks) and code blocks.
fn code_ranges(text: &str) -> Vec<Range<usize>> {
    Parser::new_ext(text, Options::empty())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Code(_) | Event::Start(Tag::CodeBlock(_)) => Some(range),
            _ => None,
        })
        .collect()
}

fn overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Whether `range` of `text` starts at the start or after whitespace, and
/// ends at the end or before a character that is not a letter or digit.
fn stands_alone(text: &str, range: &Range<usize>) -> bool {
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    before.is_none_or(char::is_whitespace) && after.is_none_or(|c| !c.is_alphanumeric())
}

/// Appends `text` with a backslash before every character that Markdown
/// could read as syntax.
fn escape_markdown(text: &str, out: &mut String) {
    for c in text.chars() {
        if r"\`*_[]()#+-.!<>|~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
}

/// A member of a room, for mention completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub user: UserId,
    pub display_name: Option<String>,
}

impl Member {
    /// The display name unless it is missing or empty.
    fn name(&self) -> Option<&str> {
        self.display_name.as_deref().filter(|name| !name.is_empty())
    }
}

/// Ranks members against `query` (the text after `@`) with nucleo on the
/// display name and the user ID's localpart: the server name is the same for
/// most members, so it would match short queries for everyone. Members who
/// sent one of the loaded messages come first, newest sender first
/// (`recent_senders`, newest first); the rest follow by match score. An empty
/// query lists the recent senders first, then everyone else by display name
/// (the localpart without one), ignoring case.
pub fn rank_members<'a>(
    query: &str,
    members: &'a [Member],
    recent_senders: &[UserId],
) -> Vec<&'a Member> {
    let mut ranked: Vec<&Member> = fuzzy::rank_by(query, members, |member| {
        member.name().into_iter().chain([localpart(&member.user)])
    })
    .into_iter()
    .map(|index| &members[index])
    .collect();
    if query.trim().is_empty() {
        ranked.sort_by_cached_key(|member| {
            member
                .name()
                .unwrap_or_else(|| localpart(&member.user))
                .to_lowercase()
        });
    }
    // Stable, so the others keep their order.
    ranked.sort_by_cached_key(|member| {
        recent_senders
            .iter()
            .position(|sender| *sender == member.user)
            .unwrap_or(usize::MAX)
    });
    ranked
}

/// `alice` of `@alice:example.org`.
fn localpart(user: &UserId) -> &str {
    let id = user.as_str();
    let id = id.strip_prefix('@').unwrap_or(id);
    id.split_once(':').map_or(id, |(localpart, _)| localpart)
}

/// The text inserted for `member`: `@<display name>`, or the user ID
/// (`@alice:example.org`) when another member has the same display name or
/// the member has none, so the text names exactly one user.
pub fn mention_text(member: &Member, members: &[Member]) -> String {
    match member.name() {
        Some(name)
            if !members
                .iter()
                .any(|other| other.user != member.user && other.name() == Some(name)) =>
        {
            format!("@{name}")
        }
        _ => member.user.to_string(),
    }
}

/// Rebuilds the mentions of a message being edited: the user mention spans
/// of its `RichText` whose text appears in `body`, without duplicates.
pub fn mentions_for_edit(content: &RichText, body: &str) -> Vec<MentionRef> {
    let mut mentions: Vec<MentionRef> = Vec::new();
    for (text, target) in content.mentions() {
        if let MentionTarget::User(user) = target
            && !text.is_empty()
            && body.contains(text)
            && !mentions
                .iter()
                .any(|mention| mention.user == *user && mention.text == text)
        {
            mentions.push(MentionRef {
                user: user.clone(),
                text: text.to_owned(),
            });
        }
    }
    mentions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich::{Block, Inline, MentionTarget, Style};

    fn user(id: &str) -> UserId {
        UserId::new(id)
    }
    fn mention(text: &str, id: &str) -> MentionRef {
        MentionRef {
            user: user(id),
            text: text.into(),
        }
    }
    fn link(text: &str, id: &str) -> String {
        format!("[{text}](https://matrix.to/#/{id})")
    }
    fn member(id: &str, name: Option<&str>) -> Member {
        Member {
            user: user(id),
            display_name: name.map(Into::into),
        }
    }
    fn ids(members: &[&Member]) -> Vec<String> {
        members.iter().map(|m| m.user.to_string()).collect()
    }

    const ALICE: &str = "@alice:example.org";
    const BOB: &str = "@bob:example.org";

    #[test]
    fn the_plain_body_is_the_text_unchanged() {
        let text = "hi @Alice, see **this**\nbye";
        let rendered = render_mentions(text, &[mention("@Alice", ALICE)]);
        assert_eq!(rendered.plain, text);
        assert_eq!(
            rendered.markdown,
            format!("hi {}, see **this**\nbye", link("@Alice", ALICE))
        );
        assert_eq!(rendered.user_ids, vec![user(ALICE)]);
    }

    #[test]
    fn a_deleted_mention_text_is_dropped() {
        let mentions = [mention("@Alice", ALICE), mention("@Bob", BOB)];
        let rendered = render_mentions("thanks @Bob", &mentions);
        assert_eq!(rendered.markdown, format!("thanks {}", link("@Bob", BOB)));
        assert_eq!(rendered.user_ids, vec![user(BOB)]);
    }

    #[test]
    fn mentions_inside_inline_code_and_code_blocks_are_not_linked() {
        let mentions = [mention("@Alice", ALICE)];
        let text = "run `@Alice` now\n\n```\n@Alice\n```\n";
        let rendered = render_mentions(text, &mentions);
        assert_eq!(rendered.markdown, text);
        assert_eq!(rendered.user_ids, vec![]);
        let text = "`x` @Alice\n\n    @Alice indented";
        assert_eq!(
            render_mentions(text, &mentions).markdown,
            format!("`x` {}\n\n    @Alice indented", link("@Alice", ALICE))
        );
    }

    #[test]
    fn markdown_in_a_display_name_is_escaped() {
        let rendered = render_mentions("@A*b*_c hi", &[mention("@A*b*_c", ALICE)]);
        assert_eq!(
            rendered.markdown,
            format!("{} hi", link(r"@A\*b\*\_c", ALICE))
        );
        let all = r"\`*_[]()#+-.!<>|~";
        let rendered = render_mentions(&format!("@{all}"), &[mention(&format!("@{all}"), ALICE)]);
        assert_eq!(
            rendered.markdown,
            link(r"@\\\`\*\_\[\]\(\)\#\+\-\.\!\<\>\|\~", ALICE)
        );
    }

    #[test]
    fn two_mentions_of_the_same_user_give_one_user_id() {
        let mentions = [
            mention("@Bob", BOB),
            mention("@Alice", ALICE),
            mention("@Alice", ALICE),
        ];
        let rendered = render_mentions("@Alice and @Bob, @Alice", &mentions);
        assert_eq!(
            rendered.markdown,
            format!(
                "{} and {}, {}",
                link("@Alice", ALICE),
                link("@Bob", BOB),
                link("@Alice", ALICE)
            )
        );
        assert_eq!(rendered.user_ids, vec![user(ALICE), user(BOB)]);
    }

    #[test]
    fn a_shorter_mention_text_does_not_match_inside_a_longer_one() {
        let al = "@al:example.org";
        let mentions = [mention("@Al", al), mention("@Alice", ALICE)];
        let rendered = render_mentions("@Alice @Al", &mentions);
        assert_eq!(
            rendered.markdown,
            format!("{} {}", link("@Alice", ALICE), link("@Al", al))
        );
        assert_eq!(rendered.user_ids, vec![user(ALICE), user(al)]);
        // A longer text that continues after a space.
        let mentions = [mention("@Alice", al), mention("@Alice Smith", ALICE)];
        let rendered = render_mentions("@Alice Smith and @Alice", &mentions);
        assert_eq!(
            rendered.markdown,
            format!("{} and {}", link("@Alice Smith", ALICE), link("@Alice", al))
        );
    }

    #[test]
    fn a_mention_text_must_stand_alone() {
        let mentions = [mention("@Bob", BOB)];
        for text in ["@Bobby", "x@Bob", "@Bobさん"] {
            let rendered = render_mentions(text, &mentions);
            assert_eq!(rendered.markdown, text);
            assert_eq!(rendered.user_ids, vec![]);
        }
        let rendered = render_mentions("@Bob: hi (@Bob) @Bob.", &mentions);
        assert_eq!(
            rendered.markdown,
            format!("{}: hi (@Bob) {}.", link("@Bob", BOB), link("@Bob", BOB))
        );
    }

    #[test]
    fn a_text_without_mentions_is_its_own_markdown() {
        let text = "plain *text* with @nobody";
        let rendered = render_mentions(text, &[]);
        assert_eq!(
            rendered,
            RenderedBody {
                plain: text.into(),
                markdown: text.into(),
                user_ids: vec![],
            }
        );
    }

    fn room_members() -> Vec<Member> {
        vec![
            member("@carol:example.org", Some("Carol")),
            member(ALICE, Some("Alice")),
            member("@dave:example.org", None),
            member(BOB, Some("bob")),
            member("@alfred:example.org", Some("Alfred")),
        ]
    }

    #[test]
    fn recent_senders_come_first_newest_first() {
        let members = room_members();
        let recent = [user(BOB), user("@zoe:example.org"), user(ALICE)];
        assert_eq!(
            ids(&rank_members("", &members, &recent)),
            [
                BOB,
                ALICE,
                "@alfred:example.org",
                "@carol:example.org",
                "@dave:example.org"
            ]
        );
        // With a query, recent senders that match come first, then the
        // other matches by score (equal scores: the shorter name first).
        let recent = [user("@carol:example.org")];
        assert_eq!(
            ids(&rank_members("al", &members, &recent)),
            ["@carol:example.org", ALICE, "@alfred:example.org"]
        );
    }

    #[test]
    fn members_match_on_display_name_and_user_id() {
        let members = room_members();
        assert_eq!(
            ids(&rank_members("Car", &members, &[])),
            ["@carol:example.org"]
        );
        assert_eq!(
            ids(&rank_members("dave", &members, &[])),
            ["@dave:example.org"]
        );
        // `alice` is in both the display name and the user ID of Alice.
        assert_eq!(ids(&rank_members("alice", &members, &[])), [ALICE]);
        // Every member has the server name, so it does not count.
        assert!(rank_members("example", &members, &[]).is_empty());
        assert!(rank_members("zzz", &members, &[]).is_empty());
    }

    #[test]
    fn an_empty_query_lists_everyone_by_display_name() {
        let members = room_members();
        assert_eq!(
            ids(&rank_members("", &members, &[])),
            [
                "@alfred:example.org",
                ALICE,
                BOB,
                "@carol:example.org",
                "@dave:example.org"
            ]
        );
    }

    #[test]
    fn mention_text_is_the_display_name_when_it_names_one_member() {
        let mut members = room_members();
        assert_eq!(mention_text(&members[1], &members), "@Alice");
        assert_eq!(mention_text(&members[2], &members), "@dave:example.org");
        members.push(member("@alice2:example.org", Some("Alice")));
        assert_eq!(mention_text(&members[1], &members), ALICE);
        assert_eq!(mention_text(&members[5], &members), "@alice2:example.org");
        let empty = member("@eve:example.org", Some(""));
        assert_eq!(mention_text(&empty, &[]), "@eve:example.org");
    }

    #[test]
    fn mentions_for_edit_keeps_the_user_mentions_still_in_the_body() {
        let pill = |text: &str, target: MentionTarget| Inline::Mention {
            text: text.into(),
            target,
        };
        let content = RichText {
            blocks: vec![
                Block::Paragraph(vec![
                    pill("@Alice", MentionTarget::User(user(ALICE))),
                    Inline::Text {
                        text: " and ".into(),
                        style: Style::default(),
                    },
                    pill("@Bob", MentionTarget::User(user(BOB))),
                    pill(
                        "#room:example.org",
                        MentionTarget::Room("#room:example.org".into()),
                    ),
                ]),
                Block::Quote(vec![Block::Paragraph(vec![pill(
                    "@Alice",
                    MentionTarget::User(user(ALICE)),
                )])]),
            ],
        };
        assert_eq!(
            mentions_for_edit(&content, "@Alice and #room:example.org"),
            vec![mention("@Alice", ALICE)]
        );
    }
}
