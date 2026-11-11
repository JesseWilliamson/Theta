//! Insert and Replace mode.
//!
//! Typing, auto-pairs and auto-indent are the shared `edit.rs` routines (the cursor is an empty
//! selection, exactly as in Helix's Insert mode). An insert *session* runs from the command
//! that entered Insert mode to the Escape that leaves it: it is one undo step, one `.` unit and
//! the thing `3ihello<Esc>` repeats.

use super::geom::*;
use super::{InsertAt, Reg, RegKind};
use crate::edit::EditSpec;
use crate::editor::{Editor, Effect, Mode};
use crate::keys::{Key, KeyInput};
use crate::selection::{Selection, Selections};
use crate::text;
use crate::transaction::Change;

#[derive(Default)]
pub(crate) struct Session {
    pub count: usize,
    /// `o` / `O`: repeating the session re-opens a line each time (`Some(below)`)
    pub open: Option<bool>,
    pub start: usize,
    pub keys: Vec<KeyInput>,
    pub in_repeat: bool,
    pub await_reg: bool,
    /// Replace mode: the chars that were overwritten (None = appended), for Backspace
    pub replaced: Vec<Option<char>>,
    /// the current line holds nothing but auto-indent: Escape / Enter remove it (`autoindent`)
    pub ai_only: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct BlockInsert {
    pub l0: usize,
    pub l1: usize,
    /// grapheme column where the text goes (`usize::MAX` = end of every line)
    pub col: usize,
    pub append: bool,
    pub start: usize,
    /// left edge of the block: where the cursor returns to
    pub left: usize,
}

fn ctrl_is(k: &KeyInput, c: char) -> bool {
    matches!(k.key, Key::Char(x) if x.eq_ignore_ascii_case(&c)) && k.modifiers.ctrl && !k.modifiers.alt
}

impl Editor {
    // ---- entering ----------------------------------------------------------------------------

    pub(crate) fn vim_enter_insert(&mut self, at: InsertAt, count: usize) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let pos = match at {
            InsertAt::Here => cur,
            InsertAt::After => {
                if line_empty(t, l) || cur >= lend(t, l) {
                    cur
                } else {
                    text::next_grapheme(t, cur)
                }
            }
            InsertAt::FirstNonBlank => text::first_non_blank(t, l).unwrap_or_else(|| lend(t, l)),
            InsertAt::LineEnd => lend(t, l),
            InsertAt::Col0 => lstart(t, l),
            InsertAt::LastInsert => self.vim.marks.get(&'^').copied().unwrap_or(cur).min(t.len_chars()),
        };
        self.start_insert_at(pos, count, false);
    }

    pub(crate) fn enter_insert_at_exact(&mut self, pos: usize) {
        self.start_insert_at(pos, 1, false);
    }

    pub(crate) fn start_insert_at(&mut self, pos: usize, count: usize, _open: bool) {
        self.vim.vis = None;
        self.mode = Mode::Insert;
        let t = self.rope.slice(..);
        let p = text::floor_grapheme(t, pos.min(t.len_chars()));
        self.set_sels(Selections::single(Selection::point(p)));
        self.vim.ins = Some(Session { count: count.max(1), start: p, ..Default::default() });
        self.vim.cmd.insert_started = true;
    }

    pub(crate) fn open_line_cmd(&mut self, below: bool, count: usize) {
        self.open_line_once(below);
        if let Some(s) = &mut self.vim.ins {
            s.count = count.max(1);
            s.open = Some(below);
        }
    }

    fn open_line_once(&mut self, below: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let l = line_of(t, self.vcur());
        let le = self.line_ending;
        self.vim.edit_at = Some(self.vcur());
        let indent = self.indent_for_new_line(l);
        let n = indent.chars().count();
        let (at, ins, cur) = if below {
            let at = lend(t, l);
            (at, format!("{le}{indent}"), at + le.chars().count() + n)
        } else {
            let at = lstart(t, l);
            (at, format!("{indent}{le}"), at + n)
        };
        self.vim.vis = None;
        self.mode = Mode::Insert;
        let tx = crate::transaction::Transaction::new(vec![Change { from: at, to: at, text: ins }]);
        self.apply_with(tx, Some(Selections::single(Selection::point(cur))));
        self.set_change_marks(cur, cur);
        self.vim.ins =
            Some(Session { count: 1, start: cur, ai_only: !indent.is_empty(), ..Default::default() });
        self.vim.cmd.insert_started = true;
    }

    pub(crate) fn start_replace(&mut self, count: usize) {
        let cur = self.vcur();
        self.vim.vis = None;
        self.mode = Mode::Replace;
        let t = self.rope.slice(..);
        let p = text::floor_grapheme(t, cur.min(t.len_chars()));
        self.set_sels(Selections::single(Selection::point(p)));
        self.vim.ins = Some(Session { count: count.max(1), start: p, ..Default::default() });
    }

    pub(crate) fn start_block_insert(
        &mut self,
        l0: usize,
        l1: usize,
        col: usize,
        append: bool,
        at: usize,
        left: usize,
    ) {
        let rope = self.rp();
        let t = rope.slice(..);
        let mut at = at;
        // `A` on a short first row pads it up to the block edge
        if append && col != usize::MAX && l0 < nlines(t) {
            let lc = col_of(t, lend(t, l0));
            if lc < col {
                let pad = " ".repeat(col - lc);
                let le = lend(t, l0);
                self.vapply(vec![Change { from: le, to: le, text: pad }]);
                let rope = self.rp();
                let t2 = rope.slice(..);
                at = pos_at_col(t2, l0, col);
            }
        }
        self.start_insert_at(at, 1, false);
        self.vim.block_insert = Some(BlockInsert { l0, l1, col, append, start: at, left });
    }

    // ---- keys ----------------------------------------------------------------------------------------

    pub(crate) fn vim_insert_key(&mut self, k: &KeyInput, esc: bool) -> bool {
        if esc {
            self.vim_leave_insert();
            return true;
        }
        if self.vim.ins.as_ref().is_some_and(|s| s.await_reg) {
            if let Some(s) = &mut self.vim.ins {
                s.await_reg = false;
            }
            if let Some(c) = k.text_char() {
                if let Some(r) = self.reg_get(Some(c)) {
                    self.insert_register_text(&r);
                }
            }
            self.log_key(k);
            return true;
        }
        let m = &k.modifiers;
        if let Some(c) = k.text_char() {
            self.log_key(k);
            if let Some(s) = &mut self.vim.ins {
                s.ai_only = false;
            }
            self.type_char(c);
            return true;
        }
        let ctrl = m.ctrl && !m.alt && !m.platform;
        if ctrl {
            match &k.key {
                Key::Char(c) => match c.to_ascii_lowercase() {
                    'w' => self.ins_delete_word_back(),
                    'u' => self.ins_delete_to_start(),
                    'h' => self.vim_backspace(),
                    't' => self.ins_shift_line(true),
                    'd' => self.ins_shift_line(false),
                    'j' | 'm' => self.insert_newline(),
                    'n' | 'p' => self.effects.push(Effect::RequestCompletion),
                    'r' => {
                        if let Some(s) = &mut self.vim.ins {
                            s.await_reg = true;
                        }
                    }
                    'o' => {
                        self.begin_one_shot();
                        return true;
                    }
                    _ => return false,
                },
                _ => return false,
            }
            self.log_key(k);
            return true;
        }
        if m.alt || m.platform {
            return false;
        }
        match &k.key {
            Key::Enter => {
                if self.vim.ins.as_ref().is_some_and(|s| s.ai_only) {
                    self.drop_ai_indent();
                }
                self.insert_newline();
                let t = self.rope.slice(..);
                let l = line_of(t, self.vcur());
                let ai = !text::leading_whitespace(t, l).is_empty() && first_nonblank_none(t, l);
                if let Some(s) = &mut self.vim.ins {
                    s.ai_only = ai;
                }
            }
            Key::Tab => self.ins_tab(),
            Key::Backspace => self.vim_backspace(),
            Key::Delete => self.delete_forward(),
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => self.ins_move(&k.key),
            Key::PageUp | Key::PageDown => {
                let lines = self.viewport.1 as i32;
                let d = if k.key == Key::PageDown { lines } else { -lines };
                self.vim_scroll_effect(d);
            }
            _ => return false,
        }
        self.log_key(k);
        true
    }

    /// Remove the auto-indent of a line the user typed nothing on.
    fn drop_ai_indent(&mut self) {
        let t = self.rope.slice(..);
        let l = line_of(t, self.vcur());
        let (ls, le) = (lstart(t, l), lend(t, l));
        if le > ls && (ls..le).all(|i| is_blank_char(t.char(i))) {
            self.vapply(vec![Change { from: ls, to: le, text: String::new() }]);
            self.set_sels(Selections::single(Selection::point(ls)));
        }
        if let Some(s) = &mut self.vim.ins {
            s.ai_only = false;
        }
    }

    /// Tab inserts spaces up to the next multiple of four (`expandtab`, `softtabstop=4`).
    fn ins_tab(&mut self) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let col = col_of(t, cur);
        let n = 4 - col % 4;
        self.insert_plain(&" ".repeat(n));
    }

    fn log_key(&mut self, k: &KeyInput) {
        if let Some(s) = &mut self.vim.ins {
            if !s.in_repeat {
                s.keys.push(k.clone());
            }
        }
    }

    fn ins_move(&mut self, key: &Key) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let pos = match key {
            Key::Left => {
                if cur > lstart(t, l) {
                    text::prev_grapheme(t, cur)
                } else {
                    cur
                }
            }
            Key::Right => {
                if cur < lend(t, l) {
                    text::next_grapheme(t, cur)
                } else {
                    cur
                }
            }
            Key::Home => lstart(t, l),
            Key::End => lend(t, l),
            Key::Up | Key::Down => {
                let down = *key == Key::Down;
                let target = if down {
                    if l + 1 >= nlines(t) {
                        return;
                    }
                    l + 1
                } else {
                    if l == 0 {
                        return;
                    }
                    l - 1
                };
                pos_at_col(t, target, self.vim.want.min(col_of(t, cur).max(self.vim.want)))
            }
            _ => cur,
        };
        if matches!(key, Key::Up | Key::Down) {
            self.vim.keep_want = true;
        }
        self.vset(pos);
        if !matches!(key, Key::Up | Key::Down) {
            let t = self.rope.slice(..);
            self.vim.want = col_of(t, pos);
        }
    }

    pub(crate) fn vim_backspace(&mut self) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        if cur == 0 {
            return;
        }
        let prev = text::prev_grapheme(t, cur);
        let pc = t.char(prev);
        let next = if cur < t.len_chars() { Some(t.char(cur)) } else { None };
        let pair = matches!(
            (pc, next),
            ('(', Some(')'))
                | ('[', Some(']'))
                | ('{', Some('}'))
                | ('\'', Some('\''))
                | ('"', Some('"'))
                | ('`', Some('`'))
        );
        let spec = if pair {
            EditSpec { from: prev, to: cur + 1, text: String::new(), back: 0 }
        } else {
            EditSpec { from: prev, to: cur, text: String::new(), back: 0 }
        };
        self.apply_specs(vec![Some(spec)]);
    }

    fn ins_delete_word_back(&mut self) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let ls = lstart(t, l);
        if cur == 0 {
            return;
        }
        let from = if cur == ls {
            // join with the previous line
            lend(t, l - 1)
        } else {
            let mut p = cur;
            while p > ls && is_blank_char(t.char(p - 1)) {
                p -= 1;
            }
            if p > ls {
                let class = |c: char| text::is_word_char(c);
                let w = class(t.char(p - 1));
                while p > ls && !is_blank_char(t.char(p - 1)) && class(t.char(p - 1)) == w {
                    p -= 1;
                }
            }
            p
        };
        // the first CTRL-W stops at the start of the inserted text (Vim's `backspace=start`)
        let from = match self.vim.ins.as_ref().map(|s| s.start) {
            Some(s) if s < cur && s > from && s >= ls => s,
            _ => from,
        };
        self.apply_specs(vec![Some(EditSpec { from, to: cur, text: String::new(), back: 0 })]);
    }

    fn ins_delete_to_start(&mut self) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let ls = lstart(t, l);
        let start = self.vim.ins.as_ref().map(|s| s.start);
        let from = match start {
            Some(s) if s < cur && s >= ls => s,
            _ => {
                let fnb = text::first_non_blank(t, l).unwrap_or(ls);
                if fnb < cur { fnb } else { ls }
            }
        };
        if from < cur {
            self.apply_specs(vec![Some(EditSpec { from, to: cur, text: String::new(), back: 0 })]);
        } else if cur == ls && l > 0 {
            self.vim_backspace();
        }
    }

    fn ins_shift_line(&mut self, right: bool) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let ls = lstart(t, l);
        let old_len = t.slice(ls..lend(t, l)).len_chars();
        let before_chars = cur - ls;
        // the indent changes (all whitespace is rewritten as spaces); the cursor keeps its place
        // relative to the text
        let mut ws = 0;
        while ls + ws < lend(t, l) && is_blank_char(t.char(ls + ws)) {
            ws += 1;
        }
        let w = indent_width(t, l);
        let nw = if right { w + 4 } else { w.saturating_sub(4) };
        self.vapply(vec![Change { from: ls, to: ls + ws, text: " ".repeat(nw) }]);
        let _ = old_len;
        let new_cur =
            if before_chars >= ws { ls + nw + (before_chars - ws) } else { ls + nw.min(before_chars) };
        self.vset(new_cur);
    }

    fn insert_register_text(&mut self, r: &Reg) {
        let mut text = r.text.clone();
        if let RegKind::Block(_) = r.kind {
            text = text.replace('\n', " ");
        }
        let body = self.nl_str(&text);
        self.insert_plain(&body);
    }

    fn begin_one_shot(&mut self) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        self.vim.ins_eol = cur >= lend(t, l) && !line_empty(t, l);
        self.vim.one_shot = 2;
        self.mode = Mode::Normal;
        let c = clamp_normal(t, cur);
        self.set_sels(Selections::single(Selection::point(c)));
        self.vim.pend = Default::default();
    }

    /// After the one command run with `ctrl-o`: back to Insert mode.
    pub(crate) fn finish_one_shot(&mut self) {
        self.vim.one_shot = 0;
        if self.vim.vis.is_some() || self.mode != Mode::Normal {
            return;
        }
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let at = if (self.vim.ins_eol && cur == last_char(t, l) && !line_empty(t, l))
            || self.vim.want == usize::MAX
        {
            lend(t, l)
        } else {
            cur
        };
        self.mode = Mode::Insert;
        self.set_sels(Selections::single(Selection::point(at)));
    }

    // ---- leaving -------------------------------------------------------------------------------------------

    pub(crate) fn vim_leave_insert(&mut self) {
        let replace = self.mode == Mode::Replace;
        let sess = self.vim.ins.take();
        let mut sess = sess.unwrap_or_default();
        // `3ihello<Esc>`: run the session's keys again
        if sess.count > 1 && !sess.keys.is_empty() {
            let keys = sess.keys.clone();
            self.vim.ins = Some(Session {
                count: 1,
                in_repeat: true,
                start: sess.start,
                open: sess.open,
                ..Default::default()
            });
            for _ in 1..sess.count {
                if sess.open.is_some() {
                    // every repeat opens its line below the previous one, `O` included
                    self.open_line_once(true);
                    if let Some(s) = &mut self.vim.ins {
                        s.in_repeat = true;
                    }
                }
                for k in &keys {
                    if replace {
                        self.vim_replace_key(k, false);
                    } else {
                        self.vim_insert_key(k, false);
                    }
                }
            }
            sess.in_repeat = false;
            self.vim.ins = None;
        }
        if sess.ai_only {
            self.drop_ai_indent();
        }
        let typed = typed_text(&sess.keys);
        if !typed.is_empty() || sess.keys.is_empty() {
            self.vim.last_insert = typed;
        }
        if let Some(b) = self.vim.block_insert.take() {
            if let Some(at) = self.finish_block_insert(b) {
                self.vim.marks.insert('^', at);
                self.mode = Mode::Normal;
                self.set_sels(Selections::single(Selection::point(at)));
                self.effects.push(Effect::DismissCompletion);
                return;
            }
        }
        let cur = self.vcur();
        self.vim.marks.insert('^', cur);
        self.vim.marks.insert(']', cur);
        self.vim.marks.insert('[', sess.start.min(cur));
        self.vim.marks.insert('.', cur.saturating_sub(1).max(sess.start.min(cur)));
        self.mode = Mode::Normal;
        let t = self.rope.slice(..);
        let l = line_of(t, cur);
        let left = if cur > lstart(t, l) { text::prev_grapheme(t, cur) } else { cur };
        let c = clamp_normal(t, left);
        self.set_sels(Selections::single(Selection::point(c)));
        self.effects.push(Effect::DismissCompletion);
    }

    /// Copy the text typed on the first row to the other rows. Returns the cursor position when
    /// something was copied (it goes to the block's top-left corner).
    fn finish_block_insert(&mut self, b: BlockInsert) -> Option<usize> {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        if cur < b.start || line_of(t, cur) != line_of(t, b.start) {
            return None;
        }
        let inserted = t.slice(b.start..cur).to_string();
        if inserted.is_empty() || inserted.contains('\n') {
            return None;
        }
        let mut changes = Vec::new();
        for l in b.l0 + 1..=b.l1.min(nlines(t) - 1) {
            let le = lend(t, l);
            if b.col == usize::MAX {
                changes.push(Change { from: le, to: le, text: inserted.clone() });
                continue;
            }
            let lc = col_of(t, le);
            if lc < b.col {
                if b.append {
                    changes.push(Change {
                        from: le,
                        to: le,
                        text: format!("{}{inserted}", " ".repeat(b.col - lc)),
                    });
                }
            } else {
                let at = pos_at_col(t, l, b.col);
                changes.push(Change { from: at, to: at, text: inserted.clone() });
            }
        }
        self.vapply(changes);
        self.vim.marks.insert('[', b.start);
        // the cursor goes back to the top-left of the block
        let t2 = self.rope.slice(..);
        let cur = clamp_normal(t2, pos_at_col(t2, b.l0, b.left));
        Some(cur)
    }

    // ---- Replace mode ------------------------------------------------------------------------------------------------

    pub(crate) fn vim_replace_key(&mut self, k: &KeyInput, esc: bool) -> bool {
        if esc {
            self.vim_leave_insert();
            return true;
        }
        let m = &k.modifiers;
        if let Some(c) = k.text_char() {
            self.log_key(k);
            self.replace_type(c);
            return true;
        }
        if m.ctrl && !m.alt && !m.platform {
            if ctrl_is(k, 'h') {
                self.log_key(k);
                self.replace_backspace();
                return true;
            }
            if ctrl_is(k, 'w') || ctrl_is(k, 'u') {
                return true;
            }
            return false;
        }
        if m.alt || m.platform {
            return false;
        }
        match &k.key {
            Key::Enter => {
                self.log_key(k);
                self.insert_newline();
                if let Some(s) = &mut self.vim.ins {
                    s.replaced.push(None);
                }
            }
            Key::Backspace => {
                self.log_key(k);
                self.replace_backspace();
            }
            Key::Tab => {
                self.log_key(k);
                self.replace_type('\t');
            }
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
                self.ins_move(&k.key);
                self.log_key(k);
            }
            _ => return false,
        }
        true
    }

    fn replace_type(&mut self, c: char) {
        let rope = self.rp();
        let t = rope.slice(..);
        let cur = self.vcur();
        let l = line_of(t, cur);
        let le = lend(t, l);
        let (to, old) = if cur < le {
            let n = text::next_grapheme(t, cur);
            (n, Some(t.slice(cur..n).to_string()))
        } else {
            (cur, None)
        };
        let _ = old;
        let orig = if cur < le { Some(t.char(cur)) } else { None };
        self.vapply(vec![Change { from: cur, to, text: c.to_string() }]);
        if let Some(s) = &mut self.vim.ins {
            s.replaced.push(orig);
        }
        self.vset(cur + 1);
    }

    fn replace_backspace(&mut self) {
        let t = self.rope.slice(..);
        let cur = self.vcur();
        let popped = self.vim.ins.as_mut().and_then(|s| s.replaced.pop());
        match popped {
            None => {
                // nothing typed in this session: just move left
                if cur > 0 && cur > lstart(t, line_of(t, cur)) {
                    let p = text::prev_grapheme(t, cur);
                    self.vset(p);
                }
            }
            Some(orig) => {
                let prev = text::prev_grapheme(t, cur);
                let restore = orig.map(|c| c.to_string()).unwrap_or_default();
                self.vapply(vec![Change { from: prev, to: cur, text: restore }]);
                self.vset(prev);
            }
        }
    }
}

fn first_nonblank_none(t: text::Text, l: usize) -> bool {
    text::first_non_blank(t, l).is_none()
}

/// The text a sequence of insert-mode keys produces (what the `.` register holds).
fn typed_text(keys: &[KeyInput]) -> String {
    let mut s = String::new();
    for k in keys {
        if let Some(c) = k.text_char() {
            s.push(c);
        } else if k.key == Key::Enter {
            s.push('\n');
        } else if k.key == Key::Tab {
            s.push('\t');
        } else if k.key == Key::Backspace {
            s.pop();
        }
    }
    s
}
