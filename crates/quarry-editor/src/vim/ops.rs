//! Registers, operators and the Normal-mode editing commands (put, join, `r`, `~`, `ctrl-a`).

use super::geom::*;
use super::nav::{MK, Mv};
use super::object;
use super::{Motion, Op, SearchOp, spaces_of};
use crate::editor::{Editor, Effect};
use crate::prompt::PromptKind;
use crate::selection::{Selection, Selections};
use crate::text::{self, Text};
use crate::transaction::{Change, Transaction};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RegKind {
    Char,
    Line,
    /// blockwise with the width of the widest line
    Block(usize),
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Reg {
    pub text: String,
    pub kind: RegKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RK {
    Char,
    Line,
    Block,
}

/// What an operator acts on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Region {
    pub kind: RK,
    /// Char range (`Char`) or the full lines including terminators (`Line`).
    pub start: usize,
    pub end: usize,
    pub l0: usize,
    pub l1: usize,
    /// Block columns (inclusive); `eol` extends every row to its line end.
    pub c0: usize,
    pub c1: usize,
    pub eol: bool,
    /// Where the cursor goes for operators that leave it at the start of the text.
    pub cur: usize,
    pub visual: bool,
    /// the motion that made this region was inclusive (affects the `']` mark)
    pub end_incl: bool,
}

pub(crate) fn region_chars(t: Text, a: usize, b: usize, cur: usize) -> Region {
    Region {
        kind: RK::Char,
        start: a,
        end: b,
        l0: line_of(t, a),
        l1: line_of(t, b.max(a)),
        c0: 0,
        c1: 0,
        eol: false,
        cur,
        visual: false,
        end_incl: true,
    }
}

pub(crate) fn region_lines(t: Text, l0: usize, l1: usize, cur: usize) -> Region {
    Region {
        kind: RK::Line,
        start: lstart(t, l0),
        end: lnext(t, l1),
        l0,
        l1,
        c0: 0,
        c1: 0,
        eol: false,
        cur,
        visual: false,
        end_incl: false,
    }
}

pub(crate) fn region_block(
    t: Text,
    l0: usize,
    l1: usize,
    c0: usize,
    c1: usize,
    eol: bool,
    cur: usize,
) -> Region {
    Region {
        kind: RK::Block,
        start: pos_at_col(t, l0, c0),
        end: lend(t, l1),
        l0,
        l1,
        c0,
        c1,
        eol,
        cur,
        visual: false,
        end_incl: false,
    }
}

/// Is everything before `pos` on its line blank? (Vim's `inindent(0)`.)
fn start_in_indent(t: Text, pos: usize) -> bool {
    let l = line_of(t, pos);
    (lstart(t, l)..pos).all(|i| is_blank_char(t.char(i)))
}

/// Turn a motion result into a region, applying the exclusive/inclusive rules of Vim.
pub(crate) fn region_from_mv(t: Text, from: usize, mv: Mv, force: Option<char>) -> Region {
    let (a, b) = if from <= mv.pos { (from, mv.pos) } else { (mv.pos, from) };
    let mut kind = mv.kind;
    match force {
        Some('v') => {
            kind = match kind {
                MK::Line | MK::Incl => MK::Excl,
                MK::Excl => MK::Incl,
            }
        }
        Some('V') => kind = MK::Line,
        Some('\x16') => {
            let (c0, c1) = (col_of(t, a).min(col_of(t, b)), col_end_of(t, a).max(col_end_of(t, b)));
            return region_block(t, line_of(t, a), line_of(t, b), c0, c1, false, a);
        }
        _ => {}
    }
    match kind {
        MK::Line => {
            let mut r = region_lines(t, line_of(t, a), line_of(t, b), a);
            r.cur = a;
            r
        }
        MK::Incl => {
            let le = lend(t, line_of(t, b));
            let end = if b >= le { b } else { text::next_grapheme(t, b) };
            region_chars(t, a, end, a)
        }
        MK::Excl => {
            let mut end = b;
            let (la, lb) = (line_of(t, a), line_of(t, b));
            if force.is_none() && a < b && lb > la && b == lstart(t, lb) {
                // "exclusive-linewise": the end moves to the end of the previous line, and the
                // motion becomes linewise when it starts in the indent
                if start_in_indent(t, a) {
                    return region_lines(t, la, lb - 1, a);
                }
                end = lend(t, lb - 1);
            }
            let mut r = region_chars(t, a, end, a);
            r.end_incl = false;
            r
        }
    }
}

fn norm_text(s: &str) -> String {
    s.replace("\r\n", "\n")
}

impl Editor {
    // ---- registers -------------------------------------------------------------------------------

    pub(crate) fn reg_get(&self, name: Option<char>) -> Option<Reg> {
        match name {
            Some('_') => None,
            Some('+' | '*') => self.clipboard.clone().filter(|c| !c.is_empty()).map(|c| {
                let c = norm_text(&c);
                let kind = if c.ends_with('\n') { RegKind::Line } else { RegKind::Char };
                Reg { text: c, kind }
            }),
            Some('/') => {
                Some(Reg { text: self.search.pattern.clone().unwrap_or_default(), kind: RegKind::Char })
            }
            Some(':') => self.vim.last_ex.clone().map(|p| Reg { text: p, kind: RegKind::Char }),
            Some('.') => Some(Reg { text: self.vim.last_insert.clone(), kind: RegKind::Char }),
            None | Some('"') => self.vim.regs.get(&'"').cloned(),
            Some(c) => {
                let c = c.to_ascii_lowercase();
                self.vim.regs.get(&c).cloned()
            }
        }
    }

    /// Write `reg` into a named register (uppercase appends); also becomes the unnamed register.
    pub(crate) fn reg_store_named(&mut self, name: char, reg: Reg) {
        match name {
            '_' => {}
            '+' | '*' => {
                let text = if self.line_ending == "\r\n" {
                    reg.text.replace('\n', "\r\n")
                } else {
                    reg.text.clone()
                };
                self.effects.push(Effect::SetClipboard(text.clone()));
                self.last_mirror = Some(text.clone());
                self.clipboard = Some(text);
                self.vim.regs.insert('"', reg);
            }
            c if c.is_ascii_uppercase() => {
                let lc = c.to_ascii_lowercase();
                let merged = match self.vim.regs.get(&lc) {
                    Some(old) => append_reg(old, &reg),
                    None => reg,
                };
                self.vim.regs.insert(lc, merged.clone());
                self.vim.regs.insert('"', merged);
            }
            c => {
                self.vim.regs.insert(c, reg.clone());
                self.vim.regs.insert('"', reg);
            }
        }
    }

    pub(crate) fn reg_yank(&mut self, name: Option<char>, reg: Reg) {
        match name {
            None | Some('"') => {
                self.vim.regs.insert('0', reg.clone());
                self.vim.regs.insert('"', reg);
            }
            Some(c) => self.reg_store_named(c, reg),
        }
    }

    pub(crate) fn reg_delete(&mut self, name: Option<char>, reg: Reg, big: bool) {
        if name == Some('_') {
            return;
        }
        let named = name.filter(|c| *c != '"');
        if let Some(c) = named {
            self.reg_store_named(c, reg.clone());
        }
        let small = reg.kind != RegKind::Line && !reg.text.contains('\n');
        if big {
            for i in (1..9).rev() {
                let from = (b'0' + i as u8) as char;
                let to = (b'0' + i as u8 + 1) as char;
                if let Some(r) = self.vim.regs.get(&from).cloned() {
                    self.vim.regs.insert(to, r);
                }
            }
            self.vim.regs.insert('1', reg.clone());
        }
        if named.is_none() && small {
            self.vim.regs.insert('-', reg.clone());
        }
        if named.is_none() {
            self.vim.regs.insert('"', reg);
        }
    }

    // ---- extracting text -------------------------------------------------------------------------------

    pub(crate) fn region_reg(&self, r: &Region) -> Reg {
        let rope = self.rp();
        let t = rope.slice(..);
        match r.kind {
            RK::Char => {
                Reg { text: norm_text(&t.slice(r.start.min(r.end)..r.end).to_string()), kind: RegKind::Char }
            }
            RK::Line => {
                let mut s = norm_text(&t.slice(r.start..r.end).to_string());
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                Reg { text: s, kind: RegKind::Line }
            }
            RK::Block => {
                let mut rows = Vec::new();
                let mut width = 0;
                for l in r.l0..=r.l1 {
                    let (a, b) = block_cols(t, l, r);
                    let seg = t.slice(a..b).to_string();
                    width = width.max(seg.chars().count());
                    rows.push(seg);
                }
                if !r.eol {
                    width = r.c1 - r.c0 + 1;
                }
                Reg { text: rows.join("\n"), kind: RegKind::Block(width) }
            }
        }
    }

    // ---- applying edits ---------------------------------------------------------------------------------

    /// Apply changes without caring where the selections end up: callers place the cursor.
    pub(crate) fn vapply(&mut self, changes: Vec<Change>) {
        let tx = Transaction::new(changes);
        self.apply(tx);
    }

    pub(crate) fn set_change_marks(&mut self, a: usize, b: usize) {
        self.vim.marks.insert('[', a);
        self.vim.marks.insert(']', b);
        self.vim.marks.insert('.', a);
    }

    pub(crate) fn nl_str(&self, s: &str) -> String {
        if self.line_ending == "\r\n" { s.replace('\n', "\r\n") } else { s.to_string() }
    }

    // ---- operator entry points ---------------------------------------------------------------------------------

    pub(crate) fn run_op_motion(
        &mut self,
        op: Op,
        m: Motion,
        count: Option<usize>,
        reg: Option<char>,
        force: Option<char>,
    ) {
        if let Motion::SearchPrompt { fwd } = m {
            self.vim.search_op = Some(SearchOp { op, reg, force });
            self.vim.search_dir_fwd = fwd;
            self.open_prompt(
                PromptKind::Search { backwards: !fwd },
                if fwd { "/" } else { "?" },
                "",
                count.unwrap_or(1),
            );
            return;
        }
        let Some(mv) = self.eval_motion(m, count, Some(op)) else { return };
        let reg_one = matches!(
            m,
            Motion::Percent
                | Motion::SentFwd
                | Motion::SentBack
                | Motion::ParaFwd
                | Motion::ParaBack
                | Motion::Mark { exact: true, .. }
                | Motion::SearchRepeat { .. }
                | Motion::Star { .. }
        );
        // `y_` is `yy`: the cursor stays where it is
        let keep_cursor = op == Op::Yank && m == Motion::UnderLine;
        self.run_op_mv(op, mv, reg, force, reg_one, keep_cursor);
    }

    pub(crate) fn run_op_with_mv(&mut self, op: Op, mv: Mv, reg: Option<char>, force: Option<char>) {
        self.run_op_mv(op, mv, reg, force, true, false);
    }

    fn run_op_mv(
        &mut self,
        op: Op,
        mv: Mv,
        reg: Option<char>,
        force: Option<char>,
        reg_one: bool,
        keep_cursor: bool,
    ) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        if mv.jump {
            self.vim_push_jump();
        }
        let mut r = region_from_mv(t, cur, mv, force);
        if keep_cursor {
            r.cur = cur;
        }
        self.operate(op, r, reg, reg_one);
    }

    pub(crate) fn run_op_lines(&mut self, op: Op, count: usize, reg: Option<char>) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        if count > 1 && l + 1 >= nlines(t) {
            return;
        }
        let l1 = (l + count - 1).min(nlines(t) - 1);
        // Vim moves the cursor to the first non-blank first (except for `yy`), so with a count of
        // one the operator's cursor is whichever of the two comes first
        let cur_pos = if op == Op::Yank || count > 1 { cur } else { cur.min(first_nonblank(t, l)) };
        let r = region_lines(t, l, l1, cur_pos);
        self.operate(op, r, reg, false);
    }

    pub(crate) fn run_op_object(
        &mut self,
        op: Op,
        inner: bool,
        ch: char,
        count: Option<usize>,
        reg: Option<char>,
    ) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let o = match object::find_or_cursor(t, cur, ch, inner, count.unwrap_or(1), None) {
            Ok(o) => o,
            Err(at) => {
                if let Some(p) = at {
                    self.vset(p);
                }
                return;
            }
        };
        let r = match o.kind {
            MK::Line => region_lines(t, line_of(t, o.start), line_of(t, o.end), o.start),
            k => region_from_mv(
                t,
                o.start,
                Mv { pos: o.end, kind: k, jump: false, want: super::nav::Want::Reset },
                None,
            ),
        };
        self.operate(op, r, reg, false);
    }

    /// Apply `op` to `r`.
    pub(crate) fn operate(&mut self, op: Op, r: Region, reg: Option<char>, reg_one: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        // undo puts the cursor back where the operator started (Vim moves it there first)
        self.vim.edit_at = Some(if r.kind == RK::Block { pos_at_col(t, r.l0, r.c0) } else { r.cur });
        match op {
            Op::Yank => {
                let rg = self.region_reg(&r);
                let lines = rg.text.matches('\n').count() + usize::from(!rg.text.ends_with('\n'));
                self.reg_yank(reg, rg);
                // `']` is the end exclusive for charwise motions, the last char when inclusive
                let end_mark = match r.kind {
                    RK::Line => lend(t, r.l1),
                    _ if r.end_incl => r.end.saturating_sub(1),
                    _ => r.end,
                };
                self.set_change_marks(r.start, end_mark);
                if r.kind == RK::Line && lines > 2 {
                    self.status(format!("{lines} lines yanked"));
                }
                let target = if r.kind == RK::Block { pos_at_col(t, r.l0, r.c0) } else { r.cur };
                self.vset(target);
            }
            Op::Delete | Op::Change => self.op_delete_change(op, r, reg, reg_one),
            Op::Indent | Op::Outdent => {
                let n = 1;
                self.shift_lines(r.l0, r.l1, n, op == Op::Indent);
                let rope = self.rp();
                let t = rope.slice(..);
                self.vset(first_nonblank(t, r.l0));
            }
            Op::Format => {
                let rope = self.rp();
                let t = rope.slice(..);
                let whole = r.l0 == 0 && r.l1 + 1 >= nlines(t);
                if whole {
                    self.effects.push(Effect::FormatDocument);
                } else {
                    // the host formats the selection: leave the lines selected for it
                    let (a, b) = (lstart(t, r.l0), lnext(t, r.l1));
                    self.set_sels(Selections::single(Selection::new(a, b)));
                    self.effects.push(Effect::FormatSelection);
                    self.vim.keep_sel = true;
                    return;
                }
                self.vset(first_nonblank(t, r.l0));
            }
            Op::Toggle | Op::Lower | Op::Upper | Op::Rot13 => {
                self.case_region(op, &r);
            }
            Op::Comment => {
                let lines: Vec<usize> = (r.l0..=r.l1).collect();
                self.toggle_comment_lines(&lines);
                let rope = self.rp();
                let t = rope.slice(..);
                self.vset(first_nonblank(t, r.l0));
            }
        }
    }

    fn op_delete_change(&mut self, op: Op, mut r: Region, reg: Option<char>, reg_one: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let change = op == Op::Change;
        if r.kind == RK::Char && r.start >= r.end {
            // an empty region deletes nothing (and touches no register)
            if change {
                self.start_insert_at(r.start, 1, false);
            } else {
                self.vset(r.start);
            }
            return;
        }
        // `d` of a multi-line charwise region that starts in the indent and ends at the end of a
        // line with nothing but blanks after it deletes whole lines (see :help d-special)
        if op == Op::Delete && r.kind == RK::Char && !r.visual && r.l1 > r.l0 {
            let le = lend(t, line_of(t, r.end));
            if (r.end >= le || (r.end..le).all(|i| is_blank_char(t.char(i)))) && start_in_indent(t, r.start) {
                let l1 = line_of(t, r.end);
                r = region_lines(t, r.l0, l1, r.cur);
            }
        }
        let rg = self.region_reg(&r);
        let big = reg_one || rg.kind == RegKind::Line || rg.text.contains('\n');
        match r.kind {
            RK::Char => {
                self.reg_delete(reg, rg, big);
                self.vapply(vec![Change { from: r.start, to: r.end, text: String::new() }]);
                self.set_change_marks(r.start, r.start);
                if change {
                    self.start_insert_at(r.start, 1, false);
                } else {
                    self.vset(r.start);
                }
            }
            RK::Line => {
                self.reg_delete(reg, rg, true);
                if change {
                    // keep one (indented) empty line to type on
                    let indent = if self.vim_autoindent() {
                        spaces_of(&text::leading_whitespace(t, r.l0))
                    } else {
                        String::new()
                    };
                    let from = lstart(t, r.l0);
                    let to = lend(t, r.l1);
                    self.vapply(vec![Change { from, to, text: indent.clone() }]);
                    self.set_change_marks(from, from);
                    self.start_insert_at(from + indent.chars().count(), 1, false);
                    if let Some(s) = &mut self.vim.ins {
                        s.ai_only = !indent.is_empty();
                    }
                } else {
                    let c = line_delete_change(t, r.l0, r.l1);
                    self.vapply(vec![c]);
                    let rope = self.rp();
                    let t2 = rope.slice(..);
                    let l = r.l0.min(nlines(t2) - 1);
                    self.set_change_marks(lstart(t2, l), lstart(t2, l));
                    self.vset(first_nonblank(t2, l));
                }
            }
            RK::Block => {
                self.reg_delete(reg, rg, big);
                let mut changes = Vec::new();
                for l in r.l0..=r.l1 {
                    let (a, b) = block_cols(t, l, &r);
                    if b > a {
                        changes.push(Change { from: a, to: b, text: String::new() });
                    }
                }
                self.vapply(changes);
                let rope = self.rp();
                let t2 = rope.slice(..);
                let top = pos_at_col(t2, r.l0, r.c0);
                if change {
                    self.start_block_insert(r.l0, r.l1, r.c0, false, top, None);
                } else {
                    self.vset(top);
                }
            }
        }
    }

    pub(crate) fn vim_autoindent(&self) -> bool {
        true
    }

    // ---- case ------------------------------------------------------------------------------------

    pub(crate) fn case_region(&mut self, op: Op, r: &Region) {
        let rope = self.rp();
        let t = rope.slice(..);
        let f = |s: &str| -> String {
            match op {
                Op::Lower => s.to_lowercase(),
                Op::Upper => s.to_uppercase(),
                Op::Rot13 => s.chars().map(rot13).collect(),
                _ => s.chars().flat_map(switch_case).collect(),
            }
        };
        let mut changes = Vec::new();
        // an empty exclusive region at the very start of the buffer still switches the first char
        // (a quirk of Vim's `op_tilde`, which cannot step back from there)
        let mut r = *r;
        if r.kind == RK::Char && r.start == r.end && r.start == 0 && r.start < lend(t, 0) {
            r.end = text::next_grapheme(t, r.start);
        }
        let r = &r;
        match r.kind {
            RK::Char | RK::Line => {
                let (a, b) = (r.start, if r.kind == RK::Line { lend(t, r.l1) } else { r.end });
                let old = t.slice(a..b).to_string();
                let new = f(&old);
                if new != old {
                    changes.push(Change { from: a, to: b, text: new });
                }
            }
            RK::Block => {
                for l in r.l0..=r.l1 {
                    let (a, b) = block_cols(t, l, r);
                    let old = t.slice(a..b).to_string();
                    let new = f(&old);
                    if new != old {
                        changes.push(Change { from: a, to: b, text: new });
                    }
                }
            }
        }
        self.vapply(changes);
        let rope = self.rp();
        let t2 = rope.slice(..);
        self.set_change_marks(r.start, r.end.saturating_sub(1));
        let target = if r.kind == RK::Block { pos_at_col(t2, r.l0, r.c0) } else { r.cur };
        self.vset(target);
    }

    pub(crate) fn tilde(&mut self, n: usize) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let le = lend(t, line_of(t, cur));
        if cur >= le {
            return;
        }
        let end = text::nth_next_grapheme(t, cur, n).min(le);
        let old = t.slice(cur..end).to_string();
        let new: String = old.chars().flat_map(switch_case).collect();
        if new != old {
            self.vapply(vec![Change { from: cur, to: end, text: new }]);
        }
        self.set_change_marks(cur, end.saturating_sub(1));
        self.vset(end);
    }

    pub(crate) fn replace_chars(&mut self, c: char, n: usize) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let le = lend(t, l);
        if cur >= le {
            return;
        }
        // there must be `n` characters to replace
        let mut end = cur;
        for _ in 0..n {
            if end >= le {
                return;
            }
            end = text::next_grapheme(t, end);
        }
        if c == '\n' {
            // the new line copies the indent of what stays in front, loses the blanks behind
            let indent: String = spaces_of(
                &t.slice(lstart(t, l)..cur)
                    .to_string()
                    .chars()
                    .take_while(|c| is_blank_char(*c))
                    .collect::<String>(),
            );
            let mut stop = end;
            while stop < lend(t, l) && is_blank_char(t.char(stop)) {
                stop += 1;
            }
            let ins = format!("{}{}", self.line_ending, indent);
            let start_of_new = cur + self.line_ending.chars().count();
            let cursor = start_of_new + indent.chars().count().saturating_sub(1);
            self.vapply(vec![Change { from: cur, to: stop, text: ins }]);
            self.set_change_marks(cur, start_of_new);
            self.vset(cursor);
            return;
        }
        let s: String = std::iter::repeat_n(c, n).collect();
        self.vapply(vec![Change { from: cur, to: end, text: s.clone() }]);
        let last = cur + s.chars().count() - 1;
        self.set_change_marks(cur, last);
        self.vset(last);
    }

    // ---- indent ----------------------------------------------------------------------------------------

    /// `>` / `<`: the indent of every non-empty line grows / shrinks by four columns per count and
    /// is rewritten with spaces (what `expandtab` does in Vim).
    pub(crate) fn shift_lines(&mut self, l0: usize, l1: usize, times: usize, right: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let mut changes = Vec::new();
        for l in l0..=l1.min(nlines(t) - 1) {
            let (ls, le) = (lstart(t, l), lend(t, l));
            if le == ls {
                continue;
            }
            let mut ws_end = ls;
            while ws_end < le && is_blank_char(t.char(ws_end)) {
                ws_end += 1;
            }
            let w = indent_width(t, l);
            let nw = if right { w + 4 * times } else { w.saturating_sub(4 * times) };
            let new = " ".repeat(nw);
            if t.slice(ls..ws_end) != new {
                changes.push(Change { from: ls, to: ws_end, text: new });
            }
        }
        self.vapply(changes);
        self.set_change_marks(lstart(t, l0), lstart(t, l1));
    }

    // ---- join ---------------------------------------------------------------------------------------------

    pub(crate) fn join_cmd(&mut self, count: usize, spaces: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let l = line_of(t, self.vcur());
        if l + 1 >= nlines(t) {
            // on the last line `J` fails; with a count above two it "joins" the one line that is
            // there, which only puts the cursor at the start of the line
            if count > 2 {
                self.vset(lstart(t, l));
            }
            return;
        }
        let last = (l + count - 1).min(nlines(t) - 1);
        self.join_lines_range(l, last, spaces);
    }

    pub(crate) fn join_lines_range(&mut self, l0: usize, l1: usize, spaces: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let l1 = l1.min(nlines(t) - 1);
        if l0 >= l1 {
            return;
        }
        let mut changes = Vec::new();
        let mut cursor_at = 0;
        let mut sum_nonempty = lend(t, l0) > lstart(t, l0);
        let mut prev_last: Option<char> = if sum_nonempty { Some(t.char(lend(t, l0) - 1)) } else { None };
        // position in the new text where the last join happened
        let mut shift = 0isize;
        for l in l0 + 1..=l1 {
            let from = lend(t, l - 1);
            let ls = lstart(t, l);
            let le = lend(t, l);
            let mut to = ls;
            let mut ins = String::new();
            if spaces {
                while to < le && is_blank_char(t.char(to)) {
                    to += 1;
                }
                let empty_next = to >= le;
                let starts_paren = !empty_next && t.char(to) == ')';
                if !empty_next && !starts_paren && sum_nonempty && prev_last != Some('\t') {
                    if prev_last == Some(' ') {
                        // already ends in a space: no extra one
                    } else {
                        ins.push(' ');
                    }
                }
            }
            let new_from = (from as isize + shift) as usize;
            cursor_at = new_from;
            changes.push(Change { from, to, text: ins.clone() });
            shift += ins.chars().count() as isize - (to - from) as isize;
            if le > to {
                sum_nonempty = true;
                prev_last = Some(t.char(le - 1));
            } else if !ins.is_empty() {
                prev_last = Some(' ');
            }
            let _ = ls;
        }
        let first_join = changes.first().map_or(0, |c| c.from);
        self.vapply(changes);
        let _ = first_join;
        self.set_change_marks(lstart(t, l0), cursor_at);
        self.vset(cursor_at);
    }

    // ---- put ----------------------------------------------------------------------------------------------------

    pub(crate) fn put_cmd(&mut self, reg: Option<char>, count: usize, before: bool, cursor_after: bool) {
        let Some(r) = self.reg_get(reg) else {
            self.status(format!("E353: Nothing in register {}", reg.unwrap_or('"')));
            return;
        };
        self.put_reg(&r, count, before, cursor_after, PutFlags::default());
    }

    pub(crate) fn put_reg(
        &mut self,
        r: &Reg,
        count: usize,
        before: bool,
        cursor_after: bool,
        flags: PutFlags,
    ) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let n = count.max(1);
        let len = t.len_chars();
        let kind = if flags.force_line && r.kind == RegKind::Char { RegKind::Line } else { r.kind };
        let mut text = r.text.clone();
        if flags.force_line && r.kind == RegKind::Char && !text.ends_with('\n') {
            text.push('\n');
        }
        match kind {
            RegKind::Char => {
                let pos = if before || line_empty(t, l) || cur >= lend(t, l) {
                    cur
                } else {
                    text::next_grapheme(t, cur)
                };
                let body = self.nl_str(&text).repeat(n);
                let total = body.chars().count();
                if total == 0 {
                    // putting the empty string changes nothing
                    self.vset(cur);
                    return;
                }
                if flags.split_line && text.ends_with('\n') {
                    // unreachable for Char registers: kept for symmetry
                }
                self.vapply(vec![Change { from: pos, to: pos, text: body.clone() }]);
                self.set_change_marks(pos, pos + total.saturating_sub(1));
                let rope = self.rp();
                let t2 = rope.slice(..);
                let target = if cursor_after {
                    pos + total
                } else if text.contains('\n') {
                    pos
                } else {
                    text::prev_grapheme(t2, pos + total)
                };
                self.vset(target);
                self.vim.last_put = Some((pos, pos + total));
            }
            RegKind::Line => {
                let nl = nlines(t);
                let mut body = self.nl_str(&text).repeat(n);
                let (pos, extra_nl);
                if flags.split_line {
                    // charwise Visual replaced by linewise text: split the line at the cursor
                    let p = if before || cur >= lend(t, l) { cur } else { text::next_grapheme(t, cur) };
                    let p = if line_empty(t, l) { cur } else { p };
                    let le = self.line_ending;
                    let ins = format!("{le}{body}");
                    let total = ins.chars().count();
                    self.vapply(vec![Change { from: p, to: p, text: ins }]);
                    self.set_change_marks(p, p + total);
                    let rope = self.rp();
                    let t2 = rope.slice(..);
                    let first = line_of(t2, p) + 1;
                    self.vset(first_nonblank(t2, first.min(nlines(t2) - 1)));
                    return;
                }
                if before {
                    pos = lstart(t, l);
                    extra_nl = false;
                } else if l + 1 == nl && !has_term(t, l) {
                    pos = len;
                    extra_nl = true;
                } else {
                    pos = lnext(t, l);
                    extra_nl = false;
                }
                if extra_nl {
                    // the last line has no terminator: break it first and keep the buffer unterminated
                    let le = self.line_ending;
                    body = format!("{le}{}", body.strip_suffix(le).unwrap_or(&body));
                }
                let total = body.chars().count();
                self.vapply(vec![Change { from: pos, to: pos, text: body }]);
                let rope = self.rp();
                let t2 = rope.slice(..);
                let first = line_of(t2, pos) + usize::from(extra_nl);
                let nlines_put = text.matches('\n').count() * n;
                self.set_change_marks(lstart(t2, first), pos + total);
                let target = if cursor_after {
                    let l2 = (first + nlines_put).min(nlines(t2) - 1);
                    lstart(t2, l2)
                } else {
                    first_nonblank(t2, first.min(nlines(t2) - 1))
                };
                self.vset(target);
                self.vim.last_put = Some((pos, pos + total));
            }
            RegKind::Block(width) => {
                let rows: Vec<&str> = text.split('\n').collect();
                let col0 = if before || line_empty(t, l) { col_of(t, cur) } else { col_of(t, cur) + 1 };
                let nl = nlines(t);
                let mut changes: Vec<Change> = Vec::new();
                let mut extra = String::new();
                for (i, row) in rows.iter().enumerate() {
                    let line = l + i;
                    let seg = pad_row(row, width);
                    let seg_rep: String = (0..n)
                        .map(|k| if k + 1 == n && flags.trim_last { row.to_string() } else { seg.clone() })
                        .collect();
                    if line < nl {
                        let le = lend(t, line);
                        let lc = col_of(t, le);
                        if lc < col0 {
                            // pad a short line with spaces up to the block column
                            let pad = " ".repeat(col0 - lc);
                            let tail_trim = seg_rep.trim_end_matches(' ').to_string();
                            changes.push(Change { from: le, to: le, text: format!("{pad}{tail_trim}") });
                        } else {
                            let at = pos_at_col(t, line, col0);
                            // do not pad rows that end the line
                            let ins =
                                if at >= le { seg_rep.trim_end_matches(' ').to_string() } else { seg_rep };
                            changes.push(Change { from: at, to: at, text: ins });
                        }
                    } else {
                        let pad = " ".repeat(col0);
                        extra.push_str(&format!(
                            "{}{pad}{}",
                            self.line_ending,
                            seg_rep.trim_end_matches(' ')
                        ));
                    }
                }
                if !extra.is_empty() {
                    // append new lines at the end of the buffer
                    let end = if has_term(t, nl - 1) { lstart(t, nl - 1) } else { len };
                    let at = lend(t, nl - 1);
                    let _ = end;
                    changes.push(Change { from: at, to: at, text: extra });
                }
                self.vapply(changes);
                let rope = self.rp();
                let t2 = rope.slice(..);
                let start = pos_at_col(t2, l, col0);
                self.set_change_marks(
                    start,
                    pos_at_col(t2, (l + rows.len() - 1).min(nlines(t2) - 1), col0 + width * n),
                );
                let target = if cursor_after {
                    pos_at_col(t2, (l + rows.len() - 1).min(nlines(t2) - 1), col0 + width * n)
                } else {
                    start
                };
                self.vset(target);
            }
        }
    }

    // ---- ctrl-a / ctrl-x ------------------------------------------------------------------------------------------

    pub(crate) fn increment_cmd(&mut self, delta: i64) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let le = lend(t, l);
        if let Some((a, b, new)) = number_edit(t, cur, le, delta, true) {
            let n = new.chars().count();
            self.vapply(vec![Change { from: a, to: b, text: new }]);
            self.set_change_marks(a, a + n - 1);
            self.vset(a + n - 1);
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct PutFlags {
    /// charwise register put as whole lines (linewise Visual replaced by charwise text)
    pub force_line: bool,
    /// linewise register put into a charwise selection: split the line
    pub split_line: bool,
    pub trim_last: bool,
}

fn pad_row(row: &str, width: usize) -> String {
    let n = row.chars().count();
    format!("{row}{}", " ".repeat(width.saturating_sub(n)))
}

fn append_reg(old: &Reg, new: &Reg) -> Reg {
    match (old.kind, new.kind) {
        (RegKind::Line, _) | (_, RegKind::Line) => {
            let mut s = old.text.clone();
            if !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&new.text);
            if !s.ends_with('\n') {
                s.push('\n');
            }
            Reg { text: s, kind: RegKind::Line }
        }
        (RegKind::Block(w), RegKind::Block(w2)) => {
            Reg { text: format!("{}\n{}", old.text, new.text), kind: RegKind::Block(w.max(w2)) }
        }
        _ => Reg { text: format!("{}{}", old.text, new.text), kind: old.kind },
    }
}

/// The char range of a block row (clamped to the line).
pub(crate) fn block_cols(t: Text, l: usize, r: &Region) -> (usize, usize) {
    let le = lend(t, l);
    let a = pos_at_col(t, l, r.c0);
    if a >= le {
        return (le, le);
    }
    let b = if r.eol { le } else { pos_at_col(t, l, r.c1 + 1) };
    (a, b.min(le))
}

/// The change that removes whole lines `l0..=l1`.
pub(crate) fn line_delete_change(t: Text, l0: usize, l1: usize) -> Change {
    let last = nlines(t) - 1;
    if l1 < last || has_term(t, last) {
        Change { from: lstart(t, l0), to: lnext(t, l1), text: String::new() }
    } else if l0 > 0 {
        Change { from: lend(t, l0 - 1), to: t.len_chars(), text: String::new() }
    } else {
        Change { from: 0, to: t.len_chars(), text: String::new() }
    }
}

fn rot13(c: char) -> char {
    match c {
        'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
        'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
        _ => c,
    }
}

fn switch_case(c: char) -> Vec<char> {
    if c.is_lowercase() {
        c.to_uppercase().collect()
    } else if c.is_uppercase() {
        c.to_lowercase().collect()
    } else {
        vec![c]
    }
}

/// The number at or after `lo` on the line, changed by `delta`: (start, end, new text). A port of
/// the observable behaviour of Vim's `do_addsub` with `nrformats=bin,hex` (Neovim's default): a
/// `-` directly before the digits is a sign, `0x1f` / `0b101` keep their width and case, and
/// decimal numbers keep their leading zeros.
pub(crate) fn number_edit(
    t: Text,
    lo: usize,
    hi: usize,
    delta: i64,
    at_cursor: bool,
) -> Option<(usize, usize, String)> {
    let hi = hi.min(t.len_chars());
    let ch = |i: usize| if i < hi { t.char(i) } else { '\0' };
    let isd = |i: usize| ch(i).is_ascii_digit();
    let isx = |i: usize| ch(i).is_ascii_hexdigit();
    let isb = |i: usize| matches!(ch(i), '0' | '1');
    let line_start = lstart(t, line_of(t, lo));
    let mut col = lo;
    let mut kind = 'd';
    if at_cursor {
        // on a hex / binary number after its prefix?
        let mut c = lo;
        while c > line_start && isb(c) {
            c -= 1;
        }
        let bin_ok = c > line_start && matches!(ch(c), 'b' | 'B') && ch(c - 1) == '0' && isb(c + 1);
        let mut c2 = lo;
        while c2 > line_start && isx(c2) {
            c2 -= 1;
        }
        let hex_ok = c2 > line_start && matches!(ch(c2), 'x' | 'X') && ch(c2 - 1) == '0' && isx(c2 + 1);
        if hex_ok {
            kind = 'x';
            col = c2 - 1;
        } else if bin_ok {
            kind = 'b';
            col = c - 1;
        }
    }
    if kind == 'd' {
        col = lo;
        while col < hi && !isd(col) {
            col += 1;
        }
        if col >= hi {
            return None;
        }
        // inside a number: walk back to its first digit
        while col > line_start && isd(col - 1) {
            col -= 1;
        }
        // `0x1f` / `0b101` starting at this digit (cursor before or on the prefix)
        if ch(col) == '0' && matches!(ch(col + 1), 'x' | 'X') && isx(col + 2) {
            kind = 'x';
        } else if ch(col) == '0' && matches!(ch(col + 1), 'b' | 'B') && isb(col + 2) {
            kind = 'b';
        }
    }
    match kind {
        'x' | 'b' => {
            let digits_from = col + 2;
            let mut end = digits_from;
            while if kind == 'x' { isx(end) } else { isb(end) } {
                end += 1;
            }
            let s: String = t.slice(digits_from..end).to_string();
            let radix = if kind == 'x' { 16 } else { 2 };
            let v = u64::from_str_radix(&s, radix).unwrap_or(u64::MAX);
            let nv = if delta >= 0 { v.wrapping_add(delta as u64) } else { v.wrapping_sub((-delta) as u64) };
            let width = s.len();
            let new = if kind == 'x' {
                let upper =
                    s.chars().rev().find(|c| c.is_ascii_alphabetic()).is_some_and(|c| c.is_ascii_uppercase());
                let body = if upper { format!("{nv:0width$X}") } else { format!("{nv:0width$x}") };
                format!("{}{body}", t.slice(col..col + 2))
            } else {
                format!("{}{nv:0width$b}", t.slice(col..col + 2))
            };
            Some((col, end, new))
        }
        _ => {
            let mut end = col;
            while isd(end) {
                end += 1;
            }
            let neg = col > line_start && ch(col - 1) == '-';
            let start = if neg { col - 1 } else { col };
            let digits: String = t.slice(col..end).to_string();
            let v: i128 = digits.parse().unwrap_or(0);
            let v = if neg { -v } else { v };
            let nv = v + delta as i128;
            let mut body = nv.unsigned_abs().to_string();
            if digits.len() > 1 && digits.starts_with('0') && body.len() < digits.len() {
                body = format!("{body:0>width$}", width = digits.len());
            }
            let new = if nv < 0 { format!("-{body}") } else { body };
            Some((start, end, new))
        }
    }
}

impl Editor {
    /// A register as (text, kind) with kind `c`harwise, `l`inewise or `b`lockwise; for tests and `:registers`.
    #[allow(dead_code)]
    pub(crate) fn vim_reg_dump(&self, name: char) -> Option<(String, char)> {
        self.reg_get(Some(name)).map(|r| {
            let k = match r.kind {
                RegKind::Char => 'c',
                RegKind::Line => 'l',
                RegKind::Block(_) => 'b',
            };
            (r.text, k)
        })
    }
}
