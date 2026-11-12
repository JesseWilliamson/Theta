//! Line and fold model of a pretty-printed JSON text. Pure.
//!
//! The editor has no folding, so the cell viewer carries its own small one. `json_pretty` puts every
//! container member on its own line and indents two spaces per level, so a fold is simply "the lines
//! between a line that ends in `{` / `[` and the line that holds its closing bracket". The bracket
//! pairs are read from the highlight spans (a bracket inside a string is part of a `Str` span, never a
//! `Punct` one), so no second JSON parse is needed.

use crate::inspect::{JsonSpans, JsonTok};
use std::collections::BTreeSet;
use std::ops::Range;

/// One line of the text with its highlight spans made relative to the line start.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub spans: Vec<(Range<usize>, JsonTok)>,
    /// Nesting depth, from the indentation.
    pub depth: usize,
}

/// What a visible row shows: the line, and how it folds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowFold {
    /// A line that cannot fold.
    None,
    /// A foldable line that is open.
    Open,
    /// A folded line; `hidden` lines are not shown and `closer` is the text of the closing line
    /// (`}` or `],`), appended after an ellipsis.
    Folded { hidden: usize, closer: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub fold: RowFold,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonDoc {
    pub lines: Vec<Line>,
    /// For a line that opens a multi-line container: the line that closes it.
    close_of: Vec<Option<usize>>,
}

impl JsonDoc {
    pub fn new(text: &str, spans: &JsonSpans) -> JsonDoc {
        let mut starts = vec![0usize];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let line_of = |offset: usize| starts.partition_point(|&s| s <= offset) - 1;

        let mut lines: Vec<Line> = Vec::with_capacity(starts.len());
        for (n, &s) in starts.iter().enumerate() {
            let e = starts.get(n + 1).map_or(text.len(), |next| next - 1);
            let body = &text[s..e];
            let indent = body.len() - body.trim_start_matches(' ').len();
            lines.push(Line { text: body.to_string(), spans: Vec::new(), depth: indent / 2 });
        }
        for (range, tok) in spans {
            let n = line_of(range.start);
            let s = starts[n];
            lines[n].spans.push((range.start - s..range.end - s, *tok));
        }

        let mut close_of = vec![None; lines.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (range, tok) in spans {
            if *tok != JsonTok::Punct {
                continue;
            }
            match &text[range.clone()] {
                "{" | "[" => stack.push(line_of(range.start)),
                "}" | "]" => {
                    if let Some(open) = stack.pop() {
                        let close = line_of(range.start);
                        if close > open {
                            close_of[open] = Some(close);
                        }
                    }
                }
                _ => {}
            }
        }
        JsonDoc { lines, close_of }
    }

    pub fn is_foldable(&self, line: usize) -> bool {
        self.close_of.get(line).is_some_and(Option::is_some)
    }

    /// Every line that can fold.
    pub fn foldable(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.lines.len()).filter(|&l| self.is_foldable(l))
    }

    /// The rows to show when the lines in `collapsed` are folded. A fold inside a folded region is
    /// not shown, and stays in `collapsed` so unfolding the outer one restores the inner state.
    pub fn visible(&self, collapsed: &BTreeSet<usize>) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut i = 0;
        while i < self.lines.len() {
            match self.close_of[i] {
                Some(close) if collapsed.contains(&i) => {
                    rows.push(Row {
                        line: i,
                        fold: RowFold::Folded {
                            hidden: close - i,
                            closer: self.lines[close].text.trim_start().to_string(),
                        },
                    });
                    i = close + 1;
                }
                Some(_) => {
                    rows.push(Row { line: i, fold: RowFold::Open });
                    i += 1;
                }
                None => {
                    rows.push(Row { line: i, fold: RowFold::None });
                    i += 1;
                }
            }
        }
        rows
    }

    /// Fold everything except the outermost container, so the top level reads as an outline.
    pub fn collapse_all(&self) -> BTreeSet<usize> {
        self.foldable().filter(|&l| self.lines[l].depth >= 1).collect()
    }

    /// The `{` / `[` at the end of `line` with the line that closes it, for tests and the toggle.
    pub fn closer_of(&self, line: usize) -> Option<usize> {
        self.close_of.get(line).copied().flatten()
    }
}

/// Flip one fold.
pub fn toggle(collapsed: &mut BTreeSet<usize>, line: usize) {
    if !collapsed.remove(&line) {
        collapsed.insert(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::json_pretty;

    fn doc(json: &str) -> JsonDoc {
        let (text, spans) = json_pretty(json).unwrap();
        JsonDoc::new(&text, &spans)
    }

    #[test]
    fn lines_match_the_text_and_carry_relative_spans() {
        let (text, spans) = json_pretty(r#"{"a":[1,"x"],"b":null}"#).unwrap();
        let d = JsonDoc::new(&text, &spans);
        assert_eq!(d.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"), text);
        let key = &d.lines[1];
        assert_eq!(key.text, "  \"a\": [");
        let (r, tok) = &key.spans[0];
        assert_eq!((&key.text[r.clone()], *tok), ("\"a\"", JsonTok::Key));
        assert_eq!(key.depth, 1);
    }

    #[test]
    fn brackets_pair_across_lines() {
        let d = doc(r#"{"a":[1,2],"o":{"k":true},"e":[]}"#);
        // 0 {   1 "a": [   2 1,   3 2   4 ],   5 "o": {   6 "k": true   7 },   8 "e": []   9 }
        assert_eq!(d.closer_of(0), Some(9));
        assert_eq!(d.closer_of(1), Some(4));
        assert_eq!(d.closer_of(5), Some(7));
        assert!(!d.is_foldable(8), "an empty container is one line");
        assert_eq!(d.foldable().collect::<Vec<_>>(), vec![0, 1, 5]);
    }

    #[test]
    fn a_bracket_inside_a_string_is_not_a_fold() {
        let d = doc(r#"{"s":"{ [ not a container","t":"}"}"#);
        assert_eq!(d.foldable().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn folding_hides_lines_and_names_the_closer() {
        let d = doc(r#"{"a":[1,2],"b":3}"#);
        let mut c = BTreeSet::new();
        assert_eq!(d.visible(&c).len(), d.lines.len());
        toggle(&mut c, 1);
        let rows = d.visible(&c);
        assert_eq!(rows.len(), d.lines.len() - 3, "the closing line is folded into the opening one");
        assert_eq!(rows[1], Row { line: 1, fold: RowFold::Folded { hidden: 3, closer: "],".into() } });
        // The line after the fold is the one after the closing bracket.
        assert_eq!(d.lines[rows[2].line].text, "  \"b\": 3");
        toggle(&mut c, 1);
        assert_eq!(d.visible(&c).len(), d.lines.len());
    }

    #[test]
    fn an_inner_fold_survives_closing_and_reopening_the_outer_one() {
        let d = doc(r#"{"o":{"a":[1,2],"b":1}}"#);
        let mut c: BTreeSet<usize> = [1, 2].into_iter().collect();
        let outer_only = d.visible(&c);
        assert!(matches!(outer_only[1].fold, RowFold::Folded { .. }));
        c.remove(&1);
        let rows = d.visible(&c);
        assert!(rows.iter().any(|r| r.line == 2 && matches!(r.fold, RowFold::Folded { .. })));
    }

    #[test]
    fn collapse_all_keeps_the_root_open() {
        let d = doc(r#"{"a":{"b":[1]},"c":[2]}"#);
        let c = d.collapse_all();
        assert!(!c.contains(&0));
        let rows = d.visible(&c);
        // `{`, `"a": {` (folded), `"c": [` (folded), `}`.
        assert_eq!(rows.len(), 4);
        assert!(matches!(rows[1].fold, RowFold::Folded { .. }));
    }

    #[test]
    fn scalars_and_empty_containers_have_no_folds() {
        assert_eq!(doc("42").foldable().count(), 0);
        assert_eq!(doc("{}").foldable().count(), 0);
        assert_eq!(doc("[]").lines.len(), 1);
    }
}
