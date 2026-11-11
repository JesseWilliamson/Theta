//! Vim text objects: `iw aw iW aW is as ip ap i( a( ib ab i[ a[ i{ a{ iB aB i< a< i' a' i" a" i``.
//!
//! The word, block and quote objects follow Vim's own algorithms (`current_word`,
//! `current_block`, `current_quote`) so that the quirks users rely on carry over: `aw` on the
//! last word of a line takes the whitespace in front, `i{` on a multi-line block selects the
//! lines between the braces, `i"` between two strings selects the text between them.

use super::geom::*;
use super::motion::*;
use super::nav::MK;
use crate::text::{self, Text};

pub(crate) fn is_object_char(c: char) -> bool {
    matches!(
        c,
        'w' | 'W' | 's' | 'p' | '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' | '\'' | '"' | '`'
    )
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Obj {
    pub start: usize,
    /// Inclusive position for `MK::Incl`, exclusive for `MK::Excl`, a position inside the last
    /// line for `MK::Line`.
    pub end: usize,
    pub kind: MK,
}

/// `sel`: the Visual selection (low, high) when it already spans more than the cursor.
pub(crate) fn find(
    t: Text,
    cur: usize,
    ch: char,
    inner: bool,
    count: usize,
    sel: Option<(usize, usize)>,
) -> Option<Obj> {
    find_or_cursor(t, cur, ch, inner, count, sel).ok()
}

/// Like `find`, but a failed word object reports where Vim leaves the cursor.
pub(crate) fn find_or_cursor(
    t: Text,
    cur: usize,
    ch: char,
    inner: bool,
    count: usize,
    sel: Option<(usize, usize)>,
) -> Result<Obj, Option<usize>> {
    let count = count.max(1);
    let r = match ch {
        'w' | 'W' => return word(t, cur, ch == 'W', !inner, count, sel),
        _ => find_other(t, cur, ch, inner, count, sel),
    };
    r.ok_or(None)
}

fn find_other(
    t: Text,
    cur: usize,
    ch: char,
    inner: bool,
    count: usize,
    sel: Option<(usize, usize)>,
) -> Option<Obj> {
    match ch {
        's' => sentence(t, cur, !inner, count),
        'p' => paragraph(t, cur, !inner, count, sel),
        '(' | ')' | 'b' => block(t, cur, '(', ')', !inner, count, sel),
        '[' | ']' => block(t, cur, '[', ']', !inner, count, sel),
        '{' | '}' | 'B' => block(t, cur, '{', '}', !inner, count, sel),
        '<' | '>' => block(t, cur, '<', '>', !inner, count, sel),
        '\'' | '"' | '`' => quote(t, cur, ch, !inner, count),
        _ => None,
    }
}

fn word(
    t: Text,
    cur: usize,
    big: bool,
    include: bool,
    count: usize,
    sel: Option<(usize, usize)>,
) -> Result<Obj, Option<usize>> {
    let mut count = count;
    let mut c = Cur::new(t, cur, big);
    let mut inclusive = true;
    let mut include_white = false;
    let start;
    match sel {
        None => {
            c.back_in_line();
            start = c.pos;
            if (c.cls() == 0) == include {
                let (p, ok) = end_word(t, c.pos, 1, big, true, true);
                if !ok {
                    return Err(Some(p));
                }
                c.pos = p;
            } else {
                let (p, _) = fwd_word(t, c.pos, 1, big, true);
                c.pos = p;
                if c.at_line_start() {
                    c.decl();
                } else {
                    c.pos = text::prev_grapheme(t, c.pos);
                }
                if include {
                    include_white = true;
                }
            }
            count -= 1;
        }
        Some((lo, hi)) => {
            // extend an existing selection forward
            start = lo;
            c.pos = hi;
        }
    }
    while count > 0 {
        inclusive = true;
        if c.incl() == -1 {
            return Err(Some(c.pos));
        }
        if include != (c.cls() == 0) {
            let (p, ok) = fwd_word(t, c.pos, 1, big, true);
            if !ok && count > 1 {
                return Err(Some(p));
            }
            c.pos = p;
            if c.at_line_start() {
                inclusive = false;
            } else {
                c.pos = text::prev_grapheme(t, c.pos);
            }
        } else {
            let (p, ok) = end_word(t, c.pos, 1, big, true, true);
            if !ok {
                return Err(Some(p));
            }
            c.pos = p;
        }
        count -= 1;
    }
    let mut start = start;
    if include_white && (c.cls() != 0 || (c.at_line_start() && !inclusive)) {
        // no white space after the word: take the white space in front instead (but not indent)
        let mut s = Cur::new(t, start, big);
        if !s.at_line_start() {
            s.pos = text::prev_grapheme(t, start);
            s.back_in_line();
            if s.cls() == 0 && !s.at_line_start() {
                start = s.pos;
            }
        }
    }
    Ok(Obj { start, end: c.pos, kind: if inclusive { MK::Incl } else { MK::Excl } })
}

/// `is` / `as`: a port of Vim's `current_sent`.
fn sentence(t: Text, cur: usize, include: bool, count: usize) -> Option<Obj> {
    let len = t.len_chars();
    if len == 0 {
        return None;
    }
    let mut start_pos = cur;
    let mut c = Cur::new(t, cur, false);
    if let Some(p) = findsent(t, c.pos, true, 1) {
        c.pos = p;
    }
    // if the cursor started on a blank, is it just before the start of the next sentence?
    let mut pos = Cur::new(t, start_pos, false);
    while is_blank_char(pos.ch()) {
        pos.incl();
    }
    let start_blank;
    if pos.pos == c.pos {
        start_blank = true;
        find_first_blank(t, &mut start_pos);
    } else {
        start_blank = false;
        if let Some(p) = findsent(t, c.pos, false, 1) {
            c.pos = p;
        }
        start_pos = c.pos;
    }
    let ncount = if include { count * 2 } else { count.saturating_sub(usize::from(start_blank)) };
    if ncount > 0 {
        findsent_forward(t, &mut c, ncount, true);
    } else {
        c.decl();
    }
    if include {
        if start_blank {
            find_first_blank(t, &mut c.pos);
            if is_blank_char(gchar(t, c.pos)) {
                c.decl();
            }
        } else if !is_blank_char(c.ch()) {
            find_first_blank(t, &mut start_pos);
        }
    }
    // include a newline after the sentence, if there is one
    let r = c.incl();
    Some(Obj { start: start_pos, end: c.pos, kind: if r == -1 { MK::Incl } else { MK::Excl } })
}

fn find_first_blank(t: Text, pos: &mut usize) {
    let mut c = Cur::new(t, *pos, false);
    while c.decl() != -1 {
        if !is_blank_char(c.ch()) {
            c.incl();
            break;
        }
    }
    *pos = c.pos;
}

fn findsent_forward(t: Text, c: &mut Cur, count: usize, at_start_sent: bool) {
    let mut count = count;
    let mut at_start = at_start_sent;
    while count > 0 {
        count -= 1;
        if let Some(p) = findsent(t, c.pos, true, 1) {
            c.pos = p;
        }
        if at_start {
            find_first_blank(t, &mut c.pos);
        }
        if count == 0 || at_start {
            c.decl();
        }
        at_start = !at_start;
    }
}

fn linewhite(t: Text, l: usize) -> bool {
    first_nonblank_opt(t, l).is_none()
}

fn first_nonblank_opt(t: Text, l: usize) -> Option<usize> {
    text::first_non_blank(t, l)
}

fn paragraph(t: Text, cur: usize, include: bool, count: usize, sel: Option<(usize, usize)>) -> Option<Obj> {
    let nl = nlines(t) as isize;
    let _ = sel;
    let line = line_of(t, cur) as isize;
    let white = |l: isize| -> bool { l >= 0 && l < nl && linewhite(t, l as usize) };
    let start_ps_line = |l: isize| -> bool { line_empty(t, l as usize) };
    let white_in_front = white(line);
    let mut start = line;
    while start > 0 {
        if white_in_front {
            if !white(start - 1) {
                break;
            }
        } else if white(start - 1) || start_ps_line(start) {
            break;
        }
        start -= 1;
    }
    let mut end = start;
    while end < nl && white(end) {
        end += 1;
    }
    end -= 1;
    let mut i = count as isize;
    if !include && white_in_front {
        i -= 1;
    }
    let mut do_white = false;
    while i > 0 {
        i -= 1;
        if end == nl - 1 {
            return None;
        }
        if !include {
            do_white = white(end + 1);
        }
        if include || !do_white {
            end += 1;
            while end < nl - 1 && !white(end + 1) && !start_ps_line(end + 1) {
                end += 1;
            }
        }
        if i == 0 && white_in_front && include {
            break;
        }
        if include || do_white {
            while end < nl - 1 && white(end + 1) {
                end += 1;
            }
        }
    }
    if !white_in_front && !white(end) && include {
        while start > 0 && white(start - 1) {
            start -= 1;
        }
    }
    if end < start {
        return None;
    }
    Some(Obj { start: lstart(t, start as usize), end: lstart(t, end as usize), kind: MK::Line })
}

fn in_indent(t: Text, pos: usize, extra: usize) -> bool {
    let l = line_of(t, pos);
    let ls = lstart(t, l);
    let mut col = 0;
    while ls + col < lend(t, l) && is_blank_char(t.char(ls + col)) {
        col += 1;
    }
    col >= (pos - ls) + extra
}

fn unmatched_back(t: Text, from: usize, open: char, close: char) -> Option<usize> {
    if from == 0 {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in crate::motion::chars_back(t, from - 1).take(crate::motion::MAX_SCAN) {
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
    }
    None
}

fn unmatched_fwd_open(t: Text, from: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in crate::motion::chars_fwd(t, from).take(crate::motion::MAX_SCAN) {
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
    }
    None
}

fn match_fwd(t: Text, open_pos: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in crate::motion::chars_fwd(t, open_pos + 1).take(crate::motion::MAX_SCAN) {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
    }
    None
}

fn block(
    t: Text,
    cur: usize,
    open: char,
    close: char,
    include: bool,
    count: usize,
    sel: Option<(usize, usize)>,
) -> Option<Obj> {
    let len = t.len_chars();
    let mut origin = cur;
    let (old_start, old_end) = match sel {
        Some((lo, hi)) => {
            origin = lo;
            (lo, hi)
        }
        None => {
            if open == '{' {
                // ignore indent
                let mut p = origin;
                while p < len && in_indent(t, p, 1) {
                    let mut c = Cur::new(t, p, false);
                    if c.inc() != 0 {
                        break;
                    }
                    p = c.pos;
                }
                origin = p;
            }
            if origin < len && t.char(origin) == open {
                origin += 1;
            }
            (usize::MAX, 0)
        }
    };
    let mut start = None;
    let mut p = origin;
    let enclosed = unmatched_back(t, p, open, close).is_some();
    for _ in 0..count {
        let s = if enclosed {
            unmatched_back(t, p, open, close)?
        } else {
            // not inside a block: Vim looks for the next opening bracket that no closer
            // before it has cancelled
            unmatched_fwd_open(t, p, open, close)?
        };
        start = Some(s);
        p = if enclosed { s } else { s + 1 };
    }
    let mut start_pos = start?;
    let mut end_pos = match_fwd(t, start_pos, open, close)?;
    if include {
        return Some(Obj { start: start_pos, end: end_pos, kind: MK::Incl });
    }
    loop {
        let mut s = Cur::new(t, start_pos, false);
        s.incl();
        let sp = s.pos;
        let mut c = Cur::new(t, end_pos, false);
        let mut sol = c.at_line_start();
        c.decl();
        while in_indent(t, c.pos, 1) {
            sol = true;
            if c.decl() != 0 {
                break;
            }
        }
        // in Visual mode an unchanged area extends to the next enclosing block
        let bigger_ok = sel.is_none() || sp < old_start || old_end < c.pos || sp == c.pos;
        if !bigger_ok {
            let o = unmatched_back(t, start_pos, open, close)?;
            start_pos = o;
            end_pos = match_fwd(t, o, open, close)?;
            continue;
        }
        let mut endc = c;
        if sol {
            endc.incl();
            return Some(Obj { start: sp, end: endc.pos, kind: MK::Excl });
        }
        if sp <= endc.pos {
            return Some(Obj { start: sp, end: endc.pos, kind: MK::Incl });
        }
        // nothing between the brackets
        return Some(Obj { start: sp, end: sp, kind: MK::Excl });
    }
}

fn quote(t: Text, cur: usize, q: char, include: bool, count: usize) -> Option<Obj> {
    let l = line_of(t, cur);
    let (ls, le) = (lstart(t, l), lend(t, l));
    let line: Vec<char> = (ls..le).map(|i| t.char(i)).collect();
    let n = line.len();
    if cur >= le {
        return None;
    }
    let col = cur - ls;
    let esc = '\\';
    let find_next = |mut c: usize, use_esc: bool| -> Option<usize> {
        loop {
            let ch = *line.get(c)?;
            if use_esc && ch == esc {
                c += 1;
                line.get(c)?;
            } else if ch == q {
                return Some(c);
            }
            c += 1;
        }
    };
    let find_prev = |mut c: usize| -> usize {
        while c > 0 {
            c -= 1;
            let mut k = 0;
            while c > k && line[c - k - 1] == esc {
                k += 1;
            }
            if k % 2 == 1 {
                c -= k;
            } else if line[c] == q {
                break;
            }
        }
        c
    };
    let first_col = col;
    let (cs, ce);
    if line[col] == q {
        let mut s = 0;
        loop {
            s = find_next(s, false)?;
            if s > first_col {
                return None;
            }
            let e = find_next(s + 1, true)?;
            if s <= first_col && first_col <= e {
                cs = s;
                ce = e;
                break;
            }
            s = e + 1;
        }
    } else {
        let mut s = find_prev(col);
        if line[s] != q {
            s = find_next(s, false)?;
        }
        cs = s;
        ce = find_next(s + 1, true)?;
    }
    let (mut cs, mut ce) = (cs, ce);
    let white = |c: usize| line.get(c).is_some_and(|&x| is_blank_char(x));
    if include {
        if white(ce + 1) {
            while white(ce + 1) {
                ce += 1;
            }
        } else {
            while cs > 0 && white(cs - 1) {
                cs -= 1;
            }
        }
    }
    let mut start = cs;
    if !include && count < 2 {
        start += 1;
    }
    let end_excl = if include || count > 1 { ce + 1 } else { ce };
    let _ = n;
    Some(Obj { start: ls + start, end: ls + end_excl, kind: MK::Excl })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn obj(s: &str, cur: usize, ch: char, inner: bool) -> Option<(usize, usize, MK)> {
        let r = Rope::from_str(s);
        find(r.slice(..), cur, ch, inner, 1, None).map(|o| (o.start, o.end, o.kind))
    }

    #[test]
    fn words() {
        assert_eq!(obj("foo bar baz", 5, 'w', true), Some((4, 6, MK::Incl)));
        assert_eq!(obj("foo bar baz", 5, 'w', false), Some((4, 7, MK::Incl)));
        assert_eq!(
            obj("foo bar", 5, 'w', false),
            Some((3, 6, MK::Incl)),
            "aw at the end takes the space before"
        );
    }

    #[test]
    fn quotes_and_blocks() {
        assert_eq!(obj("x \"ab\" y", 4, '"', true), Some((3, 5, MK::Excl)));
        assert_eq!(obj("x \"ab\" y", 4, '"', false), Some((2, 7, MK::Excl)));
        assert_eq!(obj("f(a, b)", 3, '(', true), Some((2, 5, MK::Incl)));
        assert_eq!(obj("f(a, b)", 3, '(', false), Some((1, 6, MK::Incl)));
        assert_eq!(obj("f()", 1, '(', true), Some((2, 2, MK::Excl)));
    }

    #[test]
    fn multi_line_block_is_linewise_inner() {
        let o = obj("{\n  a\n}\n", 4, '{', true).unwrap();
        assert_eq!((o.0, o.1, o.2), (2, 6, MK::Excl));
    }

    #[test]
    fn paragraphs() {
        let o = obj("a\nb\n\nc\n", 0, 'p', true).unwrap();
        assert_eq!((o.0, o.1, o.2), (0, 2, MK::Line));
        let o = obj("a\nb\n\nc\n", 0, 'p', false).unwrap();
        assert_eq!((o.0, o.1, o.2), (0, 4, MK::Line));
    }
}
