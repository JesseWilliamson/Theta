//! The bottom prompt (`:` commands, `/` search, `s`/`S`/`K` regex prompts, Standard goto-line)
//! and the regex search machinery shared by both keymaps.

use crate::editor::{Editor, Effect, Keymap, Mode};
use crate::keys::{Key, KeyInput};
use crate::selection::{Selection, Selections};
use crate::text;
use crate::transaction::{Change, Transaction};
use regex::{Regex, RegexBuilder};
use std::cell::RefCell;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PromptKind {
    Command,
    Search { backwards: bool },
    SelectRegex,
    SplitRegex,
    Keep(bool),
    Goto,
}

pub(crate) struct PromptState {
    pub kind: PromptKind,
    pub prefix: String,
    pub text: String,
    pub cursor: usize,
    /// Selections before the prompt opened: live previews are computed from these and Esc restores them.
    pub saved: Selections,
    pub hist_idx: Option<usize>,
    pub draft: String,
    pub extend: bool,
    pub count: usize,
    pub tab_state: Option<(String, usize)>,
}

#[derive(Default)]
pub(crate) struct SearchState {
    pub pattern: Option<String>,
    pub history: Vec<String>,
    pub cmd_history: Vec<String>,
    pub cache: RefCell<Option<(u64, String, Vec<Range<usize>>)>>,
}

const COMMANDS: &[&str] = &[
    "write", "quit", "quit-all", "write-quit", "run", "run-all", "explain", "analyze", "format", "theme",
    "connect", "goto", "sort", "new", "cancel", "w", "q", "qa", "wq", "r", "fmt",
];

/// Compile `pattern` with smart-case: a pattern without any uppercase literal is case-insensitive.
pub(crate) fn build_regex(pattern: &str) -> Result<Regex, String> {
    let mut has_upper = false;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c.is_uppercase() {
            has_upper = true;
        }
    }
    RegexBuilder::new(pattern).case_insensitive(!has_upper).multi_line(true).build().map_err(|e| {
        let s = e.to_string();
        s.lines().last().unwrap_or("invalid regex").trim().to_string()
    })
}

impl Editor {
    pub(crate) fn open_prompt(&mut self, kind: PromptKind, prefix: &str, initial: &str, count: usize) {
        self.prompt = Some(PromptState {
            kind,
            prefix: prefix.to_string(),
            text: initial.to_string(),
            cursor: initial.chars().count(),
            saved: self.sels.clone(),
            hist_idx: None,
            draft: String::new(),
            extend: self.mode == Mode::Select,
            count,
            tab_state: None,
        });
        if !initial.is_empty() {
            self.prompt_changed();
        }
    }

    fn close_prompt(&mut self, restore: bool) {
        if let Some(p) = self.prompt.take() {
            if restore {
                self.set_sels(p.saved);
            }
        }
    }

    pub(crate) fn prompt_insert(&mut self, text: &str) {
        let Some(p) = &mut self.prompt else { return };
        let text: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        let idx = byte_idx(&p.text, p.cursor);
        p.text.insert_str(idx, &text);
        p.cursor += text.chars().count();
        p.hist_idx = None;
        p.tab_state = None;
        self.prompt_changed();
    }

    pub(crate) fn prompt_key(&mut self, k: &KeyInput) -> bool {
        let m = k.modifiers.clone();
        let Some(p) = &mut self.prompt else { return false };
        let ctrl = m.ctrl || m.platform;
        let mut changed = false;
        match &k.key {
            Key::Escape => {
                self.close_prompt(true);
                return true;
            }
            Key::Char('c') if ctrl => {
                self.close_prompt(true);
                return true;
            }
            Key::Enter => {
                self.prompt_submit();
                return true;
            }
            Key::F(3) => {
                // Standard: F3 inside the search prompt behaves like Enter
                self.prompt_submit();
                return true;
            }
            Key::Backspace if m.alt || ctrl => {
                changed = delete_word_back(p);
            }
            Key::Backspace => {
                if p.text.is_empty() {
                    self.close_prompt(true);
                    return true;
                }
                if p.cursor > 0 {
                    let a = byte_idx(&p.text, p.cursor - 1);
                    let b = byte_idx(&p.text, p.cursor);
                    p.text.replace_range(a..b, "");
                    p.cursor -= 1;
                    changed = true;
                }
            }
            Key::Char('h') if ctrl => {
                if p.cursor > 0 {
                    let a = byte_idx(&p.text, p.cursor - 1);
                    let b = byte_idx(&p.text, p.cursor);
                    p.text.replace_range(a..b, "");
                    p.cursor -= 1;
                    changed = true;
                }
            }
            Key::Delete | Key::Char('d') if matches!(k.key, Key::Delete) || ctrl => {
                if p.cursor < p.text.chars().count() {
                    let a = byte_idx(&p.text, p.cursor);
                    let b = byte_idx(&p.text, p.cursor + 1);
                    p.text.replace_range(a..b, "");
                    changed = true;
                }
            }
            Key::Char('w') if ctrl => changed = delete_word_back(p),
            Key::Char('u') if ctrl => {
                let b = byte_idx(&p.text, p.cursor);
                p.text.replace_range(..b, "");
                p.cursor = 0;
                changed = true;
            }
            Key::Char('k') if ctrl => {
                let b = byte_idx(&p.text, p.cursor);
                p.text.truncate(b);
                changed = true;
            }
            Key::Left | Key::Char('b') if matches!(k.key, Key::Left) || ctrl => {
                if m.alt || (m.ctrl && matches!(k.key, Key::Left)) {
                    p.cursor = word_left(&p.text, p.cursor);
                } else {
                    p.cursor = p.cursor.saturating_sub(1);
                }
            }
            Key::Right | Key::Char('f') if matches!(k.key, Key::Right) || ctrl => {
                if m.alt || (m.ctrl && matches!(k.key, Key::Right)) {
                    p.cursor = word_right(&p.text, p.cursor);
                } else {
                    p.cursor = (p.cursor + 1).min(p.text.chars().count());
                }
            }
            Key::Char('b') if m.alt => p.cursor = word_left(&p.text, p.cursor),
            Key::Char('f') if m.alt => p.cursor = word_right(&p.text, p.cursor),
            Key::Home => p.cursor = 0,
            Key::Char('a') if ctrl => p.cursor = 0,
            Key::End => p.cursor = p.text.chars().count(),
            Key::Char('e') if ctrl => p.cursor = p.text.chars().count(),
            Key::Up | Key::Char('p') if matches!(k.key, Key::Up) || ctrl => {
                self.prompt_history(true);
                return true;
            }
            Key::Down | Key::Char('n') if matches!(k.key, Key::Down) || ctrl => {
                self.prompt_history(false);
                return true;
            }
            Key::Tab => {
                self.prompt_complete();
                return true;
            }
            Key::Char(c) if !ctrl && !m.alt => {
                let idx = byte_idx(&p.text, p.cursor);
                p.text.insert(idx, *c);
                p.cursor += 1;
                changed = true;
            }
            _ => {}
        }
        if changed {
            if let Some(p) = &mut self.prompt {
                p.hist_idx = None;
                p.tab_state = None;
            }
            self.prompt_changed();
        }
        true
    }

    fn prompt_history(&mut self, older: bool) {
        let Some(p) = &mut self.prompt else { return };
        let hist = if p.kind == PromptKind::Command { &self.search.cmd_history } else { &self.search.history };
        if hist.is_empty() {
            return;
        }
        let new_idx = match (p.hist_idx, older) {
            (None, true) => {
                p.draft = p.text.clone();
                Some(hist.len() - 1)
            }
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) => (i + 1 < hist.len()).then_some(i + 1),
        };
        p.hist_idx = new_idx;
        p.text = match new_idx {
            Some(i) => hist[i].clone(),
            None => p.draft.clone(),
        };
        p.cursor = p.text.chars().count();
        self.prompt_changed();
    }

    fn prompt_complete(&mut self) {
        let Some(p) = &mut self.prompt else { return };
        if p.kind != PromptKind::Command || p.text.contains(' ') {
            return;
        }
        let (base, idx) = match &p.tab_state {
            Some((b, i)) => (b.clone(), i + 1),
            None => (p.text.clone(), 0),
        };
        let cands: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(&base) && c.len() > 1).collect();
        if cands.is_empty() {
            return;
        }
        let pick = cands[idx % cands.len()];
        p.text = pick.to_string();
        p.cursor = p.text.chars().count();
        p.tab_state = Some((base, idx % cands.len()));
    }

    /// Recompute the live preview for regex/search prompts.
    fn prompt_changed(&mut self) {
        let Some(p) = &self.prompt else { return };
        let (kind, text, saved, extend) = (p.kind, p.text.clone(), p.saved.clone(), p.extend);
        match kind {
            PromptKind::Search { backwards } => {
                self.set_sels(saved.clone());
                if text.is_empty() {
                    return;
                }
                if let Ok(re) = build_regex(&text) {
                    if self.search_step(&re, backwards, 1, extend).is_none() {
                        self.set_sels(saved);
                    }
                }
            }
            PromptKind::SelectRegex | PromptKind::SplitRegex | PromptKind::Keep(_) => {
                self.set_sels(saved.clone());
                if text.is_empty() {
                    return;
                }
                if let Ok(re) = build_regex(&text) {
                    match self.regex_selection(kind, &re, &saved) {
                        Some(s) => self.set_sels(s),
                        None => self.set_sels(saved),
                    }
                }
            }
            PromptKind::Command | PromptKind::Goto => {}
        }
    }

    fn prompt_submit(&mut self) {
        let Some(p) = self.prompt.take() else { return };
        let text = p.text.clone();
        let remember = |hist: &mut Vec<String>, t: &str| {
            if !t.is_empty() && hist.last().map(String::as_str) != Some(t) {
                hist.push(t.to_string());
            }
        };
        match p.kind {
            PromptKind::Command => {
                remember(&mut self.search.cmd_history, &text);
                self.set_sels(p.saved);
                self.exec_command(text.trim());
            }
            PromptKind::Goto => {
                self.set_sels(p.saved);
                self.goto_prompt(text.trim());
            }
            PromptKind::Search { backwards } => {
                self.set_sels(p.saved.clone());
                if text.is_empty() {
                    return;
                }
                remember(&mut self.search.history, &text);
                match build_regex(&text) {
                    Err(e) => self.status(format!("invalid regex: {e}")),
                    Ok(re) => {
                        self.search.pattern = Some(text);
                        self.push_jump();
                        if self.search_step(&re, backwards, p.count.max(1), p.extend).is_none() {
                            self.status("no matches");
                        }
                    }
                }
            }
            PromptKind::SelectRegex | PromptKind::SplitRegex | PromptKind::Keep(_) => {
                self.set_sels(p.saved.clone());
                if text.is_empty() {
                    return;
                }
                remember(&mut self.search.history, &text);
                match build_regex(&text) {
                    Err(e) => self.status(format!("invalid regex: {e}")),
                    Ok(re) => match self.regex_selection(p.kind, &re, &p.saved) {
                        Some(s) => {
                            let n = s.len();
                            self.set_sels(s);
                            if n > 1 {
                                self.status(format!("{n} selections"));
                            }
                        }
                        None => self.status(if matches!(p.kind, PromptKind::Keep(_)) {
                            "no selections remaining"
                        } else {
                            "no matches"
                        }),
                    },
                }
                if self.mode == Mode::Select {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    /// Result of applying a selection-regex prompt to `saved`, or `None` when it yields nothing.
    fn regex_selection(&self, kind: PromptKind, re: &Regex, saved: &Selections) -> Option<Selections> {
        let t = self.rope.slice(..);
        let mut out: Vec<Selection> = Vec::new();
        let mut primary = 0;
        match kind {
            PromptKind::SelectRegex => {
                for (i, s) in saved.v.iter().enumerate() {
                    let hay = self.rope.slice(s.from()..s.to()).to_string();
                    let base = s.from();
                    let mut ranges = char_matches(&hay, re, true);
                    if i == saved.primary {
                        primary = out.len();
                    }
                    out.extend(ranges.drain(..).map(|r| Selection::new(base + r.start, base + r.end)));
                }
            }
            PromptKind::SplitRegex => {
                for (i, s) in saved.v.iter().enumerate() {
                    let hay = self.rope.slice(s.from()..s.to()).to_string();
                    let base = s.from();
                    let mut start = 0;
                    if i == saved.primary {
                        primary = out.len();
                    }
                    let n_chars = hay.chars().count();
                    for r in char_matches(&hay, re, true) {
                        if r.start > start {
                            out.push(Selection::new(base + start, base + r.start));
                        }
                        start = r.end;
                    }
                    if start < n_chars {
                        out.push(Selection::new(base + start, base + n_chars));
                    }
                }
                if out.is_empty() {
                    return None;
                }
            }
            PromptKind::Keep(keep) => {
                for (i, s) in saved.v.iter().enumerate() {
                    let hay = self.rope.slice(s.from()..s.to()).to_string();
                    if re.is_match(&hay) == keep {
                        if i == saved.primary {
                            primary = out.len();
                        }
                        out.push(*s);
                    }
                }
            }
            _ => return None,
        }
        let _ = t;
        if out.is_empty() {
            return None;
        }
        Some(Selections::new(out, primary))
    }

    // ---- searching --------------------------------------------------------------------------------

    /// Move to the `count`-th next/previous match. Normal mode replaces the primary selection,
    /// Select mode (`extend`) adds a selection (Helix semantics).
    pub(crate) fn search_step(&mut self, re: &Regex, backwards: bool, count: usize, extend: bool) -> Option<()> {
        let mut found = None;
        let mut pos_sel = self.sels.primary();
        for _ in 0..count.max(1) {
            let m = self.find_from(re, &pos_sel, backwards)?;
            pos_sel = Selection::new(m.start, m.end);
            found = Some(pos_sel);
        }
        let sel = found?;
        if extend {
            let mut v = self.sels.v.clone();
            v.push(sel);
            let primary = v.len() - 1;
            self.set_sels(Selections::new(v, primary));
        } else {
            let mut v = self.sels.v.clone();
            v[self.sels.primary] = sel;
            self.set_sels(Selections::new(v, self.sels.primary));
        }
        Some(())
    }

    /// First match after (or before) `from`, wrapping around the document.
    fn find_from(&mut self, re: &Regex, from: &Selection, backwards: bool) -> Option<Range<usize>> {
        let len = self.rope.len_chars();
        let non_empty = |r: &Range<usize>| r.end > r.start;
        if !backwards {
            let start = from.to().min(len);
            let hay = self.rope.slice(start..).to_string();
            if let Some(r) = char_matches(&hay, re, true).into_iter().find(non_empty) {
                return Some(start + r.start..start + r.end);
            }
            let hay = self.rope.slice(..start.min(len)).to_string();
            let r = char_matches(&hay, re, true).into_iter().find(non_empty)?;
            self.status("search wrapped around");
            Some(r)
        } else {
            let end = from.from().min(len);
            let hay = self.rope.slice(..end).to_string();
            if let Some(r) = char_matches(&hay, re, true).into_iter().rev().find(non_empty) {
                return Some(r);
            }
            let hay = self.rope.slice(end..).to_string();
            let r = char_matches(&hay, re, true).into_iter().rev().find(non_empty)?;
            self.status("search wrapped around");
            Some(end + r.start..end + r.end)
        }
    }

    /// `n` / `N` / F3 / shift-F3.
    pub(crate) fn search_repeat(&mut self, backwards: bool, count: usize, extend: bool) {
        let Some(pat) = self.search.pattern.clone() else {
            self.status("no search pattern");
            return;
        };
        match build_regex(&pat) {
            Err(e) => self.status(format!("invalid regex: {e}")),
            Ok(re) => {
                self.push_jump();
                if self.search_step(&re, backwards, count, extend).is_none() {
                    self.status("no matches");
                }
            }
        }
    }

    pub(crate) fn search_matches_impl(&self) -> Vec<Range<usize>> {
        let Some(pat) = self.search.pattern.clone() else { return Vec::new() };
        if let Some((rev, p, m)) = &*self.search.cache.borrow() {
            if *rev == self.revision && *p == pat {
                return m.clone();
            }
        }
        let Ok(re) = build_regex(&pat) else { return Vec::new() };
        let hay = self.rope.to_string();
        let mut out = Vec::new();
        let (mut last_byte, mut last_char) = (0usize, 0usize);
        for m in re.find_iter(&hay) {
            if m.start() == m.end() {
                continue;
            }
            last_char += hay[last_byte..m.start()].chars().count();
            let s = last_char;
            last_char += hay[m.start()..m.end()].chars().count();
            last_byte = m.end();
            out.push(s..last_char);
            if out.len() >= 5000 {
                break;
            }
        }
        *self.search.cache.borrow_mut() = Some((self.revision, pat, out.clone()));
        out
    }

    // ---- commands ----------------------------------------------------------------------------------

    pub(crate) fn exec_command(&mut self, line: &str) {
        let line = line.trim_start_matches(':').trim();
        if line.is_empty() {
            return;
        }
        let (name, args) = match line.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (line, ""),
        };
        if let Ok(n) = name.parse::<usize>() {
            self.goto_line_number(n);
            return;
        }
        match name {
            "w" | "write" => self.effects.push(Effect::Save),
            "q" | "quit" => self.effects.push(Effect::CloseTab),
            "qa" | "quit-all" | "qa!" | "quit!" => self.effects.push(Effect::Quit),
            "wq" | "write-quit" | "x" => {
                self.effects.push(Effect::Save);
                self.effects.push(Effect::CloseTab);
            }
            "run" | "r" => self.effects.push(Effect::RunStatementAtCursor),
            "run-all" => self.effects.push(Effect::RunAll),
            "explain" => self.effects.push(Effect::ExplainStatement { analyze: false }),
            "analyze" => self.effects.push(Effect::ExplainStatement { analyze: true }),
            "format" | "fmt" => self.effects.push(Effect::FormatDocument),
            "theme" if args.is_empty() => self.effects.push(Effect::OpenThemePicker),
            "connect" if args.is_empty() => self.effects.push(Effect::OpenConnectionPicker),
            "new" => self.effects.push(Effect::NewTab),
            "cancel" => self.effects.push(Effect::CancelQuery),
            "goto" | "g" => match args.parse::<usize>() {
                Ok(n) => self.goto_line_number(n),
                Err(_) => self.status("usage: goto <line>"),
            },
            "sort" => self.sort_command(),
            _ => self.effects.push(Effect::HostCommand(line.to_string())),
        }
    }

    fn goto_prompt(&mut self, text: &str) {
        let (l, c) = match text.split_once(':') {
            Some((l, c)) => (l.trim(), Some(c.trim())),
            None => (text, None),
        };
        match l.parse::<usize>() {
            Ok(n) => {
                self.goto_line_number(n);
                if let Some(Ok(col)) = c.map(|c| c.parse::<usize>()) {
                    let t = self.rope.slice(..);
                    let line = t.char_to_line(self.sels.primary().head.min(t.len_chars()));
                    let pos = (t.line_to_char(line) + col.saturating_sub(1)).min(text::line_end(t, line));
                    self.set_primary_only(Selection::point(pos));
                }
            }
            Err(_) => self.status("not a line number"),
        }
    }

    pub(crate) fn goto_line_number(&mut self, n: usize) {
        self.push_jump();
        let t = self.rope.slice(..);
        let line = n.saturating_sub(1).min(text::last_content_line(t));
        let pos = t.line_to_char(line);
        let extend = self.mode == Mode::Select;
        let p = self.sels.primary();
        let new = match self.keymap {
            Keymap::Helix => p.put_cursor(t, pos, extend),
            Keymap::Standard => Selection::point(pos),
        };
        self.set_primary_only(new);
    }

    fn sort_command(&mut self) {
        let t = self.rope.slice(..);
        let sels = self.sels.clone();
        let mut changes = Vec::new();
        if sels.len() > 1 {
            let texts: Vec<String> = sels.v.iter().map(|s| self.rope.slice(s.range()).to_string()).collect();
            let mut sorted = texts.clone();
            sorted.sort();
            for (s, new) in sels.v.iter().zip(sorted) {
                changes.push(Change { from: s.from(), to: s.to(), text: new });
            }
        } else {
            let s = sels.primary();
            let (l0, l1) = s.line_range(t);
            if l0 == l1 {
                self.status("nothing to sort");
                return;
            }
            let from = t.line_to_char(l0);
            let to = text::line_end(t, l1);
            let body = self.rope.slice(from..to).to_string();
            let mut lines: Vec<&str> = body.split('\n').collect();
            lines.sort();
            changes.push(Change { from, to, text: lines.join("\n") });
        }
        self.apply(Transaction::new(changes));
    }
}

fn byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

fn word_left(s: &str, cursor: usize) -> usize {
    let cs: Vec<char> = s.chars().collect();
    let mut i = cursor.min(cs.len());
    while i > 0 && !cs[i - 1].is_alphanumeric() {
        i -= 1;
    }
    while i > 0 && cs[i - 1].is_alphanumeric() {
        i -= 1;
    }
    i
}

fn word_right(s: &str, cursor: usize) -> usize {
    let cs: Vec<char> = s.chars().collect();
    let mut i = cursor.min(cs.len());
    while i < cs.len() && !cs[i].is_alphanumeric() {
        i += 1;
    }
    while i < cs.len() && cs[i].is_alphanumeric() {
        i += 1;
    }
    i
}

fn delete_word_back(p: &mut PromptState) -> bool {
    let target = word_left(&p.text, p.cursor);
    if target == p.cursor {
        return false;
    }
    let a = byte_idx(&p.text, target);
    let b = byte_idx(&p.text, p.cursor);
    p.text.replace_range(a..b, "");
    p.cursor = target;
    true
}

/// Matches of `re` in `hay` as char ranges (byte offsets converted incrementally).
pub(crate) fn char_matches(hay: &str, re: &Regex, skip_empty_at_end: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut last_byte, mut last_char) = (0usize, 0usize);
    for m in re.find_iter(hay) {
        last_char += hay[last_byte..m.start()].chars().count();
        let s = last_char;
        last_char += hay[m.start()..m.end()].chars().count();
        last_byte = m.end();
        if skip_empty_at_end && s == last_char {
            continue;
        }
        out.push(s..last_char);
    }
    out
}
