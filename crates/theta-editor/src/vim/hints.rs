//! Which-key hints for multi-key prefixes and the space leader menu.

use super::{Op, Pend, VKind};
use crate::editor::{Editor, Effect, KeyHint, Mode};
use crate::helix::FxKind;
use crate::keys::Key;

fn h(k: &str, d: &str) -> KeyHint {
    KeyHint { key: k.to_string(), description: d.to_string() }
}

pub(crate) const OBJECTS: &[(&str, &str)] = &[
    ("w", "word"),
    ("W", "WORD"),
    ("s", "sentence"),
    ("p", "paragraph"),
    ("( ) b", "parentheses"),
    ("[ ]", "square brackets"),
    ("{ } B", "curly braces"),
    ("< >", "angle brackets"),
    ("'", "single quotes"),
    ("\"", "double quotes"),
    ("`", "backticks"),
];

/// The leader menu: (key, description). Mirrors the Helix space menu.
pub(crate) const LEADER: &[(&str, &str)] = &[
    ("f", "go to table"),
    ("b", "switch connection"),
    ("? space", "command palette"),
    ("y", "yank to clipboard (operator)"),
    ("Y", "yank line to clipboard"),
    ("p", "paste clipboard after"),
    ("P", "paste clipboard before"),
    ("r", "run statement / selection"),
    ("R", "run all"),
    ("e", "explain"),
    ("E", "explain analyze"),
    ("k", "completions"),
    ("t", "theme picker"),
    ("c", "toggle line comments"),
    ("x", "cancel running query"),
    ("/", "search tables"),
    ("g", "focus results"),
    ("d", "focus explorer"),
    ("T", "toggle explorer"),
    ("j", "toggle results panel"),
    ("a", "code actions (quick fixes)"),
    ("n", "rename alias / CTE / column alias"),
    ("D", "problems"),
    ("s", "search across tables"),
    ("C", "chart of the result"),
    ("v", "cell viewer"),
    ("V", "copy cell value"),
];

impl Editor {
    pub(crate) fn vim_key_hints(&self) -> Option<(String, Vec<KeyHint>)> {
        if self.mode == Mode::Insert {
            if self.vim.ins.as_ref().is_some_and(|s| s.await_reg) {
                return Some(("Insert register".into(), reg_hints()));
            }
            return None;
        }
        if self.mode == Mode::Replace {
            return None;
        }
        let p: &Pend = &self.vim.pend;
        if p.awaiting_reg {
            return Some(("Register".into(), reg_hints()));
        }
        let visual = self.vim.vis.is_some();
        let tok = p.toks.first().and_then(|k| k.text_char());
        let second = p.toks.get(1).and_then(|k| k.text_char());
        if let Some(c) = tok {
            match (c, p.toks.len()) {
                ('g', 1) => {
                    let mut v = vec![
                        h("g", "file start"),
                        h("e E", "end of previous word"),
                        h("_", "last non-blank"),
                        h("j k", "move down / up"),
                        h("*  #", "search word (no boundaries)"),
                        h("v", "reselect last visual area"),
                        h("i", "insert at last insert"),
                        h("I", "insert at column 0"),
                        h("J", "join without spaces"),
                        h("p P", "paste, cursor after"),
                        h("~ u U ?", "toggle / lower / upper / rot13"),
                        h("c", "toggle SQL comment"),
                    ];
                    if visual {
                        v.retain(|x| x.key != "i" && x.key != "I");
                    }
                    return Some(("Goto".into(), v));
                }
                ('z', 1) => {
                    return Some((
                        "View".into(),
                        vec![
                            h("z .", "center cursor"),
                            h("t", "cursor to top"),
                            h("b -", "cursor to bottom"),
                        ],
                    ));
                }
                ('m', 1) => return Some(("Set mark".into(), vec![h("a-z A-Z", "mark name")])),
                ('\'', 1) => {
                    return Some((
                        "Jump to mark line".into(),
                        vec![
                            h("a-z", "mark"),
                            h("'", "previous position"),
                            h("< >", "visual area"),
                            h(".", "last change"),
                        ],
                    ));
                }
                ('`', 1) => {
                    return Some((
                        "Jump to mark".into(),
                        vec![
                            h("a-z", "mark"),
                            h("`", "previous position"),
                            h("< >", "visual area"),
                            h(".", "last change"),
                        ],
                    ));
                }
                ('[', 1) => return Some(("Previous".into(), vec![h("( {", "unmatched bracket")])),
                (']', 1) => return Some(("Next".into(), vec![h(") }", "unmatched bracket")])),
                ('q', 1) => return Some(("Record macro".into(), vec![h("a-z", "register")])),
                ('@', 1) => {
                    return Some((
                        "Play macro".into(),
                        vec![h("a-z", "register"), h("@", "last macro"), h(":", "last command")],
                    ));
                }
                ('r', 1) => return Some(("Replace".into(), vec![h("<char>", "replace with character")])),
                ('f' | 'F' | 't' | 'T', 1) => {
                    return Some(("Find".into(), vec![h("<char>", "character on this line")]));
                }
                ('Z', 1) => return Some(("Quit".into(), vec![h("Z", "write and close"), h("Q", "close")])),
                (' ', 1) => return Some(("Space".into(), LEADER.iter().map(|(k, d)| h(k, d)).collect())),
                ('i' | 'a', 1) if p.op.is_some() || visual => {
                    let t = if c == 'i' { "Inside" } else { "Around" };
                    return Some((t.into(), OBJECTS.iter().map(|(k, d)| h(k, d)).collect()));
                }
                _ => {}
            }
            let _ = second;
        }
        if let Some(op) = p.op
            && p.toks.is_empty()
        {
            let name = op_name(op);
            return Some((
                format!("{name} (operator pending)"),
                vec![
                    h("i", "inside object"),
                    h("a", "around object"),
                    h(name, "this line"),
                    h("w W b B e E", "words"),
                    h("$ 0 ^", "line end / start"),
                    h("} {", "paragraph"),
                    h("f t F T", "up to a character"),
                    h("%", "matching bracket"),
                    h("v V", "force charwise / linewise"),
                ],
            ));
        }
        let _ = (VKind::Char, Key::Escape);
        None
    }
}

fn op_name(op: Op) -> &'static str {
    op.name()
}

fn reg_hints() -> Vec<KeyHint> {
    vec![
        h("a-z", "named register (A-Z appends)"),
        h("0", "last yank"),
        h("1-9", "delete history"),
        h("-", "small delete"),
        h("_", "black hole"),
        h("+ *", "system clipboard"),
        h("/", "last search"),
        h(".", "last inserted text"),
        h(":", "last command"),
        h("\"", "unnamed"),
    ]
}

impl Editor {
    /// Execute `space <c>`.
    pub(crate) fn leader_key(&mut self, c: char) {
        let visual = self.vim.vis.is_some();
        let fx = |s: &mut Editor, k: FxKind| s.run_fx(k, 1);
        match c {
            'f' | '/' => fx(self, FxKind::TablePicker),
            'b' => fx(self, FxKind::ConnectionPicker),
            '?' | ' ' => fx(self, FxKind::Palette),
            'r' => {
                fx(self, FxKind::Run);
                if visual {
                    self.vis_exit();
                }
            }
            'R' => fx(self, FxKind::RunAll),
            'e' => fx(self, FxKind::Explain),
            'E' => fx(self, FxKind::Analyze),
            'k' => fx(self, FxKind::Complete),
            't' => fx(self, FxKind::Theme),
            'x' => fx(self, FxKind::Cancel),
            'g' => fx(self, FxKind::FocusResults),
            'd' => fx(self, FxKind::FocusExplorer),
            'T' => fx(self, FxKind::ToggleExplorer),
            'a' => fx(self, FxKind::CodeActions),
            'n' => fx(self, FxKind::Rename),
            'D' => fx(self, FxKind::ShowProblems),
            'j' => fx(self, FxKind::ToggleBottomPanel),
            's' => fx(self, FxKind::Host("search")),
            'C' => fx(self, FxKind::Host("chart")),
            'v' => fx(self, FxKind::Host("cell")),
            'V' => fx(self, FxKind::Host("cell copy")),
            'c' => {
                if let Some(r) = self.visual_region() {
                    self.vis_exit();
                    let lines: Vec<usize> = (r.l0..=r.l1).collect();
                    self.toggle_comment_lines(&lines);
                } else {
                    let t = self.rope.slice(..);
                    let l = super::geom::line_of(t, self.vcur());
                    self.toggle_comment_lines(&[l]);
                }
            }
            'y' => {
                if visual {
                    self.visual_operator(Op::Yank, Some('+'), None);
                    self.status("yanked to system clipboard");
                } else {
                    self.vim.pend = Pend {
                        reg: Some('+'),
                        op: Some(Op::Yank),
                        disp: "space y".into(),
                        ..Default::default()
                    };
                }
            }
            'Y' => {
                if visual {
                    self.exec_visual_simple(super::Simple::BigY, Some('+'), None);
                } else {
                    self.run_op_lines(Op::Yank, 1, Some('+'));
                }
                self.status("yanked to system clipboard");
            }
            'p' | 'P' => {
                if visual {
                    self.exec_visual_simple(
                        super::Simple::Put { before: c == 'P', cursor_after: false },
                        Some('+'),
                        None,
                    );
                } else {
                    self.put_cmd(Some('+'), 1, c == 'P', false);
                }
            }
            _ => {}
        }
        let _ = Effect::Quit;
    }
}
