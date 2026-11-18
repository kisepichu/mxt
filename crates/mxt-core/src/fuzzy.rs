//! Fuzzy matching of names with nucleo-matcher.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Ranks `candidates` against `query` with nucleo-matcher (smart case, smart
/// normalization). Returns the indices of matching candidates, best first;
/// equal scores keep the candidates' order. An empty query matches every
/// candidate in order.
///
/// Whitespace separates atoms that must all match, so `"work gen"` matches
/// `"work / #general"`.
pub fn rank<S: AsRef<str>>(query: &str, candidates: &[S]) -> Vec<usize> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    if pattern.atoms.is_empty() {
        return (0..candidates.len()).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(usize, u32)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let haystack = Utf32Str::new(candidate.as_ref(), &mut buf);
            let score = pattern.score(haystack, &mut matcher)?;
            Some((index, score))
        })
        .collect();
    // A stable sort keeps the candidates' order for equal scores.
    scored.sort_by_key(|&(_, score)| Reverse(score));
    scored.into_iter().map(|(index, _)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_keeps_every_candidate_in_order() {
        assert_eq!(rank("", &["b", "a"]), vec![0, 1]);
    }

    #[test]
    fn best_match_first_and_non_matches_dropped() {
        let names = ["home / #random", "work / #general", "home / #general"];
        let ranked = rank("homegen", &names);
        assert_eq!(ranked.first(), Some(&2));
        assert!(!ranked.contains(&1));
    }

    #[test]
    fn smart_case() {
        assert_eq!(rank("alice", &["Alice"]), vec![0]);
        assert!(rank("Alice", &["alice"]).is_empty());
    }

    #[test]
    fn matches_japanese_names() {
        assert_eq!(rank("雑談", &["work / #general", "home / 雑談"]), vec![1]);
    }
}
