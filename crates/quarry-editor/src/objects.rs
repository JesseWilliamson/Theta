//! Text objects (`mi`/`ma`) and surround pairs (`ms`/`md`/`mr`).

use crate::motion::bracket_pair;
use crate::selection::Selection;
use crate::text::{self, CharClass, Text, classify};

/// The (open, close) pair for a surround character; same char on both sides for quotes.
pub fn surround_pair(c: char) -> (char, char) {
    match c {
        '(' | ')' | 'b' => ('(', ')'),
        '[' | ']' | 'r' => ('[', ']'),
        '{' | '}' | 'B' => ('{', '}'),
        '<' | '>' | 'a' => ('<', '>'),
        c => (c, c),
    }
}

/// All bracket pairs `open..close` that enclose the whole range `from..=end`, innermost first.
fn enclosing_brackets(t: Text, from: usize, end: usize, open: char, close: char) -> Vec<(usize, usize)> {
    let len = t.len_chars();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = from as isize;
    while i >= 0 {
        let ch = t.char(i as usize);
        if ch == close {
            // The char the scan starts on is this pair's own closer, not nesting.
            if i as usize != from {
                depth += 1;
            }
        } else if ch == open {
            if depth == 0 {
                // find its partner
                let mut d = 0usize;
                let mut j = i as usize + 1;
                while j < len {
                    let c2 = t.char(j);
                    if c2 == open {
                        d += 1;
                    } else if c2 == close {
                        if d == 0 {
                            break;
                        }
                        d -= 1;
                    }
                    j += 1;
                }
                // A pair that closes before the end of the selection doesn't enclose it.
                if j < len && j >= end {
                    out.push((i as usize, j));
                }
            } else {
                depth -= 1;
            }
        }
        i -= 1;
    }
    out
}

/// Quote pairs (nearest left, nearest right) around the range, `nth` levels out.
fn enclosing_quotes(t: Text, from: usize, end: usize, q: char, nth: usize, same_line: bool) -> Option<(usize, usize)> {
    let len = t.len_chars();
    let (lo, hi) = if same_line {
        let l = t.char_to_line(from);
        (t.line_to_char(l), text::line_end(t, l))
    } else {
        (0, len)
    };
    // If the cursor sits on a quote, parity of quotes before it decides if it opens or closes.
    let on_quote = from < len && t.char(from) == q;
    let mut left_from = from;
    let mut right_from = end + 1;
    if on_quote {
        let before = (lo..from).filter(|&i| t.char(i) == q).count();
        if before % 2 == 0 {
            right_from = from + 1; // opener: partner lies to the right
            left_from = from + 1; // pretend the opener itself is the nearest left quote
        } else {
            left_from = from; // closer: opener lies to the left
            right_from = from;
        }
    }
    let mut lefts = Vec::new();
    let mut i = left_from;
    while i > lo {
        i -= 1;
        if t.char(i) == q {
            lefts.push(i);
        }
    }
    let mut rights = Vec::new();
    let mut j = right_from;
    while j < hi {
        if t.char(j) == q {
            rights.push(j);
        }
        j += 1;
    }
    if on_quote {
        let before = (lo..from).filter(|&i| t.char(i) == q).count();
        if before % 2 == 0 {
            lefts.insert(0, from);
            rights = rights.into_iter().collect();
        } else {
            rights.insert(0, from);
        }
    }
    let n = nth.max(1);
    Some((*lefts.get(n - 1)?, *rights.get(n - 1)?)).filter(|(o, c)| o < c)
}

/// Positions (open, close) of a delimiter pair surrounding `sel`.
///
/// `ch == None` means "closest of any kind" (`mim`). `nth` selects the n-th enclosing level.
pub fn find_surround(t: Text, sel: Selection, ch: Option<char>, nth: usize) -> Option<(usize, usize)> {
    let len = t.len_chars();
    if len == 0 {
        return None;
    }
    let from = sel.from().min(len - 1);
    let end = if sel.is_empty() { from } else { text::prev_grapheme(t, sel.to()).max(from) };
    let nth = nth.max(1);
    match ch {
        Some(c) => {
            let (open, close) = surround_pair(c);
            if open == close {
                enclosing_quotes(t, from, end, open, nth, false)
            } else {
                enclosing_brackets(t, from, end, open, close).get(nth - 1).copied()
            }
        }
        None => {
            let mut all = Vec::new();
            for (o, c) in [('(', ')'), ('[', ']'), ('{', '}')] {
                all.extend(enclosing_brackets(t, from, end, o, c));
            }
            for q in ['\'', '"', '`'] {
                if let Some(p) = enclosing_quotes(t, from, end, q, 1, true) {
                    all.push(p);
                }
            }
            all.sort_by_key(|&(o, c)| c - o);
            all.get(nth - 1).copied()
        }
    }
}

/// Select a text object around `sel`'s cursor. `inner` picks the inside of pairs / the word
/// without surrounding space.
pub fn text_object(t: Text, sel: Selection, kind: char, inner: bool, count: usize) -> Option<Selection> {
    match kind {
        'w' | 'W' => word_object(t, sel, kind == 'W', inner, count),
        'p' => paragraph_object(t, sel, inner, count),
        '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' | '\'' | '"' | '`' | 'm' => {
            let ch = if kind == 'm' { None } else { Some(kind) };
            let (o, c) = find_surround(t, sel, ch, count)?;
            let r = if inner {
                if c == o + 1 {
                    // empty pair: nothing inside to select; keep an empty selection after the opener
                    return Some(Selection::point(o + 1));
                }
                Selection::new(o + 1, c)
            } else {
                Selection::new(o, c + 1)
            };
            Some(r.with_direction(sel.is_forward()))
        }
        _ => None,
    }
}

fn word_object(t: Text, sel: Selection, long: bool, inner: bool, count: usize) -> Option<Selection> {
    let len = t.len_chars();
    if len == 0 {
        return None;
    }
    let cursor = sel.block_cursor(t).min(len - 1);
    let class_of = |i: usize| -> CharClass {
        let c = classify(t.char(i));
        if long && matches!(c, CharClass::Word | CharClass::Punct) { CharClass::Word } else { c }
    };
    let run = |i: usize| -> (usize, usize) {
        let cl = class_of(i);
        let mut s = i;
        while s > 0 && class_of(s - 1) == cl && cl != CharClass::Eol {
            s -= 1;
        }
        let mut e = i + 1;
        while e < len && class_of(e) == cl && cl != CharClass::Eol {
            e += 1;
        }
        (s, e)
    };
    let (mut s, mut e) = run(cursor);
    if classify(t.char(cursor)) == CharClass::Eol {
        return None;
    }
    if !inner {
        let on_space = class_of(cursor) == CharClass::Space;
        if on_space {
            // whitespace + the following word
            if e < len && classify(t.char(e)) != CharClass::Eol {
                e = run(e).1;
            }
        } else if e < len && class_of(e) == CharClass::Space {
            e = run(e).1;
        } else if s > 0 && class_of(s - 1) == CharClass::Space {
            s = run(s - 1).0;
        }
    }
    for _ in 1..count.max(1) {
        // further words: skip separators like the motion would
        let mut p = e;
        while p < len && matches!(class_of(p), CharClass::Space) {
            p += 1;
        }
        if p >= len || classify(t.char(p)) == CharClass::Eol {
            break;
        }
        e = run(p).1;
        if !inner && e < len && class_of(e) == CharClass::Space {
            e = run(e).1;
        }
    }
    Some(Selection::new(s, e))
}

fn paragraph_object(t: Text, sel: Selection, inner: bool, count: usize) -> Option<Selection> {
    let nlines = t.len_lines();
    let blank = |l: usize| text::is_blank_line(t, l);
    let mut line = t.char_to_line(sel.block_cursor(t).min(t.len_chars()));
    if line >= nlines {
        return None;
    }
    let on_blank = blank(line);
    let mut start = line;
    while start > 0 && blank(start - 1) == on_blank {
        start -= 1;
    }
    let mut end = line;
    let mut paragraphs = 0;
    loop {
        while end + 1 < nlines && blank(end + 1) == on_blank {
            end += 1;
        }
        paragraphs += 1;
        if paragraphs >= count.max(1) || end + 1 >= nlines {
            break;
        }
        end += 1;
        line = end;
    }
    if !inner && !on_blank {
        // `around` also takes the blank lines that follow
        let mut e = end;
        while e + 1 < nlines && blank(e + 1) {
            e += 1;
        }
        if e == end {
            // none after: take the ones before instead
            while start > 0 && blank(start - 1) {
                start -= 1;
            }
        }
        end = e;
    }
    let s = t.line_to_char(start);
    let e = t.line_to_char((end + 1).min(nlines));
    Some(Selection::new(s, e))
}

/// Closest bracket pair enclosing or adjacent to `pos`, for bracket highlighting.
pub fn bracket_near(t: Text, pos: usize) -> Option<(usize, usize)> {
    let len = t.len_chars();
    let mut candidates = vec![];
    if pos < len {
        candidates.push(pos);
    }
    if pos > 0 {
        candidates.push(pos - 1);
    }
    for p in candidates {
        let c = t.char(p);
        if matches!(c, '(' | ')' | '[' | ']' | '{' | '}') && bracket_pair(c).is_some() {
            if let Some(m) = crate::motion::matching_bracket_at(t, p) {
                return Some((p, m));
            }
        }
    }
    None
}
