//! Insert-style editing shared by Helix insert mode and the Standard keymap: typing with
//! auto-pairs, newline with auto-indent, backspace/delete variants and word/line kills.

use crate::editor::{Editor, Effect, Keymap, Mode};
use crate::keys::{Key, KeyInput};
use crate::motion::{self, WordKind};
use crate::selection::{Selection, Selections};
use crate::text::{self, Text};
use crate::transaction::{Change, Transaction};

pub(crate) const INDENT: &str = "    ";

/// One selection's edit. `back` moves the resulting cursor back from the end of the inserted
/// text (auto-pair: between the brackets) or, when negative, forward (type-over).
pub(crate) struct EditSpec {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub back: isize,
}

fn closer_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '\'' | '"' | '`' => Some(c),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '\'' | '"' | '`')
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    /// The (from, to) range an insertion by this selection replaces.
    fn edit_range(&self, s: &Selection) -> (usize, usize) {
        match self.keymap {
            Keymap::Helix => {
                let c = s.block_cursor(self.t());
                (c, c)
            }
            Keymap::Standard => (s.from(), s.to()),
        }
    }

    pub(crate) fn apply_specs(&mut self, specs: Vec<Option<EditSpec>>) {
        let mut changes = Vec::new();
        for s in specs.iter().flatten() {
            if !(s.from == s.to && s.text.is_empty()) {
                changes.push(Change { from: s.from, to: s.to, text: s.text.clone() });
            }
        }
        let tx = Transaction::new(changes);
        let sels = match self.keymap {
            Keymap::Helix => {
                let mut mapped = self.map_selections(&tx);
                // Auto-pair / type-over put the cursor somewhere the generic mapping can't know:
                // place those selections explicitly (the block cursor re-widens them).
                let mut shift: isize = 0;
                for (i, spec) in specs.iter().enumerate() {
                    if let Some(sp) = spec {
                        let n = sp.text.chars().count() as isize;
                        if sp.back != 0 {
                            let pos = (sp.from as isize + shift + n - sp.back).max(0) as usize;
                            mapped.v[i] = Selection::point(pos);
                        }
                        shift += n - (sp.to - sp.from) as isize;
                    }
                }
                mapped
            }
            Keymap::Standard => {
                let mut shift: isize = 0;
                let mut v = Vec::with_capacity(specs.len());
                for (i, spec) in specs.iter().enumerate() {
                    match spec {
                        Some(sp) => {
                            let n = sp.text.chars().count() as isize;
                            let pos = sp.from as isize + shift + n - sp.back;
                            v.push(Selection::point(pos.max(0) as usize));
                            shift += n - (sp.to - sp.from) as isize;
                        }
                        None => {
                            let s = self.sels.v[i];
                            v.push(Selection::new(
                                (s.anchor as isize + shift) as usize,
                                (s.head as isize + shift) as usize,
                            ));
                        }
                    }
                }
                Selections { v, primary: self.sels.primary }
            }
        };
        self.apply_with(tx, Some(sels));
    }

    fn specs_for(&self, f: impl Fn(Text, usize, usize) -> Option<EditSpec>) -> Vec<Option<EditSpec>> {
        let t = self.t();
        self.sels
            .v
            .iter()
            .map(|s| {
                let (from, to) = self.edit_range(s);
                f(t, from, to)
            })
            .collect()
    }

    pub(crate) fn type_char(&mut self, c: char) {
        let specs = self.specs_for(|t, from, to| {
            let len = t.len_chars();
            let prev = if from > 0 { Some(t.char(from - 1)) } else { None };
            let next = if to < len { Some(t.char(to)) } else { None };
            if is_closer(c) && from == to && next == Some(c) {
                return Some(EditSpec { from, to, text: String::new(), back: -1 });
            }
            if let Some(close) = closer_of(c) {
                let quote = close == c;
                let adjacent_word = prev.is_some_and(is_word) || next.is_some_and(is_word);
                let pair = if quote { !adjacent_word } else { !next.is_some_and(is_word) };
                if pair {
                    return Some(EditSpec { from, to, text: format!("{c}{close}"), back: 1 });
                }
            }
            Some(EditSpec { from, to, text: c.to_string(), back: 0 })
        });
        self.apply_specs(specs);
    }

    /// Standard keymap: typing an opener over a selection wraps it.
    pub(crate) fn surround_selection_with(&mut self, c: char) -> bool {
        let Some(close) = closer_of(c) else { return false };
        if self.sels.v.iter().all(|s| s.is_empty()) {
            return false;
        }
        let mut changes = Vec::new();
        let mut v = Vec::new();
        let mut shift = 0usize;
        for s in &self.sels.v {
            if s.is_empty() {
                v.push(Selection::new(s.anchor + shift, s.head + shift));
                continue;
            }
            changes.push(Change { from: s.from(), to: s.from(), text: c.to_string() });
            changes.push(Change { from: s.to(), to: s.to(), text: close.to_string() });
            let (a, b) = (s.from() + shift + 1, s.to() + shift + 1);
            v.push(if s.is_forward() { Selection::new(a, b) } else { Selection::new(b, a) });
            shift += 2;
        }
        let sels = Selections { v, primary: self.sels.primary };
        self.apply_with(Transaction::new(changes), Some(sels));
        true
    }

    pub(crate) fn insert_plain(&mut self, text: &str) {
        let specs =
            self.specs_for(|_, from, to| Some(EditSpec { from, to, text: text.to_string(), back: 0 }));
        self.apply_specs(specs);
    }

    pub(crate) fn hx_insert_text(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n");
        let text = if self.line_ending == "\r\n" { text.replace('\n', "\r\n") } else { text };
        self.insert_plain(&text);
    }

    pub(crate) fn insert_newline(&mut self) {
        let le = self.line_ending;
        let specs = self.specs_for(|t, from, to| {
            let line = t.char_to_line(from);
            let ls = t.line_to_char(line);
            // Only the whitespace before the cursor carries over; text after it moves down.
            let mut indent = String::new();
            for i in ls..from {
                match t.char(i) {
                    c @ (' ' | '\t') => indent.push(c),
                    _ => break,
                }
            }
            let before: String = t.slice(ls..from).to_string();
            let opens = before.trim_end().ends_with(['(', '[', '{']);
            let prev = if from > 0 { Some(t.char(from - 1)) } else { None };
            let next = if to < t.len_chars() { Some(t.char(to)) } else { None };
            let between = matches!(
                (prev, next),
                (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}'))
            );
            let inner = if opens { format!("{indent}{INDENT}") } else { indent.clone() };
            if between {
                let text = format!("{le}{inner}{le}{indent}");
                let back = (le.chars().count() + indent.chars().count()) as isize;
                Some(EditSpec { from, to, text, back })
            } else {
                Some(EditSpec { from, to, text: format!("{le}{inner}"), back: 0 })
            }
        });
        self.apply_specs(specs);
    }

    pub(crate) fn insert_tab(&mut self) {
        self.insert_plain(INDENT);
    }

    pub(crate) fn backspace(&mut self) {
        let std = self.keymap == Keymap::Standard;
        let specs = self
            .sels
            .v
            .iter()
            .map(|s| {
                let t = self.rope.slice(..);
                let (from, to) = self.edit_range(s);
                if std && from < to {
                    return Some(EditSpec { from, to, text: String::new(), back: 0 });
                }
                if from == 0 {
                    return None;
                }
                let prev = t.char(from - 1);
                let next = if from < t.len_chars() { Some(t.char(from)) } else { None };
                if closer_of(prev).is_some_and(|c| Some(c) == next) {
                    return Some(EditSpec { from: from - 1, to: from + 1, text: String::new(), back: 0 });
                }
                let line = t.char_to_line(from);
                let ls = t.line_to_char(line);
                let col = from - ls;
                if col > 0 && (ls..from).all(|i| t.char(i) == ' ') {
                    let n = if col.is_multiple_of(4) { 4 } else { col % 4 };
                    return Some(EditSpec { from: from - n, to: from, text: String::new(), back: 0 });
                }
                Some(EditSpec { from: text::prev_grapheme(t, from), to: from, text: String::new(), back: 0 })
            })
            .collect();
        self.apply_specs(specs);
    }

    pub(crate) fn delete_forward(&mut self) {
        let std = self.keymap == Keymap::Standard;
        let specs = self
            .sels
            .v
            .iter()
            .map(|s| {
                let t = self.rope.slice(..);
                let (from, to) = self.edit_range(s);
                if std && from < to {
                    return Some(EditSpec { from, to, text: String::new(), back: 0 });
                }
                if from >= t.len_chars() {
                    return None;
                }
                Some(EditSpec { from, to: text::next_grapheme(t, from), text: String::new(), back: 0 })
            })
            .collect();
        self.apply_specs(specs);
    }

    pub(crate) fn delete_word_back(&mut self) {
        let std = self.keymap == Keymap::Standard;
        let specs = self
            .sels
            .v
            .iter()
            .map(|s| {
                let t = self.rope.slice(..);
                let (from, to) = self.edit_range(s);
                if std && from < to {
                    return Some(EditSpec { from, to, text: String::new(), back: 0 });
                }
                if from == 0 {
                    return None;
                }
                let target = if std {
                    std_word_left(t, from)
                } else {
                    motion::word_move(t, Selection::point(from), 1, WordKind::PrevStart, false).head
                };
                (target < from).then(|| EditSpec { from: target, to: from, text: String::new(), back: 0 })
            })
            .collect();
        self.apply_specs(specs);
    }

    pub(crate) fn delete_word_forward(&mut self) {
        let std = self.keymap == Keymap::Standard;
        let specs = self
            .sels
            .v
            .iter()
            .map(|s| {
                let t = self.rope.slice(..);
                let (from, to) = self.edit_range(s);
                if std && from < to {
                    return Some(EditSpec { from, to, text: String::new(), back: 0 });
                }
                if from >= t.len_chars() {
                    return None;
                }
                let target = if std {
                    std_word_right(t, from)
                } else {
                    motion::word_move(t, Selection::point(from), 1, WordKind::NextStart, false).head
                };
                (target > from).then(|| EditSpec { from, to: target, text: String::new(), back: 0 })
            })
            .collect();
        self.apply_specs(specs);
    }

    pub(crate) fn kill_to_line_start(&mut self) {
        let specs = self
            .sels
            .v
            .iter()
            .map(|s| {
                let t = self.rope.slice(..);
                let (from, _) = self.edit_range(s);
                let ls = t.line_to_char(t.char_to_line(from));
                (ls < from).then(|| EditSpec { from: ls, to: from, text: String::new(), back: 0 })
            })
            .collect();
        self.apply_specs(specs);
    }

    pub(crate) fn kill_to_line_end(&mut self) {
        let specs = self
            .sels
            .v
            .iter()
            .map(|s| {
                let t = self.rope.slice(..);
                let (_, to) = self.edit_range(s);
                let line = t.char_to_line(to);
                let le = text::line_end(t, line);
                if to < le {
                    Some(EditSpec { from: to, to: le, text: String::new(), back: 0 })
                } else if to < t.len_chars() {
                    Some(EditSpec { from: to, to: to + text::eol_len(t, line), text: String::new(), back: 0 })
                } else {
                    None
                }
            })
            .collect();
        self.apply_specs(specs);
    }

    /// Helix insert mode.
    pub(crate) fn insert_key(&mut self, k: &KeyInput) -> bool {
        let m = &k.modifiers;
        if let Some(c) = k.text_char() {
            self.type_char(c);
            return true;
        }
        match (&k.key, m.ctrl, m.alt) {
            (Key::Escape, ..) => self.leave_insert(),
            (Key::Enter, false, false) => self.insert_newline(),
            (Key::Tab, false, false) => self.insert_tab(),
            (Key::Backspace, false, false) => self.backspace(),
            (Key::Backspace, _, true) | (Key::Char('w'), true, false) => self.delete_word_back(),
            (Key::Char('h'), true, false) => self.backspace(),
            (Key::Delete, false, false) | (Key::Char('d'), true, false) => self.delete_forward(),
            (Key::Delete, _, true) | (Key::Char('d'), false, true) => self.delete_word_forward(),
            (Key::Char('u'), true, false) => self.kill_to_line_start(),
            (Key::Char('k'), true, false) => self.kill_to_line_end(),
            (Key::Char('x'), true, false) => self.effects.push(Effect::RequestCompletion),
            (Key::Left, false, false) => self.move_horizontal(false, 1, false),
            (Key::Right, false, false) => self.move_horizontal(true, 1, false),
            (Key::Up, false, false) => self.move_vertical(false, 1, false),
            (Key::Down, false, false) => self.move_vertical(true, 1, false),
            // ctrl-Home / ctrl-End: document start / end (the insert caret can sit after the last char).
            (Key::Home, true, _) => self.transform_sels(|_, _| Selection::point(0)),
            (Key::End, true, _) => {
                self.transform_sels(|t, _| Selection::point(t.len_chars()));
            }
            (Key::Home, ..) => self.run(crate::helix::Cmd::LineStart, None),
            (Key::End, ..) => {
                self.transform_sels(|t, s| {
                    let line = t.char_to_line(s.block_cursor(t));
                    Selection::point(text::line_end(t, line))
                });
            }
            // ctrl/cmd-pageup/down switch tabs in the application; they are not page keys.
            (Key::PageUp | Key::PageDown, ..) if m.ctrl || m.platform => return false,
            (Key::PageUp, ..) => self.run(crate::helix::Cmd::PageUp, None),
            (Key::PageDown, ..) => self.run(crate::helix::Cmd::PageDown, None),
            (Key::Left, _, true) | (Key::Char('b'), false, true) => self.word_move_insert(false),
            (Key::Right, _, true) | (Key::Char('f'), false, true) => self.word_move_insert(true),
            _ => return false,
        }
        let _ = Mode::Insert;
        true
    }

    fn word_move_insert(&mut self, fwd: bool) {
        self.transform_sels(|t, s| {
            let w = motion::word_move(
                t,
                *s,
                1,
                if fwd { WordKind::NextStart } else { WordKind::PrevStart },
                false,
            );
            Selection::point(w.block_cursor(t))
        });
    }
}

/// ctrl-left in the Standard keymap: skip whitespace, then one run of word or punctuation chars.
pub(crate) fn std_word_left(t: Text, mut p: usize) -> usize {
    while p > 0 && t.char(p - 1).is_whitespace() && !text::is_eol(t.char(p - 1)) {
        p -= 1;
    }
    if p > 0 && text::is_eol(t.char(p - 1)) {
        // cross a single line break
        p -= 1;
        if p > 0 && t.char(p) == '\n' && t.char(p - 1) == '\r' {
            p -= 1;
        }
        return p;
    }
    if p == 0 {
        return 0;
    }
    let class = text::classify(t.char(p - 1));
    while p > 0 && text::classify(t.char(p - 1)) == class {
        p -= 1;
    }
    p
}

/// ctrl-right in the Standard keymap.
pub(crate) fn std_word_right(t: Text, mut p: usize) -> usize {
    let len = t.len_chars();
    while p < len && t.char(p).is_whitespace() && !text::is_eol(t.char(p)) {
        p += 1;
    }
    if p < len && text::is_eol(t.char(p)) {
        p += 1;
        if p < len && t.char(p - 1) == '\r' && t.char(p) == '\n' {
            p += 1;
        }
        return p;
    }
    if p >= len {
        return len;
    }
    let class = text::classify(t.char(p));
    while p < len && text::classify(t.char(p)) == class {
        p += 1;
    }
    p
}
