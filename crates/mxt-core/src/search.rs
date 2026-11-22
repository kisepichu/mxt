//! The search's matcher (design, "Search and jumps"): one rule for the items
//! `/`, `n` and `N` find and for the text the timeline and the message view
//! highlight.

use std::iter;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::timeline::MessageContent;

/// A search pattern, matched as a substring without regard to case: the
/// pattern and the text are lowercased character by character with
/// [`char::to_lowercase`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needle(String);

impl Needle {
    pub fn new(pattern: &str) -> Needle {
        Needle(pattern.chars().flat_map(char::to_lowercase).collect())
    }

    /// Whether the text of `chars` contains the pattern.
    pub fn is_in(&self, chars: impl Iterator<Item = char>) -> bool {
        let text: String = chars.flat_map(char::to_lowercase).collect();
        text.contains(self.0.as_str())
    }

    /// The matches in `text` as byte ranges, from left to right: those of
    /// [`str::match_indices`] in the lowercased text, which do not overlap,
    /// each covering the characters whose lowercase forms it touches and
    /// widened to whole grapheme clusters. Ranges that then overlap or
    /// touch are merged.
    pub fn find_in(&self, text: &str) -> Vec<Range<usize>> {
        if self.0.is_empty() {
            return Vec::new();
        }
        // For each character, where its lowercase form starts in `lowered`
        // and where the character starts in `text`.
        let mut lowered = String::with_capacity(text.len());
        let mut starts: Vec<(usize, usize)> = Vec::with_capacity(text.len());
        for (index, c) in text.char_indices() {
            starts.push((lowered.len(), index));
            lowered.extend(c.to_lowercase());
        }
        // The character whose lowercase form holds the byte `at`: every
        // lowercase form has a character, so the forms start in order.
        let char_at = |at: usize| starts.partition_point(|&(start, _)| start <= at) - 1;
        let found: Vec<Range<usize>> = lowered
            .match_indices(self.0.as_str())
            .map(|(start, matched)| {
                let first = char_at(start);
                let last = char_at(start + matched.len() - 1);
                let end = starts.get(last + 1).map_or(text.len(), |&(_, index)| index);
                starts[first].1..end
            })
            .collect();
        if found.is_empty() {
            return found;
        }
        let boundaries: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .chain(iter::once(text.len()))
            .collect();
        let mut ranges: Vec<Range<usize>> = Vec::with_capacity(found.len());
        for range in found {
            // The start of the cluster that holds the first byte, and the
            // end of the one that holds the last.
            let start = boundaries[boundaries.partition_point(|&at| at <= range.start) - 1];
            let end = boundaries[boundaries.partition_point(|&at| at < range.end)];
            match ranges.last_mut() {
                Some(last) if start <= last.end => last.end = last.end.max(end),
                _ => ranges.push(start..end),
            }
        }
        ranges
    }
}

/// The matches in `content`'s plain text ([`MessageContent::plain_text`],
/// which the search searches) for each of its lines, in order: relative to
/// the text after the line's prefix, which only rich text has (`> ` for a
/// quote, `- ` or `1. ` for a list item, see
/// [`RichText::to_plain_with_prefixes`]), and clipped to it, so a match that
/// starts in the prefix keeps its part after it.
///
/// [`RichText::to_plain_with_prefixes`]: crate::rich::RichText::to_plain_with_prefixes
pub fn plain_line_matches(content: &MessageContent, needle: &Needle) -> Vec<Vec<Range<usize>>> {
    let (text, prefixes) = match content {
        MessageContent::Text { rich, .. }
        | MessageContent::Notice(rich)
        | MessageContent::Emote(rich) => rich.to_plain_with_prefixes(),
        other => (other.plain_text(), Vec::new()),
    };
    let matches = needle.find_in(&text);
    let prefixes = prefixes.into_iter().chain(iter::repeat(0));
    let mut lines = Vec::new();
    let mut line_start = 0;
    // The first match that may reach the line.
    let mut next = 0;
    for (line, prefix) in text.split('\n').zip(prefixes) {
        let end = line_start + line.len();
        let from = line_start + prefix.min(line.len());
        let mut ranges = Vec::new();
        while let Some(range) = matches.get(next) {
            if range.start >= end {
                break;
            }
            if range.end > from {
                ranges.push(range.start.max(from) - from..range.end.min(end) - from);
            }
            if range.end > end {
                // It goes on in the next line.
                break;
            }
            next += 1;
        }
        lines.push(ranges);
        line_start = end + 1;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich::{Block, Inline, RichText, Style};

    /// The matches as `(start, end)`.
    fn find(text: &str, pattern: &str) -> Vec<(usize, usize)> {
        let ranges = Needle::new(pattern).find_in(text);
        ranges
            .iter()
            .map(|range| (range.start, range.end))
            .collect()
    }

    /// The matches of each plain line as `(start, end)`.
    fn lines(content: &MessageContent, pattern: &str) -> Vec<Vec<(usize, usize)>> {
        let lines = plain_line_matches(content, &Needle::new(pattern));
        lines
            .iter()
            .map(|ranges| {
                ranges
                    .iter()
                    .map(|range| (range.start, range.end))
                    .collect()
            })
            .collect()
    }

    fn paragraph(text: &str) -> Block {
        Block::Paragraph(vec![Inline::Text {
            text: text.into(),
            style: Style::default(),
        }])
    }

    fn quote(text: &str) -> MessageContent {
        MessageContent::Text {
            body: format!("> {text}"),
            rich: RichText {
                blocks: vec![Block::Quote(vec![paragraph(text)])],
            },
        }
    }

    #[test]
    fn matches_ignore_case_as_the_search_does() {
        assert_eq!(find("ABC abc", "b"), [(1, 2), (5, 6)]);
        assert_eq!(find("ABC abc", "B"), [(1, 2), (5, 6)]);
        assert!(Needle::new("WORLD").is_in("Hello world".chars()));
        assert!(!Needle::new("worlds").is_in("Hello world".chars()));
        assert!(find("Hello world", "xyz").is_empty());
    }

    #[test]
    fn matches_are_byte_ranges_on_grapheme_cluster_boundaries() {
        // Four characters of three bytes before the match.
        assert_eq!(find("日本語のテキスト", "テキ"), [(12, 18)]);
        // A thumb and its skin tone are one cluster of eight bytes.
        assert_eq!(find("👍🏽 ok", "👍"), [(0, 8)]);
        // `İ` (two bytes) lowercases to `i` and a combining dot.
        assert_eq!(find("İstanbul", "i"), [(0, 2)]);
        assert_eq!(find("İstanbul", "stan"), [(2, 6)]);
        // `e` and its combining accent are one cluster.
        assert_eq!(find("cafe\u{301}", "cafe"), [(0, 6)]);
    }

    #[test]
    fn matches_do_not_overlap_and_touching_ones_merge() {
        assert_eq!(find("aaa", "aa"), [(0, 2)]);
        assert_eq!(find("aaaa", "aa"), [(0, 4)]);
        assert_eq!(find("a a", "a"), [(0, 1), (2, 3)]);
    }

    #[test]
    fn plain_line_matches_are_relative_to_the_text_after_the_prefix() {
        assert_eq!(lines(&quote("hello"), "hello"), [[(0, 5)]]);
        // A match that starts in the prefix keeps its part after it.
        assert_eq!(lines(&quote("hello"), "> h"), [[(0, 1)]]);
        assert_eq!(lines(&quote("hello"), ">"), [Vec::<(usize, usize)>::new()]);
        assert_eq!(lines(&MessageContent::Redacted, "del"), [[(1, 4)]]);
        // A list of two items after a paragraph, each line with its own
        // matches.
        let list = MessageContent::Notice(RichText {
            blocks: vec![
                paragraph("milk list"),
                Block::List {
                    ordered: true,
                    start: 9,
                    items: vec![vec![paragraph("milk")], vec![paragraph("more milk")]],
                },
            ],
        });
        assert_eq!(
            lines(&list, "MILK"),
            [vec![(0, 4)], vec![(0, 4)], vec![(5, 9)]]
        );
    }
}
