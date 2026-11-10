//! The `Editor`: buffer + selections + history, plus the public API the GPUI view drives.
//!
//! The per-keymap logic lives in `helix.rs` / `standard.rs`; this file owns the state they
//! share and the single choke point for text changes (`apply_with`), which keeps selections,
//! undo history, the syntax cache and the revision counter consistent.

use crate::history::{History, Step};
use crate::keys::KeyInput;
use crate::prompt::{PromptState, SearchState};
use crate::selection::{Selection, Selections};
use crate::syntax::{HighlightSpan, Syntax};
use crate::text::{self, Text};
use crate::transaction::{Assoc, Change, Transaction};
use ropey::Rope;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keymap {
    Helix,
    Standard,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Select,
    /// the only mode of the Standard keymap
    Standard,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScrollRequest {
    CursorCenter,
    CursorTop,
    CursorBottom,
    Lines(i32),
}

/// Things the editor asks its host to do. Returned from `handle_key`.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    SetClipboard(String),
    RunStatementAtCursor,
    RunSelection(String),
    RunAll,
    CancelQuery,
    ExplainStatement { analyze: bool },
    RequestCompletion,
    DismissCompletion,
    FormatDocument,
    FormatSelection,
    Save,
    CloseTab,
    NewTab,
    NextTab,
    PreviousTab,
    OpenCommandPalette,
    OpenConnectionPicker,
    /// fuzzy "go to table"
    OpenTablePicker,
    OpenThemePicker,
    FocusExplorer,
    FocusResults,
    ToggleBottomPanel,
    ToggleExplorer,
    /// Keep the primary cursor visible; the view scrolls minimally.
    RevealCursor,
    /// Vertical scroll request the core cannot do itself (zz/zt/zb, ctrl-e/ctrl-y ...)
    Scroll(ScrollRequest),
    /// One-line status message, e.g. "3 selections", "yanked 2 lines", errors from ':' commands
    Status(String),
    Quit,
    /// A ':' command the core does not know: host may handle (e.g. ":theme", ":connect")
    HostCommand(String),
}

pub struct KeyOutcome {
    /// false -> host may treat it as an unbound key
    pub handled: bool,
    pub effects: Vec<Effect>,
}

/// Bottom prompt for ':' commands, '/' '?' search, 's' select-regex, etc.
pub struct Prompt<'a> {
    /// ":" "/" "select:" ...
    pub prefix: &'a str,
    pub text: &'a str,
    /// char index in text
    pub cursor: usize,
}

/// Helix-style which-key entry.
pub struct KeyHint {
    pub key: String,
    pub description: String,
}

pub(crate) struct Group {
    pub forward: Vec<Transaction>,
    pub inverse: Vec<Transaction>,
    pub before: Selections,
}

pub struct Editor {
    pub(crate) rope: Rope,
    pub(crate) sels: Selections,
    pub(crate) keymap: Keymap,
    pub(crate) mode: Mode,
    pub(crate) revision: u64,
    pub(crate) saved_id: u64,
    pub(crate) history: History,
    pub(crate) group: Option<Group>,
    /// Selections at the point the next undo group would start (see `begin_key`).
    pub(crate) group_base: Selections,
    pub(crate) line_ending: &'static str,
    pub(crate) registers: HashMap<char, Vec<String>>,
    pub(crate) clipboard: Option<String>,
    /// What we last mirrored to the system clipboard: if the host later reports a different
    /// clipboard, another application changed it and `p` should paste that instead.
    pub(crate) last_mirror: Option<String>,
    pub(crate) viewport: (usize, usize),
    pub(crate) effects: Vec<Effect>,
    pub(crate) syntax: Syntax,
    pub(crate) hx: crate::helix::HelixState,
    pub(crate) std: crate::standard::StdState,
    pub(crate) prompt: Option<PromptState>,
    pub(crate) search: SearchState,
    pub(crate) vert: Option<Vec<usize>>,
    pub(crate) vert_keep: bool,
    pub(crate) drag_anchor: Option<usize>,
}

impl Editor {
    pub fn new(text: &str, keymap: Keymap) -> Self {
        let rope = Rope::from_str(text);
        let line_ending = text::detect_line_ending(&rope);
        let sels = Selections::single(Selection::point(0));
        let mut e = Editor {
            rope,
            sels: sels.clone(),
            keymap,
            mode: if keymap == Keymap::Helix { Mode::Normal } else { Mode::Standard },
            revision: 0,
            saved_id: 0,
            history: History::default(),
            group: None,
            group_base: sels,
            line_ending,
            registers: HashMap::new(),
            clipboard: None,
            last_mirror: None,
            viewport: (0, 30),
            effects: Vec::new(),
            syntax: Syntax::new(),
            hx: Default::default(),
            std: Default::default(),
            prompt: None,
            search: SearchState::default(),
            vert: None,
            vert_keep: false,
            drag_anchor: None,
        };
        e.set_sels(Selections::single(Selection::point(0)));
        e
    }

    pub fn keymap(&self) -> Keymap {
        self.keymap
    }

    /// Switching resets to Normal / Standard and drops any pending input; text and selections stay.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.commit_group();
        self.keymap = keymap;
        self.mode = if keymap == Keymap::Helix { Mode::Normal } else { Mode::Standard };
        self.hx = Default::default();
        self.std = Default::default();
        self.prompt = None;
        let s = self.sels.clone();
        self.set_sels(s);
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    // ---- text access -----------------------------------------------------------------------

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line content without its terminator.
    pub fn line(&self, line: usize) -> String {
        text::line_content(self.rope.slice(..), line)
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.rope.len_lines() - 1))
    }

    pub fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx.min(self.rope.len_chars()))
    }

    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        self.rope.char_to_byte(char_idx.min(self.rope.len_chars()))
    }

    pub fn byte_to_char(&self, byte_idx: usize) -> usize {
        self.rope.byte_to_char(byte_idx.min(self.rope.len_bytes()))
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        let len = self.rope.len_chars();
        let (s, e) = (range.start.min(len), range.end.min(len));
        if s >= e { String::new() } else { self.rope.slice(s..e).to_string() }
    }

    /// Increments on every text change (not on selection changes).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_modified(&self) -> bool {
        self.group.as_ref().is_some_and(|g| !g.forward.is_empty()) || self.history.current_id() != self.saved_id
    }

    pub fn mark_saved(&mut self) {
        self.commit_group();
        self.saved_id = self.history.current_id();
    }

    // ---- selections ------------------------------------------------------------------------

    pub fn selections(&self) -> &[Selection] {
        &self.sels.v
    }

    pub fn primary_index(&self) -> usize {
        self.sels.primary
    }

    pub fn primary(&self) -> Selection {
        self.sels.primary()
    }

    pub fn set_selections(&mut self, selections: Vec<Selection>, primary: usize) {
        self.vert = None;
        self.set_sels(Selections::new(selections, primary));
    }

    /// Grapheme-exact cursor position of `sel` for the current keymap (block cursor for Helix,
    /// bar at `head` for Standard). `Selection::cursor()` is the cheaper char-based variant.
    pub fn cursor_of(&self, sel: &Selection) -> usize {
        match self.keymap {
            Keymap::Helix => sel.block_cursor(self.rope.slice(..)),
            Keymap::Standard => sel.head,
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let p = self.sels.primary();
        let t = self.rope.slice(..);
        let real = match self.keymap {
            Keymap::Standard => !p.is_empty(),
            Keymap::Helix => text::next_grapheme(t, p.from()) < p.to(),
        };
        real.then(|| self.rope.slice(p.from()..p.to()).to_string())
    }

    pub(crate) fn t(&self) -> Text<'_> {
        self.rope.slice(..)
    }

    pub(crate) fn status(&mut self, msg: impl Into<String>) {
        self.effects.push(Effect::Status(msg.into()));
    }

    /// Install selections after enforcing the invariants of the active keymap: in range,
    /// grapheme-aligned, and (Helix) at least one grapheme wide where text exists.
    pub(crate) fn set_sels(&mut self, s: Selections) {
        let t = self.rope.slice(..);
        let len = t.len_chars();
        let helix = self.keymap == Keymap::Helix;
        let v: Vec<Selection> = s
            .v
            .iter()
            .map(|r| {
                let r = Selection::new(r.anchor.min(len), r.head.min(len)).grapheme_aligned(t);
                if helix { r.min_width_1(t) } else { r }
            })
            .collect();
        self.sels = Selections::new(v, s.primary);
    }

    pub(crate) fn set_primary_only(&mut self, sel: Selection) {
        self.set_sels(Selections::single(sel));
    }

    // ---- the one place text changes ----------------------------------------------------------

    pub(crate) fn map_selections(&self, tx: &Transaction) -> Selections {
        let v = self
            .sels
            .v
            .iter()
            .map(|s| {
                let (a, h) = if s.anchor == s.head {
                    (tx.map_pos(s.anchor, Assoc::After), tx.map_pos(s.head, Assoc::After))
                } else if s.anchor < s.head {
                    (tx.map_pos(s.anchor, Assoc::After), tx.map_pos(s.head, Assoc::Before))
                } else {
                    (tx.map_pos(s.anchor, Assoc::Before), tx.map_pos(s.head, Assoc::After))
                };
                Selection::new(a, h)
            })
            .collect();
        Selections { v, primary: self.sels.primary }
    }

    fn note_syntax(&mut self, tx: &Transaction) {
        if let Some((from, _)) = tx.span() {
            let line = self.rope.char_to_line(from.min(self.rope.len_chars()));
            self.syntax.note_edit(line);
        }
    }

    /// Apply `tx`, map selections through it (or install `sels`), and record undo information.
    pub(crate) fn apply_with(&mut self, tx: Transaction, sels: Option<Selections>) {
        if tx.is_empty() {
            if let Some(s) = sels {
                self.set_sels(s);
            }
            return;
        }
        let mapped = self.map_selections(&tx);
        self.note_syntax(&tx);
        if self.group.is_none() {
            self.group = Some(Group { forward: vec![], inverse: vec![], before: self.group_base.clone() });
        }
        let inverse = tx.apply(&mut self.rope);
        self.revision += 1;
        if let Some(g) = &mut self.group {
            g.forward.push(tx);
            g.inverse.push(inverse);
        }
        self.hx.changed = true;
        self.set_sels(sels.unwrap_or(mapped));
    }

    pub(crate) fn apply(&mut self, tx: Transaction) {
        self.apply_with(tx, None);
    }

    pub(crate) fn commit_group(&mut self) {
        if let Some(g) = self.group.take() {
            if !g.forward.is_empty() {
                let id = self.history.fresh_id();
                self.history.undo.push(Step {
                    id,
                    forward: g.forward,
                    inverse: g.inverse,
                    before: g.before,
                    after: self.sels.clone(),
                });
                self.history.redo.clear();
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.commit_group();
        let Some(step) = self.history.undo.pop() else { return false };
        for inv in step.inverse.iter().rev() {
            self.note_syntax(inv);
            inv.apply(&mut self.rope);
        }
        self.revision += 1;
        let before = step.before.clone();
        self.history.redo.push(step);
        self.set_sels(before);
        self.vert = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        self.commit_group();
        let Some(step) = self.history.redo.pop() else { return false };
        for fwd in &step.forward {
            self.note_syntax(fwd);
            fwd.apply(&mut self.rope);
        }
        self.revision += 1;
        let after = step.after.clone();
        self.history.undo.push(step);
        self.set_sels(after);
        self.vert = None;
        true
    }

    // ---- programmatic edits -------------------------------------------------------------------

    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let len = self.rope.len_chars();
        let (from, to) = (range.start.min(len), range.end.min(len).max(range.start.min(len)));
        self.commit_group();
        self.group_base = self.sels.clone();
        self.apply(Transaction::new(vec![Change { from, to, text: text.to_string() }]));
        self.commit_group();
        self.vert = None;
    }

    /// Replace the whole text with a minimal diff (common prefix and suffix), so cursors outside
    /// the changed middle stay where they are.
    pub fn set_text(&mut self, text: &str) {
        let old: Vec<char> = self.rope.chars().collect();
        let new: Vec<char> = text.chars().collect();
        let mut pre = 0;
        while pre < old.len() && pre < new.len() && old[pre] == new[pre] {
            pre += 1;
        }
        let mut suf = 0;
        while suf < old.len() - pre && suf < new.len() - pre && old[old.len() - 1 - suf] == new[new.len() - 1 - suf] {
            suf += 1;
        }
        if pre + suf == old.len() && old.len() == new.len() {
            return;
        }
        let mid: String = new[pre..new.len() - suf].iter().collect();
        self.replace_range(pre..old.len() - suf, &mid);
    }

    // ---- viewport / UI state ---------------------------------------------------------------------

    pub fn set_viewport(&mut self, first_visible_line: usize, visible_lines: usize) {
        self.viewport = (first_visible_line, visible_lines.max(1));
    }

    pub fn set_clipboard(&mut self, text: Option<String>) {
        self.clipboard = text;
    }

    pub fn prompt(&self) -> Option<Prompt<'_>> {
        self.prompt.as_ref().map(|p| Prompt { prefix: &p.prefix, text: &p.text, cursor: p.cursor })
    }

    pub fn matching_bracket(&self) -> Option<(usize, usize)> {
        let pos = self.cursor_of(&self.sels.primary());
        crate::objects::bracket_near(self.rope.slice(..), pos)
    }

    pub fn highlights(&mut self, line: usize) -> Vec<HighlightSpan> {
        self.syntax.highlights(&self.rope, self.revision, line)
    }

    // ---- mouse ------------------------------------------------------------------------------------

    pub fn click(&mut self, char_idx: usize, extend: bool, add: bool) {
        self.commit_group();
        self.std.run_open = false;
        self.vert = None;
        let t = self.rope.slice(..);
        let pos = text::floor_grapheme(t, char_idx.min(t.len_chars()));
        if self.keymap == Keymap::Helix && self.mode == Mode::Insert {
            self.mode = Mode::Normal;
        }
        let p = self.sels.primary();
        let new = match (self.keymap, extend) {
            (Keymap::Helix, true) => p.put_cursor(t, pos, true),
            (Keymap::Standard, true) => Selection::new(p.anchor, pos),
            _ => Selection::point(pos),
        };
        if !extend {
            self.drag_anchor = Some(pos);
        }
        if add && !extend {
            let mut v = self.sels.v.clone();
            v.push(new);
            let primary = v.len() - 1;
            self.set_sels(Selections::new(v, primary));
        } else {
            let mut v = self.sels.v.clone();
            if add {
                v[self.sels.primary] = new;
                self.set_sels(Selections::new(v, self.sels.primary));
            } else {
                self.set_sels(Selections::single(new));
            }
        }
    }

    pub fn drag_to(&mut self, char_idx: usize) {
        let t = self.rope.slice(..);
        let pos = text::floor_grapheme(t, char_idx.min(t.len_chars()));
        let a = self.drag_anchor.unwrap_or_else(|| self.sels.primary().anchor);
        let new = match self.keymap {
            Keymap::Helix => Selection::point(a).min_width_1(t).put_cursor(t, pos, true),
            Keymap::Standard => Selection::new(a, pos),
        };
        let mut v = self.sels.v.clone();
        v[self.sels.primary] = new;
        self.set_sels(Selections::new(v, self.sels.primary));
    }

    pub fn double_click(&mut self, char_idx: usize) {
        self.click(char_idx, false, false);
        let t = self.rope.slice(..);
        let len = t.len_chars();
        if len == 0 {
            return;
        }
        let i = char_idx.min(len - 1);
        let class = text::classify(t.char(i));
        let same = |j: usize| text::classify(t.char(j)) == class;
        let (mut s, mut e) = (i, i + 1);
        while s > 0 && same(s - 1) && class != text::CharClass::Eol {
            s -= 1;
        }
        while e < len && same(e) && class != text::CharClass::Eol {
            e += 1;
        }
        self.set_primary_only(Selection::new(s, e));
    }

    pub fn triple_click(&mut self, char_idx: usize) {
        self.click(char_idx, false, false);
        let t = self.rope.slice(..);
        let line = t.char_to_line(char_idx.min(t.len_chars()));
        let s = t.line_to_char(line);
        let e = t.line_to_char((line + 1).min(t.len_lines()));
        self.set_primary_only(Selection::new(s, e));
    }

    // ---- input -------------------------------------------------------------------------------------

    pub fn handle_key(&mut self, input: &KeyInput) -> crate::editor::KeyOutcome {
        let k = input.clone().canon();
        let rev0 = self.revision;
        let sel0 = (self.sels.primary(), self.sels.len());
        self.begin_key(&k);
        let handled = self.dispatch(&k);
        self.end_key();
        if self.revision != rev0 || (self.sels.primary(), self.sels.len()) != sel0 {
            self.effects.push(Effect::RevealCursor);
        }
        KeyOutcome { handled, effects: std::mem::take(&mut self.effects) }
    }

    fn begin_key(&mut self, k: &KeyInput) {
        if self.keymap == Keymap::Helix {
            if self.group.is_none() && self.mode != Mode::Insert {
                self.group_base = self.sels.clone();
            }
            if self.hx.replaying == 0 {
                self.hx.cmd_keys.push(k.clone());
                if let Some((_, rec)) = &mut self.hx.macro_rec {
                    rec.push(k.clone());
                }
            }
        } else {
            if self.group.is_none() {
                self.group_base = self.sels.clone();
            }
            let was_run = self.std.run_open;
            self.std.run_open = false;
            self.std.was_run = was_run;
            if was_run && k.text_char().is_none() {
                self.commit_group();
                self.group_base = self.sels.clone();
            }
        }
    }

    fn end_key(&mut self) {
        if !self.vert_keep {
            self.vert = None;
        }
        self.vert_keep = false;
        match self.keymap {
            Keymap::Helix => {
                if self.mode != Mode::Insert {
                    self.commit_group();
                }
                self.hx_finish_command();
            }
            Keymap::Standard => {
                if !self.std.run_open {
                    self.commit_group();
                }
            }
        }
    }

    fn dispatch(&mut self, k: &KeyInput) -> bool {
        if self.prompt.is_some() {
            return self.prompt_key(k);
        }
        if self.global_key(k) {
            return true;
        }
        match self.keymap {
            Keymap::Helix => self.helix_key(k),
            Keymap::Standard => self.standard_key(k),
        }
    }

    /// Bindings that exist in every mode of both keymaps.
    pub(crate) fn global_key(&mut self, k: &KeyInput) -> bool {
        use crate::keys::Key;
        let m = &k.modifiers;
        match &k.key {
            Key::Enter if (m.ctrl || m.platform) && m.shift => {
                self.effects.push(Effect::RunAll);
                true
            }
            Key::Enter if m.ctrl || m.platform => {
                match self.selected_text() {
                    Some(t) => self.effects.push(Effect::RunSelection(t)),
                    None => self.effects.push(Effect::RunStatementAtCursor),
                }
                true
            }
            Key::Char(' ') if m.ctrl && !m.alt => {
                self.effects.push(Effect::RequestCompletion);
                true
            }
            Key::F(5) if !m.ctrl && !m.alt && !m.shift => {
                self.effects.push(Effect::RunAll);
                true
            }
            _ => false,
        }
    }

    pub fn insert_text(&mut self, text: &str) -> KeyOutcome {
        let rev0 = self.revision;
        let handled = match self.keymap {
            Keymap::Helix => {
                if self.mode == Mode::Insert && self.prompt.is_none() {
                    self.hx_insert_text(text);
                    true
                } else if self.prompt.is_some() {
                    self.prompt_insert(text);
                    true
                } else {
                    false
                }
            }
            Keymap::Standard => {
                if self.prompt.is_some() {
                    self.prompt_insert(text);
                } else {
                    self.std_paste_text(text);
                }
                true
            }
        };
        if self.keymap == Keymap::Helix && self.mode != Mode::Insert {
            self.commit_group();
        }
        if self.keymap == Keymap::Standard {
            self.commit_group();
        }
        if self.revision != rev0 {
            self.effects.push(Effect::RevealCursor);
        }
        KeyOutcome { handled, effects: std::mem::take(&mut self.effects) }
    }

    pub fn pending_keys(&self) -> String {
        match self.keymap {
            Keymap::Helix => self.hx_pending_string(),
            Keymap::Standard => self.std.chord.as_ref().map(|k| k.display()).unwrap_or_default(),
        }
    }

    pub fn key_hints(&self) -> Option<(String, Vec<KeyHint>)> {
        match self.keymap {
            Keymap::Helix if self.prompt.is_none() => self.hx_key_hints(),
            Keymap::Standard if self.prompt.is_none() && self.std.chord.is_some() => Some((
                "ctrl-k".to_string(),
                vec![KeyHint { key: "ctrl-t".into(), description: "select theme".into() }],
            )),
            _ => None,
        }
    }

    pub fn search_matches(&self) -> Vec<Range<usize>> {
        self.search_matches_impl()
    }
}
