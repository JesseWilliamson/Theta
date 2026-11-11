//! Line geometry in Vim's terms.
//!
//! A Vim buffer is a list of lines; a trailing line terminator is the end of the last line, not
//! an extra empty line. The rope disagrees ("a\n" has two rope lines), so everything here maps
//! rope lines onto Vim lines: `nlines` never counts the phantom empty line after a final
//! terminator. All positions are char indexes on grapheme starts.
//!
//! "NUL position" below means the place where Vim's cursor can stand on the end-of-line NUL:
//! the first char of the terminator, or the end of the text for an unterminated last line.

use crate::text::{self, Text};

/// Number of Vim lines (an empty buffer has one empty line).
pub(crate) fn nlines(t: Text) -> usize {
    text::last_content_line(t) + 1
}

pub(crate) fn lstart(t: Text, l: usize) -> usize {
    t.line_to_char(l.min(nlines(t) - 1))
}

/// End of the line's content (the NUL position).
pub(crate) fn lend(t: Text, l: usize) -> usize {
    text::line_end(t, l.min(nlines(t) - 1))
}

/// Start of the following line, or the end of the text for the last line.
pub(crate) fn lnext(t: Text, l: usize) -> usize {
    let l = l.min(nlines(t) - 1);
    if l + 1 < t.len_lines() { t.line_to_char(l + 1) } else { t.len_chars() }
}

/// Does `l` end in a line terminator? (False only for an unterminated last line.)
pub(crate) fn has_term(t: Text, l: usize) -> bool {
    text::eol_len(t, l.min(nlines(t) - 1)) > 0 || lnext(t, l) > lend(t, l)
}

pub(crate) fn line_of(t: Text, pos: usize) -> usize {
    t.char_to_line(pos.min(t.len_chars())).min(nlines(t) - 1)
}

pub(crate) fn line_empty(t: Text, l: usize) -> bool {
    lstart(t, l) == lend(t, l)
}

/// The char at `pos`, or NUL at the end of a line / of the text.
pub(crate) fn gchar(t: Text, pos: usize) -> char {
    if pos >= t.len_chars() {
        return '\0';
    }
    let c = t.char(pos);
    if text::is_eol(c) { '\0' } else { c }
}

/// Tab stops are four columns apart (the editor indents with four spaces).
pub(crate) const TAB_WIDTH: usize = 4;

/// Display width of the grapheme starting at `p` when it begins at column `col`.
fn cell_width(t: Text, p: usize, col: usize) -> usize {
    let c = t.char(p);
    if c == '\t' {
        TAB_WIDTH - col % TAB_WIDTH
    } else if is_wide(c) {
        2
    } else {
        1
    }
}

/// East Asian wide / fullwidth characters and emoji take two columns.
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115f | 0x2e80..=0x303e | 0x3041..=0x33ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xa000..=0xa4cf
        | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f | 0x1f900..=0x1f9ff | 0x20000..=0x3fffd)
}

/// Display column (0-based, tabs expanded) where the char at `pos` starts.
pub(crate) fn col_of(t: Text, pos: usize) -> usize {
    let le = lend(t, line_of(t, pos));
    let mut p = lstart(t, line_of(t, pos));
    let mut col = 0;
    while p < pos && p < le {
        col += cell_width(t, p, col);
        let q = text::next_grapheme(t, p);
        if q == p {
            break;
        }
        p = q;
    }
    col
}

/// The last display column the char at `pos` covers (a tab spans several).
pub(crate) fn col_end_of(t: Text, pos: usize) -> usize {
    let start = col_of(t, pos);
    if pos < lend(t, line_of(t, pos)) { start + cell_width(t, pos, start) - 1 } else { start }
}

/// The column Vim remembers for vertical moves: Normal mode puts the cursor at the end of a tab.
pub(crate) fn want_col(t: Text, pos: usize, at_end_of_tab: bool) -> usize {
    if at_end_of_tab && pos < t.len_chars() && t.char(pos) == '\t' {
        col_end_of(t, pos)
    } else {
        col_of(t, pos)
    }
}

/// Position of the char covering display column `col` of line `l`; the NUL position when the
/// line is shorter.
pub(crate) fn pos_at_col(t: Text, l: usize, col: usize) -> usize {
    let le = lend(t, l);
    let mut p = lstart(t, l);
    let mut c = 0;
    while p < le {
        let w = cell_width(t, p, c);
        if col < c + w {
            return p;
        }
        c += w;
        let q = text::next_grapheme(t, p);
        if q == p {
            break;
        }
        p = q;
    }
    le
}

/// Number of graphemes in `a..b`.
pub(crate) fn count_graphemes(t: Text, a: usize, b: usize) -> usize {
    let mut p = a;
    let mut n = 0;
    while p < b {
        let q = text::next_grapheme(t, p);
        if q == p {
            break;
        }
        p = q;
        n += 1;
    }
    n
}

/// Last grapheme of the line's content, or the line start when it is empty.
pub(crate) fn last_char(t: Text, l: usize) -> usize {
    let (ls, le) = (lstart(t, l), lend(t, l));
    if le > ls { text::prev_grapheme(t, le) } else { ls }
}

/// A Normal-mode cursor cannot rest on the NUL of a non-empty line.
pub(crate) fn clamp_normal(t: Text, pos: usize) -> usize {
    let len = t.len_chars();
    let pos = pos.min(len);
    let l = line_of(t, pos);
    let (ls, le) = (lstart(t, l), lend(t, l));
    if le == ls {
        ls
    } else if pos >= le {
        text::prev_grapheme(t, le)
    } else {
        text::floor_grapheme(t, pos)
    }
}

pub(crate) fn first_nonblank(t: Text, l: usize) -> usize {
    text::first_non_blank(t, l).unwrap_or_else(|| last_char(t, l))
}

/// Position just after the char at `p` for an inclusive end: a NUL position includes its
/// terminator (that is how `v$y` yanks the newline).
pub(crate) fn after_incl(t: Text, p: usize) -> usize {
    let len = t.len_chars();
    if p >= len {
        return len;
    }
    if text::is_eol(t.char(p)) {
        let l = line_of(t, p);
        if p == lend(t, l) {
            // the break of the last line is not there to be included
            if l + 1 >= nlines(t) { p } else { lnext(t, l) }
        } else {
            text::next_grapheme(t, p)
        }
    } else {
        text::next_grapheme(t, p)
    }
}

pub(crate) fn is_blank_char(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// Leading whitespace width of `l` in columns (a tab counts 4: the editor indents with 4).
pub(crate) fn indent_width(t: Text, l: usize) -> usize {
    let (ls, le) = (lstart(t, l), lend(t, l));
    let mut w = 0;
    for i in ls..le {
        match t.char(i) {
            ' ' => w += 1,
            '\t' => w += 4 - w % 4,
            _ => break,
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn trailing_newline_is_not_a_line() {
        let r = Rope::from_str("a\nb\n");
        let t = r.slice(..);
        assert_eq!(nlines(t), 2);
        assert_eq!(lnext(t, 1), 4);
        assert!(has_term(t, 1));
        let r = Rope::from_str("a\nb");
        let t = r.slice(..);
        assert_eq!(nlines(t), 2);
        assert!(!has_term(t, 1));
        assert!(has_term(t, 0));
        assert_eq!(nlines(Rope::from_str("").slice(..)), 1);
    }

    #[test]
    fn clamping_and_columns() {
        let r = Rope::from_str("abc\n\nde");
        let t = r.slice(..);
        assert_eq!(clamp_normal(t, 3), 2);
        assert_eq!(clamp_normal(t, 4), 4);
        assert_eq!(clamp_normal(t, 7), 6);
        assert_eq!(col_of(t, 6), 1);
        assert_eq!(pos_at_col(t, 0, 9), 3);
        assert_eq!(after_incl(t, 3), 4);
    }
}
