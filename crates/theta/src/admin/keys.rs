//! What a key does in the Sessions and Roles tabs, as a table: key and modifiers in, [`Cmd`] out.
//!
//! Both tabs are lists with a filter box, so they share one table. Plain keys do the work in every keymap
//! (the tabs have no text to edit while the list has focus); Helix and Vim add `j`/`k`/`gg`/`G` on top of the
//! arrows. The view applies the command; nothing here touches GPUI, so the table is tested for all three
//! keymaps and against the application's global bindings (see `tests`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Move(i32),
    Top,
    Bottom,
    /// Enter: Sessions toggles the detail pane, Roles opens Alter.
    Enter,
    /// `/`: put the keyboard in the filter box.
    Filter,
    /// Escape: clear the filter, or leave it.
    Clear,
    Refresh,
    Pause,
    /// Step the refresh interval.
    Interval,
    SortNext,
    SortReverse,
    // Sessions
    CancelQuery,
    Terminate,
    Copy,
    OpenInConsole,
    // Roles
    New,
    Alter,
    Password,
    Membership,
    Grant,
    Revoke,
    Drop,
    ToggleSystem,
    PaneNext,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Press<'a> {
    pub key: &'a str,
    pub shift: bool,
    pub alt: bool,
    pub primary: bool,
}

pub type Pending = Option<char>;

/// Interpret `press`; `pending` is the first `g` of `gg`.
pub fn interpret(press: Press, modal: bool, pending: Pending) -> (Option<Cmd>, Pending) {
    // Chords belong to the application (ctrl-w closes the tab, ctrl-enter runs): nothing here uses them.
    if press.primary || press.alt {
        return (None, None);
    }
    let k = press.key;
    let named = match (k, press.shift) {
        ("up", _) => Some(Cmd::Move(-1)),
        ("down", _) => Some(Cmd::Move(1)),
        ("pageup", _) => Some(Cmd::Move(-10)),
        ("pagedown", _) => Some(Cmd::Move(10)),
        ("home", _) => Some(Cmd::Top),
        ("end", _) => Some(Cmd::Bottom),
        ("enter", _) => Some(Cmd::Enter),
        ("escape", _) => Some(Cmd::Clear),
        ("delete", _) => Some(Cmd::Drop),
        ("tab", _) => Some(Cmd::PaneNext),
        _ => None,
    };
    if named.is_some() {
        return (named, None);
    }
    let Some(mut c) = (if k.chars().count() == 1 { k.chars().next() } else { None }) else {
        return (None, None);
    };
    if press.shift {
        c = c.to_ascii_uppercase();
    }
    if modal {
        match (pending, c) {
            (Some('g'), 'g') => return (Some(Cmd::Top), None),
            (Some(_), _) => return (None, None),
            (None, 'g') => return (None, Some('g')),
            (None, 'j') => return (Some(Cmd::Move(1)), None),
            (None, 'k') => return (Some(Cmd::Move(-1)), None),
            (None, 'G') => return (Some(Cmd::Bottom), None),
            _ => {}
        }
    }
    let cmd = match c {
        '/' => Cmd::Filter,
        'r' => Cmd::Refresh,
        'p' => Cmd::Pause,
        'i' => Cmd::Interval,
        's' => Cmd::SortNext,
        'S' => Cmd::SortReverse,
        'c' => Cmd::CancelQuery,
        'x' => Cmd::Terminate,
        'y' => Cmd::Copy,
        'o' => Cmd::OpenInConsole,
        'n' => Cmd::New,
        'e' => Cmd::Alter,
        'w' => Cmd::Password,
        'm' => Cmd::Membership,
        'a' => Cmd::Grant,
        'v' => Cmd::Revoke,
        'D' => Cmd::Drop,
        't' => Cmd::ToggleSystem,
        _ => return (None, None),
    };
    (Some(cmd), None)
}

/// Every plain key the tabs use, for the key reference and the collision test.
pub const PLAIN_KEYS: &[&str] = &[
    "up", "down", "pageup", "pagedown", "home", "end", "enter", "escape", "delete", "tab", "/", "r", "p",
    "i", "s", "c", "x", "y", "o", "n", "e", "w", "m", "a", "v", "j", "k", "g",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> Press<'_> {
        Press { key: k, ..Default::default() }
    }

    fn run(keys: &[&str], modal: bool) -> Option<Cmd> {
        let (mut pending, mut last) = (None, None);
        for k in keys {
            let (c, p) = interpret(key(k), modal, pending);
            pending = p;
            last = c;
        }
        last
    }

    #[test]
    fn the_same_plain_keys_work_in_every_keymap() {
        for modal in [false, true] {
            assert_eq!(run(&["down"], modal), Some(Cmd::Move(1)));
            assert_eq!(run(&["/"], modal), Some(Cmd::Filter));
            assert_eq!(run(&["r"], modal), Some(Cmd::Refresh));
            assert_eq!(run(&["p"], modal), Some(Cmd::Pause));
            assert_eq!(run(&["c"], modal), Some(Cmd::CancelQuery));
            assert_eq!(run(&["x"], modal), Some(Cmd::Terminate));
            assert_eq!(run(&["o"], modal), Some(Cmd::OpenInConsole));
            assert_eq!(run(&["enter"], modal), Some(Cmd::Enter));
            assert_eq!(run(&["escape"], modal), Some(Cmd::Clear));
            assert_eq!(run(&["delete"], modal), Some(Cmd::Drop));
            assert_eq!(run(&["n"], modal), Some(Cmd::New));
            assert_eq!(run(&["a"], modal), Some(Cmd::Grant));
        }
        let shift_s = Press { key: "s", shift: true, ..Default::default() };
        assert_eq!(interpret(shift_s, false, None).0, Some(Cmd::SortReverse));
        let shift_d = Press { key: "d", shift: true, ..Default::default() };
        assert_eq!(interpret(shift_d, false, None).0, Some(Cmd::Drop));
    }

    #[test]
    fn vim_motions_only_in_the_modal_keymaps() {
        assert_eq!(run(&["j"], true), Some(Cmd::Move(1)));
        assert_eq!(run(&["k"], true), Some(Cmd::Move(-1)));
        assert_eq!(run(&["g", "g"], true), Some(Cmd::Top));
        let big_g = Press { key: "g", shift: true, ..Default::default() };
        assert_eq!(interpret(big_g, true, None).0, Some(Cmd::Bottom));
        assert_eq!(run(&["j"], false), None);
        assert_eq!(run(&["g", "g"], false), None);
        assert_eq!(run(&["g", "x"], true), None, "a wrong second key cancels the g");
    }

    #[test]
    fn chords_are_left_to_the_application() {
        for k in ["w", "r", "c", "o", "n", "p", "enter", "tab"] {
            for (alt, primary) in [(true, false), (false, true)] {
                let press = Press { key: k, alt, primary, ..Default::default() };
                assert_eq!(interpret(press, true, None).0, None, "{k}");
            }
        }
    }

    /// The tabs read plain keys in a key-down handler, which GPUI runs after key *bindings*; a global binding
    /// on one of them would take it first. None does today, and this keeps it so.
    #[test]
    fn no_global_binding_takes_a_plain_key_used_here() {
        for b in crate::keymap::bindings() {
            let Some(first) = b.keystrokes().first() else { continue };
            let stroke = first.unparse(theta_ui::host_os());
            let scoped = b.predicate().is_some_and(|p| {
                let s = p.to_string();
                s.contains("Explorer") || s.contains("ThetaEditor") || s.contains("ResultGrid")
            });
            if !scoped {
                assert!(
                    !PLAIN_KEYS.contains(&stroke.as_str()),
                    "{stroke} is bound globally and used by the admin tabs"
                );
            }
        }
    }
}
