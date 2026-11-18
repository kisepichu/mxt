//! Fuzzy matching of names with nucleo-matcher.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Ranks `candidates` against `query` with nucleo-matcher (smart case, smart
/// normalization). Returns the indices of matching candidates, best first;
/// equal scores prefer the shorter candidate, then keep the candidates' order.
/// An empty query matches every candidate in order.
///
/// Spaces separate atoms that must all match, so `"work gen"` matches
/// `"work / #general"`; `\ ` is a literal space and other whitespace is part
/// of an atom. Every atom is fuzzy: fzf operators (`!`, `^`, `'`, `$`) are
/// plain characters, so a displayed name used as the query finds that name.
pub fn rank<S: AsRef<str>>(query: &str, candidates: &[S]) -> Vec<usize> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    if pattern.atoms.is_empty() {
        return (0..candidates.len()).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(usize, u32, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let haystack = Utf32Str::new(candidate.as_ref(), &mut buf);
            let len = haystack.len();
            let score = pattern.score(haystack, &mut matcher)?;
            Some((index, score, len))
        })
        .collect();
    // The matcher does not penalize unmatched trailing characters, so an
    // exact name ties with longer names that contain it; the shorter one wins.
    // A stable sort keeps the candidates' order for the remaining ties.
    scored.sort_by_key(|&(_, score, len)| (Reverse(score), len));
    scored.into_iter().map(|(index, _, _)| index).collect()
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

    #[test]
    fn fzf_operators_are_plain_characters() {
        assert_eq!(
            rank("!alerts", &["home / general", "home / !alerts"]),
            vec![1]
        );
        assert_eq!(rank("^_^", &["home / general", "home / ^_^ club"]), vec![1]);
    }

    #[test]
    fn equal_scores_prefer_the_shorter_candidate_then_the_input_order() {
        assert_eq!(rank("dev", &["devops", "dev"]), vec![1, 0]);
        assert_eq!(rank("dev", &["dev2", "dev1"]), vec![0, 1]);
    }
}
