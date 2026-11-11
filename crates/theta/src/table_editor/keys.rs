//! What a key does in the table editor's grids, as a table: key and modifiers in, [`Cmd`] out.
//!
//! The view applies the command; nothing here touches GPUI, so the key map can be tested for every
//! keymap (Helix and Vim share the modal letters, Standard has none) and checked against the
//! application's global bindings (see `tests` below and `keymap/ownership.rs`).

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Move {
        dx: i32,
        dy: i32,
    },
    /// Tab and shift-tab: the next cell, wrapping to the next row.
    Next,
    Prev,
    Top,
    Bottom,
    /// Open the cell for editing (or act on it when it is a checkbox, a cycle or a list).
    Edit,
    /// Space: toggle a checkbox, step a cycle; `-1` steps back.
    Flip(i32),
    AddBelow,
    AddAbove,
    Delete,
    Duplicate,
    MoveRow(i32),
    Undo,
    Redo,
    SectionNext,
    SectionPrev,
    TogglePreview,
    CopySql,
    OpenInConsole,
    Revert,
    /// Pick one of the foreign keys the catalog suggests for this table.
    AddSuggested,
}

/// A key press as the grid sees it: GPUI's key name plus the modifiers that matter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Press<'a> {
    pub key: &'a str,
    pub shift: bool,
    pub alt: bool,
    /// `ctrl` everywhere, `cmd` on macOS: the application's primary modifier.
    pub primary: bool,
}

/// The first half of a two-key command (`dd`, `yy`, `gg`) that is waiting for its second key.
pub type Pending = Option<char>;

/// Interpret `press`. `modal` selects the Helix / Vim letters; `pending` is the half-typed command.
/// Returns the command and what is pending afterwards.
pub fn interpret(press: Press, modal: bool, pending: Pending) -> (Option<Cmd>, Pending) {
    let k = press.key;
    if press.primary && !press.alt {
        return (
            match (k, press.shift) {
                ("z", false) => Some(Cmd::Undo),
                ("z", true) | ("y", _) => Some(Cmd::Redo),
                // Vim's redo; Helix has none, and ctrl-r is free there too.
                ("r", false) if modal => Some(Cmd::Redo),
                ("home", _) => Some(Cmd::Top),
                ("end", _) => Some(Cmd::Bottom),
                _ => None,
            },
            None,
        );
    }
    if press.alt {
        let cmd = match (k, press.primary) {
            ("up", false) => Some(Cmd::MoveRow(-1)),
            ("down", false) => Some(Cmd::MoveRow(1)),
            ("right", false) => Some(Cmd::SectionNext),
            ("left", false) => Some(Cmd::SectionPrev),
            ("p", false) => Some(Cmd::TogglePreview),
            ("c", false) => Some(Cmd::CopySql),
            ("o", false) => Some(Cmd::OpenInConsole),
            ("r", false) => Some(Cmd::Revert),
            ("a", false) => Some(Cmd::AddSuggested),
            _ => None,
        };
        return (cmd, None);
    }
    // Arrow keys, tab and the editing keys work in every keymap.
    let plain = match (k, press.shift) {
        ("up", _) => Some(Cmd::Move { dx: 0, dy: -1 }),
        ("down", _) => Some(Cmd::Move { dx: 0, dy: 1 }),
        ("left", _) => Some(Cmd::Move { dx: -1, dy: 0 }),
        ("right", _) => Some(Cmd::Move { dx: 1, dy: 0 }),
        ("tab", false) => Some(Cmd::Next),
        ("tab", true) => Some(Cmd::Prev),
        ("home", _) => Some(Cmd::Top),
        ("end", _) => Some(Cmd::Bottom),
        ("pageup", _) => Some(Cmd::Move { dx: 0, dy: -10 }),
        ("pagedown", _) => Some(Cmd::Move { dx: 0, dy: 10 }),
        ("enter", _) | ("f2", _) => Some(Cmd::Edit),
        ("space", false) => Some(Cmd::Flip(1)),
        ("space", true) => Some(Cmd::Flip(-1)),
        ("insert", false) => Some(Cmd::AddBelow),
        ("insert", true) => Some(Cmd::AddAbove),
        ("delete", _) => Some(Cmd::Delete),
        _ => None,
    };
    if plain.is_some() {
        return (plain, None);
    }
    if !modal {
        return (None, None);
    }
    // Helix and Vim letters. `d`, `y` and `g` wait for a second key.
    let letter = if k.chars().count() == 1 { k.chars().next() } else { None };
    let Some(mut c) = letter else { return (None, None) };
    if press.shift {
        c = c.to_ascii_uppercase();
    }
    match (pending, c) {
        (Some('d'), 'd') => return (Some(Cmd::Delete), None),
        (Some('y'), 'y') => return (Some(Cmd::Duplicate), None),
        (Some('g'), 'g') => return (Some(Cmd::Top), None),
        (Some(_), _) => return (None, None),
        _ => {}
    }
    match c {
        'd' | 'y' | 'g' => (None, Some(c)),
        'h' => (Some(Cmd::Move { dx: -1, dy: 0 }), None),
        'j' => (Some(Cmd::Move { dx: 0, dy: 1 }), None),
        'k' => (Some(Cmd::Move { dx: 0, dy: -1 }), None),
        'l' => (Some(Cmd::Move { dx: 1, dy: 0 }), None),
        'G' => (Some(Cmd::Bottom), None),
        'i' | 'a' => (Some(Cmd::Edit), None),
        'o' => (Some(Cmd::AddBelow), None),
        'O' => (Some(Cmd::AddAbove), None),
        'u' => (Some(Cmd::Undo), None),
        'U' => (Some(Cmd::Redo), None),
        '[' | '{' => (Some(Cmd::SectionPrev), None),
        ']' | '}' => (Some(Cmd::SectionNext), None),
        _ => (None, None),
    }
}

/// The ctrl/cmd chords the editor handles itself (by key name, without modifiers); the application's global
/// bindings must stay off these while it has focus. See `keymap::scopes`.
pub fn owns_chord(key: &str) -> bool {
    ["z", "y", "r", "home", "end"].contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> Press<'_> {
        Press { key: k, ..Default::default() }
    }
    fn run(keys: &[&str], modal: bool) -> Option<Cmd> {
        let mut pending = None;
        let mut last = None;
        for k in keys {
            let (c, p) = interpret(key(k), modal, pending);
            pending = p;
            last = c;
        }
        last
    }

    #[test]
    fn arrows_tab_and_editing_keys_work_in_every_keymap() {
        for modal in [false, true] {
            assert_eq!(run(&["down"], modal), Some(Cmd::Move { dx: 0, dy: 1 }));
            assert_eq!(run(&["tab"], modal), Some(Cmd::Next));
            assert_eq!(run(&["enter"], modal), Some(Cmd::Edit));
            assert_eq!(run(&["f2"], modal), Some(Cmd::Edit));
            assert_eq!(run(&["space"], modal), Some(Cmd::Flip(1)));
            assert_eq!(run(&["insert"], modal), Some(Cmd::AddBelow));
            assert_eq!(run(&["delete"], modal), Some(Cmd::Delete));
        }
        let shift_tab = Press { key: "tab", shift: true, ..Default::default() };
        assert_eq!(interpret(shift_tab, false, None).0, Some(Cmd::Prev));
        let shift_insert = Press { key: "insert", shift: true, ..Default::default() };
        assert_eq!(interpret(shift_insert, false, None).0, Some(Cmd::AddAbove));
    }

    #[test]
    fn letters_move_the_cursor_only_in_the_modal_keymaps() {
        assert_eq!(run(&["j"], true), Some(Cmd::Move { dx: 0, dy: 1 }));
        assert_eq!(run(&["h"], true), Some(Cmd::Move { dx: -1, dy: 0 }));
        assert_eq!(run(&["j"], false), None);
        assert_eq!(run(&["i"], true), Some(Cmd::Edit));
        assert_eq!(run(&["i"], false), None);
        assert_eq!(run(&["o"], true), Some(Cmd::AddBelow));
        let big_o = Press { key: "o", shift: true, ..Default::default() };
        assert_eq!(interpret(big_o, true, None).0, Some(Cmd::AddAbove));
        assert_eq!(run(&["u"], true), Some(Cmd::Undo));
    }

    #[test]
    fn two_key_commands_need_both_keys_and_forget_a_wrong_second() {
        assert_eq!(run(&["d", "d"], true), Some(Cmd::Delete));
        assert_eq!(run(&["y", "y"], true), Some(Cmd::Duplicate));
        assert_eq!(run(&["g", "g"], true), Some(Cmd::Top));
        assert_eq!(run(&["d"], true), None);
        assert_eq!(run(&["d", "j"], true), None, "a wrong second key cancels the first");
        assert_eq!(run(&["d", "d"], false), None, "Standard has no dd; it has Delete");
        let big_g = Press { key: "g", shift: true, ..Default::default() };
        assert_eq!(interpret(big_g, true, None).0, Some(Cmd::Bottom));
    }

    #[test]
    fn undo_and_redo_chords() {
        let ctrl = |k| Press { key: k, primary: true, ..Default::default() };
        assert_eq!(interpret(ctrl("z"), false, None).0, Some(Cmd::Undo));
        let redo = Press { key: "z", primary: true, shift: true, ..Default::default() };
        assert_eq!(interpret(redo, false, None).0, Some(Cmd::Redo));
        assert_eq!(interpret(ctrl("r"), true, None).0, Some(Cmd::Redo));
        assert_eq!(interpret(ctrl("r"), false, None).0, None, "ctrl-r is Vim's redo, not Standard's");
        assert_eq!(interpret(ctrl("y"), false, None).0, Some(Cmd::Redo));
    }

    #[test]
    fn alt_chords_reorder_and_drive_the_toolbar() {
        let alt = |k| Press { key: k, alt: true, ..Default::default() };
        assert_eq!(interpret(alt("up"), false, None).0, Some(Cmd::MoveRow(-1)));
        assert_eq!(interpret(alt("down"), true, None).0, Some(Cmd::MoveRow(1)));
        assert_eq!(interpret(alt("right"), false, None).0, Some(Cmd::SectionNext));
        assert_eq!(interpret(alt("p"), false, None).0, Some(Cmd::TogglePreview));
        assert_eq!(interpret(alt("c"), false, None).0, Some(Cmd::CopySql));
        assert_eq!(interpret(alt("o"), false, None).0, Some(Cmd::OpenInConsole));
        assert_eq!(interpret(alt("r"), false, None).0, Some(Cmd::Revert));
        assert_eq!(interpret(alt("a"), false, None).0, Some(Cmd::AddSuggested));
        assert_eq!(interpret(alt("x"), false, None).0, None);
    }

    #[test]
    fn the_chords_it_owns_are_exactly_the_ones_it_interprets() {
        for k in ["z", "y", "r", "home", "end"] {
            let press = Press { key: k, primary: true, ..Default::default() };
            let shifted = Press { shift: true, ..press };
            assert!(
                interpret(press, true, None).0.is_some() || interpret(shifted, true, None).0.is_some(),
                "{k} is claimed but does nothing"
            );
            assert!(owns_chord(k));
        }
        // Everything else with the primary modifier stays the application's (ctrl-w closes the tab).
        for k in ["w", "n", "p", "b", "j", "e", "o", "q", "enter", "s"] {
            let press = Press { key: k, primary: true, ..Default::default() };
            assert_eq!(interpret(press, true, None).0, None, "{k}");
            assert!(!owns_chord(k));
        }
    }

    /// The editor handles its chords in a key-down handler, which GPUI runs only after key *bindings*: a
    /// global binding on the same keystroke would swallow it. So no application binding may use one of
    /// them (and none does: Execute is the existing `mod-enter`, Close Tab `mod-w` stays the application's).
    #[test]
    fn no_global_binding_takes_a_chord_the_table_editor_uses() {
        let primary = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        let mut used: Vec<String> =
            ["z", "y", "r", "home", "end", "shift-z"].iter().map(|k| format!("{primary}-{k}")).collect();
        used.extend(
            ["up", "down", "left", "right", "p", "c", "o", "r", "a"].iter().map(|k| format!("alt-{k}")),
        );
        for b in crate::keymap::bindings() {
            let Some(first) = b.keystrokes().first() else { continue };
            let stroke = first.unparse(theta_ui::host_os());
            // Bindings scoped to another view (the explorer's alt-up/alt-down) never meet this editor.
            let scoped = b.predicate().is_some_and(|p| p.to_string().contains("Explorer"));
            if !scoped {
                assert!(!used.contains(&stroke), "{stroke} is bound globally and used by the table editor");
            }
        }
    }
}
