//! Pure Vim motions on text. These are line-for-line ports of the behaviour of Vim's
//! `fwd_word`/`bck_word`/`end_word`/`bckend_word`/`findsent`/`findpar` (the observable
//! algorithm, re-expressed over char indexes) so that corner cases such as empty lines being
//! words, `dw` stopping at the end of a line and sentence ends followed by closers agree with Vim.

use super::geom::*;
use crate::text::{self, CharClass, Text};

/// Vim's character classes (`utf_class`): 0 blank, 1 punctuation, 2 keyword, 3 emoji, and one class
/// per script for kana, ideographs and hangul so that `w` stops where the script changes.
pub(crate) fn vim_class(c: char) -> u8 {
    let u = c as u32;
    if c == ' ' || c == '\t' || c == '\0' {
        return 0;
    }
    if u < 0x80 {
        return if c.is_ascii_alphanumeric() || c == '_' { 2 } else { 1 };
    }
    match u {
        0x80..=0xff => {
            if c.is_whitespace() {
                0
            } else if (0xa1..=0xbf).contains(&u) && !matches!(u, 0xaa | 0xb5 | 0xba) || u == 0xd7 || u == 0xf7
            {
                1
            } else {
                2
            }
        }
        0x2000..=0x200b | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 => 0,
        0x200c..=0x206f
        | 0x2190..=0x25ff
        | 0x27c0..=0x2bff
        | 0x3001..=0x3020
        | 0x3030
        | 0x303d..=0x303f
        | 0xfe30..=0xfe6b => 1,
        0xff00..=0xff0f | 0xff1a..=0xff20 | 0xff3b..=0xff40 | 0xff5b..=0xff65 => 1,
        0x3040..=0x309f => 4,
        0x30a0..=0x30ff => 5,
        0x3300..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x2fa1f => 6,
        0xac00..=0xd7a3 => 7,
        0x1f000..=0x1faff | 0x2600..=0x27bf => 3,
        _ => match text::classify(c) {
            CharClass::Space | CharClass::Eol => 0,
            CharClass::Punct => 1,
            CharClass::Word => 2,
            CharClass::Other => 3,
        },
    }
}

/// A cursor that can also stand on the end-of-line NUL, like Vim's own cursor during motions.
#[derive(Clone, Copy)]
pub(crate) struct Cur<'a> {
    pub t: Text<'a>,
    pub pos: usize,
    pub big: bool,
}

impl<'a> Cur<'a> {
    pub fn new(t: Text<'a>, pos: usize, big: bool) -> Self {
        Cur { t, pos, big }
    }

    /// Vim's `inc()`: 0 = moved within the line, 2 = moved onto the NUL, 1 = moved to the next
    /// line, -1 = nowhere to go.
    pub fn inc(&mut self) -> i32 {
        let t = self.t;
        let len = t.len_chars();
        if self.pos >= len {
            return -1;
        }
        if text::is_eol(t.char(self.pos)) {
            let l = line_of(t, self.pos);
            if l + 1 >= nlines(t) {
                return -1;
            }
            self.pos = lnext(t, l);
            return 1;
        }
        let n = text::next_grapheme(t, self.pos);
        self.pos = n;
        if n >= len || text::is_eol(t.char(n)) { 2 } else { 0 }
    }

    /// Vim's `dec()`: 0 = moved within the line, 1 = moved to the NUL of the previous line.
    pub fn dec(&mut self) -> i32 {
        let t = self.t;
        let l = line_of(t, self.pos);
        let ls = lstart(t, l);
        if self.pos > ls {
            self.pos = text::prev_grapheme(t, self.pos);
            0
        } else if l > 0 {
            self.pos = lend(t, l - 1);
            1
        } else {
            -1
        }
    }

    /// `inc` that skips over the NUL (Vim's `incl`).
    pub fn incl(&mut self) -> i32 {
        let mut r = self.inc();
        if r >= 1 && self.col0() != 0 {
            r = self.inc();
        }
        r
    }

    /// `dec` that skips over the NUL (Vim's `decl`).
    pub fn decl(&mut self) -> i32 {
        let mut r = self.dec();
        if r == 1 && self.col0() != 0 {
            r = self.dec();
        }
        r
    }

    fn col0(&self) -> usize {
        self.pos - lstart(self.t, line_of(self.t, self.pos))
    }

    pub fn at_line_start(&self) -> bool {
        self.pos == lstart(self.t, line_of(self.t, self.pos))
    }

    pub fn on_empty_line(&self) -> bool {
        self.at_line_start() && line_empty(self.t, line_of(self.t, self.pos))
    }

    pub fn ch(&self) -> char {
        gchar(self.t, self.pos)
    }

    /// 0 blank/NUL, 1 punctuation (or anything for WORDs), 2 word, 3 other (emoji).
    pub fn cls(&self) -> u8 {
        let c = self.ch();
        if c == '\0' || c == ' ' || c == '\t' {
            return 0;
        }
        let k = vim_class(c);
        if k != 0 && self.big { 1 } else { k }
    }

    fn skip_chars(&mut self, class: u8, fwd: bool) -> bool {
        while self.cls() == class {
            let r = if fwd { self.inc() } else { self.dec() };
            if r == -1 {
                return true;
            }
        }
        false
    }

    pub fn back_in_line(&mut self) {
        let sclass = self.cls();
        loop {
            if self.at_line_start() {
                break;
            }
            self.dec();
            if self.cls() != sclass {
                self.inc();
                break;
            }
        }
    }
}

/// `w`/`W`. `eol`: an operator is pending, so stop at the end of the line of the last word.
/// Returns the new position and whether the motion succeeded.
pub(crate) fn fwd_word(t: Text, pos: usize, count: usize, big: bool, eol: bool) -> (usize, bool) {
    let mut c = Cur::new(t, pos, big);
    let mut count = count;
    while count > 0 {
        count -= 1;
        let sclass = c.cls();
        let last_line = line_of(t, c.pos) + 1 >= nlines(t);
        let mut i = c.inc();
        if i == -1 || (i >= 1 && last_line) {
            return (c.pos, false);
        }
        if i >= 1 && eol && count == 0 {
            return (c.pos, true);
        }
        if sclass != 0 {
            while c.cls() == sclass {
                i = c.inc();
                if i == -1 || (i >= 1 && eol && count == 0) {
                    return (c.pos, true);
                }
            }
        }
        while c.cls() == 0 {
            if c.on_empty_line() {
                break;
            }
            i = c.inc();
            if i == -1 || (i >= 1 && last_line) {
                return (c.pos, true);
            }
        }
    }
    (c.pos, true)
}

/// `b`/`B`. `stop`: used by `cw`-like callers; plain motions pass false.
pub(crate) fn bck_word(t: Text, pos: usize, count: usize, big: bool) -> (usize, bool) {
    let mut c = Cur::new(t, pos, big);
    let mut count = count;
    while count > 0 {
        count -= 1;
        let sclass = c.cls();
        if c.dec() == -1 {
            return (c.pos, false);
        }
        let _ = sclass;
        let mut finished = false;
        while c.cls() == 0 {
            if c.on_empty_line() {
                finished = true;
                break;
            }
            if c.dec() == -1 {
                return (c.pos, true);
            }
        }
        if finished {
            continue;
        }
        let cl = c.cls();
        if c.skip_chars(cl, false) {
            return (c.pos, true);
        }
        c.inc();
    }
    (c.pos, true)
}

/// `e`/`E` (and `cw`, which is `ce` with `stop`). `empty`: stop on an empty line.
pub(crate) fn end_word(
    t: Text,
    pos: usize,
    count: usize,
    big: bool,
    stop: bool,
    empty: bool,
) -> (usize, bool) {
    let mut c = Cur::new(t, pos, big);
    let mut count = count;
    let mut stop = stop;
    while count > 0 {
        count -= 1;
        let sclass = c.cls();
        if c.inc() == -1 {
            return (c.pos, false);
        }
        let mut finished = false;
        if c.cls() == sclass && sclass != 0 {
            if c.skip_chars(sclass, true) {
                return (c.pos, false);
            }
        } else if !stop || sclass == 0 {
            while c.cls() == 0 {
                if empty && c.on_empty_line() {
                    finished = true;
                    break;
                }
                if c.inc() == -1 {
                    return (c.pos, false);
                }
            }
            if !finished {
                let cl = c.cls();
                if c.skip_chars(cl, true) {
                    return (c.pos, false);
                }
            }
        }
        if !finished {
            c.dec();
        }
        stop = false;
    }
    (c.pos, true)
}

/// `ge`/`gE`.
pub(crate) fn bckend_word(t: Text, pos: usize, count: usize, big: bool) -> (usize, bool) {
    let mut c = Cur::new(t, pos, big);
    let mut count = count;
    while count > 0 {
        count -= 1;
        let sclass = c.cls();
        if c.dec() == -1 {
            return (c.pos, false);
        }
        if sclass != 0 {
            while c.cls() == sclass {
                if c.dec() == -1 {
                    return (c.pos, true);
                }
            }
        }
        while c.cls() == 0 {
            if c.on_empty_line() {
                break;
            }
            if c.dec() == -1 {
                return (c.pos, true);
            }
        }
    }
    (c.pos, true)
}

/// Start of a paragraph/section boundary line: an empty line (or a form feed).
fn start_ps(t: Text, l: usize) -> bool {
    line_empty(t, l) || gchar(t, lstart(t, l)) == '\x0c'
}

/// `{` / `}`. Returns (position, inclusive).
pub(crate) fn findpar(t: Text, pos: usize, dir_fwd: bool, count: usize) -> Option<(usize, bool)> {
    let n = nlines(t);
    let mut curr = line_of(t, pos) as isize;
    let mut count = count;
    let mut budget = 2_000usize;
    while count > 0 {
        count -= 1;
        let mut did_skip = false;
        let mut first = true;
        loop {
            // same bound as the paragraph object: a document without blank lines is one paragraph
            budget -= 1;
            if budget == 0 {
                return None;
            }
            // inlined `line_empty`/`start_ps` with `n` computed once: on a document without blank
            // lines this loop walks every line, so the per-line cost is what bounds the key
            let (ls, le) = (t.line_to_char(curr as usize), text::line_end(t, curr as usize));
            let empty = ls == le;
            if !empty {
                did_skip = true;
            }
            if !first && did_skip && (empty || t.char(ls) == '\x0c') {
                break;
            }
            curr += if dir_fwd { 1 } else { -1 };
            if curr < 0 || curr >= n as isize {
                if count > 0 {
                    return None;
                }
                curr -= if dir_fwd { 1 } else { -1 };
                break;
            }
            first = false;
        }
    }
    let curr = curr as usize;
    if curr + 1 == n && dir_fwd {
        let le = lend(t, curr);
        if le != lstart(t, curr) {
            return Some((text::prev_grapheme(t, le), true));
        }
    }
    Some((lstart(t, curr), false))
}

fn is_sent_punct(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

fn is_sent_close(c: char) -> bool {
    matches!(c, ')' | ']' | '"' | '\'')
}

/// `(` / `)`: a port of Vim's `findsent`.
pub(crate) fn findsent(t: Text, pos: usize, fwd: bool, count: usize) -> Option<usize> {
    // a text without sentence ends would otherwise be scanned to its far end on every `)`
    let mut budget = 1_500usize;
    let mut p = Cur::new(t, pos, false);
    let mut count = count;
    while count > 0 {
        count -= 1;
        let mut noskip = false;
        let mut goto_found = false;
        if p.ch() == '\0' {
            // on an empty line (or past the end): skip to a non-empty line
            loop {
                let r = if fwd { p.incl() } else { p.decl() };
                if r == -1 || p.ch() != '\0' {
                    break;
                }
            }
            if fwd {
                goto_found = true;
            }
        } else if fwd && p.at_line_start() && start_ps(t, line_of(t, p.pos)) {
            if line_of(t, p.pos) + 1 >= nlines(t) {
                return None;
            }
            p.pos = lstart(t, line_of(t, p.pos) + 1);
            goto_found = true;
        } else if !fwd {
            p.decl();
        }
        if !goto_found {
            // back to the previous non-white non-punctuation char
            let mut found_dot = false;
            loop {
                let c = p.ch();
                if !(is_blank_char(c) || is_sent_punct(c) || is_sent_close(c)) {
                    break;
                }
                let mut tp = p;
                if tp.decl() == -1 || (line_empty(t, line_of(t, tp.pos)) && fwd) {
                    break;
                }
                if found_dot {
                    break;
                }
                if is_sent_punct(c) {
                    found_dot = true;
                }
                if is_sent_close(c) && !(is_sent_punct(tp.ch()) || is_sent_close(tp.ch())) {
                    break;
                }
                p.decl();
            }
            let startl = line_of(t, p.pos);
            loop {
                let c = p.ch();
                if c == '\0' || (p.at_line_start() && start_ps(t, line_of(t, p.pos))) {
                    if !fwd && line_of(t, p.pos) != startl {
                        p.pos = lstart(t, line_of(t, p.pos) + 1);
                    }
                    break;
                }
                if is_sent_punct(c) {
                    let mut tp = p;
                    let mut r;
                    loop {
                        r = tp.inc();
                        if r == -1 || !is_sent_close(tp.ch()) {
                            break;
                        }
                    }
                    let c2 = tp.ch();
                    if r == -1 || c2 == ' ' || c2 == '\t' || c2 == '\0' {
                        p = tp;
                        if p.ch() == '\0' {
                            p.inc();
                        }
                        break;
                    }
                }
                let r = if fwd { p.incl() } else { p.decl() };
                budget -= 1;
                if budget == 0 {
                    return None;
                }
                if r == -1 {
                    if count > 0 {
                        return None;
                    }
                    noskip = true;
                    break;
                }
            }
        }
        // skip white space
        while !noskip && is_blank_char(p.ch()) {
            if p.incl() == -1 {
                break;
            }
        }
    }
    Some(p.pos)
}

/// Matching bracket for `%` and friends, skipping nested pairs.
pub(crate) fn match_bracket(t: Text, pos: usize) -> Option<usize> {
    let le = lend(t, line_of(t, pos));
    let mut p = pos;
    while p < le && !matches!(t.char(p), '(' | ')' | '[' | ']' | '{' | '}') {
        p += 1;
    }
    if p >= le {
        return None;
    }
    crate::motion::matching_bracket_at(t, p)
}

/// `[(`, `[{`, `])`, `]}`: the count-th unmatched bracket before/after `pos`.
pub(crate) fn unmatched(
    t: Text,
    pos: usize,
    open: char,
    close: char,
    back: bool,
    count: usize,
) -> Option<usize> {
    let mut pos = pos;
    let mut found = None;
    for _ in 0..count.max(1) {
        let mut depth = 0usize;
        let mut hit = None;
        if back {
            for (i, c) in crate::motion::chars_back(t, pos.saturating_sub(1)).take(crate::motion::MAX_SCAN) {
                if pos == 0 {
                    break;
                }
                if c == close {
                    depth += 1;
                } else if c == open {
                    if depth == 0 {
                        hit = Some(i);
                        break;
                    }
                    depth -= 1;
                }
            }
        } else {
            for (i, c) in crate::motion::chars_fwd(t, pos + 1).take(crate::motion::MAX_SCAN) {
                if c == open {
                    depth += 1;
                } else if c == close {
                    if depth == 0 {
                        hit = Some(i);
                        break;
                    }
                    depth -= 1;
                }
            }
        }
        pos = hit?;
        found = Some(pos);
    }
    found
}

/// `f` `t` `F` `T` within the line. `from` is the cursor; returns the target position.
pub(crate) fn find_in_line(
    t: Text,
    from: usize,
    ch: char,
    fwd: bool,
    till: bool,
    count: usize,
    skip_adjacent: bool,
) -> Option<usize> {
    let l = line_of(t, from);
    let (ls, le) = (lstart(t, l), lend(t, l));
    let mut n = count.max(1);
    if fwd {
        let mut p = text::next_grapheme(t, from);
        if till && skip_adjacent && p < le && t.char(p) == ch {
            // `;` after `tx` must not get stuck in front of the same char
            p = text::next_grapheme(t, p);
        }
        while p < le {
            if t.char(p) == ch {
                n -= 1;
                if n == 0 {
                    return Some(if till { text::prev_grapheme(t, p) } else { p });
                }
            }
            p = text::next_grapheme(t, p);
        }
        None
    } else {
        let mut p = from;
        if till && skip_adjacent && p > ls && t.char(text::prev_grapheme(t, p)) == ch {
            p = text::prev_grapheme(t, p);
        }
        while p > ls {
            p = text::prev_grapheme(t, p);
            if t.char(p) == ch {
                n -= 1;
                if n == 0 {
                    return Some(if till { text::next_grapheme(t, p) } else { p });
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn fw(s: &str, pos: usize, n: usize, eol: bool) -> usize {
        let r = Rope::from_str(s);
        fwd_word(r.slice(..), pos, n, false, eol).0
    }

    #[test]
    fn w_stops_at_empty_lines_and_eol_with_operator() {
        assert_eq!(fw("foo bar\n\nbaz\n", 4, 1, false), 8);
        assert_eq!(fw("foo bar\n\nbaz\n", 8, 1, false), 9);
        assert_eq!(fw("foo bar\nbaz\n", 4, 1, true), 7, "operator stops at the NUL of the line");
        assert_eq!(fw("foo.bar\n", 0, 1, false), 3);
    }

    #[test]
    fn e_and_b_basics() {
        let r = Rope::from_str("foo bar\nbaz");
        let t = r.slice(..);
        assert_eq!(end_word(t, 0, 1, false, false, false).0, 2);
        assert_eq!(end_word(t, 2, 1, false, false, false).0, 6);
        assert_eq!(bck_word(t, 6, 1, false).0, 4);
        assert_eq!(bck_word(t, 4, 1, false).0, 0);
        assert_eq!(bckend_word(t, 4, 1, false).0, 2);
    }

    #[test]
    fn paragraphs() {
        let r = Rope::from_str("a\nb\n\nc\n");
        let t = r.slice(..);
        assert_eq!(findpar(t, 0, true, 1), Some((4, false)));
        assert_eq!(findpar(t, 5, true, 1), Some((5, true)));
        assert_eq!(findpar(t, 5, false, 1), Some((4, false)));
    }

    #[test]
    fn sentences() {
        let r = Rope::from_str("One. Two three.  Four?\n");
        let t = r.slice(..);
        assert_eq!(findsent(t, 0, true, 1), Some(5));
        assert_eq!(findsent(t, 5, true, 1), Some(17));
        assert_eq!(findsent(t, 17, false, 1), Some(5));
    }

    #[test]
    fn find_in_line_semantics() {
        let r = Rope::from_str("a,b,c\nx,y");
        let t = r.slice(..);
        assert_eq!(find_in_line(t, 0, ',', true, false, 1, false), Some(1));
        assert_eq!(find_in_line(t, 0, ',', true, true, 1, false), Some(0));
        assert_eq!(find_in_line(t, 0, ',', true, false, 3, false), None);
        assert_eq!(find_in_line(t, 4, ',', false, false, 1, false), Some(3));
        assert_eq!(find_in_line(t, 0, ',', true, true, 1, true), Some(2));
    }
}
