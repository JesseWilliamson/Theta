//! Text helpers on top of ropey: grapheme navigation, character classes and line geometry.
//!
//! Everything here works in CHAR indexes. Lines end with `\n` or `\r\n` only (ropey is built
//! without `unicode_lines`), so `line_to_char` agrees with what a server reports.

use ropey::{Rope, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

pub type Text<'a> = RopeSlice<'a>;

/// Characters below U+0300 never combine with a neighbour except for the CR LF pair, which
/// lets the common ASCII case skip the heavyweight segmentation cursor.
#[inline]
fn simple_pair(a: char, b: char) -> bool {
    (a as u32) < 0x300 && (b as u32) < 0x300 && !(a == '\r' && b == '\n')
}

fn boundary(text: Text, char_idx: usize, forward: bool) -> usize {
    let len = text.len_chars();
    if forward {
        if char_idx >= len {
            return len;
        }
    } else if char_idx == 0 {
        return 0;
    }
    // Fast path: look at the pair straddling the candidate boundary.
    if forward {
        let a = text.char(char_idx);
        if char_idx + 1 >= len {
            return len;
        }
        if simple_pair(a, text.char(char_idx + 1)) {
            return char_idx + 1;
        }
    } else {
        let b = text.char(char_idx - 1);
        if char_idx - 1 == 0 {
            return 0;
        }
        if simple_pair(text.char(char_idx - 2), b) {
            return char_idx - 1;
        }
    }
    let byte_idx = text.char_to_byte(char_idx);
    let (mut chunk, mut chunk_byte_idx, _, _) = text.chunk_at_byte(byte_idx);
    let mut gc = GraphemeCursor::new(byte_idx, text.len_bytes(), true);
    loop {
        let res = if forward {
            gc.next_boundary(chunk, chunk_byte_idx)
        } else {
            gc.prev_boundary(chunk, chunk_byte_idx)
        };
        match res {
            Ok(None) => return if forward { len } else { 0 },
            Ok(Some(b)) => return text.byte_to_char(b),
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_byte_idx += chunk.len();
                chunk = text.chunk_at_byte(chunk_byte_idx).0;
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                let (c, i, _, _) = text.chunk_at_byte(chunk_byte_idx - 1);
                chunk = c;
                chunk_byte_idx = i;
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (ctx, ctx_start, _, _) = text.chunk_at_byte(n - 1);
                gc.provide_context(ctx, ctx_start);
            }
            Err(_) => return if forward { len } else { 0 },
        }
    }
}

pub fn next_grapheme(text: Text, pos: usize) -> usize {
    boundary(text, pos, true)
}

pub fn prev_grapheme(text: Text, pos: usize) -> usize {
    boundary(text, pos, false)
}

pub fn nth_next_grapheme(text: Text, mut pos: usize, n: usize) -> usize {
    for _ in 0..n {
        let p = next_grapheme(text, pos);
        if p == pos {
            break;
        }
        pos = p;
    }
    pos
}

pub fn nth_prev_grapheme(text: Text, mut pos: usize, n: usize) -> usize {
    for _ in 0..n {
        let p = prev_grapheme(text, pos);
        if p == pos {
            break;
        }
        pos = p;
    }
    pos
}

/// Snap to the nearest grapheme boundary at or before `pos`.
pub fn floor_grapheme(text: Text, pos: usize) -> usize {
    let pos = pos.min(text.len_chars());
    if pos == 0 || pos == text.len_chars() {
        return pos;
    }
    let p = prev_grapheme(text, next_grapheme(text, pos));
    if p > pos { prev_grapheme(text, pos) } else { p }
}

pub fn is_eol(c: char) -> bool {
    c == '\n' || c == '\r'
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    Eol,
    Space,
    Word,
    Punct,
    Other,
}

pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || is_combining(c)
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F | 0x200D | 0xFE0F)
}

pub fn classify(c: char) -> CharClass {
    if is_eol(c) {
        CharClass::Eol
    } else if c.is_whitespace() {
        CharClass::Space
    } else if is_word_char(c) {
        CharClass::Word
    } else if c.is_ascii_punctuation()
        || (!c.is_alphanumeric() && !c.is_control() && (c as u32) < 0x3000 && !is_emoji(c))
    {
        CharClass::Punct
    } else {
        CharClass::Other
    }
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF)
}

/// Length in chars of the line terminator at the end of `line` (0 for the last line).
pub fn eol_len(text: Text, line: usize) -> usize {
    if line + 1 >= text.len_lines() {
        return 0;
    }
    let l = text.line(line);
    let n = l.len_chars();
    if n >= 2 && l.char(n - 2) == '\r' && l.char(n - 1) == '\n' {
        2
    } else if n >= 1 && is_eol(l.char(n - 1)) {
        1
    } else {
        0
    }
}

/// Char index where the line's content ends (just before its terminator).
pub fn line_end(text: Text, line: usize) -> usize {
    let line = line.min(text.len_lines().saturating_sub(1));
    text.line_to_char(line) + text.line(line).len_chars() - eol_len(text, line)
}

/// The last line index that holds content: when the buffer ends in a newline the final empty
/// "line" is not a place the user can go to with `G`/`ge`.
pub fn last_content_line(text: Text) -> usize {
    let n = text.len_lines();
    if n > 1 && text.line(n - 1).len_chars() == 0 { n - 2 } else { n - 1 }
}

pub fn line_content(text: Text, line: usize) -> String {
    if line >= text.len_lines() {
        return String::new();
    }
    let s = text.line_to_char(line);
    text.slice(s..line_end(text, line)).to_string()
}

pub fn first_non_blank(text: Text, line: usize) -> Option<usize> {
    let s = text.line_to_char(line);
    let e = line_end(text, line);
    (s..e).find(|&i| {
        let c = text.char(i);
        c != ' ' && c != '\t'
    })
}

pub fn leading_whitespace(text: Text, line: usize) -> String {
    let s = text.line_to_char(line);
    let e = line_end(text, line);
    let mut out = String::new();
    for i in s..e {
        let c = text.char(i);
        if c == ' ' || c == '\t' {
            out.push(c);
        } else {
            break;
        }
    }
    out
}

/// Detect the document's line ending: CRLF if the first terminator is CRLF, else LF.
pub fn detect_line_ending(rope: &Rope) -> &'static str {
    if rope.len_lines() > 1 && eol_len(rope.slice(..), 0) == 2 { "\r\n" } else { "\n" }
}

pub fn is_blank_line(text: Text, line: usize) -> bool {
    first_non_blank(text, line).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn graphemes_ascii_combining_crlf_and_emoji() {
        let rope = r("ae\u{301}b\r\n🙂x");
        let t = rope.slice(..);
        assert_eq!(next_grapheme(t, 0), 1);
        assert_eq!(next_grapheme(t, 1), 3, "e + combining acute");
        assert_eq!(prev_grapheme(t, 3), 1);
        assert_eq!(next_grapheme(t, 4), 6, "CRLF is one grapheme");
        assert_eq!(prev_grapheme(t, 6), 4);
        assert_eq!(next_grapheme(t, 6), 7);
        assert_eq!(next_grapheme(t, 99), t.len_chars());
        assert_eq!(prev_grapheme(t, 0), 0);
        assert_eq!(floor_grapheme(t, 2), 1);
        assert_eq!(floor_grapheme(t, 5), 4);
    }

    #[test]
    fn zwj_sequences_are_single_graphemes() {
        let rope = r("👨\u{200d}👩\u{200d}👧!");
        let t = rope.slice(..);
        assert_eq!(next_grapheme(t, 0), 5);
        assert_eq!(prev_grapheme(t, 5), 0);
    }

    #[test]
    fn line_geometry_with_and_without_final_newline() {
        let rope = r("ab\r\ncd\n\nef");
        let t = rope.slice(..);
        assert_eq!(eol_len(t, 0), 2);
        assert_eq!(eol_len(t, 1), 1);
        assert_eq!(eol_len(t, 3), 0);
        assert_eq!(line_end(t, 0), 2);
        assert_eq!(line_end(t, 2), 7);
        assert_eq!(line_end(t, 3), 10);
        assert_eq!(last_content_line(t), 3);
        let with_nl = r("a\n");
        assert_eq!(last_content_line(with_nl.slice(..)), 0);
        assert_eq!(last_content_line(r("").slice(..)), 0);
        assert_eq!(detect_line_ending(&rope), "\r\n");
        assert_eq!(detect_line_ending(&r("a\nb\r\n")), "\n");
    }

    #[test]
    fn lone_cr_is_not_a_line_break() {
        let rope = r("a\rb");
        assert_eq!(rope.len_lines(), 1);
    }

    #[test]
    fn char_classes() {
        assert_eq!(classify('a'), CharClass::Word);
        assert_eq!(classify('_'), CharClass::Word);
        assert_eq!(classify('日'), CharClass::Word);
        assert_eq!(classify(' '), CharClass::Space);
        assert_eq!(classify('\n'), CharClass::Eol);
        assert_eq!(classify('.'), CharClass::Punct);
        assert_eq!(classify('🙂'), CharClass::Other);
    }

    #[test]
    fn first_non_blank_and_indent() {
        let rope = r("  \tab\n   \nx");
        let t = rope.slice(..);
        assert_eq!(first_non_blank(t, 0), Some(3));
        assert_eq!(first_non_blank(t, 1), None);
        assert_eq!(leading_whitespace(t, 0), "  \t");
        assert!(is_blank_line(t, 1));
        assert_eq!(line_content(t, 2), "x");
        assert_eq!(line_content(t, 9), "");
    }
}
