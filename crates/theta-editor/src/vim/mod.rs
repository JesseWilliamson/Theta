//! The Vim keymap: a modal state machine (Normal, Insert, Replace, Visual, Visual-line,
//! Visual-block) on the shared engine (rope, transactions, undo, search, prompts).
//!
//! Layout of the module:
//!
//! * `mod.rs`: state, the key grammar (`classify`) and command execution.
//! * `nav.rs`: motion evaluation, marks, jumplist, scrolling, searching.
//! * `object.rs`: text objects.
//! * `ops.rs`: registers and operators (delete/change/yank/indent/case/join/put).
//! * `visual.rs`: Visual mode state and the selections derived from it.
//! * `insert.rs`: Insert and Replace mode.
//! * `ex.rs`, `vregex.rs`: the `:` command line and the Vim -> `regex` translation.
//! * `hints.rs`: `pending_keys()` and which-key hints.
//!
//! Cursor model: the cursor is a char index on a grapheme start. In Normal mode the editor's
//! selection is the 1-wide block on that char (as in Helix, so highlight code is shared); in
//! Insert/Replace it is an empty selection (a bar). Visual modes keep their own anchor and
//! cursor in `VimState::vis` and derive the highlight selections from them.

mod ex;
mod geom;
mod hints;
mod insert;
mod keyenc;
mod motion;
mod nav;
mod object;
mod ops;
mod visual;
mod vregex;

use crate::editor::{Editor, Effect, Mode, ScrollRequest};
use crate::keys::{Key, KeyInput};
use crate::selection::{Selection, Selections};
use crate::text;
use geom::*;
use std::collections::HashMap;

pub(crate) use ops::{Reg, RegKind};
pub(crate) use visual::{VKind, Visual};

/// Counts are capped (like in the Helix keymap) so `99999999o` cannot hang the UI thread.
const MAX_COUNT: usize = 10_000;

/// Keys one command may replay (macros calling macros, `.`, `:normal`) before it is cut short.
const REPLAY_FUEL: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Format,
    Toggle,
    Lower,
    Upper,
    Rot13,
    Comment,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Delete => "d",
            Op::Change => "c",
            Op::Yank => "y",
            Op::Indent => ">",
            Op::Outdent => "<",
            Op::Format => "=",
            Op::Toggle => "g~",
            Op::Lower => "gu",
            Op::Upper => "gU",
            Op::Rot13 => "g?",
            Op::Comment => "gc",
        }
    }
}

/// Motions. Payload chars are the targets of `f`, marks etc.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    BackspaceH,
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    Column,
    WordFwd(bool),
    WordBack(bool),
    WordEnd(bool),
    WordEndBack(bool),
    Find { ch: char, fwd: bool, till: bool },
    RepeatFind { rev: bool },
    FileStart,
    FileEnd,
    ScreenTop,
    ScreenMid,
    ScreenBot,
    Percent,
    SentFwd,
    SentBack,
    ParaFwd,
    ParaBack,
    Unmatched { back: bool, open: char },
    NextLine,
    PrevLine,
    UnderLine,
    Mark { ch: char, exact: bool },
    SearchRepeat { rev: bool },
    Star { fwd: bool, whole: bool },
    SearchPrompt { fwd: bool },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Simple {
    X,
    BigX,
    BigD,
    BigC,
    S,
    BigS,
    BigY,
    ReplaceChar(char),
    Replace,
    Tilde,
    Put { before: bool, cursor_after: bool },
    Join { spaces: bool },
    Undo,
    Redo,
    Dot,
    RecordStart(char),
    RecordStop,
    Play(char),
    Increment(i64),
    Insert,
    Append,
    InsertLineStart,
    AppendLineEnd,
    OpenBelow,
    OpenAbove,
    InsertCol0,
    InsertLast,
    VisualChar,
    VisualLine,
    VisualBlock,
    Reselect,
    Colon,
    SetMark(char),
    Scroll(ScrollKind),
    JumpBack,
    JumpForward,
    SwapEnds,
    SwapCorners,
    Leader(char),
    BigU,
    SaveQuit,
    QuitNoSave,
    /// A host effect (`gd`, `gr`, `[d`, `]d`).
    Fx(crate::helix::FxKind),
    Nop,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ScrollKind {
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    LineDown,
    LineUp,
    Center,
    Top,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Cmd {
    Motion(Motion),
    Op(Op),
    Obj {
        inner: bool,
        ch: char,
    },
    Simple(Simple),
    /// `v` / `V` / `ctrl-v` after an operator: force the motion type.
    Force(char),
}

enum Cls {
    Incomplete,
    Invalid,
    Done(Cmd),
}

/// A command being typed: `["x][count]{operator|command}[count]{motion}`.
#[derive(Default, Clone)]
pub(crate) struct Pend {
    pub reg: Option<char>,
    pub count1: Option<usize>,
    pub count2: Option<usize>,
    pub op: Option<Op>,
    pub force: Option<char>,
    pub toks: Vec<KeyInput>,
    pub awaiting_reg: bool,
    pub disp: String,
}

impl Pend {
    fn idle(&self) -> bool {
        self.reg.is_none()
            && self.count1.is_none()
            && self.count2.is_none()
            && self.op.is_none()
            && self.toks.is_empty()
            && !self.awaiting_reg
    }

    fn count(&self) -> Option<usize> {
        match (self.count1, self.count2) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1)).min(MAX_COUNT)),
        }
    }
}

/// What a Visual-mode change covers, so `.` can replay it on the same amount of text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Extent {
    pub kind: VKind,
    pub lines: usize,
    /// Char: columns of the last line (or the width when on one line). Block: width.
    pub cols: usize,
    pub eol: bool,
}

#[derive(Default, Clone)]
pub(crate) struct CmdRec {
    pub keys: Vec<KeyInput>,
    pub count: Option<usize>,
    pub reg: Option<char>,
    pub rev0: u64,
    pub extent: Option<Extent>,
    pub insert_started: bool,
    pub no_dot: bool,
    pub done: bool,
}

#[derive(Clone)]
pub(crate) struct DotRec {
    pub keys: Vec<KeyInput>,
    pub count: Option<usize>,
    pub reg: Option<char>,
    pub extent: Option<Extent>,
}

/// Search/replace options set with `:set`.
#[derive(Clone, Copy)]
pub(crate) struct VimOpts {
    pub ignorecase: bool,
    pub smartcase: bool,
    pub hlsearch: bool,
    /// Like `set clipboard=unnamedplus`: the unnamed register is the host clipboard. Off by
    /// default because that is what plain Vim does; hosts that integrate with the system turn it on.
    pub clipboard_unnamed: bool,
}

impl Default for VimOpts {
    fn default() -> Self {
        // Matches the other keymaps' search: case-insensitive unless the pattern has capitals.
        VimOpts { ignorecase: true, smartcase: true, hlsearch: true, clipboard_unnamed: false }
    }
}

/// An operator waiting for the `/` prompt that supplies its motion.
#[derive(Clone, Copy)]
pub(crate) struct SearchOp {
    pub op: Op,
    pub reg: Option<char>,
    pub force: Option<char>,
}

pub(crate) struct VimState {
    pub pend: Pend,
    pub cmd: CmdRec,
    pub dot: Option<DotRec>,
    pub replaying_dot: u32,
    pub replaying_macro: u32,
    pub vis: Option<Visual>,
    pub last_vis: Option<Visual>,
    /// The desired column for vertical motions (`usize::MAX` after `$`).
    pub want: usize,
    pub keep_want: bool,
    pub marks: HashMap<char, usize>,
    pub jumps: Vec<usize>,
    pub jump_idx: usize,
    pub last_find: Option<(char, bool, bool)>,
    pub regs: HashMap<char, Reg>,
    pub macro_rec: Option<(char, Vec<KeyInput>)>,
    pub last_macro: Option<char>,
    pub ins: Option<insert::Session>,
    /// `ctrl-o` from Insert mode: 2 right after the key, 1 while the one command runs
    pub one_shot: u8,
    pub last_insert: String,
    pub last_ex: Option<String>,
    pub last_sub: Option<ex::SubSpec>,
    pub search_op: Option<SearchOp>,
    pub search_visual: bool,
    pub search_dir_fwd: bool,
    pub last_offset: String,
    pub opts: VimOpts,
    pub hl_off: bool,
    pub scroll_amount: usize,
    pub tracked: Vec<usize>,
    pub last_put: Option<(usize, usize)>,
    pub block_insert: Option<insert::BlockInsert>,
    pub ins_eol: bool,
    pub fuel: usize,
    /// where the cursor was when the next edit started, for placing it after undo
    pub edit_at: Option<usize>,
    /// `=` leaves its lines selected for the host to format: do not collapse them yet
    pub keep_sel: bool,
    pub ex_depth: u32,
    pub in_global: bool,
}

impl Default for VimState {
    fn default() -> Self {
        VimState {
            pend: Pend::default(),
            cmd: CmdRec::default(),
            dot: None,
            replaying_dot: 0,
            replaying_macro: 0,
            vis: None,
            last_vis: None,
            want: 0,
            keep_want: false,
            marks: HashMap::new(),
            jumps: Vec::new(),
            jump_idx: 0,
            last_find: None,
            regs: HashMap::new(),
            macro_rec: None,
            last_macro: None,
            ins: None,
            one_shot: 0,
            last_insert: String::new(),
            last_ex: None,
            last_sub: None,
            search_op: None,
            search_visual: false,
            search_dir_fwd: true,
            last_offset: String::new(),
            opts: VimOpts::default(),
            hl_off: false,
            scroll_amount: 0,
            tracked: Vec::new(),
            last_put: None,
            block_insert: None,
            ins_eol: false,
            fuel: REPLAY_FUEL,
            edit_at: None,
            keep_sel: false,
            ex_depth: 0,
            in_global: false,
        }
    }
}

fn plain(k: &KeyInput) -> Option<char> {
    k.text_char()
}

fn ctrl_char(k: &KeyInput) -> Option<char> {
    match k.key {
        Key::Char(c) if k.modifiers.ctrl && !k.modifiers.alt && !k.modifiers.platform => {
            Some(c.to_ascii_lowercase())
        }
        _ => None,
    }
}

impl Editor {
    pub(crate) fn rp(&self) -> ropey::Rope {
        self.rope.clone()
    }

    // ---- cursor access ------------------------------------------------------------------------

    pub(crate) fn vcur(&self) -> usize {
        if let Some(v) = &self.vim.vis {
            return v.cursor.min(self.rope.len_chars());
        }
        let p = self.sels.primary();
        match self.mode {
            Mode::Insert | Mode::Replace => p.head,
            _ => p.block_cursor(self.rope.slice(..)),
        }
    }

    /// Put the cursor at `pos` in the current mode (Normal clamps it onto a character).
    pub(crate) fn vset(&mut self, pos: usize) {
        match self.mode {
            Mode::Insert | Mode::Replace => {
                let t = self.rope.slice(..);
                let p = text::floor_grapheme(t, pos.min(t.len_chars()));
                self.set_sels(Selections::single(Selection::point(p)));
            }
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
                if let Some(v) = &mut self.vim.vis {
                    v.cursor = pos;
                }
                self.vis_sync();
            }
            _ => {
                let t = self.rope.slice(..);
                let p = clamp_normal(t, pos);
                self.set_sels(Selections::single(Selection::point(p)));
            }
        }
    }

    /// After `set_keymap(Vim)` or host selection changes: collapse to a Normal-mode cursor.
    pub(crate) fn vim_collapse_to_cursor(&mut self) {
        let t = self.rope.slice(..);
        let p = self.sels.primary();
        let pos = if p.is_empty() { p.head } else { p.block_cursor(t) };
        self.mode = Mode::Normal;
        self.vim.vis = None;
        let c = clamp_normal(t, pos);
        self.set_sels(Selections::single(Selection::point(c)));
    }

    /// The host set selections: keep one cursor (collapsing Visual state) and resync the goal column.
    pub(crate) fn vim_host_selection(&mut self) {
        self.vim.vis = None;
        self.vim.pend = Pend::default();
        let insert = matches!(self.mode, Mode::Insert | Mode::Replace);
        let t = self.rope.slice(..);
        let p = self.sels.primary();
        let pos = if insert || p.is_empty() { p.head } else { p.block_cursor(t) };
        let c = if insert { pos } else { clamp_normal(t, pos) };
        if !insert {
            self.mode = Mode::Normal;
        }
        self.set_sels(Selections::single(Selection::point(c)));
        let t = self.rope.slice(..);
        self.vim.want = want_col(t, c, !insert);
    }

    /// A programmatic edit (`replace_range`, `set_text`) moved text under the cursor: keep the
    /// Normal-mode invariant (the cursor is on a character) and any Visual state in range.
    pub(crate) fn vim_after_host_edit(&mut self) {
        let len = self.rope.len_chars();
        if let Some(v) = &mut self.vim.vis {
            v.anchor = v.anchor.min(len);
            v.cursor = v.cursor.min(len);
            self.vis_sync();
        } else if self.mode == Mode::Normal && self.sels.len() == 1 && self.sels.primary().len() <= 1 {
            let t = self.rope.slice(..);
            let c = clamp_normal(t, self.vcur());
            self.set_sels(Selections::single(Selection::point(c)));
        }
    }

    pub(crate) fn vim_cursor_of(&self, sel: &Selection) -> usize {
        match self.mode {
            Mode::Insert | Mode::Replace => sel.head,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
                if *sel == self.sels.primary() {
                    self.vim
                        .vis
                        .as_ref()
                        .map_or_else(|| sel.block_cursor(self.t()), |v| v.cursor.min(self.rope.len_chars()))
                } else {
                    sel.block_cursor(self.t())
                }
            }
            _ => sel.block_cursor(self.t()),
        }
    }

    // ---- key entry ------------------------------------------------------------------------------

    pub(crate) fn rec(&mut self, k: &KeyInput) {
        if self.vim.replaying_dot == 0 {
            self.vim.cmd.keys.push(k.clone());
        }
    }

    pub(crate) fn vim_key(&mut self, k: &KeyInput) -> bool {
        if self.vim.search_op.is_some() && self.prompt.is_none() {
            // the `/` prompt that was to supply the motion was cancelled
            self.vim.search_op = None;
            self.vim.search_visual = false;
        }
        self.vim.keep_sel = false;
        if self.vim.replaying_dot == 0 && self.vim.replaying_macro == 0 {
            self.vim.fuel = REPLAY_FUEL;
        }
        // Escape and ctrl-c / ctrl-[ are Escape everywhere
        let esc = k.key == Key::Escape || (ctrl_char(k).is_some_and(|c| c == 'c' || c == '['));
        match self.mode {
            Mode::Insert => {
                self.rec(k);
                self.vim_insert_key(k, esc)
            }
            Mode::Replace => {
                self.rec(k);
                self.vim_replace_key(k, esc)
            }
            _ => self.vim_normal_key(k, esc),
        }
    }

    /// Run a key through the whole pipeline from inside a replay (`.`, macros, `:normal`).
    pub(crate) fn vim_feed(&mut self, k: &KeyInput) {
        if self.vim.fuel == 0 {
            return;
        }
        self.vim.fuel -= 1;
        if self.prompt.is_some() {
            self.prompt_key(k);
        } else if !self.global_key(k) {
            self.vim_key(k);
        }
        self.vim_finish_command();
    }

    pub(crate) fn vim_feed_keys(&mut self, keys: &[KeyInput]) {
        for k in keys {
            self.vim_feed(k);
        }
    }

    fn vim_normal_key(&mut self, k: &KeyInput, esc: bool) -> bool {
        let visual = self.vim.vis.is_some();
        // A host-made selection (EnsureForward etc.) in Normal mode collapses to the cursor.
        if !visual && self.mode == Mode::Normal && self.sels.primary().len() > 1 && self.vim.pend.idle() {
            self.vim_collapse_to_cursor();
        }
        let one_shot = self.vim.one_shot > 0;
        if self.vim.pend.idle() && !one_shot {
            self.vim.cmd = CmdRec { rev0: self.revision, extent: self.visual_extent(), ..Default::default() };
        }
        if one_shot {
            self.rec(k);
        }
        if esc {
            let had = !self.vim.pend.idle();
            self.vim.pend = Pend::default();
            if visual {
                self.vis_exit();
            } else if !had {
                self.effects.push(Effect::DismissCompletion);
            }
            return true;
        }
        let pc = plain(k);
        // register prefix
        if self.vim.pend.awaiting_reg {
            self.vim.pend.awaiting_reg = false;
            if let Some(c) = pc
                && valid_reg(c)
            {
                self.vim.pend.reg = Some(c);
                self.vim.pend.disp.push(c);
                return true;
            }
            self.vim.pend = Pend::default();
            return true;
        }
        if pc == Some('"')
            && self.vim.pend.toks.is_empty()
            && self.vim.pend.op.is_none_or(|_| true)
            && !self.vim.pend.disp.ends_with('"')
        {
            // `"` is a register prefix wherever a count could be (before the command)
            if self.vim.pend.toks.is_empty() {
                self.vim.pend.awaiting_reg = true;
                self.vim.pend.disp.push('"');
                return true;
            }
        }
        // counts
        if let Some(d @ '0'..='9') = pc {
            let p = &mut self.vim.pend;
            if p.toks.is_empty() {
                let slot = if p.op.is_some() { &mut p.count2 } else { &mut p.count1 };
                if d != '0' || slot.is_some() {
                    let v = slot.unwrap_or(0).saturating_mul(10).saturating_add(d as usize - '0' as usize);
                    *slot = Some(v.min(MAX_COUNT));
                    p.disp.push(d);
                    return true;
                }
            }
        }
        self.vim.pend.toks.push(k.clone());
        self.vim.pend.disp.push_str(&k.display());
        if !one_shot {
            self.rec(k);
        }
        match self.classify() {
            Cls::Incomplete => true,
            Cls::Invalid => {
                let depth = self.vim.pend.toks.len();
                self.vim.pend = Pend::default();
                // an unbound ctrl/alt chord is the host's to handle
                let m = &k.modifiers;
                depth > 1 || !(m.ctrl || m.alt || m.platform) || plain(k).is_some()
            }
            Cls::Done(cmd) => {
                self.exec(cmd);
                true
            }
        }
    }

    // ---- the grammar ------------------------------------------------------------------------------

    fn classify(&self) -> Cls {
        let p = &self.vim.pend;
        let toks = &p.toks;
        let op_pending = p.op.is_some();
        let visual = self.vim.vis.is_some();
        let k0 = &toks[0];
        let n = toks.len();
        use Cls::*;
        // ctrl / special keys first
        if let Some(c) = ctrl_char(k0) {
            if n > 1 {
                return Invalid;
            }
            let s = match c {
                'r' => Simple::Redo,
                'o' => Simple::JumpBack,
                'i' => Simple::JumpForward,
                'd' => Simple::Scroll(ScrollKind::HalfDown),
                'u' => Simple::Scroll(ScrollKind::HalfUp),
                'f' => Simple::Scroll(ScrollKind::PageDown),
                'b' => Simple::Scroll(ScrollKind::PageUp),
                'e' => Simple::Scroll(ScrollKind::LineDown),
                'y' => Simple::Scroll(ScrollKind::LineUp),
                'a' => Simple::Increment(1),
                'x' => Simple::Increment(-1),
                'v' | 'q' => {
                    if op_pending {
                        return Done(Cmd::Force('\x16'));
                    }
                    Simple::VisualBlock
                }
                'h' => return Done(Cmd::Motion(Motion::BackspaceH)),
                'j' | 'n' => return Done(Cmd::Motion(Motion::Down)),
                'k' | 'p' => return Done(Cmd::Motion(Motion::Up)),
                'm' => return Done(Cmd::Motion(Motion::NextLine)),
                'l' | 'g' => Simple::Nop,
                _ => return Invalid,
            };
            return Done(Cmd::Simple(s));
        }
        if k0.modifiers.ctrl || k0.modifiers.alt || k0.modifiers.platform {
            return Invalid;
        }
        match &k0.key {
            Key::Left => return Done(Cmd::Motion(Motion::Left)),
            Key::Right => return Done(Cmd::Motion(Motion::Right)),
            Key::Up => return Done(Cmd::Motion(Motion::Up)),
            Key::Down => return Done(Cmd::Motion(Motion::Down)),
            Key::Home => return Done(Cmd::Motion(Motion::LineStart)),
            Key::End => return Done(Cmd::Motion(Motion::LineEnd)),
            Key::PageUp => return Done(Cmd::Simple(Simple::Scroll(ScrollKind::PageUp))),
            Key::PageDown => return Done(Cmd::Simple(Simple::Scroll(ScrollKind::PageDown))),
            Key::Enter => return Done(Cmd::Motion(Motion::NextLine)),
            Key::Backspace => return Done(Cmd::Motion(Motion::BackspaceH)),
            Key::Tab => return Done(Cmd::Simple(Simple::JumpForward)),
            Key::Delete => return Done(Cmd::Simple(Simple::X)),
            Key::Char(_) => {}
            _ => return Invalid,
        }
        let c0 = plain(k0).unwrap_or('\0');
        let c1 = toks.get(1).and_then(plain);
        let m = |m: Motion| Done(Cmd::Motion(m));
        let s = |s: Simple| Done(Cmd::Simple(s));
        let o = |o: Op| Done(Cmd::Op(o));
        // Operators and their doubled forms (the doubled forms are resolved in `exec`).
        if let Some(cur) = p.op {
            // second key of a doubled operator
            let doubled = match cur {
                Op::Delete => c0 == 'd',
                Op::Change => c0 == 'c',
                Op::Yank => c0 == 'y',
                Op::Indent => c0 == '>',
                Op::Outdent => c0 == '<',
                Op::Format => c0 == '=',
                Op::Toggle => c0 == '~' || (c0 == 'g' && c1 == Some('~')),
                Op::Lower => c0 == 'u' || (c0 == 'g' && c1 == Some('u')),
                Op::Upper => c0 == 'U' || (c0 == 'g' && c1 == Some('U')),
                Op::Rot13 => c0 == '?' || (c0 == 'g' && c1 == Some('?')),
                Op::Comment => c0 == 'c' || (c0 == 'g' && c1 == Some('c')),
            };
            if doubled && n == 1 && c0 != 'g' {
                return o(cur);
            }
            if doubled && c0 == 'g' {
                return if n == 2 { o(cur) } else { Incomplete };
            }
            if matches!(c0, 'v' | 'V') {
                return Done(Cmd::Force(c0));
            }
        }
        if (op_pending || visual) && matches!(c0, 'i' | 'a') {
            return match c1 {
                None => Incomplete,
                Some(ch) if object::is_object_char(ch) => Done(Cmd::Obj { inner: c0 == 'i', ch }),
                Some(_) => Invalid,
            };
        }
        match c0 {
            'h' => m(Motion::Left),
            'l' => m(Motion::Right),
            ' ' => {
                if op_pending {
                    m(Motion::Right)
                } else {
                    match c1 {
                        None => Incomplete,
                        Some(ch) => s(Simple::Leader(ch)),
                    }
                }
            }
            'j' => m(Motion::Down),
            'k' => m(Motion::Up),
            '0' => m(Motion::LineStart),
            '^' => m(Motion::FirstNonBlank),
            '$' => m(Motion::LineEnd),
            '|' => m(Motion::Column),
            'w' => m(Motion::WordFwd(false)),
            'W' => m(Motion::WordFwd(true)),
            'b' => m(Motion::WordBack(false)),
            'B' => m(Motion::WordBack(true)),
            'e' => m(Motion::WordEnd(false)),
            'E' => m(Motion::WordEnd(true)),
            'f' | 'F' | 't' | 'T' => match toks.get(1) {
                None => Incomplete,
                Some(k1) => {
                    let ch = match k1.key {
                        Key::Char(c) if !k1.modifiers.ctrl => c,
                        Key::Tab => '\t',
                        Key::Enter => '\n',
                        _ => return Invalid,
                    };
                    m(Motion::Find { ch, fwd: c0 == 'f' || c0 == 't', till: c0 == 't' || c0 == 'T' })
                }
            },
            ';' => m(Motion::RepeatFind { rev: false }),
            ',' => m(Motion::RepeatFind { rev: true }),
            'G' => m(Motion::FileEnd),
            'H' => m(Motion::ScreenTop),
            'M' => m(Motion::ScreenMid),
            'L' => m(Motion::ScreenBot),
            '%' => m(Motion::Percent),
            '(' => m(Motion::SentBack),
            ')' => m(Motion::SentFwd),
            '{' => m(Motion::ParaBack),
            '}' => m(Motion::ParaFwd),
            '+' => m(Motion::NextLine),
            '-' => m(Motion::PrevLine),
            '_' => m(Motion::UnderLine),
            'n' => m(Motion::SearchRepeat { rev: false }),
            'N' => m(Motion::SearchRepeat { rev: true }),
            '*' => m(Motion::Star { fwd: true, whole: true }),
            '#' => m(Motion::Star { fwd: false, whole: true }),
            '/' => m(Motion::SearchPrompt { fwd: true }),
            '?' => m(Motion::SearchPrompt { fwd: false }),
            '\'' | '`' => match c1 {
                None => Incomplete,
                Some(ch) => m(Motion::Mark { ch, exact: c0 == '`' }),
            },
            '[' | ']' => match c1 {
                None => Incomplete,
                Some(ch @ ('(' | '{')) if c0 == '[' => m(Motion::Unmatched { back: true, open: ch }),
                Some(ch @ (')' | '}')) if c0 == ']' => m(Motion::Unmatched { back: false, open: ch }),
                Some('p') => Invalid,
                Some('d') if c0 == '[' => s(Simple::Fx(crate::helix::FxKind::PrevProblem)),
                Some('d') => s(Simple::Fx(crate::helix::FxKind::NextProblem)),
                Some(_) => Invalid,
            },
            'g' => match c1 {
                None => Incomplete,
                Some(ch) => match ch {
                    'g' => m(Motion::FileStart),
                    'e' => m(Motion::WordEndBack(false)),
                    'E' => m(Motion::WordEndBack(true)),
                    '_' => m(Motion::LastNonBlank),
                    'j' => m(Motion::Down),
                    'k' => m(Motion::Up),
                    '0' => m(Motion::LineStart),
                    '^' => m(Motion::FirstNonBlank),
                    '$' => m(Motion::LineEnd),
                    '*' => m(Motion::Star { fwd: true, whole: false }),
                    '#' => m(Motion::Star { fwd: false, whole: false }),
                    '~' => o(Op::Toggle),
                    'u' => o(Op::Lower),
                    'U' => o(Op::Upper),
                    '?' => o(Op::Rot13),
                    'c' => o(Op::Comment),
                    'J' => s(Simple::Join { spaces: false }),
                    'p' => s(Simple::Put { before: false, cursor_after: true }),
                    'P' => s(Simple::Put { before: true, cursor_after: true }),
                    'd' => s(Simple::Fx(crate::helix::FxKind::GoToDefinition)),
                    'r' => s(Simple::Fx(crate::helix::FxKind::FindUsages)),
                    'v' => s(Simple::Reselect),
                    'i' => s(Simple::InsertLast),
                    'I' => s(Simple::InsertCol0),
                    _ => Invalid,
                },
            },
            'z' => match c1 {
                None => Incomplete,
                Some('z') | Some('.') => s(Simple::Scroll(ScrollKind::Center)),
                Some('t') => s(Simple::Scroll(ScrollKind::Top)),
                Some('b') | Some('-') => s(Simple::Scroll(ScrollKind::Bottom)),
                Some(_) => Invalid,
            },
            'Z' => match c1 {
                None => Incomplete,
                Some('Z') => s(Simple::SaveQuit),
                Some('Q') => s(Simple::QuitNoSave),
                Some(_) => Invalid,
            },
            'd' => o(Op::Delete),
            'c' => o(Op::Change),
            'y' => o(Op::Yank),
            '>' => o(Op::Indent),
            '<' => o(Op::Outdent),
            '=' => o(Op::Format),
            'x' => s(Simple::X),
            'X' => s(Simple::BigX),
            'D' => s(Simple::BigD),
            'C' => s(Simple::BigC),
            's' => s(Simple::S),
            'S' => s(Simple::BigS),
            'Y' => s(Simple::BigY),
            'r' => match toks.get(1) {
                None => Incomplete,
                Some(k1) => match k1.key {
                    Key::Char(ch) if !k1.modifiers.ctrl && !k1.modifiers.alt => s(Simple::ReplaceChar(ch)),
                    Key::Enter => s(Simple::ReplaceChar('\n')),
                    Key::Tab => s(Simple::ReplaceChar('\t')),
                    _ => Invalid,
                },
            },
            'R' => s(Simple::Replace),
            '~' => s(Simple::Tilde),
            'p' => s(Simple::Put { before: false, cursor_after: false }),
            'P' => s(Simple::Put { before: true, cursor_after: false }),
            'J' => s(Simple::Join { spaces: true }),
            'u' => s(Simple::Undo),
            '.' => s(Simple::Dot),
            'q' => {
                if self.vim.macro_rec.is_some() {
                    s(Simple::RecordStop)
                } else {
                    match c1 {
                        None => Incomplete,
                        Some(ch) if ch.is_ascii_alphanumeric() || ch == '"' => s(Simple::RecordStart(ch)),
                        Some(_) => Invalid,
                    }
                }
            }
            '@' => match c1 {
                None => Incomplete,
                Some(ch) => s(Simple::Play(ch)),
            },
            'i' => s(Simple::Insert),
            'a' => s(Simple::Append),
            'I' => s(Simple::InsertLineStart),
            'A' => s(Simple::AppendLineEnd),
            'o' => {
                if visual {
                    s(Simple::SwapEnds)
                } else {
                    s(Simple::OpenBelow)
                }
            }
            'O' => {
                if visual {
                    s(Simple::SwapCorners)
                } else {
                    s(Simple::OpenAbove)
                }
            }
            'v' => s(Simple::VisualChar),
            'V' => s(Simple::VisualLine),
            ':' => s(Simple::Colon),
            'm' => match c1 {
                None => Incomplete,
                Some(ch) => s(Simple::SetMark(ch)),
            },
            'U' => s(Simple::BigU),
            '&' | 'K' => s(Simple::Nop),
            _ => Invalid,
        }
    }

    // ---- execution ---------------------------------------------------------------------------------

    fn exec(&mut self, cmd: Cmd) {
        let pend = std::mem::take(&mut self.vim.pend);
        let count = pend.count();
        let reg = pend.reg;
        self.vim.cmd.count = count;
        self.vim.cmd.reg = reg;
        match cmd {
            Cmd::Force(c) => {
                // keep waiting for the motion
                self.vim.pend = Pend { force: Some(c), ..pend };
                self.vim.pend.toks.clear();
            }
            Cmd::Op(op) => {
                if let Some(cur) = pend.op {
                    // doubled operator (`dd`, `g~~`): linewise on `count` lines; a different
                    // operator (`dc`) just cancels
                    if cur == op {
                        self.vim.keep_want = false;
                        self.run_op_lines(cur, count.unwrap_or(1), reg);
                    }
                } else if self.vim.vis.is_some() {
                    self.visual_operator(op, reg, count);
                } else {
                    // wait for the motion
                    self.vim.pend = Pend {
                        reg,
                        count1: pend.count1,
                        op: Some(op),
                        disp: format!("{}{}", pend.disp, ""),
                        ..Default::default()
                    };
                }
            }
            Cmd::Motion(m) => {
                if let Some(op) = pend.op {
                    self.run_op_motion(op, m, count, reg, pend.force);
                } else {
                    self.move_motion(m, count);
                }
            }
            Cmd::Obj { inner, ch } => {
                if let Some(op) = pend.op {
                    self.run_op_object(op, inner, ch, count, reg);
                } else if self.vim.vis.is_some() {
                    self.visual_object(inner, ch, count.unwrap_or(1));
                }
            }
            Cmd::Simple(s) => {
                if pend.op.is_some() {
                    // anything but a motion or object cancels the pending operator
                    return;
                }
                if self.vim.vis.is_some() {
                    self.exec_visual_simple(s, reg, count);
                } else {
                    self.exec_simple(s, reg, count);
                }
            }
        }
    }

    fn exec_simple(&mut self, s: Simple, reg: Option<char>, count: Option<usize>) {
        let n = count.unwrap_or(1);
        match s {
            Simple::X => {
                if self.cur_line_empty() {
                    return;
                }
                self.run_op_motion(Op::Delete, Motion::Right, count, reg, None);
            }
            Simple::BigX => self.run_op_motion(Op::Delete, Motion::Left, count, reg, None),
            Simple::BigD => self.run_op_motion(Op::Delete, Motion::LineEnd, count, reg, None),
            Simple::BigC => self.run_op_motion(Op::Change, Motion::LineEnd, count, reg, None),
            Simple::S => self.run_op_motion(Op::Change, Motion::Right, count, reg, None),
            Simple::BigS => self.run_op_lines(Op::Change, n, reg),
            Simple::BigY => self.run_op_motion(Op::Yank, Motion::LineEnd, count, reg, None),
            Simple::ReplaceChar(c) => self.replace_chars(c, n),
            Simple::Replace => {
                self.vim.cmd.insert_started = true;
                self.start_replace(n);
            }
            Simple::Tilde => self.tilde(n),
            Simple::Put { before, cursor_after } => self.put_cmd(reg, n, before, cursor_after),
            Simple::Join { spaces } => self.join_cmd(n.max(2), spaces),
            Simple::Undo => {
                self.vim.cmd.no_dot = true;
                self.vim_undo(n, false);
            }
            Simple::Redo => {
                self.vim.cmd.no_dot = true;
                self.vim_undo(n, true);
            }
            Simple::Dot => {
                self.vim.cmd.no_dot = true;
                self.dot_repeat(count);
            }
            Simple::RecordStart(r) => {
                self.vim.macro_rec = Some((r, Vec::new()));
                self.status(format!("recording @{r}"));
            }
            Simple::RecordStop => self.stop_recording(),
            Simple::Play(r) => {
                self.vim.cmd.no_dot = true;
                self.play_macro(r, n);
            }
            Simple::Increment(d) => self.increment_cmd(d * n as i64),
            Simple::Insert => self.vim_enter_insert(InsertAt::Here, n),
            Simple::Append => self.vim_enter_insert(InsertAt::After, n),
            Simple::InsertLineStart => self.vim_enter_insert(InsertAt::FirstNonBlank, n),
            Simple::AppendLineEnd => self.vim_enter_insert(InsertAt::LineEnd, n),
            Simple::InsertCol0 => self.vim_enter_insert(InsertAt::Col0, n),
            Simple::InsertLast => self.vim_enter_insert(InsertAt::LastInsert, n),
            Simple::OpenBelow => self.open_line_cmd(true, n),
            Simple::OpenAbove => self.open_line_cmd(false, n),
            Simple::VisualChar => self.vis_enter(VKind::Char),
            Simple::VisualLine => self.vis_enter(VKind::Line),
            Simple::VisualBlock => self.vis_enter(VKind::Block),
            Simple::Reselect => self.vis_reselect(),
            Simple::Colon => {
                let initial = match count {
                    Some(1) => ".".to_string(),
                    Some(n) => format!(".,.+{}", n - 1),
                    None => String::new(),
                };
                self.open_prompt(crate::prompt::PromptKind::Command, ":", &initial, 0);
            }
            Simple::SetMark(c) => self.set_mark_cmd(c),
            Simple::Scroll(kind) => self.scroll_cmd(kind, count),
            Simple::JumpBack => self.jump_cmd(true, n),
            Simple::JumpForward => self.jump_cmd(false, n),
            Simple::SwapEnds | Simple::SwapCorners => {}
            Simple::Leader(c) => self.leader_key(c),
            Simple::BigU => {}
            Simple::SaveQuit => {
                self.effects.push(Effect::Save);
                self.effects.push(Effect::CloseTab);
            }
            Simple::QuitNoSave => self.effects.push(Effect::CloseTab),
            Simple::Fx(kind) => self.run_fx(kind, 1),
            Simple::Nop => {}
        }
    }

    pub(crate) fn cur_line_empty(&self) -> bool {
        let t = self.rope.slice(..);
        line_empty(t, line_of(t, self.vcur()))
    }

    // ---- end of command -------------------------------------------------------------------------------

    /// Called after every key: settles the cursor and finalises dot-repeat bookkeeping.
    pub(crate) fn vim_finish_command(&mut self) {
        if self.prompt.is_some() || self.vim.search_op.is_some() {
            return;
        }
        self.vim.edit_at = None;
        let busy = !self.vim.pend.idle();
        if !busy && self.mode == Mode::Normal && self.vim.vis.is_none() && !self.vim.keep_sel {
            // Normal mode keeps the cursor on a character
            let t = self.rope.slice(..);
            let cur = self.vcur();
            let c = clamp_normal(t, cur);
            let want = Selection::point(c).min_width_1(t);
            if self.sels.v.len() != 1 || self.sels.primary() != want {
                self.set_sels(Selections::single(Selection::point(c)));
            }
        }
        if !busy {
            if !self.vim.keep_want {
                let t = self.rope.slice(..);
                let normal = self.mode == Mode::Normal;
                self.vim.want = want_col(t, self.vcur(), normal);
            }
            self.vim.keep_want = false;
        }
        // the one command after `ctrl-o` is done: back to Insert mode
        if self.vim.one_shot == 2 {
            self.vim.one_shot = 1;
            return;
        }
        if self.vim.one_shot == 1 && !busy && self.vim.vis.is_none() {
            self.finish_one_shot();
        }
        if busy || matches!(self.mode, Mode::Insert | Mode::Replace) || self.vim.one_shot > 0 {
            return;
        }
        let cmd = &mut self.vim.cmd;
        if cmd.done {
            return;
        }
        cmd.done = true;
        if self.vim.replaying_dot > 0 || cmd.no_dot {
            return;
        }
        let changed = self.revision != cmd.rev0;
        if (changed || cmd.insert_started) && !cmd.keys.is_empty() {
            self.vim.dot =
                Some(DotRec { keys: cmd.keys.clone(), count: cmd.count, reg: cmd.reg, extent: cmd.extent });
        }
    }

    pub(crate) fn dot_repeat(&mut self, count: Option<usize>) {
        let Some(d) = self.vim.dot.clone() else { return };
        if self.vim.replaying_dot > 4 {
            return;
        }
        let count = count.or(d.count);
        let mut reg = d.reg;
        // "1p . . walks through the numbered registers
        if let Some(r) = reg
            && ('1'..='8').contains(&r)
        {
            reg = Some((r as u8 + 1) as char);
            if let Some(dd) = &mut self.vim.dot {
                dd.reg = reg;
            }
        }
        self.vim.replaying_dot += 1;
        if let Some(ext) = d.extent {
            self.visual_from_extent(ext);
        }
        let mut keys: Vec<KeyInput> = Vec::new();
        if let Some(r) = reg {
            keys.push(KeyInput::ch('"'));
            keys.push(KeyInput::ch(r));
        }
        if let Some(c) = count {
            keys.extend(c.to_string().chars().map(KeyInput::ch));
        }
        keys.extend(d.keys.iter().cloned());
        self.vim_feed_keys(&keys);
        self.vim.replaying_dot -= 1;
        if let (Some(c), Some(dd)) = (count, &mut self.vim.dot) {
            dd.count = Some(c);
        }
    }

    // ---- macros -----------------------------------------------------------------------------------------

    fn stop_recording(&mut self) {
        if let Some((reg, mut keys)) = self.vim.macro_rec.take() {
            keys.pop(); // the `q` that stopped the recording
            let text = keyenc::encode(&keys);
            let r = Reg { text, kind: RegKind::Char };
            self.reg_store_named(reg, r);
            self.vim.last_macro = Some(reg);
            self.status(format!("recorded @{reg}"));
        }
    }

    fn play_macro(&mut self, r: char, n: usize) {
        let r = if r == '@' { self.vim.last_macro.unwrap_or('@') } else { r };
        let text = if r == ':' {
            // `@:` repeats the last command line
            self.vim.last_macro = Some(':');
            match self.vim.last_ex.clone() {
                Some(c) => {
                    for _ in 0..n {
                        self.vim_ex(&c);
                    }
                    return;
                }
                None => return,
            }
        } else {
            match self.reg_get(Some(r)) {
                Some(reg) => reg.text,
                None => {
                    self.status(format!("E748: No previously used register: {r}"));
                    return;
                }
            }
        };
        self.vim.last_macro = Some(r);
        if self.vim.replaying_macro > 8 {
            return;
        }
        let keys = keyenc::decode(&text);
        self.vim.replaying_macro += 1;
        'outer: for _ in 0..n {
            for k in &keys {
                let before = self.revision;
                self.vim_feed(k);
                let _ = before;
                if self.vim.replaying_macro == 0 || self.vim.fuel == 0 {
                    break 'outer;
                }
            }
        }
        self.vim.replaying_macro -= 1;
    }

    // ---- misc ---------------------------------------------------------------------------------------------

    pub(crate) fn vim_undo(&mut self, n: usize, redo: bool) {
        for _ in 0..n {
            let ok = if redo { self.redo() } else { self.undo() };
            if !ok {
                self.status(if redo { "Already at newest change" } else { "Already at oldest change" });
                break;
            }
        }
        if self.vim.vis.is_some() {
            self.vis_exit();
        }
    }

    pub(crate) fn vim_pending_string(&self) -> String {
        if self.mode == Mode::Insert || self.mode == Mode::Replace {
            return String::new();
        }
        self.vim.pend.disp.clone()
    }

    pub(crate) fn set_mark_cmd(&mut self, c: char) {
        let pos = self.vcur();
        match c {
            'a'..='z' | 'A'..='Z' => {
                self.vim.marks.insert(c, pos);
            }
            '\'' | '`' => {
                self.vim.marks.insert('\'', pos);
            }
            '[' | ']' | '<' | '>' => {
                self.vim.marks.insert(c, pos);
            }
            _ => {}
        }
    }

    pub(crate) fn scroll_cmd(&mut self, kind: ScrollKind, count: Option<usize>) {
        let (first, vis) = self.viewport;
        let rope = self.rp();
        let t = rope.slice(..);
        let last = nlines(t) - 1;
        let cur_line = line_of(t, self.vcur());
        let half = (vis / 2).max(1);
        let target_line;
        match kind {
            ScrollKind::HalfDown | ScrollKind::HalfUp => {
                if let Some(c) = count {
                    self.vim.scroll_amount = c;
                }
                let amt = if self.vim.scroll_amount > 0 { self.vim.scroll_amount } else { half };
                let down = kind == ScrollKind::HalfDown;
                if (down && cur_line >= last) || (!down && cur_line == 0) {
                    return;
                }
                target_line = if down { (cur_line + amt).min(last) } else { cur_line.saturating_sub(amt) };
                self.scroll_viewport(if down { amt as i32 } else { -(amt as i32) });
            }
            ScrollKind::PageDown | ScrollKind::PageUp => {
                let amt = vis.saturating_sub(2).max(1) * count.unwrap_or(1);
                let down = kind == ScrollKind::PageDown;
                if (down && cur_line >= last) || (!down && cur_line == 0) {
                    return;
                }
                target_line = if down { (cur_line + amt).min(last) } else { cur_line.saturating_sub(amt) };
                self.scroll_viewport(if down { amt as i32 } else { -(amt as i32) });
            }
            ScrollKind::LineDown | ScrollKind::LineUp => {
                let n = count.unwrap_or(1) as i32;
                let d = if kind == ScrollKind::LineDown { n } else { -n };
                self.scroll_viewport(d);
                let (nf, _) = self.viewport;
                // the cursor must stay on screen
                target_line = cur_line.clamp(nf, (nf + vis.saturating_sub(1)).min(last).max(nf));
                if target_line == cur_line {
                    return;
                }
            }
            ScrollKind::Center => {
                self.effects.push(Effect::Scroll(ScrollRequest::CursorCenter));
                return;
            }
            ScrollKind::Top => {
                self.effects.push(Effect::Scroll(ScrollRequest::CursorTop));
                return;
            }
            ScrollKind::Bottom => {
                self.effects.push(Effect::Scroll(ScrollRequest::CursorBottom));
                return;
            }
        }
        let _ = first;
        let pos = if matches!(kind, ScrollKind::LineDown | ScrollKind::LineUp) {
            let col = self.vim.want;
            self.vim.keep_want = true;
            pos_at_col(t, target_line, col)
        } else {
            first_nonblank(t, target_line)
        };
        self.vset(pos);
    }

    fn scroll_viewport(&mut self, lines: i32) {
        self.effects.push(Effect::Scroll(ScrollRequest::Lines(lines)));
        let t = self.rope.slice(..);
        let last = nlines(t) as i32 - 1;
        let first = (self.viewport.0 as i32 + lines).clamp(0, last.max(0));
        self.viewport.0 = first as usize;
    }

    /// A mouse click: moves the cursor (Normal/Insert) or starts Visual mode with shift.
    pub(crate) fn vim_click(&mut self, pos: usize, extend: bool) {
        let rope = self.rp();
        let t = rope.slice(..);
        let p = text::floor_grapheme(t, pos.min(t.len_chars()));
        self.vim.pend = Pend::default();
        if extend {
            let anchor = self.vcur();
            self.vim_select_visual(anchor, p, VKind::Char);
            return;
        }
        if self.vim.vis.is_some() {
            self.vis_exit();
        }
        self.vim.one_shot = 0;
        let c = if matches!(self.mode, Mode::Insert | Mode::Replace) { p } else { clamp_normal(t, p) };
        self.set_sels(Selections::single(Selection::point(c)));
        let t = self.rope.slice(..);
        self.vim.want = col_of(t, self.vcur());
    }

    pub(crate) fn vim_drag(&mut self, anchor: usize, pos: usize) {
        let t = self.rope.slice(..);
        let p = text::floor_grapheme(t, pos.min(t.len_chars()));
        if anchor == p && self.vim.vis.is_none() {
            return;
        }
        self.vim_select_visual(anchor, p, VKind::Char);
    }

    /// Double / triple click: select `from..to` (exclusive end) as Visual / Visual-line.
    pub(crate) fn vim_select_range(&mut self, from: usize, to: usize, lines: bool) {
        let t = self.rope.slice(..);
        let last = if to > from { text::prev_grapheme(t, to) } else { from };
        let (a, c) = (from, if lines { last.min(lend(t, line_of(t, last))) } else { last });
        self.vim_select_visual(a, c, if lines { VKind::Line } else { VKind::Char });
    }

    fn vim_select_visual(&mut self, anchor: usize, cursor: usize, kind: VKind) {
        let t = self.rope.slice(..);
        let (anchor, cursor) = (
            text::floor_grapheme(t, anchor.min(t.len_chars())),
            text::floor_grapheme(t, cursor.min(t.len_chars())),
        );
        if matches!(self.mode, Mode::Insert | Mode::Replace) {
            self.vim.ins = None;
            self.mode = Mode::Normal;
        }
        self.vim.vis = Some(Visual { kind, anchor, cursor });
        self.mode = match kind {
            VKind::Char => Mode::Visual,
            VKind::Line => Mode::VisualLine,
            VKind::Block => Mode::VisualBlock,
        };
        self.vis_sync();
    }

    /// After undo/redo. Vim puts the cursor back where it was before the change when that was in
    /// (or next to) the changed lines, else at the first changed line (first non-blank).
    pub(crate) fn vim_after_undo(&mut self, info: UndoInfo, before: &Selections) {
        self.vim.vis = None;
        self.vim.ins = None;
        self.vim.pend = Pend::default();
        self.mode = Mode::Normal;
        let rope = self.rp();
        let t = rope.slice(..);
        let len = t.len_chars();
        let saved = clamp_normal(t, before.primary().block_cursor(t).min(len));
        let Some((first, newsize)) = info.first else {
            self.vset(saved);
            return;
        };
        let first_line = line_of(t, first.min(len));
        let s_line = line_of(t, saved);
        let in_range = s_line + 1 >= first_line && s_line < first_line + newsize + 1;
        let mut line = if in_range { s_line } else { first_line };
        if s_line + 1 == line && line > 0 {
            line = s_line;
        }
        let pos = if line >= nlines(t) {
            lstart(t, nlines(t) - 1)
        } else if line == s_line {
            pos_at_col(t, line, col_of(t, saved))
        } else {
            first_nonblank(t, line)
        };
        self.vset(pos);
    }
}

/// What an undo/redo step changed, as far as the cursor placement needs to know.
#[derive(Default, Clone, Copy)]
pub(crate) struct UndoInfo {
    /// (first changed position, lines the restored text spans: 0 for removed whole lines)
    first: Option<(usize, usize)>,
}

impl UndoInfo {
    /// Look at `tx` (about to be applied to `rope`).
    pub(crate) fn note(&mut self, rope: &ropey::Rope, tx: &crate::transaction::Transaction) {
        let Some(c) = tx.changes.iter().min_by_key(|c| c.from) else { return };
        if self.first.is_some_and(|(f, _)| f <= c.from) {
            return;
        }
        let at_line_start = |i: usize| i == 0 || rope.char(i - 1) == '\n';
        let removed_whole_lines = c.text.is_empty()
            && c.to > c.from
            && at_line_start(c.from)
            && (rope.char(c.to - 1) == '\n' || c.to >= rope.len_chars());
        let restored_lines = c.text.matches('\n').count();
        let size = if removed_whole_lines {
            0
        } else if c.from == c.to && c.text.ends_with('\n') && at_line_start(c.from) {
            restored_lines
        } else {
            restored_lines + 1
        };
        self.first = Some((c.from, size));
    }
}

/// An indent as spaces of the same width (tab stops every four columns).
pub(crate) fn spaces_of(indent: &str) -> String {
    let mut w = 0;
    for c in indent.chars() {
        w += if c == '\t' { geom::TAB_WIDTH - w % geom::TAB_WIDTH } else { 1 };
    }
    " ".repeat(w)
}

pub(crate) fn valid_reg(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '"' | '_' | '+' | '*' | '-' | '/' | ':' | '.' | '%' | '#')
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum InsertAt {
    Here,
    After,
    FirstNonBlank,
    LineEnd,
    Col0,
    LastInsert,
}
