//! Fuzzy subsequence matching for every list in the app (command palette,
//! pickers, explorer filter).
//!
//! The matcher finds the *best-scoring* subsequence rather than the first one,
//! so "usr" ranks `users` above `us_rows_r…` and highlights the characters a
//! person would expect. Scoring rewards consecutive runs and matches at word
//! boundaries (`_`, `-`, `.`, `/`, space, camelCase), and penalises gaps.
//! Cost is O(query × candidate) thanks to a running maximum over gap starts.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i64,
    /// Byte offsets (into the candidate) of the first byte of every matched char.
    pub positions: Vec<usize>,
}

const MATCH: i64 = 16;
const SAME_CASE: i64 = 1;
const CONSECUTIVE: i64 = 24;
const BOUNDARY: i64 = 14;
const CAMEL: i64 = 10;
const FIRST_CHAR: i64 = 10;
const GAP: i64 = 3;
const LEADING_GAP: i64 = 1;
const LEADING_GAP_CAP: i64 = 6;
const NEG: i64 = i64::MIN / 4;

fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '_' | '-' | '.' | '/' | ':' | '(' | ')' | '\\')
}

fn eq_ignore_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Match `query` against `candidate`. Whitespace in the query is ignored, so
/// "new con" finds "New Console". An empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let needle: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() {
        return Some(FuzzyMatch { score: 0, positions: Vec::new() });
    }
    let hay: Vec<(usize, char)> = candidate.char_indices().collect();
    let (n, h) = (needle.len(), hay.len());
    if n > h {
        return None;
    }

    // Per-position bonus for matching there, from the preceding character.
    let bonus: Vec<i64> = (0..h)
        .map(|j| {
            let c = hay[j].1;
            if j == 0 {
                return FIRST_CHAR + BOUNDARY;
            }
            let p = hay[j - 1].1;
            if is_separator(p) {
                BOUNDARY
            } else if p.is_lowercase() && c.is_uppercase() {
                CAMEL
            } else {
                0
            }
        })
        .collect();

    // dp[i][j]: best score with needle[i] matched exactly at hay[j]; from[i][j]
    // remembers the previous match position for the traceback.
    let mut dp = vec![vec![NEG; h]; n];
    let mut from = vec![vec![usize::MAX; h]; n];
    for i in 0..n {
        // Best (score, k) over k <= j-2 of dp[i-1][k] - GAP*(j-1-k), maintained as j advances.
        let mut gapped: (i64, usize) = (NEG, usize::MAX);
        for j in i..h {
            if i > 0 && j >= 2 {
                let cand = dp[i - 1][j - 2];
                let carried = (gapped.0.saturating_sub(GAP), gapped.1);
                gapped = if cand > NEG { (cand - GAP, j - 2).max_by_score(carried) } else { carried };
            }
            if !eq_ignore_case(needle[i], hay[j].1) {
                continue;
            }
            let mut base = MATCH + bonus[j] + if needle[i] == hay[j].1 { SAME_CASE } else { 0 };
            if i == 0 {
                base -= (j as i64 * LEADING_GAP).min(LEADING_GAP_CAP);
                dp[i][j] = base;
            } else {
                let mut best = (NEG, usize::MAX);
                if j >= 1 && dp[i - 1][j - 1] > NEG {
                    best = (dp[i - 1][j - 1] + CONSECUTIVE, j - 1);
                }
                if gapped.0 > NEG && gapped.0 > best.0 {
                    best = gapped;
                }
                if best.0 > NEG {
                    dp[i][j] = base + best.0;
                    from[i][j] = best.1;
                }
            }
        }
    }

    let (mut best_j, mut best) = (usize::MAX, NEG);
    for (j, &v) in dp[n - 1].iter().enumerate() {
        if v > best {
            best = v;
            best_j = j;
        }
    }
    if best_j == usize::MAX {
        return None;
    }
    let mut positions = vec![0; n];
    let mut j = best_j;
    for i in (0..n).rev() {
        positions[i] = hay[j].0;
        if i > 0 {
            j = from[i][j];
        }
    }
    // Prefer shorter candidates on otherwise equal matches.
    let score = best - (h as i64) / 4;
    Some(FuzzyMatch { score, positions })
}

trait MaxByScore {
    fn max_by_score(self, other: Self) -> Self;
}
impl MaxByScore for (i64, usize) {
    fn max_by_score(self, other: Self) -> Self {
        if other.0 > self.0 { other } else { self }
    }
}

/// Rank `items` by `key`, best first; ties keep the original order.
pub fn fuzzy_filter<T>(items: &[T], query: &str, key: impl Fn(&T) -> &str) -> Vec<(usize, FuzzyMatch)> {
    let mut out: Vec<(usize, FuzzyMatch)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| fuzzy_match(query, key(item)).map(|m| (i, m)))
        .collect();
    out.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    out
}

/// Turn matched start offsets into merged byte ranges for text highlighting.
pub fn highlight_ranges(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for &p in positions {
        let Some(c) = text.get(p..).and_then(|s| s.chars().next()) else { continue };
        let end = p + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == p => last.end = end,
            _ => ranges.push(p..end),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, c: &str) -> Option<FuzzyMatch> {
        fuzzy_match(q, c)
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(m("", "anything").unwrap().positions, Vec::<usize>::new());
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert!(m("xyz", "users").is_none());
        assert!(m("sru", "users").is_none());
        assert!(m("userss", "users").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(m("USERS", "users").is_some());
        assert!(m("users", "USERS").is_some());
    }

    #[test]
    fn positions_are_reported() {
        assert_eq!(m("usr", "users").unwrap().positions, vec![0, 1, 3]);
    }

    #[test]
    fn consecutive_beats_scattered() {
        let a = m("ord", "orders").unwrap().score;
        let b = m("ord", "o_r_d_x").unwrap().score;
        assert!(a > b, "{a} <= {b}");
    }

    #[test]
    fn word_boundaries_win() {
        // "nc" should pick the starts of "New Console", not an interior 'n..c'.
        let r = m("nc", "New Console").unwrap();
        assert_eq!(r.positions, vec![0, 4]);
        let camel = m("gt", "getTable").unwrap();
        assert_eq!(camel.positions, vec![0, 3]);
    }

    #[test]
    fn best_alignment_is_chosen_not_first() {
        // First 'a' is at 0 but the run "ab" later is the better alignment for "ab".
        let r = m("ab", "a_xxab").unwrap();
        assert_eq!(r.positions, vec![4, 5]);
    }

    #[test]
    fn prefix_ranks_above_suffix() {
        let items = ["my_users", "users", "superusers"];
        let ranked = fuzzy_filter(&items, "users", |s| s);
        assert_eq!(items[ranked[0].0], "users");
    }

    #[test]
    fn spaces_in_query_are_ignored() {
        assert!(m("new con", "New Console").is_some());
    }

    #[test]
    fn multibyte_positions_are_byte_offsets() {
        let r = m("é", "café").unwrap();
        assert_eq!(r.positions, vec![3]);
        assert_eq!(highlight_ranges("café", &r.positions), vec![3..5]);
    }

    #[test]
    fn highlight_ranges_merge_adjacent() {
        assert_eq!(highlight_ranges("users", &[0, 1, 3]), vec![0..2, 3..4]);
    }

    #[test]
    fn ties_keep_original_order() {
        let items = ["ab", "ab"];
        let r = fuzzy_filter(&items, "ab", |s| s);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 1]);
    }
}
