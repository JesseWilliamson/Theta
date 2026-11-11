//! Which keys the SQL editor owns, derived by asking the editing engine.
//!
//! GPUI resolves key *bindings* before the focused view's key-down handler runs, so an
//! application binding on `ctrl-w` would silently pre-empt the editor's own `ctrl-w`
//! (delete word back in Helix Insert mode). The rule that prevents this class of bug is
//! "while the SQL editor is focused, its engine's keys for the active keymap and mode win".
//! Instead of keeping a hand-written list of conflicts that rots when the engine gains a
//! key, this module *probes* the engine: for a keystroke and an editor state it reports
//! whether the engine (or the completion popup, or the prompt bar) uses it. `keymap::bind`
//! turns the answer into a context predicate on each application binding, and the headless
//! tests in `keymap::tests` check the result end to end through real GPUI dispatch.

use quarry_editor::{Editor, Effect, KeyInput, Keymap, Mode};
use quarry_editor_view::{KEY_CONTEXT, completion_owns, keymap_name, mode_name};

/// One state of the editor as far as key handling is concerned; mirrors the context keys
/// `quarry_editor_view::key_context` publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorState {
    pub keymap: Keymap,
    pub mode: Mode,
    /// The `:` / `/` prompt bar is open.
    pub prompt: bool,
    /// The completion popup is open.
    pub completion: bool,
    /// Vim only: a command is half typed (`d`, `g`, `"`). Published as the `pending` context.
    pub pending: bool,
}

impl EditorState {
    /// Every state the editor can be in (a prompt only opens outside Insert and Replace mode,
    /// where `:` is text; the popup can be open next to any of them; Vim's half-typed commands
    /// are probed in the modes that have them and exclude a prompt or popup).
    pub fn all() -> Vec<EditorState> {
        let mut out = Vec::new();
        for (keymap, modes) in [
            (Keymap::Helix, &[Mode::Normal, Mode::Insert, Mode::Select][..]),
            (
                Keymap::Vim,
                &[Mode::Normal, Mode::Insert, Mode::Visual, Mode::VisualLine, Mode::VisualBlock, Mode::Replace][..],
            ),
            (Keymap::Standard, &[Mode::Standard][..]),
        ] {
            for &mode in modes {
                let prompt_possible = !matches!(mode, Mode::Insert | Mode::Replace);
                for prompt in [false, true] {
                    if prompt && !prompt_possible {
                        continue;
                    }
                    for completion in [false, true] {
                        out.push(EditorState { keymap, mode, prompt, completion, pending: false });
                    }
                }
                if keymap == Keymap::Vim && prompt_possible {
                    out.push(EditorState { keymap, mode, prompt: false, completion: false, pending: true });
                }
            }
        }
        out
    }

    pub fn describe(&self) -> String {
        format!(
            "{}/{}{}{}{}",
            keymap_name(self.keymap),
            mode_name(self.mode),
            if self.prompt { "+prompt" } else { "" },
            if self.completion { "+completion" } else { "" },
            if self.pending { "+pending" } else { "" }
        )
    }
}

/// Parse a GPUI keystroke (`ctrl-shift-p`, `f5`, `ctrl-pageup`, `mod-,` already expanded)
/// into the engine's key type.
pub fn engine_key(gpui_keystroke: &str) -> Option<KeyInput> {
    let mut parts: Vec<&str> = gpui_keystroke.split('-').collect();
    // A trailing "-" key splits into two empty tails: "ctrl--" -> ["ctrl", "", ""].
    let key = if gpui_keystroke.ends_with("--") || gpui_keystroke == "-" {
        parts.truncate(parts.len().saturating_sub(2));
        "-".to_string()
    } else {
        parts.pop()?.to_string()
    };
    let key = match key.as_str() {
        k if k.len() >= 2 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) => {
            format!("F{}", &k[1..])
        }
        k => k.to_string(),
    };
    let mut spec: Vec<String> = parts
        .into_iter()
        .map(|m| match m {
            "cmd" | "super" | "win" => "cmd".to_string(),
            other => other.to_string(),
        })
        .collect();
    spec.push(key);
    KeyInput::parse(&spec.join("-"))
}

/// Put `ed` into the non-Normal mode of `state` (the keys a user types to get there).
pub fn enter_mode(ed: &mut Editor, keymap: Keymap, mode: Mode) {
    let key = |ed: &mut Editor, k: &str| {
        ed.handle_key(&KeyInput::parse(k).expect("valid key"));
    };
    if ed.mode() == mode {
        return;
    }
    match (keymap, mode) {
        (Keymap::Helix, Mode::Insert) | (Keymap::Vim, Mode::Insert) => key(ed, "i"),
        (Keymap::Helix, Mode::Select) | (Keymap::Vim, Mode::Visual) => key(ed, "v"),
        (Keymap::Vim, Mode::VisualLine) => key(ed, "V"),
        (Keymap::Vim, Mode::VisualBlock) => key(ed, "ctrl-v"),
        (Keymap::Vim, Mode::Replace) => key(ed, "R"),
        _ => {}
    }
}

/// An editor put into `state` (prompt text and history prepared so that every prompt key
/// has a visible effect).
fn editor_in(state: EditorState) -> Editor {
    let mut ed = Editor::new("select 1;\nselect 2;\nselect 3;\n", state.keymap);
    let press = |ed: &mut Editor, s: &str| {
        for k in s.chars() {
            ed.handle_key(&KeyInput::ch(k));
        }
    };
    ed.set_selections(vec![quarry_editor::Selection::point(14)], 0);
    enter_mode(&mut ed, state.keymap, state.mode);
    if state.pending {
        // An operator in Normal mode, `g` (which also starts commands) in the Visual modes.
        press(&mut ed, if state.mode == Mode::Normal { "d" } else { "g" });
    }
    if state.prompt {
        let open = |ed: &mut Editor| match state.keymap {
            Keymap::Helix => press(ed, if state.mode == Mode::Select { "/" } else { ":" }),
            // `:` leaves Visual mode (the prompt opens in Normal mode), `/` keeps it.
            Keymap::Vim => press(ed, if matches!(state.mode, Mode::Normal) { ":" } else { "/" }),
            Keymap::Standard => {
                ed.handle_key(&KeyInput::parse("ctrl-f").expect("valid key"));
            }
        };
        // Two history entries, so up/down have somewhere to go, then a prompt with text and a
        // cursor in the middle of it.
        for entry in ["one", "two"] {
            open(&mut ed);
            press(&mut ed, entry);
            ed.handle_key(&KeyInput::parse("ret").expect("valid key"));
            if state.keymap == Keymap::Helix && state.mode == Mode::Select {
                press(&mut ed, "v");
                press(&mut ed, "v");
            }
            if state.keymap == Keymap::Vim {
                // `<ret>` ended the Visual mode the prompt was opened from.
                enter_mode(&mut ed, state.keymap, state.mode);
            }
        }
        open(&mut ed);
        press(&mut ed, "abc def ghi");
        for _ in 0..4 {
            ed.handle_key(&KeyInput::parse("left").expect("valid key"));
        }
    }
    ed
}

fn prompt_snapshot(ed: &Editor) -> Option<(String, usize)> {
    ed.prompt().map(|p| (p.text.to_string(), p.cursor))
}

/// Plain `ctrl` letters that Vim gives meaning in some mode (window prefix, scrolling, redo,
/// jumplist, increment, visual-block, completion ...). In Vim they belong to the editor in every
/// state, also where the engine has nothing bound (`ctrl-w` is Vim's window prefix and the
/// engine has no windows): a habitual `ctrl-w` must not close the console.
const VIM_RESERVED: [char; 15] = ['w', 'b', 'f', 'd', 'u', 'e', 'y', 'r', 'o', 'i', 'a', 'x', 'v', 'n', 'p'];

fn vim_reserved(state: EditorState, key: &KeyInput) -> bool {
    let m = &key.modifiers;
    state.keymap == Keymap::Vim
        && m.ctrl
        && !m.alt
        && !m.shift
        && !m.platform
        && matches!(key.key, quarry_editor::Key::Char(c) if VIM_RESERVED.contains(&c))
}

/// Does the editor, in `state`, use `key` for something of its own?
pub fn engine_owns(state: EditorState, key: &KeyInput) -> bool {
    if vim_reserved(state, key) {
        return true;
    }
    if state.completion && !state.prompt && completion_owns(key) {
        return true;
    }
    let mut ed = editor_in(state);
    if state.prompt {
        // The prompt bar swallows every key it does not know, so `handled` says nothing
        // there; a key is the prompt's when it visibly changes (or closes) the prompt.
        let before = prompt_snapshot(&ed);
        assert!(before.is_some(), "probe editor must have its prompt open ({})", state.describe());
        let out = ed.handle_key(key);
        return prompt_snapshot(&ed) != before || out.effects.iter().any(is_real_effect);
    }
    let out = ed.handle_key(key);
    out.handled || out.effects.iter().any(is_real_effect)
}

fn is_real_effect(e: &Effect) -> bool {
    !matches!(e, Effect::RevealCursor)
}

/// The states in which the editor owns `key`.
pub fn owning_states(key: &KeyInput) -> Vec<EditorState> {
    EditorState::all().into_iter().filter(|s| engine_owns(*s, key)).collect()
}

/// One implicant of the ownership function: `None` means "any value" for that dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Term {
    keymap: Option<Keymap>,
    mode: Option<Mode>,
    prompt: Option<bool>,
    completion: Option<bool>,
    pending: Option<bool>,
}

impl Term {
    fn covers(&self, s: &EditorState) -> bool {
        self.keymap.is_none_or(|k| k == s.keymap)
            && self.mode.is_none_or(|m| m == s.mode)
            && self.prompt.is_none_or(|p| p == s.prompt)
            && self.completion.is_none_or(|c| c == s.completion)
            && self.pending.is_none_or(|p| p == s.pending)
    }

    fn generality(&self) -> usize {
        [
            self.keymap.is_none(),
            self.mode.is_none(),
            self.prompt.is_none(),
            self.completion.is_none(),
            self.pending.is_none(),
        ]
        .iter()
            .filter(|b| **b)
            .count()
    }

    fn predicate(&self) -> String {
        let mut parts = Vec::new();
        if let Some(k) = self.keymap {
            parts.push(format!("keymap == {}", keymap_name(k)));
        }
        if let Some(m) = self.mode {
            parts.push(format!("mode == {}", mode_name(m)));
        }
        match self.prompt {
            Some(true) => parts.push("prompt".into()),
            Some(false) => parts.push("!prompt".into()),
            None => {}
        }
        match self.completion {
            Some(true) => parts.push("completion".into()),
            Some(false) => parts.push("!completion".into()),
            None => {}
        }
        match self.pending {
            Some(true) => parts.push("pending".into()),
            Some(false) => parts.push("!pending".into()),
            None => {}
        }
        parts.join(" && ")
    }
}

/// A context predicate that is true when the editor is focused and in one of `states`
/// (`None` for no states). Parenthesised, so it can be negated.
///
/// The states are covered greedily with the most general terms that never cover a state
/// the engine does *not* own, which keeps the predicates short (`QuarryEditor` when the
/// editor owns the key everywhere) and readable in `keymap::bind`'s output.
pub fn owned_predicate(states: &[EditorState]) -> Option<String> {
    if states.is_empty() {
        return None;
    }
    let all = EditorState::all();
    let mut candidates = Vec::new();
    let modes = [
        None,
        Some(Mode::Normal),
        Some(Mode::Insert),
        Some(Mode::Select),
        Some(Mode::Standard),
        Some(Mode::Visual),
        Some(Mode::VisualLine),
        Some(Mode::VisualBlock),
        Some(Mode::Replace),
    ];
    for keymap in [None, Some(Keymap::Helix), Some(Keymap::Vim), Some(Keymap::Standard)] {
        for mode in modes {
            for prompt in [None, Some(false), Some(true)] {
                for completion in [None, Some(false), Some(true)] {
                    for pending in [None, Some(false), Some(true)] {
                        let t = Term { keymap, mode, prompt, completion, pending };
                        let covered: Vec<&EditorState> = all.iter().filter(|s| t.covers(s)).collect();
                        if !covered.is_empty() && covered.iter().all(|s| states.contains(s)) {
                            candidates.push(t);
                        }
                    }
                }
            }
        }
    }
    let mut left: Vec<EditorState> = states.to_vec();
    let mut chosen: Vec<Term> = Vec::new();
    while !left.is_empty() {
        let best = candidates
            .iter()
            .max_by_key(|t| (left.iter().filter(|s| t.covers(s)).count(), t.generality()))
            .copied()
            .expect("every owned state is covered by its own exact term");
        left.retain(|s| !best.covers(s));
        chosen.push(best);
    }
    if chosen.iter().any(|t| t.generality() == 5) {
        return Some(KEY_CONTEXT.to_string());
    }
    let terms: Vec<String> = chosen.iter().map(|t| format!("({})", t.predicate())).collect();
    Some(format!("{KEY_CONTEXT} && ({})", terms.join(" || ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystroke_parsing() {
        assert_eq!(engine_key("ctrl-shift-p"), KeyInput::parse("ctrl-shift-p"));
        assert_eq!(engine_key("f5"), KeyInput::parse("F5"));
        assert_eq!(engine_key("ctrl--"), KeyInput::parse("ctrl--"));
        assert_eq!(engine_key("ctrl-pageup"), KeyInput::parse("ctrl-pageup"));
        assert_eq!(engine_key("cmd-,"), KeyInput::parse("cmd-,"));
        assert!(engine_key("ctrl-w").is_some());
    }

    #[test]
    fn vim_owns_its_control_keys_in_every_state() {
        let vim: Vec<EditorState> = EditorState::all().into_iter().filter(|s| s.keymap == Keymap::Vim).collect();
        assert!(vim.len() >= 20, "all the Vim modes, prompt, popup and pending states: {}", vim.len());
        for k in VIM_RESERVED {
            let key = engine_key(&format!("ctrl-{k}")).unwrap();
            for s in &vim {
                assert!(engine_owns(*s, &key), "ctrl-{k} must belong to the editor in {}", s.describe());
            }
        }
    }

    #[test]
    fn vim_does_not_swallow_ctrl_shift_chords_of_the_application() {
        // The engine ignores shift on ctrl-letters (ctrl-E == ctrl-e); the engine must not take
        // ctrl-shift-f for ctrl-f, or Format Document would die in Vim Normal mode.
        for chord in ["ctrl-shift-f", "ctrl-shift-e", "ctrl-shift-n", "ctrl-shift-t", "ctrl-shift-p"] {
            let key = engine_key(chord).unwrap();
            for s in EditorState::all().into_iter().filter(|s| s.keymap == Keymap::Vim && !s.pending) {
                assert!(!engine_owns(s, &key), "{chord} is the application's in {}", s.describe());
            }
        }
    }

    #[test]
    fn known_conflicts_are_found() {
        let key = engine_key("ctrl-w").unwrap();
        let s = |keymap, mode, prompt, completion| EditorState { keymap, mode, prompt, completion, pending: false };
        assert!(engine_owns(s(Keymap::Helix, Mode::Insert, false, false), &key));
        assert!(engine_owns(s(Keymap::Helix, Mode::Normal, true, false), &key));
        assert!(engine_owns(s(Keymap::Standard, Mode::Standard, false, false), &key));
        assert!(!engine_owns(s(Keymap::Helix, Mode::Normal, false, false), &key));
        let n = engine_key("ctrl-n").unwrap();
        assert!(engine_owns(s(Keymap::Helix, Mode::Insert, false, true), &n), "popup owns ctrl-n");
        assert!(!engine_owns(s(Keymap::Helix, Mode::Insert, false, false), &n));
    }
}

