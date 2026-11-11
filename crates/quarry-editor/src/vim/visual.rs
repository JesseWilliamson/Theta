//! Visual, Visual-line and Visual-block mode.
//!
//! The anchor and cursor live in `VimState::vis`; the editor's selections are derived from
//! them (`vis_sync`) so the view can paint highlights with the same `Selection` type as in
//! every other mode: Visual is inclusive of the char under the cursor, Visual-line covers whole
//! lines including the terminator, and Visual-block is one selection per row with the primary
//! on the cursor's row.

use super::geom::*;
use super::nav::MK;
use super::object;
use super::ops::*;
use super::{Extent, Op, Simple};
use crate::editor::{Editor, Effect, Mode};
use crate::selection::{Selection, Selections};
use crate::text;
use crate::transaction::Change;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VKind {
    Char,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Visual {
    pub kind: VKind,
    pub anchor: usize,
    pub cursor: usize,
}

/// Block columns spanned by two corner chars: the left edge of the left one to the last cell of the
/// right one (a tab at a corner counts completely).
fn block_cols_of(t: crate::text::Text, a: usize, c: usize) -> (usize, usize) {
    let (sa, ea) = (col_of(t, a), col_end_of(t, a));
    let (sc, ec) = (col_of(t, c), col_end_of(t, c));
    (sa.min(sc), ea.max(ec))
}

fn mode_of(k: VKind) -> Mode {
    match k {
        VKind::Char => Mode::Visual,
        VKind::Line => Mode::VisualLine,
        VKind::Block => Mode::VisualBlock,
    }
}

impl Editor {
    pub(crate) fn vis_enter(&mut self, kind: VKind) {
        match self.vim.vis {
            Some(v) if v.kind == kind => self.vis_exit(),
            Some(mut v) => {
                v.kind = kind;
                self.vim.vis = Some(v);
                self.mode = mode_of(kind);
                self.vis_sync();
            }
            None => {
                let c = self.vcur();
                self.vim.vis = Some(Visual { kind, anchor: c, cursor: c });
                self.mode = mode_of(kind);
                self.vis_sync();
            }
        }
    }

    pub(crate) fn vis_exit(&mut self) {
        if let Some(v) = self.vim.vis.take() {
            self.vim.last_vis = Some(v);
            let t = self.rope.slice(..);
            let (lo, hi) = (v.anchor.min(v.cursor), v.anchor.max(v.cursor));
            if v.kind == VKind::Line {
                // linewise: `'<` is column 0 and `'>` the end of the line
                self.vim.marks.insert('<', lstart(t, line_of(t, lo)));
                self.vim.marks.insert('>', lend(t, line_of(t, hi)));
            } else {
                self.vim.marks.insert('<', lo);
                self.vim.marks.insert('>', hi);
            }
            self.mode = Mode::Normal;
            let c = clamp_normal(t, v.cursor);
            self.set_sels(Selections::single(Selection::point(c)));
        }
    }

    /// Derive the editor selections from the Visual state.
    pub(crate) fn vis_sync(&mut self) {
        let Some(v) = self.vim.vis else { return };
        let rope = self.rp();
        let t = rope.slice(..);
        let len = t.len_chars();
        let (a, c) = (v.anchor.min(len), v.cursor.min(len));
        let forward = a <= c;
        let sels = match v.kind {
            VKind::Char => {
                let (lo, hi) = (a.min(c), a.max(c));
                let end = after_incl(t, hi);
                let s = if forward { Selection::new(lo, end) } else { Selection::new(end, lo) };
                Selections::single(s)
            }
            VKind::Line => {
                let (l0, l1) = (line_of(t, a.min(c)), line_of(t, a.max(c)));
                let (s0, e0) = (lstart(t, l0), lnext(t, l1));
                Selections::single(if forward { Selection::new(s0, e0) } else { Selection::new(e0, s0) })
            }
            VKind::Block => {
                let (la, lc) = (line_of(t, a), line_of(t, c));
                let (c0, c1) = block_cols_of(t, a, c);
                let eol = self.vim.want == usize::MAX;
                let r = region_block(t, la.min(lc), la.max(lc), c0, c1, eol, 0);
                let mut v = Vec::new();
                let mut primary = 0;
                for l in r.l0..=r.l1 {
                    let (s, e) = block_cols(t, l, &r);
                    // an empty row (line shorter than the block) has nothing to highlight
                    if e > s || (line_empty(t, l) && c0 == 0) {
                        if l == lc {
                            primary = v.len();
                        }
                        v.push(Selection::new(s, e));
                    }
                }
                if v.is_empty() {
                    v.push(Selection::point(c));
                }
                Selections::new(v, primary)
            }
        };
        self.set_sels(sels);
    }

    pub(crate) fn visual_region(&self) -> Option<Region> {
        let v = self.vim.vis?;
        let rope = self.rp();
        let t = rope.slice(..);
        let len = t.len_chars();
        let (a, c) = (v.anchor.min(len), v.cursor.min(len));
        let (lo, hi) = (a.min(c), a.max(c));
        let mut r = match v.kind {
            VKind::Char => {
                let mut r = region_chars(t, lo, after_incl(t, hi), lo);
                r.l1 = line_of(t, hi);
                r
            }
            VKind::Line => {
                // the operator's cursor: column 0 when the selection starts at the anchor, the
                // cursor's own position when it was extended upwards
                let cur = if a <= c { lstart(t, line_of(t, lo)) } else { lo };
                region_lines(t, line_of(t, lo), line_of(t, hi), cur)
            }
            VKind::Block => {
                let (la, lc) = (line_of(t, a), line_of(t, c));
                let (c0, c1) = block_cols_of(t, a, c);
                let eol = self.vim.want == usize::MAX;
                region_block(t, la.min(lc), la.max(lc), c0, c1, eol, lo)
            }
        };
        r.visual = true;
        Some(r)
    }

    pub(crate) fn visual_extent(&self) -> Option<Extent> {
        let v = self.vim.vis?;
        let r = self.visual_region()?;
        let rope = self.rp();
        let t = rope.slice(..);
        let lines = r.l1 - r.l0 + 1;
        Some(match v.kind {
            VKind::Char => {
                let hi = v.anchor.max(v.cursor);
                let cols = if lines == 1 {
                    count_graphemes(t, v.anchor.min(v.cursor), hi) + 1
                } else {
                    col_of(t, hi)
                };
                Extent { kind: VKind::Char, lines, cols, eol: false }
            }
            VKind::Line => Extent { kind: VKind::Line, lines, cols: 0, eol: false },
            VKind::Block => Extent { kind: VKind::Block, lines, cols: r.c1 - r.c0 + 1, eol: r.eol },
        })
    }

    pub(crate) fn visual_from_extent(&mut self, e: Extent) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let last = (l + e.lines - 1).min(nlines(t) - 1);
        let cursor = match e.kind {
            VKind::Char => {
                if e.lines == 1 {
                    text::nth_next_grapheme(t, cur, e.cols.saturating_sub(1)).min(lend(t, l))
                } else {
                    clamp_normal(t, pos_at_col(t, last, e.cols))
                }
            }
            VKind::Line => pos_at_col(t, last, col_of(t, cur)),
            VKind::Block => pos_at_col(t, last, col_of(t, cur) + e.cols.saturating_sub(1)),
        };
        self.vim.vis = Some(Visual { kind: e.kind, anchor: cur, cursor });
        self.mode = mode_of(e.kind);
        if e.eol {
            self.vim.want = usize::MAX;
        }
        self.vis_sync();
    }

    pub(crate) fn vis_reselect(&mut self) {
        let Some(last) = self.vim.last_vis else { return };
        let rope = self.rp();
        let len = rope.len_chars();
        let cur = self.vim.vis;
        self.vim.vis = Some(Visual { cursor: last.cursor.min(len), anchor: last.anchor.min(len), ..last });
        if let Some(c) = cur {
            self.vim.last_vis = Some(c);
        }
        self.mode = mode_of(last.kind);
        self.vis_sync();
    }

    pub(crate) fn visual_object(&mut self, inner: bool, ch: char, count: usize) {
        let Some(v) = self.vim.vis else { return };
        let rope = self.rp();
        let t = rope.slice(..);
        let sel = (v.anchor != v.cursor).then(|| (v.anchor.min(v.cursor), v.anchor.max(v.cursor)));
        let Some(o) = object::find(t, v.cursor, ch, inner, count, sel) else { return };
        let mut nv = v;
        match o.kind {
            MK::Line => {
                if nv.kind == VKind::Char {
                    nv.kind = VKind::Line;
                    self.mode = Mode::VisualLine;
                }
                if sel.is_none() {
                    nv.anchor = o.start;
                } else {
                    nv.anchor = nv.anchor.min(o.start);
                }
                nv.cursor = o.end;
            }
            k => {
                let end = match k {
                    MK::Excl => {
                        if o.end > o.start {
                            text::prev_grapheme(t, o.end)
                        } else {
                            return;
                        }
                    }
                    _ => o.end,
                };
                if sel.is_none() {
                    nv.anchor = o.start;
                } else if o.start < nv.anchor.min(nv.cursor) {
                    nv.anchor = o.start;
                }
                nv.cursor = end;
            }
        }
        self.vim.vis = Some(nv);
        self.vis_sync();
    }

    // ---- operators on the selection ---------------------------------------------------------------------

    pub(crate) fn visual_operator(&mut self, op: Op, reg: Option<char>, count: Option<usize>) {
        let Some(mut r) = self.visual_region() else { return };
        let kind = self.vim.vis.map(|v| v.kind);
        self.vis_exit();
        match op {
            Op::Indent | Op::Outdent if kind == Some(VKind::Block) => {
                self.shift_block(&r, count.unwrap_or(1), op == Op::Indent);
            }
            Op::Indent | Op::Outdent => {
                self.shift_lines(r.l0, r.l1, count.unwrap_or(1), op == Op::Indent);
                let rope = self.rp();
                let t = rope.slice(..);
                self.vset(first_nonblank(t, r.l0));
            }
            Op::Change if kind == Some(VKind::Block) => self.operate(op, r, reg, false),
            _ => {
                if matches!(op, Op::Delete | Op::Change | Op::Yank) && r.kind == RK::Char {
                    // multi-line `v` regions stay charwise
                }
                r.visual = true;
                self.operate(op, r, reg, false);
            }
        }
    }

    pub(crate) fn exec_visual_simple(&mut self, s: Simple, reg: Option<char>, count: Option<usize>) {
        let Some(v) = self.vim.vis else { return };
        let Some(r) = self.visual_region() else { return };
        let rope = self.rp();
        let t = rope.slice(..);
        match s {
            Simple::X => self.visual_operator(Op::Delete, reg, count),
            Simple::S => self.visual_operator(Op::Change, reg, count),
            Simple::BigX | Simple::BigD => {
                self.vis_exit();
                let mut rr = if v.kind == VKind::Block {
                    let mut b = r;
                    b.eol = true;
                    b
                } else {
                    region_lines(t, r.l0, r.l1, r.cur)
                };
                rr.visual = true;
                self.operate(Op::Delete, rr, reg, true);
            }
            Simple::BigC | Simple::BigS | Simple::Replace => {
                self.vis_exit();
                let mut rr = if v.kind == VKind::Block && s == Simple::BigC {
                    let mut b = r;
                    b.eol = true;
                    b
                } else {
                    region_lines(t, r.l0, r.l1, r.cur)
                };
                rr.visual = true;
                self.operate(Op::Change, rr, reg, true);
            }
            Simple::BigY => {
                self.vis_exit();
                let rr = if v.kind == VKind::Block { r } else { region_lines(t, r.l0, r.l1, r.cur) };
                self.operate(Op::Yank, rr, reg, false);
            }
            Simple::ReplaceChar(c) => {
                self.vis_exit();
                self.visual_replace(&r, c);
            }
            Simple::Tilde => {
                self.vis_exit();
                self.case_region(Op::Toggle, &r);
            }
            Simple::Undo => {
                self.vis_exit();
                self.case_region(Op::Lower, &r);
            }
            Simple::BigU => {
                self.vis_exit();
                self.case_region(Op::Upper, &r);
            }
            Simple::Join { spaces } => {
                self.vis_exit();
                let l1 = if r.l1 == r.l0 { r.l0 + 1 } else { r.l1 };
                self.join_lines_range(r.l0, l1, spaces);
            }
            Simple::Put { before, .. } => {
                self.vis_exit();
                self.visual_put(&r, v.kind, reg, count.unwrap_or(1), before);
            }
            Simple::Increment(d) => {
                self.vis_exit();
                self.visual_increment(&r, d * count.unwrap_or(1) as i64);
            }
            Simple::Insert | Simple::InsertLineStart => {
                self.vis_exit();
                if v.kind == VKind::Block {
                    let top = pos_at_col(t, r.l0, r.c0);
                    self.start_block_insert(r.l0, r.l1, r.c0, false, top, Some(r.c0));
                } else {
                    // `I` without a block (observed Vim behaviour): at the start of the first line, or
                    // at the cursor when the selection was extended upwards
                    let (la, lc) = (line_of(t, v.anchor), line_of(t, v.cursor));
                    let at = if lc < la { v.cursor } else { lstart(t, r.l0) };
                    self.vset(at);
                    self.vim_enter_insert(super::InsertAt::Here, 1);
                }
            }
            Simple::Append | Simple::AppendLineEnd => {
                self.vis_exit();
                if v.kind == VKind::Block {
                    let l = r.l0;
                    let at = if r.eol { lend(t, l) } else { pos_at_col(t, l, r.c1 + 1) };
                    self.start_block_insert(
                        r.l0,
                        r.l1,
                        if r.eol { usize::MAX } else { r.c1 + 1 },
                        true,
                        at,
                        Some(r.c0),
                    );
                } else {
                    // `A` without a block (observed Vim behaviour): after the cursor char; at the cursor
                    // in column 0 or on the line end; at column 0 of the anchor's line when the
                    // selection was extended upwards over several lines
                    let c = v.cursor.min(t.len_chars());
                    let (la, lc) = (line_of(t, v.anchor), line_of(t, c));
                    let at = if lc < la {
                        lstart(t, la)
                    } else if c >= lend(t, lc) {
                        // on the line end: after `$` before the last char, else after it
                        if self.vim.want == usize::MAX { last_char(t, lc) } else { lend(t, lc) }
                    } else if col_of(t, c) == 0 {
                        c
                    } else {
                        text::next_grapheme(t, c)
                    };
                    self.enter_insert_at_exact(at);
                }
            }
            Simple::SwapEnds => {
                if let Some(vv) = &mut self.vim.vis {
                    std::mem::swap(&mut vv.anchor, &mut vv.cursor);
                }
                self.vis_sync();
            }
            Simple::SwapCorners => {
                let mut nv = v;
                if v.kind == VKind::Block {
                    let (ca, cc) = (col_of(t, v.anchor), col_of(t, v.cursor));
                    let (la, lc) = (line_of(t, v.anchor), line_of(t, v.cursor));
                    nv.anchor = pos_at_col(t, la, cc);
                    nv.cursor = pos_at_col(t, lc, ca);
                } else {
                    std::mem::swap(&mut nv.anchor, &mut nv.cursor);
                }
                self.vim.vis = Some(nv);
                self.vis_sync();
            }
            Simple::VisualChar => self.vis_enter(VKind::Char),
            Simple::VisualLine => self.vis_enter(VKind::Line),
            Simple::VisualBlock => self.vis_enter(VKind::Block),
            Simple::Reselect => self.vis_reselect(),
            Simple::Colon => {
                self.vis_exit();
                self.open_prompt(crate::prompt::PromptKind::Command, ":", "'<,'>", 0);
            }
            Simple::Leader(c) => self.leader_key(c),
            Simple::OpenBelow | Simple::OpenAbove => {
                // `o` swaps ends in Visual mode; classify maps it, nothing to do here
            }
            Simple::Dot | Simple::RecordStart(_) | Simple::RecordStop | Simple::Play(_) => {
                self.exec_simple(s, reg, count)
            }
            Simple::Esc | Simple::Nop => {}
            Simple::Scroll(_) | Simple::JumpBack | Simple::JumpForward | Simple::SetMark(_) => {
                self.exec_simple(s, reg, count)
            }
            Simple::Redo
            | Simple::InsertCol0
            | Simple::InsertLast
            | Simple::SaveQuit
            | Simple::QuitNoSave => {}
        }
        let _ = Effect::Quit;
    }

    /// Block `>` / `<`: shift only the text from the block's left edge onwards.
    fn shift_block(&mut self, r: &Region, times: usize, right: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let mut changes = Vec::new();
        for l in r.l0..=r.l1 {
            let le = lend(t, l);
            let at = pos_at_col(t, l, r.c0);
            if at >= le {
                continue;
            }
            if right {
                changes.push(Change { from: at, to: at, text: " ".repeat(4 * times) });
            } else {
                let mut n = 0;
                while n < 4 * times && at + n < le && t.char(at + n) == ' ' {
                    n += 1;
                }
                if n > 0 {
                    changes.push(Change { from: at, to: at + n, text: String::new() });
                }
            }
        }
        self.vapply(changes);
        let rope = self.rp();
        let t2 = rope.slice(..);
        self.vset(pos_at_col(t2, r.l0, r.c0));
    }

    fn visual_replace(&mut self, r: &Region, c: char) {
        let rope = self.rp();
        let t = rope.slice(..);
        let mut changes = Vec::new();
        let rep = |old: &str| -> String {
            old.chars().map(|x| if x == '\n' || x == '\r' { x } else { c }).collect()
        };
        match r.kind {
            RK::Char | RK::Line => {
                let end = if r.kind == RK::Line { lend(t, r.l1) } else { r.end };
                let old = t.slice(r.start..end).to_string();
                changes.push(Change { from: r.start, to: end, text: rep(&old) });
            }
            RK::Block => {
                for l in r.l0..=r.l1 {
                    let (a, b) = block_cols(t, l, r);
                    if b > a {
                        let old = t.slice(a..b).to_string();
                        changes.push(Change { from: a, to: b, text: rep(&old) });
                    }
                }
            }
        }
        self.vapply(changes);
        self.set_change_marks(r.start, r.end.saturating_sub(1));
        let rope = self.rp();
        let t2 = rope.slice(..);
        let at = if r.kind == RK::Block { pos_at_col(t2, r.l0, r.c0) } else { r.start };
        self.vset(at);
    }

    fn visual_increment(&mut self, r: &Region, delta: i64) {
        let rope = self.rp();
        let t = rope.slice(..);
        let mut changes = Vec::new();
        for l in r.l0..=r.l1 {
            let (mut lo, mut hi) = (lstart(t, l), lend(t, l));
            if r.kind == RK::Char {
                lo = lo.max(r.start);
                hi = hi.min(r.end);
            } else if r.kind == RK::Block {
                let (a, b) = block_cols(t, l, r);
                lo = a;
                hi = b;
            }
            if lo >= hi {
                continue;
            }
            if let Some((a, b, new)) = number_edit(t, lo, hi, delta, false) {
                changes.push(Change { from: a, to: b, text: new });
            }
        }
        self.vapply(changes);
        let rope = self.rp();
        let t2 = rope.slice(..);
        self.vset(if r.kind == RK::Block { pos_at_col(t2, r.l0, r.c0) } else { r.start });
    }

    /// Visual `p`: replace the selection with a register (see `:help v_p`).
    fn visual_put(&mut self, r: &Region, kind: VKind, reg: Option<char>, count: usize, keep_unnamed: bool) {
        let Some(src) = self.reg_get(reg) else {
            // Vim deletes the selection even though there is nothing to put
            let mut rr = *r;
            rr.visual = true;
            self.operate(Op::Delete, rr, None, false);
            self.status(format!("E353: Nothing in register {}", reg.unwrap_or('"')));
            return;
        };
        // delete the selection into the registers (the saved copy is what gets put)
        let rope = self.rp();
        let t = rope.slice(..);
        let nl_before = nlines(t);
        let start_before = r.start;
        let mut rr = *r;
        rr.visual = true;
        let del_reg = self.vim_put_delete_reg(keep_unnamed);
        self.operate(Op::Delete, rr, del_reg, true);
        let rope = self.rp();
        let t2 = rope.slice(..);
        let cur = self.vcur();
        let mut flags = PutFlags::default();
        match kind {
            VKind::Line => flags.force_line = true,
            VKind::Char => flags.split_line = src.kind == RegKind::Line,
            VKind::Block => {}
        }
        // the deleted text was at the end of the line/buffer: put after the cursor
        let forward = match kind {
            VKind::Line => {
                nlines(t2) < nl_before && line_of(t2, cur) < r.l0
                    || (nlines(t2) < nl_before && r.l0 >= nlines(t2))
            }
            _ => {
                cur < start_before
                    && lend(t2, line_of(t2, cur)) <= start_before
                    && !line_empty(t2, line_of(t2, cur))
            }
        };
        let before = !forward;
        self.put_reg(&src, count, before, false, flags);
    }

    /// Visual `P` keeps the unnamed register: its deletion goes to the black hole.
    fn vim_put_delete_reg(&self, keep: bool) -> Option<char> {
        keep.then_some('_')
    }
}
