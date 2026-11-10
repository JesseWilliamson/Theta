//! Pure motions and finders on text. Nothing here knows about modes or the editor.
//!
//! Word motions re-implement Helix's observable behaviour: the motion starts from a one-grapheme
//! range on the cursor, skips line breaks, and returns the range spanning the word moved over.

use crate::selection::Selection;
use crate::text::{self, CharClass, Text, classify, is_eol};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WordKind {
    NextStart,
    NextEnd,
    PrevStart,
    PrevEnd,
}

fn boundary(a: char, b: char, long: bool) -> bool {
    let (ca, cb) = (classify(a), classify(b));
    if long {
        // WORDs are separated by whitespace only; word/punctuation transitions don't count.
        let w = |c: CharClass| matches!(c, CharClass::Word | CharClass::Punct);
        if w(ca) && w(cb) {
            return false;
        }
    }
    ca != cb
}

fn reached(kind: WordKind, prev: char, next: char, long: bool) -> bool {
    match kind {
        WordKind::NextStart | WordKind::PrevEnd => {
            boundary(prev, next, long) && (is_eol(next) || !next.is_whitespace())
        }
        WordKind::NextEnd | WordKind::PrevStart => {
            boundary(prev, next, long) && (!prev.is_whitespace() || is_eol(next))
        }
    }
}

/// One application of a word motion from `(anchor, head)`.
fn word_step(t: Text, anchor: usize, head: usize, kind: WordKind, long: bool) -> (usize, usize) {
    let len = t.len_chars();
    let back = matches!(kind, WordKind::PrevStart | WordKind::PrevEnd);
    // next char to consume in motion direction
    let peek = |pos: usize| -> Option<char> {
        if back {
            if pos == 0 { None } else { Some(t.char(pos - 1)) }
        } else if pos >= len {
            None
        } else {
            Some(t.char(pos))
        }
    };
    let advance = |pos: usize| if back { pos - 1 } else { pos + 1 };
    let (mut anchor, mut head) = (anchor, head);
    let mut prev: Option<char> = if back {
        if head < len { Some(t.char(head)) } else { None }
    } else if head > 0 {
        Some(t.char(head - 1))
    } else {
        None
    };
    // skip leading line breaks
    while let Some(c) = peek(head) {
        if is_eol(c) {
            prev = Some(c);
            head = advance(head);
        } else {
            break;
        }
    }
    if prev.is_some_and(is_eol) {
        anchor = head;
    }
    let head_start = head;
    while let Some(next) = peek(head) {
        if prev.is_none() || reached(kind, prev.unwrap(), next, long) {
            if head == head_start {
                anchor = head;
            } else {
                break;
            }
        }
        prev = Some(next);
        head = advance(head);
    }
    (anchor, head)
}

/// Helix-style word motion: returns the selection that covers the word moved over.
pub fn word_move(t: Text, sel: Selection, count: usize, kind: WordKind, long: bool) -> Selection {
    let len = t.len_chars();
    let back = matches!(kind, WordKind::PrevStart | WordKind::PrevEnd);
    if (back && sel.head == 0) || (!back && sel.head >= len) {
        return sel;
    }
    // Start from a 1-grapheme range on the cursor so the old anchor can't influence the result.
    let (mut a, mut h) = if back {
        if sel.anchor < sel.head {
            (sel.head, text::prev_grapheme(t, sel.head))
        } else {
            (text::next_grapheme(t, sel.head), sel.head)
        }
    } else if sel.anchor < sel.head {
        (text::prev_grapheme(t, sel.head), sel.head)
    } else {
        (sel.head, text::next_grapheme(t, sel.head))
    };
    for _ in 0..count.max(1) {
        let (na, nh) = word_step(t, a, h, kind, long);
        if (na, nh) == (a, h) {
            break;
        }
        a = na;
        h = nh;
    }
    Selection::new(a, h)
}

/// `f`/`t`/`F`/`T`: position of the n-th occurrence of `ch`. Returns the char index of the
/// match (for `till` the neighbouring position is returned by the caller).
pub fn find_char(t: Text, from: usize, ch: char, count: usize, forward: bool) -> Option<usize> {
    let len = t.len_chars();
    let mut n = count.max(1);
    if forward {
        let mut i = from + 1;
        while i < len {
            if t.char(i) == ch {
                n -= 1;
                if n == 0 {
                    return Some(i);
                }
            }
            i += 1;
        }
    } else {
        let mut i = from;
        while i > 0 {
            i -= 1;
            if t.char(i) == ch {
                n -= 1;
                if n == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

pub fn is_blank(t: Text, line: usize) -> bool {
    text::is_blank_line(t, line)
}

/// Line index of the next paragraph boundary start (`]p`).
pub fn next_paragraph_line(t: Text, line: usize, count: usize) -> usize {
    let last = t.len_lines().saturating_sub(1);
    let mut l = line;
    for _ in 0..count.max(1) {
        while l < last && !is_blank(t, l) {
            l += 1;
        }
        while l < last && is_blank(t, l) {
            l += 1;
        }
    }
    l
}

pub fn prev_paragraph_line(t: Text, line: usize, count: usize) -> usize {
    let mut l = line;
    for _ in 0..count.max(1) {
        let start = l;
        while l > 0 && is_blank(t, l) {
            l -= 1;
        }
        while l > 0 && !is_blank(t, l - 1) {
            l -= 1;
        }
        if l == start && l > 0 {
            // already at a paragraph start: go to the previous paragraph's start
            l -= 1;
            while l > 0 && is_blank(t, l) {
                l -= 1;
            }
            while l > 0 && !is_blank(t, l - 1) {
                l -= 1;
            }
        }
    }
    l
}

pub fn bracket_pair(c: char) -> Option<(char, char, bool)> {
    // (open, close, c_is_open)
    match c {
        '(' => Some(('(', ')', true)),
        ')' => Some(('(', ')', false)),
        '[' => Some(('[', ']', true)),
        ']' => Some(('[', ']', false)),
        '{' => Some(('{', '}', true)),
        '}' => Some(('{', '}', false)),
        '<' => Some(('<', '>', true)),
        '>' => Some(('<', '>', false)),
        _ => None,
    }
}

/// Partner of the bracket at `pos`, skipping nesting. Quotes inside strings are not tracked:
/// SQL strings in practice hold few brackets, and a lexer-aware match would need the highlighter.
pub fn matching_bracket_at(t: Text, pos: usize) -> Option<usize> {
    if pos >= t.len_chars() {
        return None;
    }
    let c = t.char(pos);
    let (open, close, is_open) = bracket_pair(c)?;
    let mut depth = 0usize;
    if is_open {
        let mut i = pos + 1;
        let len = t.len_chars();
        while i < len {
            let ch = t.char(i);
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            i += 1;
        }
    } else {
        let mut i = pos;
        while i > 0 {
            i -= 1;
            let ch = t.char(i);
            if ch == close {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
        }
    }
    None
}
