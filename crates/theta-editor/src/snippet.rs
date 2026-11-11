//! Snippets: text with tab stops (`$1`, `${2:default}`, `$0`) inserted into the buffer and navigated with
//! Tab / Shift-Tab.
//!
//! A tab stop that appears more than once is *linked*: when it is active all its occurrences are selected
//! at once, so typing edits every one of them. That is the engine's existing multi-selection editing, not a
//! second mechanism. The session keeps the stops' ranges in step with every edit (they are mapped through
//! each transaction in `Editor::apply_with`) and ends when the cursor leaves the active stop, on Escape, or
//! after the last stop.
//!
//! Variables (`${table}`, `${date}`, ...) are the host's business: it substitutes them before calling
//! [`Editor::insert_snippet`], which only knows tab stops.

use crate::editor::{Editor, Keymap, Mode};
use crate::keys::{Key, KeyInput};
use crate::selection::Selection;
use crate::selection::Selections;
use crate::transaction::{Assoc, Change, Transaction};
use std::ops::Range;

/// A parsed snippet: the text to insert and its stops in navigation order (`$1`, `$2`, ..., then `$0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    pub stops: Vec<Stop>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub index: u32,
    /// Char ranges in `text`; several when the stop is linked.
    pub ranges: Vec<Range<usize>>,
}

impl Snippet {
    /// A snippet with no tab stops.
    pub fn plain(text: impl Into<String>) -> Snippet {
        Snippet { text: text.into(), stops: Vec::new() }
    }

    /// Parse `$n`, `${n}`, `${n:default}` and `$0`. `\$`, `\}` and `\\` escape themselves. Anything else
    /// that starts with `$` is literal text. A stop's default is shown at every occurrence of the stop.
    pub fn parse(src: &str) -> Snippet {
        let chars: Vec<char> = src.chars().collect();
        let mut text = String::new();
        let mut len = 0usize;
        // (index, range) in order of appearance; defaults are filled in afterwards.
        let mut found: Vec<(u32, Range<usize>)> = Vec::new();
        let mut defaults: std::collections::HashMap<u32, String> = Default::default();
        let mut i = 0;
        let push = |text: &mut String, len: &mut usize, c: char| {
            text.push(c);
            *len += 1;
        };
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' && matches!(chars.get(i + 1), Some('$' | '}' | '\\')) {
                push(&mut text, &mut len, chars[i + 1]);
                i += 2;
            } else if c == '$' {
                // $n
                let mut j = i + 1;
                if chars.get(j).is_some_and(char::is_ascii_digit) {
                    let mut n = 0u32;
                    while let Some(d) = chars.get(j).and_then(|c| c.to_digit(10)) {
                        n = n.saturating_mul(10).saturating_add(d);
                        j += 1;
                    }
                    found.push((n, len..len));
                    i = j;
                } else if chars.get(j) == Some(&'{') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
                    j += 1;
                    let mut n = 0u32;
                    while let Some(d) = chars.get(j).and_then(|c| c.to_digit(10)) {
                        n = n.saturating_mul(10).saturating_add(d);
                        j += 1;
                    }
                    match chars.get(j) {
                        Some('}') => {
                            found.push((n, len..len));
                            i = j + 1;
                        }
                        Some(':') => {
                            // Default text up to the closing brace; `\}` and `\\` are escapes.
                            let mut k = j + 1;
                            let mut def = String::new();
                            let mut closed = false;
                            while k < chars.len() {
                                match chars[k] {
                                    '\\' if matches!(chars.get(k + 1), Some('}' | '$' | '\\')) => {
                                        def.push(chars[k + 1]);
                                        k += 2;
                                    }
                                    '}' => {
                                        closed = true;
                                        break;
                                    }
                                    c => {
                                        def.push(c);
                                        k += 1;
                                    }
                                }
                            }
                            if closed {
                                let start = len;
                                for c in def.chars() {
                                    push(&mut text, &mut len, c);
                                }
                                found.push((n, start..len));
                                defaults.entry(n).or_insert(def);
                                i = k + 1;
                            } else {
                                push(&mut text, &mut len, '$');
                                i += 1;
                            }
                        }
                        _ => {
                            push(&mut text, &mut len, '$');
                            i += 1;
                        }
                    }
                } else {
                    push(&mut text, &mut len, '$');
                    i += 1;
                }
            } else {
                push(&mut text, &mut len, c);
                i += 1;
            }
        }
        // A linked stop shows the default of its first occurrence that has one at every occurrence that has
        // none: insert it, shifting everything after. Done by rebuilding with the defaults applied.
        let needs_fill = found.iter().any(|(n, r)| r.is_empty() && defaults.contains_key(n));
        if needs_fill {
            let mut out = String::new();
            let mut ranges: Vec<(u32, Range<usize>)> = Vec::new();
            let text_chars: Vec<char> = text.chars().collect();
            let mut pos = 0usize;
            let mut sorted: Vec<(u32, Range<usize>)> = found.clone();
            sorted.sort_by_key(|(_, r)| r.start);
            let mut written = 0usize;
            for (n, r) in sorted {
                for c in &text_chars[pos..r.start.min(text_chars.len())] {
                    out.push(*c);
                    written += 1;
                }
                pos = r.start;
                if r.is_empty() {
                    let def = defaults.get(&n).cloned().unwrap_or_default();
                    let start = written;
                    for c in def.chars() {
                        out.push(c);
                        written += 1;
                    }
                    ranges.push((n, start..written));
                } else {
                    let start = written;
                    for c in &text_chars[r.start..r.end] {
                        out.push(*c);
                        written += 1;
                    }
                    pos = r.end;
                    ranges.push((n, start..written));
                }
            }
            for c in &text_chars[pos.min(text_chars.len())..] {
                out.push(*c);
            }
            text = out;
            found = ranges;
        }
        let mut stops: Vec<Stop> = Vec::new();
        for (n, r) in found {
            match stops.iter_mut().find(|s| s.index == n) {
                Some(s) => s.ranges.push(r),
                None => stops.push(Stop { index: n, ranges: vec![r] }),
            }
        }
        // $0 is the final position: it goes last.
        stops.sort_by_key(|s| if s.index == 0 { u32::MAX } else { s.index });
        Snippet { text, stops }
    }

    /// Indent every line after the first by `indent`, so a multi-line snippet follows the line it lands on.
    pub fn indented(mut self, indent: &str) -> Snippet {
        if indent.is_empty() || !self.text.contains('\n') {
            return self;
        }
        let indent_len = indent.chars().count();
        let mut out = String::new();
        // Char offsets at which an indent was inserted (position of the first char of each later line).
        let mut inserted_at: Vec<usize> = Vec::new();
        let mut n = 0usize;
        for c in self.text.chars() {
            out.push(c);
            n += 1;
            if c == '\n' {
                inserted_at.push(n);
                out.push_str(indent);
            }
        }
        let shift = |p: usize, end: bool| {
            // Insertions at or before `p` move it; an insertion exactly at the start of a range moves the
            // start too (the indent is not part of the placeholder), and at the end of a range moves
            // nothing extra unless the range itself contains the line break.
            let k = inserted_at.iter().filter(|&&at| if end { at < p } else { at <= p }).count();
            p + k * indent_len
        };
        for s in &mut self.stops {
            for r in &mut s.ranges {
                *r = shift(r.start, false)..shift(r.end, true).max(shift(r.start, false));
            }
        }
        self.text = out;
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Session {
    stops: Vec<Stop>,
    current: usize,
}

impl Session {
    pub(crate) fn map(&mut self, tx: &Transaction) {
        for s in &mut self.stops {
            for r in &mut s.ranges {
                let start = tx.map_pos(r.start, Assoc::Before);
                let end = tx.map_pos(r.end, Assoc::After).max(start);
                *r = start..end;
            }
        }
    }
}

impl Editor {
    /// Whether a snippet's tab stops are being navigated.
    pub fn snippet_active(&self) -> bool {
        self.snippet.is_some()
    }

    pub fn end_snippet(&mut self) {
        self.snippet = None;
    }

    /// Insert `snippet` in place of the primary selection and activate its first stop. In the modal key
    /// maps this enters Insert mode, since typing over the placeholder is what comes next.
    pub fn insert_snippet(&mut self, snippet: &Snippet) {
        let p = self.sels.primary();
        let (from, to) = (p.from(), p.to());
        let base = from;
        let tx = Transaction::new(vec![Change { from, to, text: snippet.text.clone() }]);
        let end = base + snippet.text.chars().count();
        self.apply_with(tx, Some(Selections::single(Selection::point(end))));
        self.snippet = None;
        if self.keymap != Keymap::Standard {
            self.mode = Mode::Insert;
        }
        if snippet.stops.is_empty() {
            return;
        }
        let stops: Vec<Stop> = snippet
            .stops
            .iter()
            .map(|s| Stop {
                index: s.index,
                ranges: s.ranges.iter().map(|r| r.start + base..r.end + base).collect(),
            })
            .collect();
        self.snippet = Some(Session { stops, current: 0 });
        self.snippet_select_current();
    }

    fn snippet_select_current(&mut self) {
        let Some(s) = &self.snippet else { return };
        let stop = &s.stops[s.current];
        let sels: Vec<Selection> = stop.ranges.iter().map(|r| Selection::new(r.start, r.end)).collect();
        let final_empty = stop.index == 0 && stop.ranges.iter().all(|r| r.is_empty());
        self.set_sels(Selections::new(sels, 0));
        // Arriving at an empty `$0` is the end of the snippet: there is nothing left to navigate to.
        if final_empty {
            self.snippet = None;
        }
    }

    /// Tab / Shift-Tab. Returns whether a session was active. Moving past the last stop ends the session.
    pub fn snippet_navigate(&mut self, forward: bool) -> bool {
        let Some(s) = &mut self.snippet else { return false };
        if forward {
            if s.current + 1 >= s.stops.len() {
                // Leaving the last stop: the cursor stays where it is, at the end of what was typed.
                let collapse = Selections::new(
                    self.sels.v.iter().map(|x| Selection::point(x.to())).collect(),
                    self.sels.primary,
                );
                self.snippet = None;
                self.set_sels(collapse);
                return true;
            }
            s.current += 1;
        } else if s.current > 0 {
            s.current -= 1;
        }
        self.snippet_select_current();
        true
    }

    /// Key handling while a session is active. `Some(consumed)` means the key was dealt with here.
    pub(crate) fn snippet_key(&mut self, k: &KeyInput) -> Option<bool> {
        self.snippet.as_ref()?;
        let plain = !k.modifiers.ctrl && !k.modifiers.alt && !k.modifiers.platform;
        match k.key {
            Key::Tab if plain => {
                let forward = !k.modifiers.shift;
                Some(self.snippet_navigate(forward))
            }
            Key::Escape => {
                self.snippet = None;
                None
            }
            Key::Char(_) | Key::Backspace | Key::Delete | Key::Enter if plain => {
                // A placeholder that is still selected is replaced by what is typed.
                if self.snippet_placeholder_selected() {
                    let changes: Vec<Change> = self
                        .sels
                        .v
                        .iter()
                        .filter(|s| !s.is_empty())
                        .map(|s| Change { from: s.from(), to: s.to(), text: String::new() })
                        .collect();
                    self.apply(Transaction::new(changes));
                    let points = Selections::new(
                        self.sels.v.iter().map(|s| Selection::point(s.from())).collect(),
                        self.sels.primary,
                    );
                    self.set_sels(points);
                    if matches!(k.key, Key::Backspace | Key::Delete) {
                        return Some(true);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// The selections are exactly the active stop's (non-empty) placeholders: nothing was typed yet. In Helix
    /// a collapsed cursor is a one-character block, so "some selection is non-empty" would be wrong here.
    fn snippet_placeholder_selected(&self) -> bool {
        let Some(s) = &self.snippet else { return false };
        let stop = &s.stops[s.current];
        stop.ranges.iter().all(|r| !r.is_empty())
            && self.sels.v.len() == stop.ranges.len()
            && self.sels.v.iter().zip(&stop.ranges).all(|(sel, r)| sel.from() == r.start && sel.to() == r.end)
    }

    /// After every key: a session ends when the cursor has left the active stop (arrow keys, a click).
    pub(crate) fn snippet_check(&mut self) {
        let Some(s) = &self.snippet else { return };
        let stop = &s.stops[s.current];
        // The cursor's start must be within a range (touching its end counts: that is where typing goes).
        let inside = self
            .sels
            .v
            .iter()
            .all(|sel| stop.ranges.iter().any(|r| sel.from() >= r.start && sel.from() <= r.end));
        if !inside {
            self.snippet = None;
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // the tests compare char ranges, one per stop
mod tests {
    use super::*;

    fn stops(s: &Snippet) -> Vec<(u32, Vec<Range<usize>>)> {
        s.stops.iter().map(|s| (s.index, s.ranges.clone())).collect()
    }

    #[test]
    fn parses_numbered_stops_defaults_and_final_position() {
        let s = Snippet::parse("SELECT $1 FROM ${2:tbl} WHERE $3$0");
        assert_eq!(s.text, "SELECT  FROM tbl WHERE ");
        assert_eq!(stops(&s), [(1, vec![7..7]), (2, vec![13..16]), (3, vec![23..23]), (0, vec![23..23])]);
    }

    #[test]
    fn braces_without_default_and_escapes() {
        let s = Snippet::parse("a ${1} \\$2 \\} \\\\ $x $");
        assert_eq!(s.text, "a  $2 } \\ $x $");
        assert_eq!(stops(&s), [(1, vec![2..2])]);
    }

    #[test]
    fn linked_stops_share_the_first_default() {
        let s = Snippet::parse("${1:t} JOIN $1 ON $1.id");
        assert_eq!(s.text, "t JOIN t ON t.id");
        assert_eq!(stops(&s), [(1, vec![0..1, 7..8, 12..13])]);
    }

    #[test]
    fn default_may_contain_escaped_braces_and_colons() {
        let s = Snippet::parse("${1:a\\}b:c}");
        assert_eq!(s.text, "a}b:c");
        assert_eq!(stops(&s), [(1, vec![0..5])]);
    }

    #[test]
    fn unterminated_default_is_literal() {
        let s = Snippet::parse("x ${1:oops");
        assert!(s.stops.is_empty());
        assert!(s.text.starts_with("x $"));
    }

    #[test]
    fn indentation_follows_the_landing_line() {
        let s = Snippet::parse("SELECT $1\nFROM ${2:t}").indented("  ");
        assert_eq!(s.text, "SELECT \n  FROM t");
        assert_eq!(stops(&s), [(1, vec![7..7]), (2, vec![15..16])]);
    }

    #[test]
    fn plain_text_has_no_stops() {
        assert!(Snippet::parse("just text").stops.is_empty());
    }
}
