use super::*;

#[test]
fn text_access_api() {
    let e = Editor::new("héllo\nwörld\n", Keymap::Helix);
    assert_eq!(e.text(), "héllo\nwörld\n");
    assert_eq!(e.len_chars(), 12);
    assert_eq!(e.line_count(), 3);
    assert_eq!(e.line(0), "héllo");
    assert_eq!(e.line(1), "wörld");
    assert_eq!(e.line(2), "");
    assert_eq!(e.line(99), "");
    assert_eq!(e.line_to_char(1), 6);
    assert_eq!(e.char_to_line(7), 1);
    assert_eq!(e.char_to_byte(2), 3);
    assert_eq!(e.byte_to_char(3), 2);
    assert_eq!(e.slice(1..4), "éll");
    assert_eq!(e.slice(5..999), "\nwörld\n");
    assert_eq!(e.slice(4..2), "");
}

#[test]
fn crlf_lines() {
    let e = Editor::new("a\r\nb\r\n", Keymap::Helix);
    assert_eq!(e.line_count(), 3);
    assert_eq!(e.line(0), "a");
    assert_eq!(e.line(1), "b");
    assert_eq!(e.line_to_char(1), 3);
}

#[test]
fn empty_document_never_panics() {
    for keymap in [Keymap::Helix, Keymap::Standard] {
        let mut e = Editor::new("", keymap);
        assert_eq!(e.line_count(), 1);
        assert_eq!(e.line(0), "");
        for keys in [
            "h", "l", "j", "k", "w", "b", "e", "x", "d", "c<esc>", "p", "P", "y", "%", "gg", "ge", "G", "J", "u", "U", "o<esc>",
            "O<esc>", "<C-a>", "<C-x>", "~", "r1", ">", "<lt>", "mm", "ms(", "mi(", "n", "N", "*", ";", "<A-;>", "C", "<A-s>", "_", "&",
            "<left>", "<right>", "<up>", "<down>", "<home>", "<end>", "<backspace>", "<del>", "<C-a>", "<C-c>", "<C-x>", "<C-v>",
            "<C-z>", "<C-d>", "<C-l>", "<A-up>", "<A-down>", "<C-S-d>", "<C-S-k>", "<tab>", "<S-tab>", "<C-/>",
        ] {
            feed(&mut e, keys);
            let len = e.len_chars();
            for s in e.selections() {
                assert!(s.anchor <= len && s.head <= len, "{keymap:?} {keys}");
            }
            let _ = state(&e);
        }
    }
}

#[test]
fn missing_final_newline_everywhere() {
    for keys in ["l", "j", "w", "e", "b", "x", "d", "gl", "ge", "G", "A", "a", "o<esc>", "O<esc>", "J", "p", "%", "mm"] {
        let mut e = Editor::new("abc", Keymap::Helix);
        feed(&mut e, keys);
        let mut e = Editor::new("abc\ndef", Keymap::Helix);
        feed(&mut e, &format!("j{keys}"));
        feed(&mut e, "<esc>");
        for s in e.selections() {
            assert!(s.anchor <= e.len_chars() && s.head <= e.len_chars());
        }
    }
}

#[test]
fn revision_tracks_text_changes_only() {
    let mut e = Editor::new("abc", Keymap::Helix);
    let r0 = e.revision();
    feed(&mut e, "lll");
    assert_eq!(e.revision(), r0);
    feed(&mut e, "d");
    assert!(e.revision() > r0);
    let r1 = e.revision();
    feed(&mut e, "u");
    assert!(e.revision() > r1, "undo changes the text");
    let r2 = e.revision();
    e.replace_range(0..1, "x");
    assert!(e.revision() > r2);
}

#[test]
fn modified_flag_follows_undo_history() {
    let mut e = Editor::new("abc", Keymap::Helix);
    assert!(!e.is_modified());
    feed(&mut e, "d");
    assert!(e.is_modified());
    feed(&mut e, "u");
    assert!(!e.is_modified(), "undoing back to the saved state clears the flag");
    feed(&mut e, "d");
    e.mark_saved();
    assert!(!e.is_modified());
    feed(&mut e, "u");
    assert!(e.is_modified());
    feed(&mut e, "U");
    assert!(!e.is_modified());
    // mid insert-session edits count as modified
    feed(&mut e, "ix");
    assert!(e.is_modified());
}

#[test]
fn replace_range_is_one_undo_step_and_maps_selections() {
    let mut e = editor_from(Keymap::Helix, "ab#[c|]#def\n");
    e.replace_range(0..1, "XXX");
    assert_eq!(state(&e), "XXXb#[c|]#def\n");
    e.replace_range(0..0, "->");
    assert_eq!(state(&e), "->XXXb#[c|]#def\n");
    assert!(e.undo());
    assert_eq!(e.text(), "XXXbcdef\n");
    assert!(e.undo());
    assert_eq!(e.text(), "abcdef\n");
    assert!(!e.undo());
    assert!(e.redo());
    assert!(e.redo());
    assert_eq!(e.text(), "->XXXbcdef\n");
    assert!(!e.redo());
    e.replace_range(2..2000, "!");
    assert_eq!(e.text(), "->!");
}

#[test]
fn set_text_diffs_so_cursors_stay_put() {
    let mut e = editor_from(Keymap::Standard, "select  1 from   t;\n#[|]#select 2");
    e.set_text("select 1 from t;\nselect 2");
    assert_eq!(e.text(), "select 1 from t;\nselect 2");
    assert_eq!(e.primary().head, 17);
    assert!(e.undo());
    assert_eq!(e.text(), "select  1 from   t;\nselect 2");
    assert!(!e.undo(), "formatting is a single undo step");
    // identical text is a no-op
    let r = e.revision();
    let t = e.text();
    e.set_text(&t);
    assert_eq!(e.revision(), r);
}

#[test]
fn set_text_handles_unicode_and_empties() {
    let mut e = Editor::new("héllo wörld", Keymap::Standard);
    e.set_text("héllo, wörld");
    assert_eq!(e.text(), "héllo, wörld");
    e.set_text("");
    assert_eq!(e.text(), "");
    e.set_text("🙂");
    assert_eq!(e.text(), "🙂");
    assert!(e.undo() && e.undo() && e.undo());
    assert_eq!(e.text(), "héllo wörld");
}

#[test]
fn selected_text_rules() {
    let e = editor_from(Keymap::Helix, "#[a|]#bc");
    assert_eq!(e.selected_text(), None, "one char is a bare cursor in Helix");
    let e = editor_from(Keymap::Helix, "#[ab|]#c");
    assert_eq!(e.selected_text(), Some("ab".into()));
    let e = editor_from(Keymap::Helix, "#[e\u{0301}|]#x");
    assert_eq!(e.selected_text(), None, "a combining sequence is one grapheme");
    let e = editor_from(Keymap::Standard, "#[a|]#bc");
    assert_eq!(e.selected_text(), Some("a".into()));
    let e = editor_from(Keymap::Standard, "a#[|]#bc");
    assert_eq!(e.selected_text(), None);
}

#[test]
fn selection_api_helpers() {
    let s = Selection { anchor: 2, head: 5 };
    assert_eq!(s.range(), 2..5);
    assert_eq!(s.cursor(), 4);
    assert!(!s.is_empty());
    let b = Selection { anchor: 5, head: 2 };
    assert_eq!(b.range(), 2..5);
    assert_eq!(b.cursor(), 2);
    assert!(Selection { anchor: 3, head: 3 }.is_empty());
}

#[test]
fn set_selections_normalizes() {
    let mut e = Editor::new("hello world", Keymap::Helix);
    e.set_selections(vec![Selection { anchor: 6, head: 8 }, Selection { anchor: 0, head: 3 }, Selection { anchor: 2, head: 4 }], 0);
    assert_eq!(e.selections().len(), 2);
    assert_eq!(e.selections()[0].range(), 0..4);
    assert_eq!(e.selections()[1].range(), 6..8);
    assert_eq!(e.primary_index(), 1);
    e.set_selections(vec![Selection { anchor: 99, head: 120 }], 7);
    assert_eq!(e.primary().range(), 11..11);
    e.set_selections(vec![], 0);
    assert_eq!(e.selections().len(), 1);
}

#[test]
fn unicode_graphemes_in_motion_and_delete() {
    // combining mark: one grapheme, two chars
    hx("#[e|]#\u{0301}x\n", "l", "e\u{0301}#[x|]#\n");
    hx("#[e\u{0301}|]#x\n", "d", "#[x|]#\n");
    // emoji ZWJ sequence
    let fam = "👨\u{200d}👩\u{200d}👧";
    hx(&format!("#[{fam}|]#x\n"), "l", &format!("{fam}#[x|]#\n"));
    hx(&format!("{fam}#[x|]#\n"), "h", &format!("#[{fam}|]#x\n"));
    // CJK are ordinary chars
    hx("#[日|]#本語\n", "l", "日#[本|]#語\n");
    hx("#[日|]#本語\n", "w", "#[日本語|]#\n");
    // backspace removes the whole cluster
    let mut e = editor_from(Keymap::Standard, "xe\u{0301}#[|]#");
    feed(&mut e, "<backspace>");
    assert_eq!(e.text(), "x");
    let mut e = editor_from(Keymap::Standard, &format!("a#[|]#{fam}b"));
    feed(&mut e, "<del>");
    assert_eq!(e.text(), "ab");
    feed(&mut e, "<right><left>");
    assert_eq!(e.primary().head, 1);
    // standard arrows skip clusters
    let mut e = editor_from(Keymap::Standard, "#[|]#e\u{0301}x");
    feed(&mut e, "<right>");
    assert_eq!(e.primary().head, 2);
}

#[test]
fn unicode_search_and_chars_vs_bytes() {
    let mut e = editor_from(Keymap::Helix, "#[é|]#é日本 日本\n");
    feed(&mut e, "/日本<ret>");
    assert_eq!(e.primary().range(), 2..4);
    feed(&mut e, "n");
    assert_eq!(e.primary().range(), 5..7);
    assert_eq!(e.char_to_byte(2), 4);
    assert_eq!(e.byte_to_char(4), 2);
}

#[test]
fn matching_bracket_highlight_api() {
    let e = editor_from(Keymap::Helix, "f#[(|]#a, (b))");
    assert_eq!(e.matching_bracket(), Some((1, 8)));
    let e = editor_from(Keymap::Helix, "f(a, (b)#[)|]#");
    assert_eq!(e.matching_bracket(), Some((8, 1)));
    let e = editor_from(Keymap::Standard, "f(a)#[|]#");
    assert_eq!(e.matching_bracket(), Some((3, 1)));
    let e = editor_from(Keymap::Helix, "#[a|]# b");
    assert_eq!(e.matching_bracket(), None);
}

#[test]
fn mouse_click_drag_and_multi() {
    let mut e = Editor::new("hello world\nsecond line\n", Keymap::Helix);
    e.click(7, false, false);
    assert_eq!(e.primary().range(), 7..8);
    e.drag_to(10);
    assert_eq!(e.primary().range(), 7..11);
    e.drag_to(3);
    assert_eq!(e.primary().range(), 3..8);
    e.click(0, false, true);
    assert_eq!(e.selections().len(), 2);
    assert_eq!(e.primary().range(), 0..1);
    e.click(14, true, false);
    assert_eq!(e.selections().len(), 1, "shift-click extended the new selection over the old one");
    e.double_click(8);
    assert_eq!(state(&e).matches("#[").count(), 1);
    assert_eq!(e.primary().range(), 6..11);
    e.triple_click(14);
    assert_eq!(e.primary().range(), 12..24);
}

#[test]
fn mouse_in_standard() {
    let mut e = Editor::new("hello world", Keymap::Standard);
    e.click(3, false, false);
    assert_eq!(state(&e), "hel#[|]#lo world");
    e.drag_to(8);
    assert_eq!(state(&e), "hel#[lo wo|]#rld");
    e.click(0, true, false);
    assert_eq!(e.primary().range(), 0..3, "shift-click extends from the original anchor");
    e.double_click(1);
    assert_eq!(state(&e), "#[hello|]# world");
    e.click(2, false, false);
    e.click(9, false, true);
    assert_eq!(e.selections().len(), 2);
    e.click(100, false, false);
    assert_eq!(e.primary().head, 11);
    // clicking leaves Helix insert mode like Esc would
    let mut h = Editor::new("abc", Keymap::Helix);
    feed(&mut h, "i");
    h.click(2, false, false);
    assert_eq!(h.mode(), Mode::Normal);
}

#[test]
fn undo_restores_selections() {
    let mut e = editor_from(Keymap::Helix, "#[ab|]#c #(de|)#f\n");
    feed(&mut e, "d");
    assert_eq!(e.text(), "c f\n");
    feed(&mut e, "u");
    assert_eq!(state(&e), "#[ab|]#c #(de|)#f\n");
    feed(&mut e, "U");
    assert_eq!(e.text(), "c f\n");
}

#[test]
fn keymap_switch_resets_pending_state() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    feed(&mut e, "2g");
    e.set_keymap(Keymap::Helix);
    assert_eq!(e.pending_keys(), "");
    feed(&mut e, "i");
    assert_eq!(e.mode(), Mode::Insert);
    e.set_keymap(Keymap::Helix);
    assert_eq!(e.mode(), Mode::Normal);
}

#[test]
fn reveal_cursor_effect_follows_movement() {
    let mut e = Editor::new("a\nb\n", Keymap::Helix);
    let out = e.handle_key(&KeyInput::parse("j").unwrap());
    assert!(out.effects.contains(&Effect::RevealCursor));
    let out = e.handle_key(&KeyInput::parse("F9").unwrap());
    assert!(!out.effects.contains(&Effect::RevealCursor));
}

#[test]
fn key_parse() {
    let k = KeyInput::parse("ctrl-d").unwrap();
    assert!(k.modifiers.ctrl && k.key == Key::Char('d'));
    let k = KeyInput::parse("alt-;").unwrap();
    assert!(k.modifiers.alt && k.key == Key::Char(';'));
    assert_eq!(KeyInput::parse("A").unwrap().key, Key::Char('A'));
    assert_eq!(KeyInput::parse("space").unwrap().key, Key::Char(' '));
    assert_eq!(KeyInput::parse("ret").unwrap().key, Key::Enter);
    assert_eq!(KeyInput::parse("esc").unwrap().key, Key::Escape);
    let k = KeyInput::parse("cmd-shift-p").unwrap();
    assert!(k.modifiers.platform && k.key == Key::Char('P') && !k.modifiers.shift);
    let k = KeyInput::parse("shift-tab").unwrap();
    assert!(k.modifiers.shift && k.key == Key::Tab);
    assert_eq!(KeyInput::parse("F5").unwrap().key, Key::F(5));
    assert_eq!(KeyInput::parse("ctrl--").unwrap().key, Key::Char('-'));
    assert_eq!(KeyInput::parse("-").unwrap().key, Key::Char('-'));
    assert!(KeyInput::parse("C-x").unwrap().modifiers.ctrl);
    assert!(KeyInput::parse("nonsense-key").is_none());
    assert!(KeyInput::parse("").is_none());
    // the Shift flag on a character is folded into the character itself
    let a = KeyInput { key: Key::Char('a'), modifiers: crate::Modifiers { shift: true, ..Default::default() } };
    assert_eq!(a.canon().key, Key::Char('A'));
}

#[test]
fn host_can_send_shifted_chars_with_shift_flag() {
    let mut e = Editor::new("a", Keymap::Helix);
    let k = KeyInput { key: Key::Char('A'), modifiers: crate::Modifiers { shift: true, ..Default::default() } };
    e.handle_key(&k);
    assert_eq!(e.mode(), Mode::Insert, "A appends at line end");
}
