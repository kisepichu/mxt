//! The emoji of the reaction picker, from the `emojis` crate (Unicode
//! emoji with GitHub shortcodes).

use std::sync::LazyLock;

use crate::fuzzy;

/// An emoji in its default skin tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emoji(&'static emojis::Emoji);

/// The reactions most people use, in the picker's order for an empty query.
const COMMON: [&str; 8] = ["👍", "❤️", "😂", "😮", "😢", "🎉", "👀", "🙏"];

/// Every emoji in `emojis` order, built once for searching.
static ALL: LazyLock<Vec<Emoji>> = LazyLock::new(|| all().collect());

/// Every shortcode in alphabetical order, built once for completion.
static SHORTCODES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut shortcodes: Vec<&'static str> = emojis::iter()
        .flat_map(|emoji| emoji.shortcodes())
        .collect();
    shortcodes.sort_unstable();
    shortcodes.dedup();
    shortcodes
});

impl Emoji {
    /// The emoji `text` is, also when it lacks the variation selector of its
    /// fully qualified form (`❤` for `❤️`).
    pub fn get(text: &str) -> Option<Emoji> {
        emojis::get(text).map(Emoji)
    }

    /// The emoji an argument names (design, "Commands"): an emoji, as for
    /// [`Emoji::get`], or a shortcode with colons on both sides or none
    /// (`thumbsup`, `:+1:`; not `:+1`).
    pub fn parse(text: &str) -> Option<Emoji> {
        if let Some(emoji) = Emoji::get(text) {
            return Some(emoji);
        }
        let shortcode = match text.strip_prefix(':') {
            Some(rest) => rest.strip_suffix(':')?,
            None if text.ends_with(':') => return None,
            None => text,
        };
        emojis::get_by_shortcode(shortcode).map(Emoji)
    }

    /// The fully qualified emoji, the key of a reaction with it.
    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }

    /// Its first shortcode and the emoji (`:+1: 👍`); without a shortcode,
    /// the emoji and its name.
    pub fn label(self) -> String {
        match self.0.shortcode() {
            Some(shortcode) => format!(":{shortcode}: {}", self.as_str()),
            None => format!("{} {}", self.as_str(), self.0.name()),
        }
    }
}

/// Every emoji in `emojis` order.
pub fn all() -> impl Iterator<Item = Emoji> {
    emojis::iter().map(Emoji)
}

/// The most common reactions: 👍 ❤️ 😂 😮 😢 🎉 👀 🙏.
pub fn common() -> impl Iterator<Item = Emoji> {
    COMMON.into_iter().filter_map(Emoji::get)
}

/// The emojis whose shortcodes or name match `query`, best first (see
/// [`fuzzy::rank_by`]); every emoji in order for an empty query.
pub fn search(query: &str) -> Vec<Emoji> {
    let all = ALL.as_slice();
    fuzzy::rank_by(query, all, |emoji| {
        emoji.0.shortcodes().chain([emoji.0.name()])
    })
    .into_iter()
    .map(|index| all[index])
    .collect()
}

/// The shortcodes that start with `prefix`, without colons, in alphabetical
/// order.
pub fn shortcodes_starting_with(prefix: &str) -> &'static [&'static str] {
    let all = SHORTCODES.as_slice();
    let start = all.partition_point(|shortcode| *shortcode < prefix);
    let len = all[start..].partition_point(|shortcode| shortcode.starts_with(prefix));
    &all[start..start + len]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str) -> Vec<&'static str> {
        search(query).into_iter().map(Emoji::as_str).collect()
    }

    #[test]
    fn search_matches_shortcodes_and_names() {
        assert_eq!(found("thumbsup").first(), Some(&"👍"));
        assert_eq!(found("+1").first(), Some(&"👍"));
        // 🙏 has the shortcode `pray` and the name "folded hands".
        assert_eq!(found("folded hands").first(), Some(&"🙏"));
        assert!(found("zzzzzz").is_empty());
        assert_eq!(search("").len(), all().count());
    }

    #[test]
    fn labels_and_unqualified_lookups() {
        let thumbs = Emoji::get("👍").unwrap();
        assert_eq!(thumbs.label(), ":+1: 👍");
        assert_eq!(Emoji::get("\u{2764}").unwrap().as_str(), "\u{2764}\u{fe0f}");
        assert_eq!(common().count(), COMMON.len());
    }

    #[test]
    fn an_argument_is_an_emoji_or_a_shortcode_with_colons_on_both_sides_or_none() {
        let thumbs = Emoji::get("👍");
        assert!(thumbs.is_some());
        for text in ["👍", "thumbsup", "+1", ":+1:", ":thumbsup:"] {
            assert_eq!(Emoji::parse(text), thumbs, "{text}");
        }
        for text in [":+1", "+1:", "nonsense", "", ":", "::", "Thumbsup", " 👍"] {
            assert_eq!(Emoji::parse(text), None, "{text}");
        }
    }

    #[test]
    fn shortcodes_complete_by_prefix_in_alphabetical_order() {
        assert_eq!(
            shortcodes_starting_with("thumbs"),
            ["thumbsdown", "thumbsup"]
        );
        let every = shortcodes_starting_with("");
        assert!(every.is_sorted() && every.len() > all().count() / 2);
        assert!(shortcodes_starting_with("Thumbs").is_empty());
        assert!(shortcodes_starting_with("zzzz").is_empty());
    }
}
