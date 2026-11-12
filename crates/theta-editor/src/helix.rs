//! The Helix keymap: key trie, input state machine and the Normal/Select commands.
//!
//! Commands are written from the documented behaviour of Helix (book + observable results),
//! not copied from it. Each command works on *all* selections at once.

use crate::editor::{Editor, Effect, KeyHint, Mode, ScrollRequest};
use crate::keys::{Key, KeyInput};
use crate::motion::{self, WordKind};
use crate::objects;
use crate::prompt::PromptKind;
use crate::selection::{Selection, Selections};
use crate::text::{self, Text};
use crate::transaction::{Change, Transaction};
use std::collections::HashMap;
use std::sync::OnceLock;

const MAX_COUNT: usize = 10_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Await {
    Register,
    Find { forward: bool, till: bool },
    Replace,
    TextObject { inner: bool },
    SurroundAdd,
    SurroundDelete,
    SurroundReplaceFrom,
    SurroundReplaceTo(Option<char>),
}

#[derive(Default)]
pub(crate) struct HelixState {
    pub pending: Vec<KeyInput>,
    pub count: Option<usize>,
    pub reg: Option<char>,
    pub await_: Option<Await>,
    pub last_find: Option<(char, bool, bool)>,
    pub jumps: Vec<Selections>,
    pub jump_idx: usize,
    pub macro_rec: Option<(char, Vec<KeyInput>)>,
    pub macros: HashMap<char, Vec<KeyInput>>,
    pub cmd_keys: Vec<KeyInput>,
    pub last_change: Vec<KeyInput>,
    pub changed: bool,
    pub replaying: u32,
    pub no_record: bool,
    pub restore_cursor: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Cmd {
    CharLeft,
    CharRight,
    LineDown,
    LineUp,
    NextWordStart,
    PrevWordStart,
    NextWordEnd,
    NextLongWordStart,
    PrevLongWordStart,
    NextLongWordEnd,
    PrevWordEnd,
    FindNext,
    FindTill,
    FindPrev,
    TillPrev,
    RepeatFind,
    LineStart,
    LineEnd,
    FirstNonBlank,
    FileStart,
    FileEnd,
    GotoLine,
    WindowTop,
    WindowCenter,
    WindowBottom,
    GotoColumn,
    NextParagraph,
    PrevParagraph,
    AddNewlineBelow,
    AddNewlineAbove,
    PageUp,
    PageDown,
    HalfUp,
    HalfDown,
    SelectAll,
    ExtendLine,
    ExtendToLineBounds,
    ShrinkToLineBounds,
    Collapse,
    Flip,
    EnsureForward,
    KeepPrimary,
    RemovePrimary,
    RotateBack,
    RotateFwd,
    RotateContentsBack,
    RotateContentsFwd,
    CopySelNext,
    CopySelPrev,
    SelectRegex,
    SplitRegex,
    SplitNewline,
    Keep,
    Remove,
    Trim,
    Align,
    MergeSelections,
    MergeConsecutive,
    SelectMode,
    Insert,
    Append,
    InsertLineStart,
    InsertLineEnd,
    OpenBelow,
    OpenAbove,
    CommandMode,
    Escape,
    Delete,
    DeleteNoYank,
    ChangeSel,
    ChangeSelNoYank,
    Yank,
    PasteAfter,
    PasteBefore,
    ReplaceWithYanked,
    ReplaceChar,
    SwitchCase,
    Lowercase,
    Uppercase,
    Indent,
    Unindent,
    Format,
    Join,
    JoinSpace,
    Undo,
    Redo,
    ToggleComments,
    Increment,
    Decrement,
    RepeatLast,
    RecordMacro,
    ReplayMacro,
    MatchBracket,
    SurroundAdd,
    SurroundDelete,
    SurroundReplace,
    TextObjInner,
    TextObjAround,
    Search,
    RSearch,
    SearchNext,
    SearchPrev,
    SearchSelection,
    SearchSelectionRaw,
    JumpBack,
    JumpForward,
    SaveSelection,
    SelectRegister,
    Scroll(ScrollRequest),
    Fx(FxKind),
}

/// Space-menu style commands that are pure host effects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FxKind {
    TablePicker,
    ConnectionPicker,
    Palette,
    YankClipboard,
    YankPrimaryClipboard,
    PasteClipboardAfter,
    PasteClipboardBefore,
    Run,
    RunAll,
    Explain,
    Analyze,
    Complete,
    Theme,
    CommentLines,
    FocusExplorer,
    FocusResults,
    ToggleExplorer,
    ToggleBottomPanel,
    Cancel,
    GoToDefinition,
    FindUsages,
    Rename,
    CodeActions,
    ShowProblems,
    NextProblem,
    PrevProblem,
}

pub(crate) enum Node {
    Leaf(Cmd, &'static str),
    Sub(Sub),
}

pub(crate) struct Sub {
    pub name: &'static str,
    pub entries: Vec<(KeyInput, Node)>,
}

impl Sub {
    fn new(name: &'static str) -> Self {
        Sub { name, entries: Vec::new() }
    }

    fn get(&self, k: &KeyInput) -> Option<&Node> {
        self.entries.iter().find(|(key, _)| key == k).map(|(_, n)| n)
    }

    fn sub_mut(&mut self, key: &str, name: &'static str) -> &mut Sub {
        let ki = KeyInput::parse(key).expect("valid key");
        self.entries.push((ki, Node::Sub(Sub::new(name))));
        match &mut self.entries.last_mut().unwrap().1 {
            Node::Sub(s) => s,
            _ => unreachable!(),
        }
    }

    /// Bind one or several `|`-separated keys.
    fn bind(&mut self, keys: &str, cmd: Cmd, desc: &'static str) {
        for key in keys.split(' ') {
            let ki = KeyInput::parse(key).unwrap_or_else(|| panic!("bad key {key}"));
            self.entries.push((ki, Node::Leaf(cmd, desc)));
        }
    }
}

enum Lookup {
    Leaf(Cmd),
    Sub,
    None,
}

fn lookup(root: &Sub, keys: &[KeyInput]) -> Lookup {
    let mut cur = root;
    for (i, k) in keys.iter().enumerate() {
        match cur.get(k) {
            Some(Node::Leaf(c, _)) if i + 1 == keys.len() => return Lookup::Leaf(*c),
            Some(Node::Sub(s)) => {
                if i + 1 == keys.len() {
                    return Lookup::Sub;
                }
                cur = s;
            }
            _ => return Lookup::None,
        }
    }
    Lookup::None
}

pub(crate) fn trie() -> &'static Sub {
    static TRIE: OnceLock<Sub> = OnceLock::new();
    TRIE.get_or_init(build_trie)
}

fn build_trie() -> Sub {
    use Cmd::*;
    let mut r = Sub::new("Normal mode");
    r.bind("h left", CharLeft, "move left");
    r.bind("l right", CharRight, "move right");
    r.bind("j down", LineDown, "move down");
    r.bind("k up", LineUp, "move up");
    r.bind("w", NextWordStart, "next word start");
    r.bind("b", PrevWordStart, "previous word start");
    r.bind("e", NextWordEnd, "next word end");
    r.bind("W", NextLongWordStart, "next WORD start");
    r.bind("B", PrevLongWordStart, "previous WORD start");
    r.bind("E", NextLongWordEnd, "next WORD end");
    r.bind("alt-e", PrevWordEnd, "previous word end");
    r.bind("f", FindNext, "find next char");
    r.bind("t", FindTill, "till next char");
    r.bind("F", FindPrev, "find previous char");
    r.bind("T", TillPrev, "till previous char");
    r.bind("alt-.", RepeatFind, "repeat last find");
    r.bind("home", LineStart, "line start");
    r.bind("end", LineEnd, "line end");
    r.bind("ctrl-home", FileStart, "file start");
    r.bind("ctrl-end", FileEnd, "last line");
    r.bind("G", GotoLine, "goto line");
    r.bind("%", SelectAll, "select all");
    r.bind("x", ExtendLine, "select line");
    r.bind("X", ExtendToLineBounds, "extend to line bounds");
    r.bind("alt-x", ShrinkToLineBounds, "shrink to line bounds");
    r.bind(";", Collapse, "collapse selection");
    r.bind("alt-;", Flip, "flip selection");
    r.bind("alt-:", EnsureForward, "ensure forward");
    r.bind(",", KeepPrimary, "keep primary selection");
    r.bind("alt-,", RemovePrimary, "remove primary selection");
    r.bind("(", RotateBack, "rotate selections backward");
    r.bind(")", RotateFwd, "rotate selections forward");
    r.bind("alt-(", RotateContentsBack, "rotate contents backward");
    r.bind("alt-)", RotateContentsFwd, "rotate contents forward");
    r.bind("C", CopySelNext, "copy selection to next line");
    r.bind("alt-C", CopySelPrev, "copy selection to previous line");
    r.bind("s", SelectRegex, "select regex matches");
    r.bind("S", SplitRegex, "split selection on regex");
    r.bind("alt-s", SplitNewline, "split selection on newlines");
    r.bind("K", Keep, "keep matching selections");
    r.bind("alt-K", Remove, "remove matching selections");
    r.bind("_", Trim, "trim whitespace from selections");
    r.bind("&", Align, "align selections");
    r.bind("alt-minus", MergeSelections, "merge selections");
    r.bind("alt-_", MergeConsecutive, "merge consecutive selections");
    r.bind("v", SelectMode, "select mode");
    r.bind("i", Insert, "insert before selection");
    r.bind("a", Append, "append after selection");
    r.bind("I", InsertLineStart, "insert at line start");
    r.bind("A", InsertLineEnd, "append at line end");
    r.bind("o", OpenBelow, "open line below");
    r.bind("O", OpenAbove, "open line above");
    r.bind(":", CommandMode, "command mode");
    r.bind("esc", Escape, "normal mode");
    r.bind("d", Delete, "delete selection");
    r.bind("alt-d", DeleteNoYank, "delete without yanking");
    r.bind("c", ChangeSel, "change selection");
    r.bind("alt-c", ChangeSelNoYank, "change without yanking");
    r.bind("y", Yank, "yank selection");
    r.bind("p", PasteAfter, "paste after");
    r.bind("P", PasteBefore, "paste before");
    r.bind("R", ReplaceWithYanked, "replace with yanked text");
    r.bind("r", ReplaceChar, "replace with character");
    r.bind("~", SwitchCase, "switch case");
    r.bind("`", Lowercase, "to lowercase");
    r.bind("alt-`", Uppercase, "to uppercase");
    r.bind(">", Indent, "indent");
    r.bind("<", Unindent, "unindent");
    r.bind("=", Format, "format selection");
    r.bind("J", Join, "join lines");
    r.bind("alt-J", JoinSpace, "join lines, select spaces");
    r.bind("u", Undo, "undo");
    r.bind("U", Redo, "redo");
    r.bind("ctrl-c", ToggleComments, "toggle line comments");
    r.bind("ctrl-a", Increment, "increment number");
    r.bind("ctrl-x", Decrement, "decrement number");
    r.bind(".", RepeatLast, "repeat last change");
    r.bind("Q", RecordMacro, "record macro");
    r.bind("q", ReplayMacro, "replay macro");
    r.bind("/", Search, "search");
    r.bind("?", RSearch, "reverse search");
    r.bind("n", SearchNext, "next match");
    r.bind("N", SearchPrev, "previous match");
    r.bind("*", SearchSelection, "search for selection (word boundaries)");
    r.bind("alt-*", SearchSelectionRaw, "search for selection");
    r.bind("ctrl-o", JumpBack, "jump backward");
    r.bind("ctrl-i tab", JumpForward, "jump forward");
    r.bind("ctrl-s", SaveSelection, "save selection to jumplist");
    r.bind("ctrl-b pageup", PageUp, "page up");
    r.bind("ctrl-f pagedown", PageDown, "page down");
    r.bind("ctrl-u", HalfUp, "half page up");
    r.bind("ctrl-d", HalfDown, "half page down");
    r.bind("\"", SelectRegister, "select register");
    {
        let g = r.sub_mut("g", "Goto");
        g.bind("g", FileStart, "file start");
        g.bind("e", FileEnd, "last line");
        g.bind("h", LineStart, "line start");
        g.bind("l", LineEnd, "line end");
        g.bind("s", FirstNonBlank, "first non-blank in line");
        g.bind("t", WindowTop, "window top");
        g.bind("c", WindowCenter, "window center");
        g.bind("b", WindowBottom, "window bottom");
        g.bind("j", LineDown, "move down");
        g.bind("k", LineUp, "move up");
        g.bind("|", GotoColumn, "goto column");
        g.bind("d", Cmd::Fx(FxKind::GoToDefinition), "go to definition");
        g.bind("r", Cmd::Fx(FxKind::FindUsages), "find usages");
    }
    {
        let m = r.sub_mut("m", "Match");
        m.bind("m", MatchBracket, "goto matching bracket");
        m.bind("s", SurroundAdd, "surround add");
        m.bind("r", SurroundReplace, "surround replace");
        m.bind("d", SurroundDelete, "surround delete");
        m.bind("a", TextObjAround, "select around object");
        m.bind("i", TextObjInner, "select inside object");
    }
    {
        let l = r.sub_mut("[", "Left bracket");
        l.bind("p", PrevParagraph, "previous paragraph");
        l.bind("d", Cmd::Fx(FxKind::PrevProblem), "previous problem");
        l.bind("space", AddNewlineAbove, "add newline above");
        let rr = r.sub_mut("]", "Right bracket");
        rr.bind("p", NextParagraph, "next paragraph");
        rr.bind("d", Cmd::Fx(FxKind::NextProblem), "next problem");
        rr.bind("space", AddNewlineBelow, "add newline below");
    }
    {
        let s = r.sub_mut("space", "Space");
        s.bind("f", Cmd::Fx(FxKind::TablePicker), "go to table");
        s.bind("b", Cmd::Fx(FxKind::ConnectionPicker), "switch connection");
        s.bind("? space", Cmd::Fx(FxKind::Palette), "command palette");
        s.bind("y", Cmd::Fx(FxKind::YankClipboard), "yank to clipboard");
        s.bind("Y", Cmd::Fx(FxKind::YankPrimaryClipboard), "yank main selection to clipboard");
        s.bind("p", Cmd::Fx(FxKind::PasteClipboardAfter), "paste clipboard after");
        s.bind("P", Cmd::Fx(FxKind::PasteClipboardBefore), "paste clipboard before");
        s.bind("r", Cmd::Fx(FxKind::Run), "run statement / selection");
        s.bind("R", Cmd::Fx(FxKind::RunAll), "run all");
        s.bind("e", Cmd::Fx(FxKind::Explain), "explain");
        s.bind("E", Cmd::Fx(FxKind::Analyze), "explain analyze");
        s.bind("k", Cmd::Fx(FxKind::Complete), "completions");
        s.bind("t", Cmd::Fx(FxKind::Theme), "theme picker");
        s.bind("c", Cmd::Fx(FxKind::CommentLines), "toggle line comments");
        s.bind("x", Cmd::Fx(FxKind::Cancel), "cancel running query");
        s.bind("/", Cmd::Fx(FxKind::TablePicker), "search tables");
        s.bind("g", Cmd::Fx(FxKind::FocusResults), "focus results");
        s.bind("d", Cmd::Fx(FxKind::FocusExplorer), "focus explorer");
        s.bind("T", Cmd::Fx(FxKind::ToggleExplorer), "toggle explorer");
        s.bind("a", Cmd::Fx(FxKind::CodeActions), "code actions (quick fixes)");
        s.bind("n", Cmd::Fx(FxKind::Rename), "rename alias / CTE / column alias");
        s.bind("D", Cmd::Fx(FxKind::ShowProblems), "problems");
        s.bind("j", Cmd::Fx(FxKind::ToggleBottomPanel), "toggle results panel");
    }
    for (key, name) in [("z", "View"), ("Z", "View")] {
        let z = r.sub_mut(key, name);
        z.bind("z c", Scroll(ScrollRequest::CursorCenter), "center cursor");
        z.bind("t", Scroll(ScrollRequest::CursorTop), "cursor to top");
        z.bind("b", Scroll(ScrollRequest::CursorBottom), "cursor to bottom");
        z.bind("m", Scroll(ScrollRequest::CursorCenter), "center cursor");
        z.bind("j down", Scroll(ScrollRequest::Lines(1)), "scroll down");
        z.bind("k up", Scroll(ScrollRequest::Lines(-1)), "scroll up");
        z.bind("ctrl-b pageup", PageUp, "page up");
        z.bind("ctrl-f pagedown", PageDown, "page down");
        z.bind("ctrl-u backspace", HalfUp, "half page up");
        z.bind("ctrl-d space", HalfDown, "half page down");
        z.bind("/", Search, "search");
        z.bind("?", RSearch, "reverse search");
        z.bind("n", SearchNext, "next match");
        z.bind("N", SearchPrev, "previous match");
    }
    r
}

const OBJECT_HINTS: &[(&str, &str)] = &[
    ("w", "word"),
    ("W", "WORD"),
    ("p", "paragraph"),
    ("(", "parentheses"),
    ("[", "square brackets"),
    ("{", "curly braces"),
    ("<", "angle brackets"),
    ("'", "single quotes"),
    ("\"", "double quotes"),
    ("`", "backticks"),
    ("m", "closest surrounding pair"),
];

impl Editor {
    pub(crate) fn hx_pending_string(&self) -> String {
        let mut s = String::new();
        if let Some(r) = self.hx.reg {
            s.push('"');
            s.push(r);
        } else if self.hx.await_ == Some(Await::Register) {
            s.push('"');
        }
        if let Some(c) = self.hx.count {
            s.push_str(&c.to_string());
        }
        for k in &self.hx.pending {
            s.push_str(&k.display());
        }
        s
    }

    pub(crate) fn hx_key_hints(&self) -> Option<(String, Vec<KeyHint>)> {
        let hint = |k: &str, d: &str| KeyHint { key: k.to_string(), description: d.to_string() };
        match self.hx.await_ {
            Some(Await::Register) => {
                return Some((
                    "Register".into(),
                    vec![
                        hint("a-z", "named register"),
                        hint("_", "black hole"),
                        hint("+", "system clipboard"),
                        hint("*", "system clipboard"),
                        hint("/", "search register"),
                        hint("\"", "default register"),
                    ],
                ));
            }
            Some(Await::TextObject { inner }) => {
                let t = if inner { "Match inside" } else { "Match around" };
                return Some((t.into(), OBJECT_HINTS.iter().map(|(k, d)| hint(k, d)).collect()));
            }
            Some(Await::SurroundAdd) => {
                return Some((
                    "Surround with".into(),
                    OBJECT_HINTS.iter().skip(3).take(7).map(|(k, d)| hint(k, d)).collect(),
                ));
            }
            _ => {}
        }
        if self.hx.pending.is_empty() || self.mode == Mode::Insert {
            return None;
        }
        let mut cur = trie();
        for k in &self.hx.pending {
            match cur.get(k) {
                Some(Node::Sub(s)) => cur = s,
                _ => return None,
            }
        }
        // group keys that share a description (e.g. "h"/"left")
        let mut hints: Vec<KeyHint> = Vec::new();
        for (k, n) in &cur.entries {
            let (d, is_sub) = match n {
                Node::Leaf(_, d) => (d.to_string(), false),
                Node::Sub(s) => (s.name.to_string(), true),
            };
            let _ = is_sub;
            match hints.iter_mut().find(|h| h.description == d) {
                Some(h) => {
                    h.key.push(' ');
                    h.key.push_str(&k.display());
                }
                None => hints.push(KeyHint { key: k.display(), description: d }),
            }
        }
        Some((cur.name.to_string(), hints))
    }

    /// Called at the end of every Helix key: finalises dot-repeat bookkeeping.
    pub(crate) fn hx_finish_command(&mut self) {
        if self.mode == Mode::Insert
            || self.hx.await_.is_some()
            || !self.hx.pending.is_empty()
            || self.prompt.is_some()
        {
            return;
        }
        if self.hx.replaying == 0 {
            if self.hx.no_record {
                // the `.` key itself must not overwrite what it replays
            } else if self.hx.changed {
                self.hx.last_change = std::mem::take(&mut self.hx.cmd_keys);
            }
            self.hx.cmd_keys.clear();
        }
        self.hx.no_record = false;
        self.hx.changed = false;
    }

    pub(crate) fn helix_key(&mut self, k: &KeyInput) -> bool {
        if self.mode == Mode::Insert {
            return self.insert_key(k);
        }
        self.normal_key(k)
    }

    fn normal_key(&mut self, k: &KeyInput) -> bool {
        if let Some(aw) = self.hx.await_.take() {
            if k.key == Key::Escape {
                self.hx.count = None;
                self.hx.reg = None;
                return true;
            }
            return self.finish_await(aw, k);
        }
        let plain = !k.modifiers.ctrl && !k.modifiers.alt && !k.modifiers.platform;
        if self.hx.pending.is_empty()
            && plain
            && let Key::Char(d @ '0'..='9') = k.key
            && (d != '0' || self.hx.count.is_some())
        {
            // Counts are capped so `99999999o` can't hang the UI thread.
            let v = self.hx.count.unwrap_or(0).saturating_mul(10).saturating_add(d as usize - '0' as usize);
            self.hx.count = Some(v.min(MAX_COUNT));
            return true;
        }
        if k.key == Key::Escape {
            let had = !self.hx.pending.is_empty() || self.hx.count.is_some() || self.hx.reg.is_some();
            self.hx.pending.clear();
            self.hx.count = None;
            self.hx.reg = None;
            if had {
                return true;
            }
        }
        self.hx.pending.push(k.clone());
        match lookup(trie(), &self.hx.pending) {
            Lookup::Leaf(cmd) => {
                self.hx.pending.clear();
                let count = self.hx.count.take();
                self.run(cmd, count);
                if self.hx.await_.is_none() {
                    self.hx.reg = None;
                }
                true
            }
            Lookup::Sub => true,
            Lookup::None => {
                let depth = self.hx.pending.len();
                self.hx.pending.clear();
                self.hx.count = None;
                self.hx.reg = None;
                depth > 1
            }
        }
    }

    fn finish_await(&mut self, aw: Await, k: &KeyInput) -> bool {
        let count = self.hx.count.take();
        let cnt = count.unwrap_or(1);
        let ch = match k.key {
            Key::Char(c) if !k.modifiers.ctrl && !k.modifiers.alt && !k.modifiers.platform => Some(c),
            Key::Enter => Some('\n'),
            Key::Tab => Some('\t'),
            _ => None,
        };
        let extend = self.mode == Mode::Select;
        match aw {
            Await::Register => {
                if let Some(c) = ch {
                    self.hx.reg = Some(c);
                    self.hx.count = count;
                }
            }
            Await::Find { forward, till } => {
                if let Some(c) = ch {
                    self.hx.last_find = Some((c, forward, till));
                    self.find_char(c, forward, till, cnt, extend);
                }
            }
            Await::Replace => {
                if let Some(c) = ch {
                    let s = if c == '\n' { self.line_ending.to_string() } else { c.to_string() };
                    self.replace_graphemes(&s);
                    self.exit_select();
                }
            }
            Await::TextObject { inner } => {
                if let Some(c) = ch {
                    self.select_object(c, inner, cnt);
                }
            }
            Await::SurroundAdd => {
                if let Some(c) = ch {
                    self.surround_add(c);
                    self.exit_select();
                } else if k.key == Key::Enter {
                    // handled via '\n' above
                }
            }
            Await::SurroundDelete => {
                if let Some(c) = ch {
                    self.surround_delete(if c == 'm' { None } else { Some(c) }, cnt);
                    self.exit_select();
                }
            }
            Await::SurroundReplaceFrom => {
                if let Some(c) = ch {
                    self.hx.count = Some(cnt);
                    self.hx.await_ = Some(Await::SurroundReplaceTo(if c == 'm' { None } else { Some(c) }));
                }
            }
            Await::SurroundReplaceTo(from) => {
                if let Some(c) = ch {
                    self.surround_replace(from, c, cnt);
                    self.exit_select();
                }
            }
        }
        true
    }

    pub(crate) fn exit_select(&mut self) {
        if self.mode == Mode::Select {
            self.mode = Mode::Normal;
        }
    }

    // ---- helpers -------------------------------------------------------------------------------

    /// Replace every selection by `f(text, selection)`; primary follows its selection.
    pub(crate) fn transform_sels(&mut self, mut f: impl FnMut(Text, &Selection) -> Selection) {
        let t = self.rope.slice(..);
        let v: Vec<Selection> = self.sels.v.iter().map(|s| f(t, s)).collect();
        let primary = self.sels.primary;
        self.set_sels(Selections::new(v, primary));
    }

    pub(crate) fn push_jump(&mut self) {
        let cur = self.sels.clone();
        let idx = self.hx.jump_idx.min(self.hx.jumps.len());
        self.hx.jumps.truncate(idx);
        if self.hx.jumps.last() != Some(&cur) {
            self.hx.jumps.push(cur);
        }
        if self.hx.jumps.len() > 100 {
            self.hx.jumps.remove(0);
        }
        self.hx.jump_idx = self.hx.jumps.len();
    }

    fn jump(&mut self, back: bool, count: usize) {
        for _ in 0..count {
            if back {
                if self.hx.jump_idx == 0 {
                    return;
                }
                if self.hx.jump_idx == self.hx.jumps.len() {
                    // remember where we were so ctrl-i can return
                    let cur = self.sels.clone();
                    if self.hx.jumps.last() != Some(&cur) {
                        self.hx.jumps.push(cur);
                    } else {
                        self.hx.jump_idx -= 1;
                    }
                }
                self.hx.jump_idx = self.hx.jump_idx.saturating_sub(1);
                let s = self.hx.jumps[self.hx.jump_idx].clone();
                self.set_sels(s);
            } else {
                if self.hx.jump_idx + 1 >= self.hx.jumps.len() {
                    return;
                }
                self.hx.jump_idx += 1;
                let s = self.hx.jumps[self.hx.jump_idx].clone();
                self.set_sels(s);
            }
        }
    }

    pub(crate) fn lines_viewport(&self) -> usize {
        self.viewport.1.max(1)
    }

    // ---- registers --------------------------------------------------------------------------------

    pub(crate) fn reg_write(&mut self, name: Option<char>, values: Vec<String>, mirror: bool) {
        match name {
            Some('_') => {}
            Some(c @ ('+' | '*')) => {
                let joined = values.join(self.line_ending);
                self.registers.insert(c, values);
                self.effects.push(Effect::SetClipboard(joined.clone()));
                self.last_mirror = Some(joined.clone());
                self.clipboard = Some(joined);
            }
            None | Some('"') => {
                let joined = values.join(self.line_ending);
                self.registers.insert('"', values);
                if mirror {
                    self.effects.push(Effect::SetClipboard(joined.clone()));
                    self.last_mirror = Some(joined.clone());
                    self.clipboard = Some(joined);
                } else {
                    // Mark the current clipboard as seen so `p` keeps preferring this register.
                    self.last_mirror = self.clipboard.clone();
                }
            }
            Some(c) => {
                self.registers.insert(c, values);
            }
        }
    }

    pub(crate) fn reg_read(&self, name: Option<char>) -> Vec<String> {
        match name {
            Some('_') => Vec::new(),
            Some('+' | '*') => self.clipboard.clone().into_iter().collect(),
            Some('/') => self.search.pattern.clone().into_iter().collect(),
            None | Some('"') => match &self.clipboard {
                Some(c) if self.last_mirror.as_ref() != Some(c) => vec![c.clone()],
                _ => self.registers.get(&'"').cloned().unwrap_or_default(),
            },
            Some(c) => self.registers.get(&c).cloned().unwrap_or_default(),
        }
    }

    fn fragments(&self) -> Vec<String> {
        self.sels.v.iter().map(|s| self.rope.slice(s.from()..s.to()).to_string()).collect()
    }

    // ---- command dispatch -----------------------------------------------------------------------------

    pub(crate) fn run(&mut self, cmd: Cmd, n: Option<usize>) {
        use Cmd::*;
        let cnt = n.unwrap_or(1);
        let extend = self.mode == Mode::Select;
        match cmd {
            CharLeft => self.move_horizontal(false, cnt, extend),
            CharRight => self.move_horizontal(true, cnt, extend),
            LineDown => self.move_vertical(true, cnt, extend),
            LineUp => self.move_vertical(false, cnt, extend),
            NextWordStart => self.word_cmd(WordKind::NextStart, false, cnt, extend),
            PrevWordStart => self.word_cmd(WordKind::PrevStart, false, cnt, extend),
            NextWordEnd => self.word_cmd(WordKind::NextEnd, false, cnt, extend),
            PrevWordEnd => self.word_cmd(WordKind::PrevEnd, false, cnt, extend),
            NextLongWordStart => self.word_cmd(WordKind::NextStart, true, cnt, extend),
            PrevLongWordStart => self.word_cmd(WordKind::PrevStart, true, cnt, extend),
            NextLongWordEnd => self.word_cmd(WordKind::NextEnd, true, cnt, extend),
            FindNext | FindTill | FindPrev | TillPrev => {
                self.hx.count = n;
                let (forward, till) = match cmd {
                    FindNext => (true, false),
                    FindTill => (true, true),
                    FindPrev => (false, false),
                    _ => (false, true),
                };
                self.hx.await_ = Some(Await::Find { forward, till });
            }
            RepeatFind => {
                if let Some((c, fwd, till)) = self.hx.last_find {
                    self.find_char(c, fwd, till, cnt, extend);
                }
            }
            LineStart => self.transform_sels(|t, s| {
                let line = t.char_to_line(s.block_cursor(t));
                s.put_cursor(t, t.line_to_char(line), extend)
            }),
            LineEnd => self.transform_sels(|t, s| {
                let line = t.char_to_line(s.block_cursor(t));
                let ls = t.line_to_char(line);
                let pos = text::prev_grapheme(t, text::line_end(t, line)).max(ls);
                s.put_cursor(t, pos, extend)
            }),
            FirstNonBlank => self.transform_sels(|t, s| {
                let line = t.char_to_line(s.block_cursor(t));
                match text::first_non_blank(t, line) {
                    Some(p) => s.put_cursor(t, p, extend),
                    None => *s,
                }
            }),
            FileStart => {
                if n.is_some() {
                    self.goto_line_number(cnt);
                } else {
                    self.push_jump();
                    self.transform_sels(|t, s| s.put_cursor(t, 0, extend));
                }
            }
            FileEnd => {
                self.push_jump();
                self.transform_sels(|t, s| {
                    let l = text::last_content_line(t);
                    s.put_cursor(t, t.line_to_char(l), extend)
                });
            }
            GotoLine => {
                if n.is_some() {
                    self.goto_line_number(cnt);
                }
            }
            WindowTop | WindowCenter | WindowBottom => {
                let (first, vis) = self.viewport;
                let last_line = self.rope.len_lines().saturating_sub(1);
                let line = match cmd {
                    WindowTop => first,
                    WindowCenter => first + vis / 2,
                    _ => first + vis.saturating_sub(1),
                }
                .min(last_line);
                self.push_jump();
                self.transform_sels(|t, s| s.put_cursor(t, t.line_to_char(line), extend));
            }
            GotoColumn => self.transform_sels(|t, s| {
                let line = t.char_to_line(s.block_cursor(t));
                let pos = (t.line_to_char(line) + cnt.saturating_sub(1)).min(text::line_end(t, line));
                s.put_cursor(t, pos, extend)
            }),
            NextParagraph => self.transform_sels(|t, s| {
                let line = t.char_to_line(s.block_cursor(t));
                let l = motion::next_paragraph_line(t, line, cnt);
                s.put_cursor(t, t.line_to_char(l), extend)
            }),
            PrevParagraph => self.transform_sels(|t, s| {
                let line = t.char_to_line(s.block_cursor(t));
                let l = motion::prev_paragraph_line(t, line, cnt);
                s.put_cursor(t, t.line_to_char(l), extend)
            }),
            AddNewlineBelow | AddNewlineAbove => {
                let below = cmd == AddNewlineBelow;
                let t = self.rope.slice(..);
                let le = self.line_ending;
                let mut changes = Vec::new();
                for s in &self.sels.v {
                    let line = t.char_to_line(s.block_cursor(t));
                    let pos =
                        if below {
                            t.line_to_char((line + 1).min(t.len_lines() - 1))
                                .max(if line + 1 < t.len_lines() { 0 } else { t.len_chars() })
                        } else {
                            t.line_to_char(line)
                        };
                    changes.push(Change { from: pos, to: pos, text: le.repeat(cnt) });
                }
                self.apply(Transaction::new(changes));
            }
            PageUp => self.page(false, self.lines_viewport() * cnt, extend),
            PageDown => self.page(true, self.lines_viewport() * cnt, extend),
            HalfUp => self.page(false, (self.lines_viewport() / 2).max(1) * cnt, extend),
            HalfDown => self.page(true, (self.lines_viewport() / 2).max(1) * cnt, extend),
            SelectAll => {
                self.push_jump();
                let len = self.rope.len_chars();
                self.set_primary_only(Selection::new(0, len));
            }
            ExtendLine => self.extend_line(cnt),
            ExtendToLineBounds => self.transform_sels(|t, s| {
                let (l0, l1) = s.line_range(t);
                let a = t.line_to_char(l0);
                let b = t.line_to_char((l1 + 1).min(t.len_lines()));
                Selection::new(a, b).with_direction(s.is_forward())
            }),
            ShrinkToLineBounds => self.transform_sels(|t, s| {
                let (l0, l1) = s.line_range(t);
                if l0 == l1 {
                    return *s;
                }
                let mut a = s.from();
                let mut b = s.to();
                let ls0 = t.line_to_char(l0);
                if a != ls0 {
                    a = t.line_to_char(l0 + 1);
                }
                let le1 = t.line_to_char((l1 + 1).min(t.len_lines()));
                if b != le1 {
                    b = t.line_to_char(l1);
                }
                if a >= b { *s } else { Selection::new(a, b).with_direction(s.is_forward()) }
            }),
            Collapse => self.transform_sels(|t, s| Selection::point(s.block_cursor(t))),
            Flip => self.transform_sels(|_, s| s.flip()),
            EnsureForward => self.transform_sels(|_, s| s.with_direction(true)),
            KeepPrimary => {
                let p = self.sels.primary();
                self.set_primary_only(p);
                self.status("1 selection");
            }
            RemovePrimary => {
                if self.sels.len() > 1 {
                    let mut v = self.sels.v.clone();
                    v.remove(self.sels.primary);
                    let p = self.sels.primary.min(v.len() - 1);
                    self.set_sels(Selections::new(v, p));
                } else {
                    self.status("can't remove the only selection");
                }
            }
            RotateBack | RotateFwd => {
                let len = self.sels.len();
                let p = if cmd == RotateFwd {
                    (self.sels.primary + cnt) % len
                } else {
                    (self.sels.primary + len - cnt % len) % len
                };
                self.sels.primary = p;
            }
            RotateContentsBack | RotateContentsFwd => self.rotate_contents(cmd == RotateContentsFwd, cnt),
            CopySelNext => self.copy_selection(true, cnt),
            CopySelPrev => self.copy_selection(false, cnt),
            SelectRegex => self.open_prompt(PromptKind::SelectRegex, "select:", "", cnt),
            SplitRegex => self.open_prompt(PromptKind::SplitRegex, "split:", "", cnt),
            Keep => self.open_prompt(PromptKind::Keep(true), "keep:", "", cnt),
            Remove => self.open_prompt(PromptKind::Keep(false), "remove:", "", cnt),
            SplitNewline => self.split_newline(),
            Trim => self.trim_selections(),
            Align => self.align_selections(),
            MergeSelections => {
                let (a, b) =
                    (self.sels.v.first().map_or(0, |s| s.from()), self.sels.v.last().map_or(0, |s| s.to()));
                self.set_primary_only(Selection::new(a, b));
            }
            MergeConsecutive => {
                let mut out: Vec<Selection> = Vec::new();
                for s in &self.sels.v {
                    match out.last_mut() {
                        Some(l) if l.to() == s.from() => *l = Selection::new(l.from(), s.to()),
                        _ => out.push(*s),
                    }
                }
                self.set_sels(Selections::new(out, 0));
            }
            SelectMode => {
                if self.mode == Mode::Select {
                    self.mode = Mode::Normal;
                } else {
                    // At the very end of the text a bare cursor has no char to be 1-wide on:
                    // step the anchor back so the selection still covers the last grapheme.
                    self.transform_sels(|t, s| {
                        if s.is_empty() && s.head == t.len_chars() && s.head > 0 {
                            Selection::new(text::prev_grapheme(t, s.head), s.head)
                        } else {
                            *s
                        }
                    });
                    self.mode = Mode::Select;
                }
            }
            Insert => self.enter_insert(),
            Append => self.enter_append(),
            InsertLineStart => {
                self.transform_sels(|t, s| {
                    let line = t.char_to_line(s.block_cursor(t));
                    Selection::point(text::first_non_blank(t, line).unwrap_or_else(|| t.line_to_char(line)))
                });
                self.start_insert();
            }
            InsertLineEnd => {
                self.transform_sels(|t, s| {
                    let line = t.char_to_line(s.block_cursor(t));
                    Selection::point(text::line_end(t, line))
                });
                self.start_insert();
            }
            OpenBelow => self.open_line(true, cnt),
            OpenAbove => self.open_line(false, cnt),
            CommandMode => self.open_prompt(PromptKind::Command, ":", "", cnt),
            Escape => {
                self.exit_select();
                self.effects.push(Effect::DismissCompletion);
            }
            Delete => self.delete_selections(true, false),
            DeleteNoYank => self.delete_selections(false, false),
            ChangeSel => self.delete_selections(true, true),
            ChangeSelNoYank => self.delete_selections(false, true),
            Yank => {
                self.yank();
                self.exit_select();
            }
            PasteAfter => {
                let reg = self.hx.reg;
                let vals = self.reg_read(reg);
                self.paste(&vals, true, cnt);
                self.exit_select();
            }
            PasteBefore => {
                let reg = self.hx.reg;
                let vals = self.reg_read(reg);
                self.paste(&vals, false, cnt);
                self.exit_select();
            }
            ReplaceWithYanked => {
                let reg = self.hx.reg;
                let vals = self.reg_read(reg);
                self.replace_with_values(&vals, cnt);
                self.exit_select();
            }
            ReplaceChar => self.hx.await_ = Some(Await::Replace),
            SwitchCase => self.map_case(|s| s.chars().flat_map(switch_char).collect()),
            Lowercase => self.map_case(|s| s.to_lowercase()),
            Uppercase => self.map_case(|s| s.to_uppercase()),
            Indent => {
                self.indent_lines(cnt, true);
                self.exit_select();
            }
            Unindent => {
                self.indent_lines(cnt, false);
                self.exit_select();
            }
            Format => self.effects.push(Effect::FormatSelection),
            Join => self.join_lines(false),
            JoinSpace => self.join_lines(true),
            Undo => {
                for _ in 0..cnt {
                    if !self.undo() {
                        self.status("already at oldest change");
                        break;
                    }
                }
                self.exit_select();
            }
            Redo => {
                for _ in 0..cnt {
                    if !self.redo() {
                        self.status("already at newest change");
                        break;
                    }
                }
                self.exit_select();
            }
            ToggleComments => self.toggle_line_comments(),
            Increment => self.increment(cnt as i64),
            Decrement => self.increment(-(cnt as i64)),
            RepeatLast => {
                let keys = self.hx.last_change.clone();
                if keys.is_empty() {
                    return;
                }
                self.hx.no_record = true;
                self.replay(&keys, 1);
            }
            RecordMacro => {
                if let Some((reg, mut keys)) = self.hx.macro_rec.take() {
                    keys.pop(); // the Q that stopped the recording
                    self.hx.macros.insert(reg, keys);
                    self.status(format!("recorded macro to register '{reg}'"));
                } else {
                    let reg = self.hx.reg.unwrap_or('@');
                    self.hx.macro_rec = Some((reg, Vec::new()));
                    self.status(format!("recording to register '{reg}'"));
                }
            }
            ReplayMacro => {
                let reg = self.hx.reg.unwrap_or('@');
                match self.hx.macros.get(&reg).cloned() {
                    Some(keys) => self.replay(&keys, cnt),
                    None => self.status(format!("register '{reg}' is empty")),
                }
            }
            MatchBracket => self.transform_sels(|t, s| {
                let pos = s.block_cursor(t);
                // Helix looks for the nearest bracket at or after the cursor on the line.
                let line_end = text::line_end(t, t.char_to_line(pos));
                let mut p = pos;
                while p < line_end && !matches!(t.char(p), '(' | ')' | '[' | ']' | '{' | '}') {
                    p += 1;
                }
                match motion::matching_bracket_at(t, p) {
                    Some(m) => s.put_cursor(t, m, extend),
                    None => *s,
                }
            }),
            SurroundAdd => self.hx.await_ = Some(Await::SurroundAdd),
            SurroundDelete => {
                self.hx.count = n;
                self.hx.await_ = Some(Await::SurroundDelete);
            }
            SurroundReplace => {
                self.hx.count = n;
                self.hx.await_ = Some(Await::SurroundReplaceFrom);
            }
            TextObjInner => {
                self.hx.count = n;
                self.hx.await_ = Some(Await::TextObject { inner: true });
            }
            TextObjAround => {
                self.hx.count = n;
                self.hx.await_ = Some(Await::TextObject { inner: false });
            }
            Search => self.open_prompt(PromptKind::Search { backwards: false }, "/", "", cnt),
            RSearch => self.open_prompt(PromptKind::Search { backwards: true }, "?", "", cnt),
            SearchNext => self.search_repeat(false, cnt, extend),
            SearchPrev => self.search_repeat(true, cnt, extend),
            SearchSelection => self.search_from_selection(true),
            SearchSelectionRaw => self.search_from_selection(false),
            JumpBack => self.jump(true, cnt),
            JumpForward => self.jump(false, cnt),
            SaveSelection => {
                self.push_jump();
                self.status("selection saved to jumplist");
            }
            SelectRegister => self.hx.await_ = Some(Await::Register),
            Scroll(req) => {
                let req = match req {
                    ScrollRequest::Lines(l) => ScrollRequest::Lines(l * cnt as i32),
                    r => r,
                };
                self.effects.push(Effect::Scroll(req));
            }
            Fx(kind) => self.run_fx(kind, cnt),
        }
    }

    pub(crate) fn run_fx(&mut self, kind: FxKind, cnt: usize) {
        match kind {
            FxKind::TablePicker => self.effects.push(Effect::OpenTablePicker),
            FxKind::ConnectionPicker => self.effects.push(Effect::OpenConnectionPicker),
            FxKind::Palette => self.effects.push(Effect::OpenCommandPalette),
            FxKind::YankClipboard => {
                let vals = self.fragments();
                self.reg_write(Some('+'), vals, true);
                self.status("yanked to system clipboard");
                self.exit_select();
            }
            FxKind::YankPrimaryClipboard => {
                let p = self.sels.primary();
                let v = vec![self.rope.slice(p.from()..p.to()).to_string()];
                self.reg_write(Some('+'), v, true);
                self.status("yanked main selection to system clipboard");
                self.exit_select();
            }
            FxKind::PasteClipboardAfter => {
                let v = self.reg_read(Some('+'));
                self.paste(&v, true, cnt);
                self.exit_select();
            }
            FxKind::PasteClipboardBefore => {
                let v = self.reg_read(Some('+'));
                self.paste(&v, false, cnt);
                self.exit_select();
            }
            FxKind::Run => match self.selected_text() {
                Some(t) => self.effects.push(Effect::RunSelection(t)),
                None => self.effects.push(Effect::RunStatementAtCursor),
            },
            FxKind::RunAll => self.effects.push(Effect::RunAll),
            FxKind::Explain => self.effects.push(Effect::ExplainStatement { analyze: false }),
            FxKind::Analyze => self.effects.push(Effect::ExplainStatement { analyze: true }),
            FxKind::Complete => self.effects.push(Effect::RequestCompletion),
            FxKind::Theme => self.effects.push(Effect::OpenThemePicker),
            FxKind::CommentLines => self.toggle_line_comments(),
            FxKind::FocusExplorer => self.effects.push(Effect::FocusExplorer),
            FxKind::FocusResults => self.effects.push(Effect::FocusResults),
            FxKind::ToggleExplorer => self.effects.push(Effect::ToggleExplorer),
            FxKind::ToggleBottomPanel => self.effects.push(Effect::ToggleBottomPanel),
            FxKind::Cancel => self.effects.push(Effect::CancelQuery),
            FxKind::GoToDefinition => self.effects.push(Effect::GoToDefinition),
            FxKind::FindUsages => self.effects.push(Effect::FindUsages),
            FxKind::Rename => self.effects.push(Effect::RenameSymbol),
            FxKind::CodeActions => self.effects.push(Effect::CodeActions),
            FxKind::ShowProblems => self.effects.push(Effect::ShowProblems),
            FxKind::NextProblem => self.effects.push(Effect::NextProblem),
            FxKind::PrevProblem => self.effects.push(Effect::PreviousProblem),
        }
    }

    /// Feed recorded keys back through the normal key path.
    pub(crate) fn replay(&mut self, keys: &[KeyInput], times: usize) {
        if self.hx.replaying > 8 {
            return;
        }
        self.hx.replaying += 1;
        let saved_pending = std::mem::take(&mut self.hx.pending);
        for _ in 0..times {
            for k in keys {
                let k = k.clone();
                if self.prompt.is_some() {
                    self.prompt_key(&k);
                } else if !self.global_key(&k) {
                    self.helix_key(&k);
                }
                if self.mode != Mode::Insert {
                    self.commit_group();
                }
            }
        }
        self.hx.pending = saved_pending;
        self.hx.replaying -= 1;
    }

    // ---- motions ---------------------------------------------------------------------------------------

    pub(crate) fn move_horizontal(&mut self, right: bool, cnt: usize, extend: bool) {
        self.transform_sels(|t, s| {
            let pos = s.block_cursor(t);
            let new = if right {
                text::nth_next_grapheme(t, pos, cnt)
            } else {
                text::nth_prev_grapheme(t, pos, cnt)
            };
            s.put_cursor(t, new, extend)
        });
    }

    pub(crate) fn move_vertical(&mut self, down: bool, cnt: usize, extend: bool) {
        let t = self.rope.slice(..);
        let goals = self.vert.clone().filter(|g| g.len() == self.sels.len());
        let mut new_goals = Vec::with_capacity(self.sels.len());
        let nlines = t.len_lines();
        let mut v = Vec::with_capacity(self.sels.len());
        for (i, s) in self.sels.v.iter().enumerate() {
            let pos = s.block_cursor(t);
            let line = t.char_to_line(pos);
            let ls = t.line_to_char(line);
            let goal = goals.as_ref().map_or(pos - ls, |g| g[i]);
            new_goals.push(goal);
            let new_line = if down { (line + cnt).min(nlines - 1) } else { line.saturating_sub(cnt) };
            if new_line == line && !down || (new_line == line && down) {
                // nowhere to go
                v.push(*s);
                continue;
            }
            let nls = t.line_to_char(new_line);
            let content = text::line_end(t, new_line) - nls;
            if extend && new_line == nlines - 1 && t.line(new_line).len_chars() == 0 && nlines > 1 {
                v.push(*s);
                continue;
            }
            v.push(s.put_cursor(t, nls + goal.min(content), extend));
        }
        let primary = self.sels.primary;
        self.set_sels(Selections::new(v, primary));
        self.vert = Some(new_goals);
        self.vert_keep = true;
    }

    fn page(&mut self, down: bool, lines: usize, extend: bool) {
        self.move_vertical(down, lines, extend);
        let l = lines as i32;
        self.effects.push(Effect::Scroll(ScrollRequest::Lines(if down { l } else { -l })));
    }

    fn word_cmd(&mut self, kind: WordKind, long: bool, cnt: usize, extend: bool) {
        self.transform_sels(|t, s| {
            let w = motion::word_move(t, *s, cnt, kind, long);
            if extend {
                let pos = w.block_cursor(t);
                s.put_cursor(t, pos, true)
            } else {
                w
            }
        });
    }

    fn find_char(&mut self, ch: char, forward: bool, till: bool, cnt: usize, extend: bool) {
        self.transform_sels(|t, s| {
            let start = if s.anchor < s.head { s.head - 1 } else { s.head };
            let Some(mut pos) = motion::find_char(t, start, ch, cnt, forward) else { return *s };
            if till {
                pos = if forward { pos.saturating_sub(1) } else { pos + 1 };
            }
            if extend {
                s.put_cursor(t, pos, true)
            } else {
                Selection::point(s.block_cursor(t)).min_width_1(t).put_cursor(t, pos, true)
            }
        });
    }

    fn extend_line(&mut self, cnt: usize) {
        self.transform_sels(|t, s| {
            let (l0, l1) = s.line_range(t);
            let start = t.line_to_char(l0);
            let end = t.line_to_char((l1 + 1).min(t.len_lines()));
            let nl = t.len_lines();
            let head = if s.from() == start && s.to() == end {
                t.line_to_char((l1 + cnt + 1).min(nl))
            } else {
                t.line_to_char((l1 + cnt).min(nl))
            };
            Selection::new(start, head)
        });
    }

    fn copy_selection(&mut self, down: bool, cnt: usize) {
        let t = self.rope.slice(..);
        let mut out = self.sels.v.clone();
        let mut primary = self.sels.primary;
        let nlines = t.len_lines();
        let mut new_primary = None;
        for s in &self.sels.v {
            let (l0, l1) = s.line_range(t);
            let h = l1 - l0 + 1;
            let col_a = s.anchor - t.line_to_char(t.char_to_line(s.anchor));
            let col_h = s.head - t.line_to_char(t.char_to_line(s.head));
            let (la, lh) = (t.char_to_line(s.anchor), t.char_to_line(s.head));
            let mut shift = 0isize;
            let mut added = 0;
            loop {
                shift += if down { h as isize } else { -(h as isize) };
                let (na, nh) = (la as isize + shift, lh as isize + shift);
                if na < 0 || nh < 0 || na as usize >= nlines || nh as usize >= nlines {
                    break;
                }
                let (na, nh) = (na as usize, nh as usize);
                let ok_a = col_a <= text::line_end(t, na) - t.line_to_char(na);
                let ok_h = col_h <= text::line_end(t, nh) - t.line_to_char(nh);
                if ok_a && ok_h {
                    let ns = Selection::new(t.line_to_char(na) + col_a, t.line_to_char(nh) + col_h);
                    out.push(ns);
                    new_primary = Some(ns);
                    added += 1;
                    if added >= cnt {
                        break;
                    }
                }
            }
        }
        let prim = new_primary.unwrap_or(self.sels.primary());
        let _ = &mut primary;
        let mut sel = Selections::new(out, 0);
        if let Some(i) = sel.v.iter().position(|s| *s == prim) {
            sel.primary = i;
        }
        self.set_sels(sel);
        let n = self.sels.len();
        self.status(format!("{n} selections"));
    }

    fn rotate_contents(&mut self, fwd: bool, cnt: usize) {
        let n = self.sels.len();
        if n < 2 {
            return;
        }
        let mut frags = self.fragments();
        let k = cnt % n;
        if fwd {
            frags.rotate_right(k);
        } else {
            frags.rotate_left(k);
        }
        let changes: Vec<Change> = self
            .sels
            .v
            .iter()
            .zip(frags)
            .map(|(s, f)| Change { from: s.from(), to: s.to(), text: f })
            .collect();
        self.apply(Transaction::new(changes));
    }

    fn split_newline(&mut self) {
        let t = self.rope.slice(..);
        let mut out = Vec::new();
        let mut primary = 0;
        for (i, s) in self.sels.v.iter().enumerate() {
            if i == self.sels.primary {
                primary = out.len();
            }
            let (l0, l1) = s.line_range(t);
            for l in l0..=l1 {
                let a = t.line_to_char(l).max(s.from());
                let b = text::line_end(t, l).min(s.to());
                if a < b {
                    out.push(Selection::new(a, b));
                }
            }
        }
        if out.is_empty() {
            return;
        }
        let n = out.len();
        self.set_sels(Selections::new(out, primary));
        if n > 1 {
            self.status(format!("{n} selections"));
        }
    }

    fn trim_selections(&mut self) {
        let t = self.rope.slice(..);
        let mut out = Vec::new();
        let mut primary = 0;
        for (i, s) in self.sels.v.iter().enumerate() {
            let (mut a, mut b) = (s.from(), s.to());
            while a < b && t.char(a).is_whitespace() {
                a += 1;
            }
            while b > a && t.char(b - 1).is_whitespace() {
                b -= 1;
            }
            if a < b {
                if i == self.sels.primary {
                    primary = out.len();
                }
                out.push(Selection::new(a, b).with_direction(s.is_forward()));
            }
        }
        if out.is_empty() {
            self.status("no selections remaining");
            return;
        }
        self.set_sels(Selections::new(out, primary));
    }

    fn align_selections(&mut self) {
        let t = self.rope.slice(..);
        // group selections by line (in order), then pad so the k-th selection of every line starts at one column
        let mut rows: Vec<(usize, Vec<usize>)> = Vec::new(); // (line, selection indexes)
        for (i, s) in self.sels.v.iter().enumerate() {
            let line = t.char_to_line(s.from());
            match rows.last_mut() {
                Some((l, v)) if *l == line => v.push(i),
                _ => rows.push((line, vec![i])),
            }
        }
        let max_cols = rows.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
        let mut pad = vec![0usize; self.sels.len()];
        for k in 0..max_cols {
            let mut target = 0;
            let mut cols = Vec::new();
            for (line, v) in &rows {
                if let Some(&i) = v.get(k) {
                    let col = self.sels.v[i].head - t.line_to_char(*line)
                        + v[..k].iter().map(|&j| pad[j]).sum::<usize>();
                    target = target.max(col);
                    cols.push((i, col));
                }
            }
            for (i, col) in cols {
                pad[i] = target - col;
            }
        }
        let changes: Vec<Change> = pad
            .iter()
            .enumerate()
            .filter(|(_, p)| **p > 0)
            .map(|(i, p)| {
                let at = self.sels.v[i].from();
                Change { from: at, to: at, text: " ".repeat(*p) }
            })
            .collect();
        self.apply(Transaction::new(changes));
    }

    // ---- insert-entering commands ------------------------------------------------------------------------------

    fn start_insert(&mut self) {
        self.mode = Mode::Insert;
    }

    fn enter_insert(&mut self) {
        self.transform_sels(|_, s| Selection::new(s.to(), s.from()));
        self.start_insert();
    }

    fn enter_append(&mut self) {
        let t = self.rope.slice(..);
        let len = t.len_chars();
        let at_eof = self.sels.v.iter().any(|s| !s.is_empty() && s.to() == len)
            || self.sels.v.iter().any(|s| s.to() >= len);
        self.hx.restore_cursor = true;
        self.transform_sels(|t, s| {
            if s.to() >= t.len_chars() {
                // Nothing after the last char: sit at the very end (Helix would append a newline).
                Selection::point(t.len_chars())
            } else {
                Selection::new(s.from(), text::next_grapheme(t, s.to()))
            }
        });
        let _ = at_eof;
        self.start_insert();
    }

    pub(crate) fn leave_insert(&mut self) {
        self.mode = Mode::Normal;
        self.effects.push(Effect::DismissCompletion);
        if self.hx.restore_cursor {
            self.hx.restore_cursor = false;
            self.transform_sels(|t, s| {
                let head = if s.head > s.anchor || (s.is_empty() && s.head == t.len_chars() && s.head > 0) {
                    text::prev_grapheme(t, s.to())
                } else {
                    s.to()
                };
                Selection::new(s.from(), head)
            });
        }
    }

    pub(crate) fn indent_for_new_line(&self, line: usize) -> String {
        let t = self.rope.slice(..);
        let mut ind = text::leading_whitespace(t, line);
        let content = text::line_content(t, line);
        if content.trim_end().ends_with(['(', '[', '{']) {
            ind.push_str("    ");
        }
        ind
    }

    fn open_line(&mut self, below: bool, cnt: usize) {
        let t = self.rope.slice(..);
        let le = self.line_ending;
        let mut changes = Vec::new();
        let mut points = Vec::new();
        let mut offs = 0usize;
        for s in &self.sels.v {
            let cur_line = if below {
                t.char_to_line(text::prev_grapheme(t, s.to()).max(s.from()))
            } else {
                t.char_to_line(s.from())
            };
            let indent = self.indent_for_new_line(cur_line);
            let unit = format!("{le}{indent}");
            let body = unit.repeat(cnt);
            let (pos, text_, first_pos) = if below {
                let p = text::line_end(t, cur_line);
                (p, body.clone(), p + le.len() + indent.chars().count())
            } else if cur_line == 0 {
                let b = format!("{indent}{le}").repeat(cnt);
                (0, b, indent.chars().count())
            } else {
                let p = text::line_end(t, cur_line - 1);
                (p, body.clone(), p + le.len() + indent.chars().count())
            };
            let unit_len = le.chars().count() + indent.chars().count();
            for i in 0..cnt {
                let at = if below || cur_line > 0 {
                    first_pos + i * unit_len
                } else {
                    i * unit_len + indent.chars().count()
                };
                points.push(offs + at);
            }
            offs += text_.chars().count();
            changes.push(Change { from: pos, to: pos, text: text_ });
        }
        let n = self.sels.len();
        let primary_idx = self.sels.primary * cnt + cnt - 1;
        let _ = n;
        let new = Selections::new(points.into_iter().map(Selection::point).collect(), primary_idx);
        self.apply_with(Transaction::new(changes), Some(new));
        self.start_insert();
    }

    // ---- editing commands ------------------------------------------------------------------------------------------

    fn yank(&mut self) {
        let vals = self.fragments();
        let reg = self.hx.reg;
        let lines: usize = vals.iter().map(|v| v.matches('\n').count()).sum();
        let msg = if vals.iter().all(|v| v.ends_with('\n')) && !vals.is_empty() {
            format!("yanked {lines} line{}", if lines == 1 { "" } else { "s" })
        } else {
            format!("yanked {} selection{}", vals.len(), if vals.len() == 1 { "" } else { "s" })
        };
        self.reg_write(reg, vals, true);
        self.status(msg);
    }

    fn selections_linewise(&self) -> bool {
        let t = self.rope.slice(..);
        self.sels.v.iter().all(|s| {
            if t.slice(s.from()..s.to()).len_lines() < 2 {
                return false;
            }
            let (l0, l1) = s.line_range(t);
            t.line_to_char(l0) == s.from() && t.line_to_char((l1 + 1).min(t.len_lines())) == s.to()
        })
    }

    fn delete_selections(&mut self, yank: bool, change: bool) {
        let linewise = self.selections_linewise();
        if yank {
            let vals = self.fragments();
            let reg = self.hx.reg;
            self.reg_write(reg, vals, false);
        }
        let changes: Vec<Change> =
            self.sels.v.iter().map(|s| Change { from: s.from(), to: s.to(), text: String::new() }).collect();
        self.apply(Transaction::new(changes));
        if change {
            if linewise {
                self.open_line(false, 1);
            } else {
                self.start_insert();
            }
        } else {
            self.exit_select();
        }
    }

    pub(crate) fn paste(&mut self, values: &[String], after: bool, cnt: usize) {
        if values.is_empty() {
            self.status("nothing to paste");
            return;
        }
        let t = self.rope.slice(..);
        let le = self.line_ending;
        let linewise = values.iter().any(|v| v.ends_with('\n'));
        let norm = |v: &str| -> String {
            let v = v.replace("\r\n", "\n");
            let v = if le == "\r\n" { v.replace('\n', "\r\n") } else { v };
            v.repeat(cnt)
        };
        let mut changes = Vec::new();
        let mut ranges = Vec::new();
        let mut offset = 0usize;
        let last = norm(values.last().unwrap());
        for (i, s) in self.sels.v.iter().enumerate() {
            let pos = match (after, linewise) {
                (false, true) => t.line_to_char(t.char_to_line(s.from())),
                (true, true) => {
                    let (_, l1) = s.line_range(t);
                    t.line_to_char((l1 + 1).min(t.len_lines()))
                }
                (false, false) => s.from(),
                (true, false) => s.to(),
            };
            let value = values.get(i).map(|v| norm(v)).unwrap_or_else(|| last.clone());
            let n = value.chars().count();
            // pasting linewise after the last line without a final newline needs one first
            let mut ins = value;
            let mut extra = 0;
            if linewise
                && after
                && pos == t.len_chars()
                && t.len_chars() > 0
                && !matches!(t.char(t.len_chars() - 1), '\n')
            {
                ins = format!("{le}{ins}");
                extra = le.chars().count();
            }
            let anchor = offset + pos + extra;
            ranges.push(Selection::new(anchor, anchor + n).with_direction(s.is_forward()));
            offset += n + extra;
            changes.push(Change { from: pos, to: pos, text: ins });
        }
        let sels = Selections::new(ranges, self.sels.primary);
        self.apply_with(Transaction::new(changes), Some(sels));
    }

    fn replace_with_values(&mut self, values: &[String], cnt: usize) {
        if values.is_empty() {
            self.status("nothing to paste");
            return;
        }
        let last = values.last().unwrap().repeat(cnt);
        let changes: Vec<Change> = self
            .sels
            .v
            .iter()
            .enumerate()
            .map(|(i, s)| Change {
                from: s.from(),
                to: s.to(),
                text: values.get(i).map(|v| v.repeat(cnt)).unwrap_or_else(|| last.clone()),
            })
            .collect();
        self.apply(Transaction::new(changes));
    }

    fn replace_graphemes(&mut self, with: &str) {
        let t = self.rope.slice(..);
        let mut changes = Vec::new();
        for s in &self.sels.v {
            if s.is_empty() {
                continue;
            }
            let mut out = String::new();
            let mut p = s.from();
            while p < s.to() {
                out.push_str(with);
                p = text::next_grapheme(t, p);
            }
            changes.push(Change { from: s.from(), to: s.to(), text: out });
        }
        self.apply(Transaction::new(changes));
    }

    fn map_case(&mut self, f: impl Fn(&str) -> String) {
        let changes: Vec<Change> = self
            .sels
            .v
            .iter()
            .map(|s| {
                let old = self.rope.slice(s.from()..s.to()).to_string();
                Change { from: s.from(), to: s.to(), text: f(&old) }
            })
            .filter(|c| c.from < c.to)
            .collect();
        self.apply(Transaction::new(changes));
        self.exit_select();
    }

    pub(crate) fn indent_lines(&mut self, cnt: usize, indent: bool) {
        let t = self.rope.slice(..);
        let mut lines: Vec<usize> = Vec::new();
        for s in &self.sels.v {
            let (a, b) = s.line_range(t);
            for l in a..=b {
                if lines.last().is_none_or(|&x| l > x) {
                    lines.push(l);
                }
            }
        }
        let unit = "    ";
        let mut changes = Vec::new();
        for l in lines {
            let ls = t.line_to_char(l);
            if indent {
                if text::line_end(t, l) == ls {
                    continue; // never indent empty lines
                }
                changes.push(Change { from: ls, to: ls, text: unit.repeat(cnt) });
            } else {
                let mut remove = 0;
                let mut budget = cnt * 4;
                let end = text::line_end(t, l);
                while ls + remove < end && budget > 0 {
                    match t.char(ls + remove) {
                        ' ' => budget -= 1,
                        '\t' => budget = budget.saturating_sub(4),
                        _ => break,
                    }
                    remove += 1;
                }
                if remove > 0 {
                    changes.push(Change { from: ls, to: ls + remove, text: String::new() });
                }
            }
        }
        self.apply(Transaction::new(changes));
    }

    fn join_lines(&mut self, select_space: bool) {
        let t = self.rope.slice(..);
        let mut changes: Vec<Change> = Vec::new();
        for s in &self.sels.v {
            let (a, mut b) = s.line_range(t);
            if a == b {
                b = (b + 1).min(t.len_lines() - 1);
            }
            for line in a..b {
                let start = text::line_end(t, line);
                let mut end = t.line_to_char(line + 1);
                while end < t.len_chars() && matches!(t.char(end), ' ' | '\t') {
                    end += 1;
                }
                let sep = if end == text::line_end(t, line + 1) { "" } else { " " };
                changes.push(Change { from: start, to: end, text: sep.to_string() });
            }
        }
        changes.sort_by_key(|c| c.from);
        changes.dedup_by(|a, b| a.from == b.from);
        if changes.is_empty() {
            return;
        }
        let spaces: Vec<usize> = {
            let mut shift = 0usize;
            let mut out = Vec::new();
            for c in &changes {
                if !c.text.is_empty() {
                    out.push(c.from - shift);
                }
                shift += (c.to - c.from) - c.text.chars().count();
            }
            out
        };
        let tx = Transaction::new(changes);
        if select_space && !spaces.is_empty() {
            let sel = Selections::new(spaces.into_iter().map(|p| Selection::new(p, p + 1)).collect(), 0);
            self.apply_with(tx, Some(sel));
        } else {
            self.apply(tx);
        }
    }

    pub(crate) fn toggle_line_comments(&mut self) {
        let t = self.rope.slice(..);
        let mut lines: Vec<usize> = Vec::new();
        for s in &self.sels.v {
            let (a, b) = s.line_range(t);
            for l in a..=b {
                if lines.last().is_none_or(|&x| l > x) {
                    lines.push(l);
                }
            }
        }
        self.toggle_comment_lines(&lines);
    }

    /// Toggle `--` comments on the given (sorted) lines: comment them all unless all are commented.
    pub(crate) fn toggle_comment_lines(&mut self, lines: &[usize]) {
        let t = self.rope.slice(..);
        let lines = lines.to_vec();
        let non_blank: Vec<usize> = lines.iter().copied().filter(|&l| !text::is_blank_line(t, l)).collect();
        if non_blank.is_empty() {
            return;
        }
        let all_commented = non_blank.iter().all(|&l| {
            let p = text::first_non_blank(t, l).unwrap();
            t.slice(p..text::line_end(t, l)).to_string().starts_with("--")
        });
        let mut changes = Vec::new();
        if all_commented {
            for &l in &non_blank {
                let p = text::first_non_blank(t, l).unwrap();
                let mut e = p + 2;
                if e < text::line_end(t, l) && t.char(e) == ' ' {
                    e += 1;
                }
                changes.push(Change { from: p, to: e, text: String::new() });
            }
        } else {
            let col = non_blank
                .iter()
                .map(|&l| text::first_non_blank(t, l).unwrap() - t.line_to_char(l))
                .min()
                .unwrap_or(0);
            for &l in &non_blank {
                let p = t.line_to_char(l) + col;
                changes.push(Change { from: p, to: p, text: "-- ".to_string() });
            }
        }
        self.apply(Transaction::new(changes));
    }

    fn increment(&mut self, delta: i64) {
        let t = self.rope.slice(..);
        let mut changes = Vec::new();
        let mut sels = Vec::new();
        let mut offs: isize = 0;
        for s in &self.sels.v {
            let (lo, hi) = if s.len() <= 1 || text::next_grapheme(t, s.from()) >= s.to() {
                let c = s.block_cursor(t);
                let l = t.char_to_line(c.min(t.len_chars()));
                (c, text::line_end(t, l))
            } else {
                (s.from(), s.to())
            };
            let found = find_number(t, lo, hi, s.len() <= 1 || text::next_grapheme(t, s.from()) >= s.to());
            match found {
                Some((a, b, val, radix, width_neg)) => {
                    let padded = radix == 10
                        && b - a - usize::from(width_neg) > 1
                        && t.char(a + usize::from(width_neg)) == '0';
                    let new = format_number(val + delta as i128, radix, b - a, width_neg, padded);
                    let n = new.chars().count();
                    changes.push(Change { from: a, to: b, text: new });
                    sels.push(Selection::new((a as isize + offs) as usize, (a as isize + offs) as usize + n));
                    offs += n as isize - (b - a) as isize;
                }
                None => sels.push(Selection::new(
                    (s.anchor as isize + offs) as usize,
                    (s.head as isize + offs) as usize,
                )),
            }
        }
        if changes.is_empty() {
            return;
        }
        let p = self.sels.primary;
        self.apply_with(Transaction::new(changes), Some(Selections::new(sels, p)));
    }

    // ---- search from selection ------------------------------------------------------------------------------------------

    fn search_from_selection(&mut self, word_boundaries: bool) {
        let t = self.rope.slice(..);
        let mut parts = Vec::new();
        for s in &self.sels.v {
            let mut frag = self.rope.slice(s.from()..s.to()).to_string();
            let mut is_word = word_boundaries;
            if text::next_grapheme(t, s.from()) >= s.to() && word_boundaries {
                // a bare cursor: use the word under it
                let c = s.block_cursor(t);
                if c < t.len_chars() && text::is_word_char(t.char(c)) {
                    let (mut a, mut b) = (c, c + 1);
                    while a > 0 && text::is_word_char(t.char(a - 1)) {
                        a -= 1;
                    }
                    while b < t.len_chars() && text::is_word_char(t.char(b)) {
                        b += 1;
                    }
                    frag = t.slice(a..b).to_string();
                } else {
                    is_word = false;
                }
            }
            if frag.is_empty() {
                continue;
            }
            let esc = regex::escape(&frag);
            let all_word = frag.chars().all(text::is_word_char);
            parts.push(if is_word && all_word { format!("\\b{esc}\\b") } else { esc });
        }
        if parts.is_empty() {
            self.status("nothing to search for");
            return;
        }
        parts.dedup();
        let pat = parts.join("|");
        self.search.pattern = Some(pat.clone());
        if self.search.history.last() != Some(&pat) {
            self.search.history.push(pat.clone());
        }
        self.status(format!("search pattern: {pat}"));
        self.exit_select();
    }

    // ---- text objects & surround --------------------------------------------------------------------------------------------

    fn select_object(&mut self, kind: char, inner: bool, cnt: usize) {
        let extend = self.mode == Mode::Select;
        let t = self.rope.slice(..);
        let mut any = false;
        let v: Vec<Selection> = self
            .sels
            .v
            .iter()
            .map(|s| match objects::text_object(t, *s, kind, inner, cnt) {
                Some(o) => {
                    any = true;
                    if extend { Selection::new(s.anchor.min(o.from()), s.head.max(o.to())) } else { o }
                }
                None => *s,
            })
            .collect();
        if !any {
            self.status("no such text object here");
            return;
        }
        let p = self.sels.primary;
        self.set_sels(Selections::new(v, p));
    }

    fn surround_add(&mut self, c: char) {
        let (o, cl) = if c == '\n' { ('\n', '\n') } else { objects::surround_pair(c) };
        let le = self.line_ending;
        let (os, cs) =
            if c == '\n' { (le.to_string(), le.to_string()) } else { (o.to_string(), cl.to_string()) };
        let (ol, cl_len) = (os.chars().count(), cs.chars().count());
        let mut changes = Vec::new();
        let mut ranges = Vec::new();
        let mut offs = 0;
        for s in &self.sels.v {
            changes.push(Change { from: s.from(), to: s.from(), text: os.clone() });
            changes.push(Change { from: s.to(), to: s.to(), text: cs.clone() });
            ranges.push(
                Selection::new(offs + s.from(), offs + s.to() + ol + cl_len).with_direction(s.is_forward()),
            );
            offs += ol + cl_len;
        }
        let sels = Selections::new(ranges, self.sels.primary);
        self.apply_with(Transaction::new(changes), Some(sels));
    }

    fn surround_positions(&self, ch: Option<char>, cnt: usize) -> Option<Vec<(usize, usize)>> {
        let t = self.rope.slice(..);
        self.sels.v.iter().map(|s| objects::find_surround(t, *s, ch, cnt)).collect()
    }

    fn surround_delete(&mut self, ch: Option<char>, cnt: usize) {
        let Some(pairs) = self.surround_positions(ch, cnt) else {
            self.status("surround not found");
            return;
        };
        let mut changes = Vec::new();
        for (o, c) in pairs {
            changes.push(Change { from: o, to: o + 1, text: String::new() });
            changes.push(Change { from: c, to: c + 1, text: String::new() });
        }
        self.apply(Transaction::new(changes));
    }

    fn surround_replace(&mut self, from: Option<char>, to: char, cnt: usize) {
        let Some(pairs) = self.surround_positions(from, cnt) else {
            self.status("surround not found");
            return;
        };
        let (o, c) = objects::surround_pair(to);
        let mut changes = Vec::new();
        for (op, cl) in pairs {
            changes.push(Change { from: op, to: op + 1, text: o.to_string() });
            changes.push(Change { from: cl, to: cl + 1, text: c.to_string() });
        }
        self.apply(Transaction::new(changes));
    }
}

fn switch_char(c: char) -> Vec<char> {
    if c.is_lowercase() {
        c.to_uppercase().collect()
    } else if c.is_uppercase() {
        c.to_lowercase().collect()
    } else {
        vec![c]
    }
}

/// Find a number in `lo..hi`. With `at_cursor` the number containing `lo` wins, else the first
/// number at or after it. Returns (start, end, value, radix, had_minus_included).
fn find_number(t: Text, lo: usize, hi: usize, at_cursor: bool) -> Option<(usize, usize, i128, u32, bool)> {
    let len = t.len_chars();
    let hi = hi.min(len);
    if lo >= hi.max(lo + 1).min(len + 1) && lo >= len {
        return None;
    }
    let is_digit = |i: usize| i < len && t.char(i).is_ascii_digit();
    let is_hex = |i: usize| i < len && t.char(i).is_ascii_hexdigit();
    // walk back if the cursor is inside a number
    let mut i = lo;
    if at_cursor && (is_digit(i) || (is_hex(i) && hex_prefix_before(t, i))) {
        while i > 0
            && (is_hex(i - 1) || (i >= 2 && matches!(t.char(i - 1), 'x' | 'X') && t.char(i - 2) == '0'))
        {
            if !is_digit(i - 1) && !hex_prefix_before(t, i - 1) && !(matches!(t.char(i - 1), 'x' | 'X')) {
                break;
            }
            i -= 1;
        }
    } else {
        while i < hi && !is_digit(i) {
            i += 1;
        }
        if i >= hi {
            return None;
        }
    }
    let start = i;
    if start + 1 < len && t.char(start) == '0' && matches!(t.char(start + 1), 'x' | 'X') && is_hex(start + 2)
    {
        let mut e = start + 2;
        while is_hex(e) {
            e += 1;
        }
        let s: String = t.slice(start + 2..e).to_string();
        let v = i128::from_str_radix(&s, 16).ok()?;
        return Some((start, e, v, 16, false));
    }
    let mut s0 = start;
    while s0 < len && !is_digit(s0) {
        s0 += 1;
    }
    let mut e = s0;
    while is_digit(e) {
        e += 1;
    }
    let mut a = s0;
    let mut neg = false;
    if a > 0 && t.char(a - 1) == '-' && (a < 2 || !(t.char(a - 2).is_alphanumeric() || t.char(a - 2) == '_'))
    {
        a -= 1;
        neg = true;
    }
    let digits = t.slice(s0..e).to_string();
    let v: i128 = digits.parse().ok()?;
    Some((a, e, if neg { -v } else { v }, 10, neg))
}

fn hex_prefix_before(t: Text, i: usize) -> bool {
    let mut j = i;
    while j > 0 && t.char(j - 1).is_ascii_hexdigit() {
        j -= 1;
    }
    j >= 2 && matches!(t.char(j - 1), 'x' | 'X') && t.char(j - 2) == '0'
}

fn format_number(v: i128, radix: u32, old_len: usize, was_neg: bool, padded: bool) -> String {
    if radix == 16 {
        let w = old_len.saturating_sub(2);
        return format!("0x{:0w$x}", v.max(0), w = w);
    }
    let digits = v.unsigned_abs().to_string();
    // Keep the width of zero-padded numbers: "007" + 1 = "008".
    let old_digits = old_len - usize::from(was_neg);
    let digits = if padded && digits.len() < old_digits { format!("{digits:0>old_digits$}") } else { digits };
    if v < 0 { format!("-{digits}") } else { digits }
}
