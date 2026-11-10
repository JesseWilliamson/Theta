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
    } else if c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_control() && (c as u32) < 0x3000 && !is_emoji(c)) {
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
