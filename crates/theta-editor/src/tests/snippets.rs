//! Snippet sessions: insertion, Tab / Shift-Tab, linked placeholders, Escape, in all three key maps.

use super::*;
use crate::Snippet;

fn run(keymap: Keymap, before: &str, snippet: &str, keys: &str) -> Editor {
    let mut e = editor_from(keymap, before);
    e.insert_snippet(&Snippet::parse(snippet));
    feed(&mut e, keys);
    e
}

const MAPS: [Keymap; 3] = [Keymap::Helix, Keymap::Vim, Keymap::Standard];

#[test]
fn first_placeholder_is_selected_and_typing_replaces_it() {
    for k in MAPS {
        let e = run(k, "#[|]#", "SELECT ${1:cols} FROM ${2:t}", "id");
        assert_eq!(e.text(), "SELECT id FROM t", "{k:?}");
        assert!(e.snippet_active(), "{k:?}");
    }
}

#[test]
fn tab_walks_the_stops_and_leaves_after_the_last() {
    for k in MAPS {
        let e = run(k, "#[|]#", "SELECT ${1:cols} FROM ${2:t} WHERE $0", "a<tab>b<tab>x");
        assert_eq!(e.text(), "SELECT a FROM b WHERE x", "{k:?}");
        assert!(!e.snippet_active(), "{k:?}: arriving at an empty $0 ends the session");
    }
}

#[test]
fn shift_tab_goes_back() {
    for k in MAPS {
        let e = run(k, "#[|]#", "${1:a} ${2:b}", "<tab><S-tab>X");
        assert_eq!(e.text(), "X b", "{k:?}");
    }
}

#[test]
fn linked_placeholders_are_edited_together() {
    for k in MAPS {
        let e = run(k, "#[|]#", "${1:t} JOIN ${2:u} ON $1.id = $2.id", "orders<tab>items");
        assert_eq!(e.text(), "orders JOIN items ON orders.id = items.id", "{k:?}");
    }
}

#[test]
fn backspace_clears_a_selected_placeholder_without_eating_a_char() {
    // Vim's Normal-mode cursor sits on a character, so the insertion point differs; Insert-mode use is covered above.
    for k in [Keymap::Helix, Keymap::Standard] {
        let e = run(k, "x #[|]#", "a${1:name}b", "<backspace>");
        assert_eq!(e.text(), "x ab", "{k:?}");
    }
}

#[test]
fn escape_leaves_snippet_mode_and_keeps_the_text() {
    for k in MAPS {
        let e = run(k, "#[|]#", "A ${1:b} C", "<esc>");
        assert_eq!(e.text(), "A b C", "{k:?}");
        assert!(!e.snippet_active(), "{k:?}");
    }
}

#[test]
fn moving_the_cursor_away_ends_the_session() {
    for k in [Keymap::Helix, Keymap::Standard] {
        let e = run(k, "#[|]#", "A ${1:b} C", "<left><left><left>");
        assert!(!e.snippet_active(), "{k:?}");
    }
}

#[test]
fn a_snippet_with_no_stops_just_inserts() {
    for k in MAPS {
        let mut e = editor_from(k, "#[|]#");
        e.insert_snippet(&Snippet::parse("plain"));
        assert_eq!(e.text(), "plain", "{k:?}");
        assert!(!e.snippet_active());
    }
}

#[test]
fn replaces_the_selection() {
    let mut e = editor_from(Keymap::Standard, "x #[abc|]# y");
    e.insert_snippet(&Snippet::parse("[$1]"));
    assert_eq!(e.text(), "x [] y");
}

#[test]
fn one_undo_step_removes_the_whole_snippet() {
    let mut e = editor_from(Keymap::Standard, "#[|]#");
    e.insert_snippet(&Snippet::parse("SELECT ${1:a}"));
    feed(&mut e, "zz<esc>");
    e.undo();
    assert_eq!(e.text(), "SELECT a".replace("SELECT a", "").to_string() + "");
}

#[test]
fn multi_line_snippets_follow_the_indent() {
    let mut e = editor_from(Keymap::Standard, "  #[|]#");
    e.insert_snippet(&Snippet::parse("SELECT $1\nFROM ${2:t}").indented("  "));
    assert_eq!(e.text(), "  SELECT \n  FROM t");
}
