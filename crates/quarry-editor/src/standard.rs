//! The Standard (non-modal) keymap, following Zed's default bindings. `ctrl` and `platform`
//! (cmd) are interchangeable "primary" modifiers so one table serves Linux and macOS.

use crate::edit::{EditSpec, std_word_left, std_word_right};
use crate::editor::{Editor, Effect, ScrollRequest};
use crate::keys::{Key, KeyInput};
use crate::prompt::PromptKind;
use crate::selection::{Selection, Selections};
use crate::text::{self, Text};
use crate::transaction::{Change, Transaction};

#[derive(Default)]
pub(crate) struct StdState {
    /// First key of a two-key chord (`ctrl-k` ...).
    pub chord: Option<KeyInput>,
    /// A run of typing is in progress and its undo group is still open.
    pub run_open: bool,
    pub was_run: bool,
    pub last_typed: Option<char>,
    /// Text of the last whole-line copy, so pasting it can insert above the cursor line.
    pub linewise_clip: Option<String>,
}

impl Editor {
    fn std_move(&mut self, extend: bool, f: impl Fn(Text, &Selection) -> usize) {
        let t = self.rope.slice(..);
        let v: Vec<Selection> = self
            .sels
            .v
            .iter()
            .map(|s| {
                let h = f(t, s);
                if extend { Selection::new(s.anchor, h) } else { Selection::point(h) }
            })
            .collect();
        let p = self.sels.primary;
        self.set_sels(Selections::new(v, p));
    }

    fn std_vertical(&mut self, down: bool, lines: usize, extend: bool) {
        let t = self.rope.slice(..);
        let goals = self.vert.clone().filter(|g| g.len() == self.sels.len());
        let mut new_goals = Vec::new();
        let nlines = t.len_lines();
        let mut v = Vec::new();
        for (i, s) in self.sels.v.iter().enumerate() {
            let line = t.char_to_line(s.head);
            let ls = t.line_to_char(line);
            let goal = goals.as_ref().map_or(s.head - ls, |g| g[i]);
            new_goals.push(goal);
            let target = if down {
                if line + lines >= nlines { None } else { Some(line + lines) }
            } else if line < lines {
                None
            } else {
                Some(line - lines)
            };
            let h = match target {
                Some(l) => {
                    let nls = t.line_to_char(l);
                    nls + goal.min(text::line_end(t, l) - nls)
                }
                // Past the first/last line the cursor goes to the document edge, like most editors.
                None => {
                    if down {
                        t.len_chars()
                    } else {
                        0
                    }
                }
            };
            v.push(if extend { Selection::new(s.anchor, h) } else { Selection::point(h) });
        }
        let p = self.sels.primary;
        self.set_sels(Selections::new(v, p));
        self.vert = Some(new_goals);
        self.vert_keep = true;
    }

    fn smart_home(t: Text, head: usize) -> usize {
        let line = t.char_to_line(head);
        let ls = t.line_to_char(line);
        match text::first_non_blank(t, line) {
            Some(p) if p != head => p,
            _ => ls,
        }
    }

    pub(crate) fn standard_key(&mut self, k: &KeyInput) -> bool {
        if let Some(first) = self.std.chord.take() {
            let _ = first;
            if k.primary() && matches!(k.key, Key::Char('t')) {
                self.effects.push(Effect::OpenThemePicker);
            }
            return true;
        }
        let m = &k.modifiers;
        let prim = k.primary();
        let (alt, shift) = (m.alt, m.shift);
        // Letter shortcuts: lowercase letter + shift flag recovered from the case.
        let (lc, upper) = match k.key {
            Key::Char(c) => (c.to_ascii_lowercase(), c.is_ascii_uppercase()),
            _ => ('\0', false),
        };
        if !matches!(k.key, Key::Char(_)) || prim || alt {
            self.std.last_typed = None;
        }

        // typing
        if let Some(c) = k.text_char() {
            self.std_type(c);
            return true;
        }

        let key = k.key.clone();
        match key {
            Key::Escape => {
                self.effects.push(Effect::DismissCompletion);
                if self.sels.len() > 1 {
                    let p = self.sels.primary();
                    self.set_primary_only(p);
                } else if !self.sels.primary().is_empty() {
                    let h = self.sels.primary().head;
                    self.set_primary_only(Selection::point(h));
                }
            }
            Key::Left | Key::Right => {
                let right = key == Key::Right;
                if prim && !alt {
                    if m.platform {
                        // cmd-left/right: line edges (macOS)
                        if right {
                            self.std_move(shift, |t, s| text::line_end(t, t.char_to_line(s.head)));
                        } else {
                            self.std_move(shift, |t, s| Self::smart_home(t, s.head));
                        }
                    } else if right {
                        self.std_move(shift, |t, s| std_word_right(t, s.head));
                    } else {
                        self.std_move(shift, |t, s| std_word_left(t, s.head));
                    }
                } else if alt {
                    if right {
                        self.std_move(shift, |t, s| std_word_right(t, s.head));
                    } else {
                        self.std_move(shift, |t, s| std_word_left(t, s.head));
                    }
                } else if !shift && self.sels.v.iter().any(|s| !s.is_empty()) {
                    let t = self.rope.slice(..);
                    let v: Vec<Selection> = self
                        .sels
                        .v
                        .iter()
                        .map(|s| {
                            if s.is_empty() {
                                let h = if right {
                                    text::next_grapheme(t, s.head)
                                } else {
                                    text::prev_grapheme(t, s.head)
                                };
                                Selection::point(h)
                            } else {
                                Selection::point(if right { s.to() } else { s.from() })
                            }
                        })
                        .collect();
                    let p = self.sels.primary;
                    self.set_sels(Selections::new(v, p));
                } else if right {
                    self.std_move(shift, |t, s| text::next_grapheme(t, s.head));
                } else {
                    self.std_move(shift, |t, s| text::prev_grapheme(t, s.head));
                }
            }
            Key::Up | Key::Down => {
                let down = key == Key::Down;
                if prim && alt {
                    self.add_cursor(down);
                } else if alt && shift {
                    if down {
                        self.duplicate_lines();
                    } else {
                        self.add_cursor(false);
                    }
                } else if alt {
                    self.move_lines(down);
                } else if m.platform {
                    let len = self.rope.len_chars();
                    self.std_move(shift, |_, _| if down { len } else { 0 });
                } else if prim {
                    // ctrl-up/down scroll the view by a line in Zed
                    self.effects.push(Effect::Scroll(ScrollRequest::Lines(if down { 1 } else { -1 })));
                } else {
                    self.std_vertical(down, 1, shift);
                }
            }
            Key::Home => {
                if prim {
                    self.std_move(shift, |_, _| 0);
                } else {
                    self.std_move(shift, |t, s| Self::smart_home(t, s.head));
                }
            }
            Key::End => {
                if prim {
                    let len = self.rope.len_chars();
                    self.std_move(shift, |_, _| len);
                } else {
                    self.std_move(shift, |t, s| text::line_end(t, t.char_to_line(s.head)));
                }
            }
            Key::PageUp | Key::PageDown => {
                let lines = self.lines_viewport();
                self.std_vertical(key == Key::PageDown, lines, shift);
                let l = lines as i32;
                self.effects.push(Effect::Scroll(ScrollRequest::Lines(if key == Key::PageDown {
                    l
                } else {
                    -l
                })));
            }
            Key::Enter => self.insert_newline(),
            Key::Backspace => {
                if m.platform {
                    if self.sels.v.iter().any(|s| !s.is_empty()) {
                        self.backspace();
                    } else {
                        self.kill_to_line_start();
                    }
                } else if prim || alt {
                    self.delete_word_back();
                } else {
                    self.backspace();
                }
            }
            Key::Delete => {
                if prim || alt {
                    self.delete_word_forward();
                } else {
                    self.delete_forward();
                }
            }
            Key::Tab => {
                if prim {
                    self.effects.push(if shift { Effect::PreviousTab } else { Effect::NextTab });
                } else if shift {
                    self.indent_lines(1, false);
                } else if self.sels.v.iter().any(|s| !s.is_empty()) {
                    self.indent_lines(1, true);
                } else {
                    self.insert_tab();
                }
            }
            Key::F(1) => self.effects.push(Effect::OpenCommandPalette),
            Key::F(3) => self.search_repeat(shift, 1, false),
            Key::Char(_) if prim && !alt => return self.std_primary_char(lc, upper),
            Key::Char(_) if prim && alt => match lc {
                'l' => self.effects.push(Effect::FormatDocument),
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    fn std_primary_char(&mut self, lc: char, upper: bool) -> bool {
        match (lc, upper) {
            ('a', false) => {
                let len = self.rope.len_chars();
                self.set_primary_only(Selection::new(0, len));
            }
            ('c', false) => self.std_copy(false),
            ('x', false) => self.std_copy(true),
            ('v', false) => match self.clipboard.clone() {
                Some(t) => self.std_paste(&t, true),
                None => self.status("clipboard is empty"),
            },
            ('z', false) => {
                if !self.undo() {
                    self.status("nothing to undo");
                }
            }
            ('z', true) | ('y', false) => {
                if !self.redo() {
                    self.status("nothing to redo");
                }
            }
            ('/', _) => self.toggle_line_comments(),
            ('d', false) => self.select_next_occurrence(),
            ('d', true) => self.duplicate_lines(),
            ('l', false) => self.select_lines(),
            ('l', true) => self.select_all_occurrences(),
            ('k', false) => self.std.chord = Some(KeyInput::parse("ctrl-k").unwrap()),
            ('k', true) => self.delete_lines(),
            ('f', false) => {
                let init = self.selected_text().filter(|s| !s.contains('\n')).unwrap_or_default();
                self.open_prompt(PromptKind::Search { backwards: false }, "find:", &init, 1);
            }
            ('g', false) => self.open_prompt(PromptKind::Goto, "goto line:", "", 1),
            ('s', false) => self.effects.push(Effect::Save),
            ('w', false) => self.effects.push(Effect::CloseTab),
            ('n', false) | ('t', false) => self.effects.push(Effect::NewTab),
            ('p', true) => self.effects.push(Effect::OpenCommandPalette),
            ('p', false) => self.effects.push(Effect::OpenTablePicker),
            ('i', true) => self.effects.push(Effect::FormatDocument),
            ('b', false) => self.effects.push(Effect::ToggleExplorer),
            ('j', false) => self.effects.push(Effect::ToggleBottomPanel),
            ('e', true) => self.effects.push(Effect::FocusExplorer),
            _ => return false,
        }
        true
    }

    /// Typing a character: replaces selections, auto-pairs, and groups runs for undo.
    fn std_type(&mut self, c: char) {
        let boundary =
            self.std.was_run && self.std.last_typed.is_some_and(|p| p.is_whitespace()) && !c.is_whitespace();
        if boundary {
            self.commit_group();
            self.group_base = self.sels.clone();
        }
        if !self.surround_selection_with(c) {
            self.type_char(c);
        }
        self.std.run_open = true;
        self.std.last_typed = Some(c);
    }

    pub(crate) fn std_paste_text(&mut self, text: &str) {
        self.std_paste(text, false);
    }

    fn std_paste(&mut self, text: &str, allow_linewise: bool) {
        let text = text.replace("\r\n", "\n");
        let le = self.line_ending;
        let conv = |s: &str| if le == "\r\n" { s.replace('\n', "\r\n") } else { s.to_string() };
        let t = self.rope.slice(..);
        let n = self.sels.len();
        let linewise = allow_linewise
            && self.std.linewise_clip.as_deref() == Some(text.as_str())
            && self.sels.v.iter().all(|s| s.is_empty());
        let parts: Vec<&str> = text.split('\n').collect();
        let distribute = n > 1 && parts.len() == n && !linewise;
        let specs = self
            .sels
            .v
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if linewise {
                    let line = t.char_to_line(s.head);
                    let ls = t.line_to_char(line);
                    let body = conv(&text);
                    let body = if body.ends_with('\n') { body } else { format!("{body}{le}") };
                    return Some(EditSpec {
                        from: ls,
                        to: ls,
                        text: body,
                        back: ls as isize - s.head as isize,
                    });
                }
                let body = if distribute { parts[i].to_string() } else { text.clone() };
                Some(EditSpec { from: s.from(), to: s.to(), text: conv(&body), back: 0 })
            })
            .collect();
        self.apply_specs(specs);
    }

    fn std_copy(&mut self, cut: bool) {
        let t = self.rope.slice(..);
        let whole_line = self.sels.v.iter().all(|s| s.is_empty());
        let le = self.line_ending;
        let text = if whole_line {
            let mut lines: Vec<usize> = self.sels.v.iter().map(|s| t.char_to_line(s.head)).collect();
            lines.dedup();
            lines.iter().map(|&l| format!("{}\n", text::line_content(t, l))).collect::<String>()
        } else {
            self.sels
                .v
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| self.rope.slice(s.from()..s.to()).to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let text = text.replace("\r\n", "\n");
        self.std.linewise_clip = whole_line.then(|| text.clone());
        self.effects.push(Effect::SetClipboard(text.clone()));
        self.clipboard = Some(text);
        let _ = le;
        if cut {
            if whole_line {
                self.delete_lines();
            } else {
                self.backspace();
            }
        }
    }

    /// Merge the line ranges covered by the selections into blocks of whole lines.
    fn line_blocks(&self) -> Vec<(usize, usize)> {
        let t = self.rope.slice(..);
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for s in &self.sels.v {
            let (a, b) = s.line_range(t);
            match blocks.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => blocks.push((a, b)),
            }
        }
        blocks
    }

    fn block_text(t: Text, l0: usize, l1: usize, le: &str) -> String {
        (l0..=l1).map(|l| text::line_content(t, l)).collect::<Vec<_>>().join(le)
    }

    fn move_lines(&mut self, down: bool) {
        let t = self.rope.slice(..);
        let le = self.line_ending;
        let nlines = t.len_lines();
        let mut changes = Vec::new();
        let mut deltas: Vec<(usize, usize, isize)> = Vec::new(); // (from, to, shift) for selections inside block
        let mut used_to: Option<usize> = None;
        for (l0, l1) in self.line_blocks() {
            let (lo, hi) = if down {
                if l1 + 1 >= nlines
                    || (l1 + 1 == nlines - 1 && t.line(l1 + 1).len_chars() == 0 && nlines > 1 && false)
                {
                    continue;
                }
                (l0, l1 + 1)
            } else {
                if l0 == 0 {
                    continue;
                }
                (l0 - 1, l1)
            };
            if used_to.is_some_and(|u| lo <= u) {
                continue;
            }
            used_to = Some(hi);
            let block = Self::block_text(t, l0, l1, le);
            let neighbor_line = if down { l1 + 1 } else { l0 - 1 };
            let neighbor = text::line_content(t, neighbor_line);
            let new = if down { format!("{neighbor}{le}{block}") } else { format!("{block}{le}{neighbor}") };
            let from = t.line_to_char(lo);
            let to = text::line_end(t, hi);
            changes.push(Change { from, to, text: new });
            let shift = (neighbor.chars().count() + le.chars().count()) as isize * if down { 1 } else { -1 };
            deltas.push((t.line_to_char(l0), text::line_end(t, l1), shift));
        }
        if changes.is_empty() {
            return;
        }
        let v: Vec<Selection> = self
            .sels
            .v
            .iter()
            .map(|s| {
                let shift = deltas
                    .iter()
                    .find(|(a, b, _)| s.from() >= *a && s.to() <= (*b).max(*a) + le.len())
                    .map_or(0, |d| d.2);
                Selection::new((s.anchor as isize + shift) as usize, (s.head as isize + shift) as usize)
            })
            .collect();
        let sels = Selections { v, primary: self.sels.primary };
        self.apply_with(Transaction::new(changes), Some(sels));
    }

    fn duplicate_lines(&mut self) {
        let t = self.rope.slice(..);
        let le = self.line_ending;
        let mut changes = Vec::new();
        let mut shifts: Vec<(usize, usize, isize)> = Vec::new();
        for (l0, l1) in self.line_blocks() {
            let block = Self::block_text(t, l0, l1, le);
            let at = text::line_end(t, l1);
            let n = (block.chars().count() + le.chars().count()) as isize;
            changes.push(Change { from: at, to: at, text: format!("{le}{block}") });
            shifts.push((t.line_to_char(l0), at, n));
        }
        let mut acc = 0isize;
        let mut v = Vec::new();
        for s in &self.sels.v {
            // everything before this block shifted by earlier duplicates; the block itself moves onto its copy
            let mut before = 0isize;
            let mut own = 0isize;
            for (a, b, n) in &shifts {
                if s.to() <= *b && s.from() >= *a {
                    own = *n;
                    break;
                }
                if *b < s.from() {
                    before += *n;
                }
            }
            let d = before + own;
            v.push(Selection::new((s.anchor as isize + d) as usize, (s.head as isize + d) as usize));
        }
        let _ = &mut acc;
        let sels = Selections { v, primary: self.sels.primary };
        self.apply_with(Transaction::new(changes), Some(sels));
    }

    fn delete_lines(&mut self) {
        let t = self.rope.slice(..);
        let mut changes = Vec::new();
        for (l0, l1) in self.line_blocks() {
            let last = t.len_lines() - 1;
            let (from, to) = if l1 >= last && l0 > 0 {
                (text::line_end(t, l0 - 1), text::line_end(t, l1))
            } else {
                (t.line_to_char(l0), t.line_to_char((l1 + 1).min(t.len_lines())))
            };
            changes.push(Change { from, to, text: String::new() });
        }
        self.apply(Transaction::new(changes));
    }

    fn select_lines(&mut self) {
        let t = self.rope.slice(..);
        let v: Vec<Selection> = self
            .sels
            .v
            .iter()
            .map(|s| {
                let (l0, l1) = s.line_range(t);
                let a = t.line_to_char(l0);
                let b = t.line_to_char((l1 + 1).min(t.len_lines()));
                if s.from() == a && s.to() == b && b < t.len_chars() {
                    Selection::new(a, t.line_to_char((l1 + 2).min(t.len_lines())))
                } else {
                    Selection::new(a, b)
                }
            })
            .collect();
        let p = self.sels.primary;
        self.set_sels(Selections::new(v, p));
    }

    fn add_cursor(&mut self, down: bool) {
        let t = self.rope.slice(..);
        let mut v = self.sels.v.clone();
        let mut added = None;
        for s in &self.sels.v {
            let line = t.char_to_line(s.head);
            let col = s.head - t.line_to_char(line);
            let nl = if down { line + 1 } else { line.wrapping_sub(1) };
            if nl >= t.len_lines() {
                continue;
            }
            let nls = t.line_to_char(nl);
            let ns = Selection::point(nls + col.min(text::line_end(t, nl) - nls));
            v.push(ns);
            added = Some(ns);
        }
        if let Some(a) = added {
            let mut sel = Selections::new(v, 0);
            if let Some(i) = sel.v.iter().position(|s| *s == a) {
                sel.primary = i;
            }
            self.set_sels(sel);
            let n = self.sels.len();
            self.status(format!("{n} selections"));
        }
    }

    fn word_range_at(&self, pos: usize) -> Option<(usize, usize)> {
        let t = self.rope.slice(..);
        let len = t.len_chars();
        let at = if pos < len && text::is_word_char(t.char(pos)) {
            pos
        } else if pos > 0 && text::is_word_char(t.char(pos - 1)) {
            pos - 1
        } else {
            return None;
        };
        let (mut a, mut b) = (at, at + 1);
        while a > 0 && text::is_word_char(t.char(a - 1)) {
            a -= 1;
        }
        while b < len && text::is_word_char(t.char(b)) {
            b += 1;
        }
        Some((a, b))
    }

    /// ctrl-d: first press selects the word under each cursor, later presses add the next occurrence.
    fn select_next_occurrence(&mut self) {
        if self.sels.v.iter().any(|s| s.is_empty()) {
            let v: Vec<Selection> = self
                .sels
                .v
                .iter()
                .map(|s| match self.word_range_at(s.head) {
                    Some((a, b)) if s.is_empty() => Selection::new(a, b),
                    _ => *s,
                })
                .collect();
            let p = self.sels.primary;
            self.set_sels(Selections::new(v, p));
            return;
        }
        let needle = self.selected_text().unwrap_or_default();
        if needle.is_empty() {
            return;
        }
        let hay = self.rope.to_string();
        let from_char = self.sels.v.iter().map(|s| s.to()).max().unwrap_or(0);
        let from_byte = self.rope.char_to_byte(from_char);
        let overlaps = |a: usize, b: usize| self.sels.v.iter().any(|s| a < s.to() && s.from() < b);
        let mut found = None;
        let mut start = from_byte;
        // forward, then wrap
        for pass in 0..2 {
            let (lo, hi) = if pass == 0 { (start, hay.len()) } else { (0, from_byte.min(hay.len())) };
            let mut cur = lo;
            while cur <= hi {
                let Some(i) = hay[cur..hi].find(&needle) else { break };
                let bs = cur + i;
                let (a, b) = (self.rope.byte_to_char(bs), self.rope.byte_to_char(bs + needle.len()));
                if !overlaps(a, b) {
                    found = Some((a, b));
                    break;
                }
                cur = bs + needle.len().max(1);
            }
            if found.is_some() {
                break;
            }
            start = 0;
        }
        match found {
            Some((a, b)) => {
                let mut v = self.sels.v.clone();
                v.push(Selection::new(a, b));
                let p = v.len() - 1;
                self.set_sels(Selections::new(v, p));
                let n = self.sels.len();
                self.status(format!("{n} selections"));
            }
            None => self.status("no more occurrences"),
        }
    }

    fn select_all_occurrences(&mut self) {
        let prim = self.sels.primary();
        let (a0, b0) = if prim.is_empty() {
            match self.word_range_at(prim.head) {
                Some(r) => r,
                None => return,
            }
        } else {
            (prim.from(), prim.to())
        };
        let needle = self.rope.slice(a0..b0).to_string();
        let hay = self.rope.to_string();
        let mut out = Vec::new();
        let mut primary = 0;
        let mut cur = 0;
        while let Some(i) = hay[cur..].find(&needle) {
            let bs = cur + i;
            let (a, b) = (self.rope.byte_to_char(bs), self.rope.byte_to_char(bs + needle.len()));
            if a == a0 {
                primary = out.len();
            }
            out.push(Selection::new(a, b));
            cur = bs + needle.len().max(1);
        }
        let n = out.len();
        self.set_sels(Selections::new(out, primary));
        self.status(format!("{n} selections"));
    }
}
