//! Motion evaluation, marks, the jumplist and searching.

use super::geom::*;
use super::motion::*;
use super::{Motion, Op, SearchOp};
use crate::editor::{Editor, Effect, Mode};
use crate::prompt::PromptKind;
use crate::text;
use crate::transaction::{Assoc, Transaction};
use regex::Regex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MK {
    Excl,
    Incl,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Want {
    /// the column of the new position becomes the desired column
    Reset,
    /// vertical motion: keep the desired column
    Keep,
    /// `$`: stick to the end of every line
    Eol,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Mv {
    pub pos: usize,
    pub kind: MK,
    pub jump: bool,
    pub want: Want,
}

fn mv(pos: usize, kind: MK) -> Option<Mv> {
    Some(Mv { pos, kind, jump: false, want: Want::Reset })
}

fn jmp(pos: usize, kind: MK) -> Option<Mv> {
    Some(Mv { pos, kind, jump: true, want: Want::Reset })
}

impl Editor {
    /// Evaluate `m` from the cursor. `op` is the pending operator (it changes `w`, `l`, ...).
    pub(crate) fn eval_motion(&mut self, m: Motion, count: Option<usize>, op: Option<Op>) -> Option<Mv> {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let n = count.unwrap_or(1);
        let line = line_of(t, cur);
        let nl = nlines(t);
        let visual = self.vim.vis.is_some();
        match m {
            Motion::Left => {
                let ls = lstart(t, line);
                if cur <= ls {
                    // `dh` / `yh` at the start of a line act on an empty region
                    return if op.is_some() { mv(cur, MK::Excl) } else { None };
                }
                mv(text::nth_prev_grapheme(t, cur, n).max(ls), MK::Excl)
            }
            Motion::Right => {
                let le = lend(t, line);
                if op.is_some() {
                    if cur >= le {
                        return mv(cur, MK::Excl);
                    }
                    return mv(text::nth_next_grapheme(t, cur, n).min(le), MK::Excl);
                }
                if visual {
                    // Visual mode may move onto the line end
                    if cur >= le {
                        return None;
                    }
                    return mv(text::nth_next_grapheme(t, cur, n).min(le), MK::Excl);
                }
                let last = last_char(t, line);
                if cur >= last {
                    return None;
                }
                mv(text::nth_next_grapheme(t, cur, n).min(last), MK::Excl)
            }
            Motion::BackspaceH => {
                let mut p = cur;
                for _ in 0..n {
                    let l = line_of(t, p);
                    if p > lstart(t, l) {
                        p = text::prev_grapheme(t, p);
                    } else if l > 0 {
                        p = last_char(t, l - 1);
                    } else {
                        break;
                    }
                }
                if p == cur { None } else { mv(p, MK::Excl) }
            }
            Motion::Down | Motion::Up => {
                let down = m == Motion::Down;
                let target = if down {
                    if line + 1 >= nl {
                        return None;
                    }
                    (line + n).min(nl - 1)
                } else {
                    if line == 0 {
                        return None;
                    }
                    line.saturating_sub(n)
                };
                let want = self.vim.want;
                let pos = if want == usize::MAX {
                    if visual && has_term(t, target) { lend(t, target) } else { last_char(t, target) }
                } else if visual {
                    pos_at_col(t, target, want)
                } else {
                    clamp_normal(t, pos_at_col(t, target, want))
                };
                Some(Mv { pos, kind: MK::Line, jump: false, want: Want::Keep })
            }
            Motion::LineStart => mv(lstart(t, line), MK::Excl),
            Motion::FirstNonBlank => mv(first_nonblank(t, line), MK::Excl),
            Motion::LineEnd => {
                if n > 1 && line + 1 >= nl {
                    return None;
                }
                let l = (line + n - 1).min(nl - 1);
                let want = Want::Eol;
                if line_empty(t, l) {
                    return Some(Mv {
                        pos: lstart(t, l),
                        kind: if l == line { MK::Excl } else { MK::Incl },
                        jump: false,
                        want,
                    });
                }
                let pos = if visual && has_term(t, l) { lend(t, l) } else { last_char(t, l) };
                Some(Mv { pos, kind: MK::Incl, jump: false, want })
            }
            Motion::LastNonBlank => {
                if n > 1 && line + 1 >= nl {
                    return None;
                }
                let l = (line + n - 1).min(nl - 1);
                let (ls, le) = (lstart(t, l), lend(t, l));
                let mut p = le;
                while p > ls && is_blank_char(t.char(text::prev_grapheme(t, p))) {
                    p = text::prev_grapheme(t, p);
                }
                let pos = if p > ls { text::prev_grapheme(t, p) } else { ls };
                mv(pos, if line_empty(t, l) { MK::Excl } else { MK::Incl })
            }
            Motion::Column => {
                let pos = pos_at_col(t, line, n - 1);
                mv(if op.is_some() { pos } else { clamp_normal(t, pos) }, MK::Excl)
            }
            Motion::WordFwd(big) => {
                if op == Some(Op::Change) && !self.cw_on_blank(t, cur) {
                    let (p, ok) = end_word(t, cur, n, big, true, false);
                    if !ok && p == cur && cur + 1 >= t.len_chars() {
                        return mv(p, MK::Incl);
                    }
                    return mv(p, MK::Incl);
                }
                let (mut p, ok) = fwd_word(t, cur, n, big, op.is_some());
                let mut kind = MK::Excl;
                if !visual && p > cur && gchar(t, p) == '\0' && p > lstart(t, line_of(t, p)) {
                    p = text::prev_grapheme(t, p);
                    kind = MK::Incl;
                }
                if !ok && op.is_none() && p == cur {
                    return None;
                }
                mv(p, kind)
            }
            Motion::WordBack(big) => {
                let (p, ok) = bck_word(t, cur, n, big);
                if !ok && p == cur {
                    return None;
                }
                mv(p, MK::Excl)
            }
            Motion::WordEnd(big) => {
                let (p, ok) = end_word(t, cur, n, big, false, false);
                if !ok && p == cur {
                    return None;
                }
                mv(p, MK::Incl)
            }
            Motion::WordEndBack(big) => {
                let (p, ok) = bckend_word(t, cur, n, big);
                if !ok && p == cur {
                    return None;
                }
                mv(p, MK::Incl)
            }
            Motion::Find { ch, fwd, till } => {
                self.vim.last_find = Some((ch, fwd, till));
                let p = find_in_line(t, cur, ch, fwd, till, n, false)?;
                mv(p, if fwd { MK::Incl } else { MK::Excl })
            }
            Motion::RepeatFind { rev } => {
                let (ch, fwd0, till) = self.vim.last_find?;
                let fwd = fwd0 != rev;
                let p = find_in_line(t, cur, ch, fwd, till, n, true)?;
                mv(p, if fwd { MK::Incl } else { MK::Excl })
            }
            Motion::FileStart | Motion::FileEnd => {
                let l = match (m, count) {
                    (_, Some(c)) => c.saturating_sub(1).min(nl - 1),
                    (Motion::FileStart, None) => 0,
                    _ => nl - 1,
                };
                jmp(first_nonblank(t, l), MK::Line)
            }
            Motion::ScreenTop | Motion::ScreenMid | Motion::ScreenBot => {
                let (first, vis) = self.viewport;
                let first = first.min(nl - 1);
                let visible = vis.min(nl - first);
                let l = match m {
                    Motion::ScreenTop => first + (n - 1).min(visible.saturating_sub(1)),
                    Motion::ScreenMid => first + visible.saturating_sub(1) / 2,
                    _ => first + visible.saturating_sub(1).saturating_sub(n - 1),
                };
                jmp(first_nonblank(t, l.min(nl - 1)), MK::Line)
            }
            Motion::Percent => {
                if let Some(c) = count {
                    if c > 100 {
                        return None;
                    }
                    let l = (c * nl).div_ceil(100).saturating_sub(1).min(nl - 1);
                    return jmp(first_nonblank(t, l), MK::Line);
                }
                let p = match_bracket(t, cur)?;
                jmp(p, MK::Incl)
            }
            Motion::SentFwd | Motion::SentBack => {
                let p = findsent(t, cur, m == Motion::SentFwd, n)?;
                let p = if p >= t.len_chars() && p > 0 { text::prev_grapheme(t, t.len_chars()) } else { p };
                jmp(p, MK::Excl)
            }
            Motion::ParaFwd | Motion::ParaBack => {
                let (p, incl) = findpar(t, cur, m == Motion::ParaFwd, n)?;
                jmp(p, if incl { MK::Incl } else { MK::Excl })
            }
            Motion::Unmatched { back, open } => {
                let (o, c) = if open == '(' || open == ')' { ('(', ')') } else { ('{', '}') };
                let p = unmatched(t, cur, o, c, back, n)?;
                jmp(p, MK::Excl)
            }
            Motion::NextLine | Motion::PrevLine | Motion::UnderLine => {
                let target = match m {
                    Motion::NextLine => {
                        if line + 1 >= nl {
                            return None;
                        }
                        (line + n).min(nl - 1)
                    }
                    Motion::PrevLine => {
                        if line == 0 {
                            return None;
                        }
                        line.saturating_sub(n)
                    }
                    _ => {
                        if n > 1 && line + 1 >= nl {
                            return None;
                        }
                        (line + n - 1).min(nl - 1)
                    }
                };
                mv(first_nonblank(t, target), MK::Line)
            }
            Motion::Mark { ch, exact } => {
                let p = self.mark_pos(ch)?;
                let p = p.min(t.len_chars());
                if exact {
                    jmp(clamp_normal(t, p), MK::Excl)
                } else {
                    jmp(first_nonblank(t, line_of(t, p)), MK::Line)
                }
            }
            Motion::SearchRepeat { rev } => {
                let fwd = self.vim.search_dir_fwd != rev;
                let pat = self.search.pattern.clone();
                let Some(pat) = pat else {
                    self.status("E35: No previous regular expression");
                    return None;
                };
                let off = self.vim.last_offset.clone();
                self.vim_search_motion(&pat, &off, fwd, n, cur, false)
            }
            Motion::Star { fwd, whole } => {
                let pat = self.star_pattern(whole)?;
                self.search.pattern = Some(pat.clone());
                self.vim.hl_off = false;
                self.vim.search_dir_fwd = fwd;
                self.vim.last_offset.clear();
                if self.search.history.last() != Some(&pat) {
                    self.search.history.push(pat.clone());
                }
                // the search starts at the start of the word so `*` finds the *next* occurrence
                self.vim_search_motion(&pat, "", fwd, n, cur, true)
            }
            Motion::SearchPrompt { .. } => None,
        }
    }

    /// `cw` on a blank behaves like `dw` (Vim without `cpo-w`).
    fn cw_on_blank(&self, t: crate::text::Text, cur: usize) -> bool {
        let c = gchar(t, cur);
        c == '\0' || is_blank_char(c)
    }

    pub(crate) fn mark_pos(&mut self, ch: char) -> Option<usize> {
        let len = self.rope.len_chars();
        let key = if ch == '`' { '\'' } else { ch };
        let p = match key {
            '\'' => Some(self.vim.marks.get(&key).copied().unwrap_or(0)),
            'a'..='z' | 'A'..='Z' | '<' | '>' | '.' | '^' | '[' | ']' | '"' => {
                self.vim.marks.get(&key).copied()
            }
            _ => None,
        };
        if p.is_none() {
            self.status("E20: Mark not set");
        }
        p.map(|p| p.min(len))
    }

    /// Execute a motion in Normal or Visual mode (no operator).
    pub(crate) fn move_motion(&mut self, m: Motion, count: Option<usize>) {
        if let Motion::SearchPrompt { fwd } = m {
            self.vim.search_visual = self.vim.vis.is_some();
            self.vim.search_dir_fwd = fwd;
            let prefix = if fwd { "/" } else { "?" };
            self.open_prompt(PromptKind::Search { backwards: !fwd }, prefix, "", count.unwrap_or(1));
            return;
        }
        let Some(mv) = self.eval_motion(m, count, None) else {
            // a failed motion leaves the remembered column alone
            self.vim.keep_want = true;
            return;
        };
        self.apply_move(mv);
    }

    pub(crate) fn apply_move(&mut self, mv: Mv) {
        let before = self.vcur();
        let prev_mark = self.vim.marks.get(&'\'').copied();
        if mv.jump {
            self.vim_push_jump();
        }
        let rope = self.rp();
        let t = rope.slice(..);
        let mut pos = mv.pos.min(t.len_chars());
        if self.vim.vis.is_some() {
            // in Visual mode the cursor may stand on the line end (the "NUL"), selecting the break
            let l = line_of(t, pos);
            pos = pos.min(lend(t, l));
            pos = text::floor_grapheme(t, pos);
        } else {
            pos = clamp_normal(t, pos);
        }
        match mv.want {
            Want::Reset => {}
            Want::Keep => self.vim.keep_want = true,
            Want::Eol => {
                self.vim.want = usize::MAX;
                self.vim.keep_want = true;
            }
        }
        self.vset(pos);
        // a jump that did not move keeps the previous context mark (Vim's `checkpcmark`)
        if mv.jump
            && self.vcur() == before
            && let Some(m) = prev_mark
        {
            self.vim.marks.insert('\'', m);
        }
    }

    // ---- jumplist -----------------------------------------------------------------------------------

    pub(crate) fn vim_push_jump(&mut self) {
        let pos = self.vcur();
        self.vim.marks.insert('\'', pos);
        let rope = self.rp();
        let t = rope.slice(..);
        let _ = t;
        self.vim.jumps.push(pos);
        if self.vim.jumps.len() > 100 {
            self.vim.jumps.remove(0);
        }
        self.vim.jump_idx = self.vim.jumps.len();
    }

    /// Vim removes duplicate lines from the jumplist lazily, keeping the newest entry of each.
    fn cleanup_jumps(&mut self) {
        let rope = self.rp();
        let t = rope.slice(..);
        let len = t.len_chars();
        let old = std::mem::take(&mut self.vim.jumps);
        let at_end = self.vim.jump_idx >= old.len();
        let mut kept: Vec<usize> = Vec::new();
        let mut new_idx = 0;
        for (i, &p) in old.iter().enumerate() {
            if i == self.vim.jump_idx {
                new_idx = kept.len();
            }
            let l = line_of(t, p.min(len));
            if !old[i + 1..].iter().any(|&q| line_of(t, q.min(len)) == l) {
                kept.push(p);
            }
        }
        self.vim.jump_idx = if at_end { kept.len() } else { new_idx.min(kept.len()) };
        self.vim.jumps = kept;
    }

    pub(crate) fn jump_cmd(&mut self, back: bool, n: usize) {
        let rope = self.rp();
        let t = rope.slice(..);
        let len = t.len_chars();
        let cur = self.vcur();
        self.cleanup_jumps();
        if back {
            if self.vim.jump_idx >= self.vim.jumps.len() {
                self.vim.jumps.push(cur);
                self.vim.jump_idx = self.vim.jumps.len() - 1;
            }
            let Some(i) = self.vim.jump_idx.checked_sub(n) else { return };
            self.vim.jump_idx = i;
        } else {
            let i = self.vim.jump_idx + n;
            if i >= self.vim.jumps.len() {
                return;
            }
            self.vim.jump_idx = i;
        }
        let p = self.vim.jumps[self.vim.jump_idx].min(len);
        self.vim.marks.insert('\'', cur);
        self.vset(clamp_normal(t, p));
    }

    /// Keep marks, jumps and tracked positions pointing at the same text across edits.
    pub(crate) fn vim_map(&mut self, tx: &Transaction) {
        // a mark loses its line when the whole line (with its break) is deleted
        {
            let rope = &self.rope;
            let len = rope.len_chars();
            let dead: Vec<char> = self
                .vim
                .marks
                .iter()
                .filter(|(k, _)| k.is_ascii_alphabetic())
                .filter(|(_, p)| {
                    let p = (**p).min(len);
                    let l = rope.char_to_line(p);
                    let (ls, le) = (
                        rope.line_to_char(l),
                        if l + 1 < rope.len_lines() { rope.line_to_char(l + 1) } else { len },
                    );
                    tx.changes.iter().any(|c| c.to > c.from && c.from <= ls && c.to >= le && le > ls)
                })
                .map(|(k, _)| *k)
                .collect();
            for k in dead {
                self.vim.marks.remove(&k);
            }
        }
        let v = &mut self.vim;
        for p in v.marks.values_mut() {
            *p = tx.map_pos(*p, Assoc::After);
        }
        for p in v.jumps.iter_mut() {
            *p = tx.map_pos(*p, Assoc::After);
        }
        for p in v.tracked.iter_mut() {
            if *p == usize::MAX {
                continue;
            }
            // a line that was deleted or joined into its predecessor loses its mark
            let rope = &self.rope;
            let at_line_start = |i: usize| i == 0 || rope.char(i - 1) == '\n';
            let mut dead = false;
            for c in &tx.changes {
                let inside = c.from < *p && *p < c.to;
                let whole_line = c.from == *p
                    && c.to > *p
                    && c.text.is_empty()
                    && at_line_start(c.from)
                    && rope.char(c.to - 1) == '\n';
                let joined =
                    c.to == *p && c.from < *p && !at_line_start(c.from) && rope.char(c.to - 1) == '\n';
                dead |= inside || whole_line || joined;
            }
            *p = if dead { usize::MAX } else { tx.map_pos(*p, Assoc::After) };
        }
        if let Some(vv) = &mut v.vis {
            vv.anchor = tx.map_pos(vv.anchor, Assoc::After);
            vv.cursor = tx.map_pos(vv.cursor, Assoc::After);
        }
        if let Some(l) = &mut v.last_vis {
            l.anchor = tx.map_pos(l.anchor, Assoc::After);
            l.cursor = tx.map_pos(l.cursor, Assoc::After);
        }
    }

    // ---- search --------------------------------------------------------------------------------------

    /// The pattern for `*` / `#`: the keyword under (or after) the cursor, else the non-blank run.
    fn star_pattern(&mut self, whole: bool) -> Option<String> {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let (ls, le) = (lstart(t, l), lend(t, l));
        let is_kw = |c: char| text::is_word_char(c);
        let mut p = cur;
        while p < le && !is_kw(t.char(p)) {
            p += 1;
        }
        let (mut a, mut b);
        if p < le {
            a = p;
            while a > ls && is_kw(t.char(a - 1)) {
                a -= 1;
            }
            b = p;
            while b < le && is_kw(t.char(b)) {
                b += 1;
            }
            let word: String = t.slice(a..b).to_string();
            let esc = super::vregex::escape_literal(&word);
            return Some(if whole { format!("\\<{esc}\\>") } else { esc });
        }
        // no keyword: the run of same-class (non-blank) chars under or after the cursor
        p = cur;
        while p < le && is_blank_char(t.char(p)) {
            p += 1;
        }
        if p >= le {
            self.status("E348: No string under cursor");
            return None;
        }
        let class = super::motion::vim_class(t.char(p));
        a = p;
        while a > ls && super::motion::vim_class(t.char(a - 1)) == class {
            a -= 1;
        }
        b = p;
        while b < le && super::motion::vim_class(t.char(b)) == class {
            b += 1;
        }
        Some(super::vregex::escape_literal(&t.slice(a..b).to_string()))
    }

    pub(crate) fn vim_regex(&self, pat: &str) -> Result<Regex, String> {
        super::vregex::compile(pat, self.vim.opts.ignorecase, self.vim.opts.smartcase)
    }

    /// Find the count-th match of `pat` from `from` in the given direction, with wrap-around and
    /// Vim's messages. `from_word_start` makes `*` skip the word the cursor is in.
    pub(crate) fn vim_find_match(
        &mut self,
        re: &Regex,
        pat: &str,
        fwd: bool,
        count: usize,
        from: usize,
        skip_cur_word: bool,
    ) -> Option<(usize, usize)> {
        let mut pos = from;
        let mut found = None;
        for i in 0..count.max(1) {
            let start = if i == 0 && skip_cur_word { self.word_start_before(pos) } else { pos };
            let m = if fwd { self.match_after(re, start, true) } else { self.match_before(re, start, true) };
            let Some((r, wrapped)) = m else {
                self.status(format!("E486: Pattern not found: {pat}"));
                return None;
            };
            if wrapped {
                self.status(if fwd {
                    "search hit BOTTOM, continuing at TOP"
                } else {
                    "search hit TOP, continuing at BOTTOM"
                });
            }
            pos = r.0;
            found = Some(r);
        }
        found
    }

    fn word_start_before(&self, pos: usize) -> usize {
        let t = self.rope.slice(..);
        let mut p = pos;
        let l = line_of(t, pos);
        let ls = lstart(t, l);
        if p < t.len_chars() && text::is_word_char(t.char(p)) {
            while p > ls && text::is_word_char(t.char(p - 1)) {
                p -= 1;
            }
        }
        p
    }

    /// First match starting after `from`, wrapping around. Empty matches count (`/^$`).
    fn match_after(&self, re: &Regex, from: usize, wrap: bool) -> Option<((usize, usize), bool)> {
        let t = self.rope.slice(..);
        let len = t.len_chars();
        let lo = text::next_grapheme(t, from.min(len));
        if let Some(r) = self.scan_fwd(re, lo, len + 1) {
            return Some((r, false));
        }
        if wrap {
            return self.scan_fwd(re, 0, lo.max(from + 1)).map(|r| (r, true));
        }
        None
    }

    /// Last match starting before `from`, wrapping around.
    fn match_before(&self, re: &Regex, from: usize, wrap: bool) -> Option<((usize, usize), bool)> {
        let len = self.rope.len_chars();
        if let Some(r) = self.scan_back(re, 0, from) {
            return Some((r, false));
        }
        if wrap {
            return self.scan_back(re, from, len + 1).map(|r| (r, true));
        }
        None
    }

    /// First match whose start lies in `lo..hi`, scanning line-aligned windows that grow
    /// geometrically so a hit near the cursor never pays for the rest of a huge buffer.
    fn scan_fwd(&self, re: &Regex, lo: usize, hi: usize) -> Option<(usize, usize)> {
        let t = self.rope.slice(..);
        let len = t.len_chars();
        if lo > len || lo >= hi {
            return None;
        }
        let mut width = 16 * 1024;
        let mut wstart = lstart(t, line_of(t, lo));
        loop {
            let mut wend = (wstart + width).min(len);
            if wend < len {
                wend = lnext(t, line_of(t, wend)).min(len);
            }
            let hay = self.rope.slice(wstart..wend).to_string();
            for (s, e) in char_ranges(&hay, re) {
                let (s, e) = (wstart + s, wstart + e);
                if s >= lo {
                    return if s < hi { Some((s, e)) } else { None };
                }
            }
            if wend >= len {
                return None;
            }
            wstart = wend;
            width = width.saturating_mul(2);
        }
    }

    /// Last match whose start lies in `lo..hi`.
    fn scan_back(&self, re: &Regex, lo: usize, hi: usize) -> Option<(usize, usize)> {
        let t = self.rope.slice(..);
        let len = t.len_chars();
        let hi = hi.min(len + 1);
        if lo >= hi {
            return None;
        }
        let mut width = 16 * 1024;
        let mut wend = if hi > len { len } else { lnext(t, line_of(t, hi)).min(len) };
        let floor = lstart(t, line_of(t, lo.min(len)));
        loop {
            let wstart = lstart(t, line_of(t, wend.saturating_sub(width).max(floor)));
            let hay = self.rope.slice(wstart..wend).to_string();
            for (s, e) in char_ranges(&hay, re).into_iter().rev() {
                let (s, e) = (wstart + s, wstart + e);
                if s >= lo && s < hi {
                    return Some((s, e));
                }
            }
            if wstart <= floor {
                return None;
            }
            wend = wstart;
            width = width.saturating_mul(2);
        }
    }

    /// Parse `pattern/offset`, find the match and turn it into a motion.
    pub(crate) fn vim_search_motion(
        &mut self,
        pat: &str,
        offset: &str,
        fwd: bool,
        count: usize,
        from: usize,
        star: bool,
    ) -> Option<Mv> {
        let re = match self.vim_regex(pat) {
            Ok(r) => r,
            Err(e) => {
                self.status(e);
                return None;
            }
        };
        let (s, e) = self.vim_find_match(&re, pat, fwd, count, from, star)?;
        self.vim.hl_off = false;
        let rope = self.rp();
        let t = rope.slice(..);
        let len = t.len_chars();
        let off = offset.trim();
        if off.is_empty() {
            return jmp(s.min(len), MK::Excl);
        }
        let (kind, rest) = match off.chars().next() {
            Some('e') => ('e', &off[1..]),
            Some('s') | Some('b') => ('s', &off[1..]),
            _ => ('l', off),
        };
        let num: isize = match rest {
            "" => 0,
            "+" => 1,
            "-" => -1,
            r => r.parse().unwrap_or(0),
        };
        match kind {
            'l' => {
                let l = (line_of(t, s) as isize + num).clamp(0, nlines(t) as isize - 1) as usize;
                jmp(first_nonblank(t, l), MK::Line)
            }
            'e' => {
                let base = if e > s { text::prev_grapheme(t, e) } else { s };
                let p = shift_chars(t, base, num);
                jmp(p, MK::Incl)
            }
            _ => {
                let p = shift_chars(t, s, num);
                jmp(p, MK::Excl)
            }
        }
    }

    /// Submit of the `/` or `?` prompt.
    pub(crate) fn vim_search_submit(&mut self, text: &str, backwards: bool, count: usize) {
        let fwd = !backwards;
        let sep = if fwd { '/' } else { '?' };
        let (mut pat, off) = split_offset(text, sep);
        if pat.is_empty() {
            match self.search.pattern.clone() {
                Some(p) => pat = p,
                None => {
                    self.status("E35: No previous regular expression");
                    self.vim.search_op = None;
                    return;
                }
            }
        }
        self.search.pattern = Some(pat.clone());
        self.vim.search_dir_fwd = fwd;
        self.vim.last_offset = off.clone();
        let cur = self.vcur();
        let mv = self.vim_search_motion(&pat, &off, fwd, count.max(1), cur, false);
        let sop = self.vim.search_op.take();
        self.vim.search_visual = false;
        let Some(mv) = mv else { return };
        match sop {
            Some(SearchOp { op, reg, force }) => self.run_op_with_mv(op, mv, reg, force),
            None => self.apply_move(mv),
        }
    }

    pub(crate) fn vim_search_matches(&self) -> Vec<std::ops::Range<usize>> {
        if self.vim.hl_off || !self.vim.opts.hlsearch {
            // an open prompt still previews what is being typed
            if self.prompt.is_none() {
                return Vec::new();
            }
        }
        let live = self
            .prompt
            .as_ref()
            .filter(|p| matches!(p.kind, PromptKind::Search { .. }) && !p.text.is_empty())
            .map(|p| split_offset(&p.text, if p.prefix == "?" { '?' } else { '/' }).0);
        let Some(pat) = live.or_else(|| self.search.pattern.clone()) else { return Vec::new() };
        if let Some((rev, p, m)) = &*self.search.cache.borrow()
            && *rev == self.revision
            && *p == pat
        {
            return m.clone();
        }
        let Ok(re) = self.vim_regex(&pat) else { return Vec::new() };
        let hay = self.rope.to_string();
        let mut out = Vec::new();
        for (s, e) in char_ranges(&hay, &re) {
            if e > s {
                out.push(s..e);
            }
            if out.len() >= 5000 {
                break;
            }
        }
        *self.search.cache.borrow_mut() = Some((self.revision, pat, out.clone()));
        out
    }

    pub(crate) fn vim_scroll_effect(&mut self, lines: i32) {
        self.effects.push(Effect::Scroll(crate::editor::ScrollRequest::Lines(lines)));
    }

    pub(crate) fn mode_is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }
}

fn shift_chars(t: crate::text::Text, base: usize, n: isize) -> usize {
    if n >= 0 {
        text::nth_next_grapheme(t, base, n as usize)
    } else {
        text::nth_prev_grapheme(t, base, (-n) as usize)
    }
}

/// Split `pat/offset` at the first unescaped separator.
pub(crate) fn split_offset(text: &str, sep: char) -> (String, String) {
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == sep {
            return (text[..i].to_string(), text[i + c.len_utf8()..].to_string());
        }
    }
    (text.to_string(), String::new())
}

/// Matches (including empty ones) of `re` in `hay` as char ranges.
pub(crate) fn char_ranges(hay: &str, re: &Regex) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut last_byte, mut last_char) = (0usize, 0usize);
    for m in re.find_iter(hay) {
        last_char += hay[last_byte..m.start()].chars().count();
        let s = last_char;
        last_char += hay[m.start()..m.end()].chars().count();
        last_byte = m.end();
        out.push((s, last_char));
    }
    out
}
