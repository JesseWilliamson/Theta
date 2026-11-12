use super::*;

#[test]
fn typing() {
    st("#[|]#", "abc", "abc#[|]#");
    st("a#[|]#c", "b", "ab#[|]#c");
    st("#[ab|]#c", "x", "x#[|]#c");
    st("#[a|]#b", "x", "x#[|]#b");
    st("a#[|]#\nb#(|)#", "x", "ax#[|]#\nbx#(|)#");
    assert_eq!(effects_of(Keymap::Standard, "#[|]#", "a").len(), 0);
}

#[test]
fn left_right_with_selection_collapse() {
    st("a#[|]#bc", "<right>", "ab#[|]#c");
    st("a#[|]#bc", "<left>", "#[|]#abc");
    st("#[ab|]#c", "<left>", "#[|]#abc");
    st("#[ab|]#c", "<right>", "ab#[|]#c");
    st("#[|ab]#c", "<right>", "ab#[|]#c");
    st("a#[|]#bc", "<S-right>", "a#[b|]#c");
    st("a#[|]#bc", "<S-left>", "#[|a]#bc");
    st("a#[|]#bc", "<S-right><S-right>", "a#[bc|]#");
    st("a#[b|]#c", "<S-left>", "a#[|]#bc");
    st("#[|]#abc", "<left>", "#[|]#abc");
    st("abc#[|]#", "<right>", "abc#[|]#");
}

#[test]
fn up_down_keep_column() {
    st("ab#[|]#c\nd\nefgh", "<down>", "abc\nd#[|]#\nefgh");
    st("ab#[|]#c\nd\nefgh", "<down><down>", "abc\nd\nef#[|]#gh");
    st("abc\nd\nef#[|]#gh", "<up><up>", "ab#[|]#c\nd\nefgh");
    st("ab#[|]#c", "<up>", "#[|]#abc");
    st("ab#[|]#c", "<down>", "abc#[|]#");
    st("ab#[|]#c\ndef", "<S-down>", "ab#[c\nde|]#f");
    st("ab#[|]#c\ndef", "<S-up>", "#[|ab]#c\ndef");
}

#[test]
fn smart_home_and_end() {
    st("  ab#[|]#c", "<home>", "  #[|]#abc");
    st("  #[|]#abc", "<home>", "#[|]#  abc");
    st("#[|]#  abc", "<home>", "  #[|]#abc");
    st("  ab#[|]#c\n", "<end>", "  abc#[|]#\n");
    st("  ab#[|]#c", "<S-home>", "  #[|ab]#c");
    st("  ab#[|]#c", "<S-end>", "  ab#[c|]#");
    st("ab#[|]#c\nd", "<C-home>", "#[|]#abc\nd");
    st("ab#[|]#c\nd", "<C-end>", "abc\nd#[|]#");
    st("ab#[|]#c\nd", "<C-S-end>", "ab#[c\nd|]#");
    st("   #[|]#   \n", "<home>", "#[|]#      \n");
}

#[test]
fn word_motions() {
    st("#[|]#foo bar baz", "<C-right>", "foo#[|]# bar baz");
    st("foo#[|]# bar baz", "<C-right>", "foo bar#[|]# baz");
    st("foo bar#[|]# baz", "<C-left>", "foo #[|]#bar baz");
    st("foo #[|]#bar baz", "<C-left>", "#[|]#foo bar baz");
    st("#[|]#foo.bar", "<C-right>", "foo#[|]#.bar");
    st("foo#[|]#.bar", "<C-right>", "foo.#[|]#bar");
    st("#[|]#foo bar", "<C-S-right>", "#[foo|]# bar");
    st("foo#[|]#\nbar", "<C-right>", "foo\n#[|]#bar");
    st("foo\n#[|]#bar", "<C-left>", "foo#[|]#\nbar");
    st("#[|]#foo bar", "<A-right>", "foo#[|]# bar");
}

#[test]
fn page_keys_use_viewport() {
    let text: String = (0..100).map(|i| format!("{i}\n")).collect();
    let mut e = Editor::new(&text, Keymap::Standard);
    e.set_viewport(0, 25);
    feed(&mut e, "<pagedown>");
    assert_eq!(e.char_to_line(e.primary().head), 25);
    feed(&mut e, "<pageup>");
    assert_eq!(e.char_to_line(e.primary().head), 0);
    feed(&mut e, "<S-pagedown>");
    assert_eq!(e.primary().anchor, 0);
    assert_eq!(e.char_to_line(e.primary().head), 25);
}

#[test]
fn select_all_and_clipboard() {
    st("a#[|]#b\nc", "<C-a>", "#[ab\nc|]#");
    st("a#[|]#b\nc", "<cmd-a>", "#[ab\nc|]#");
    let mut e = editor_from(Keymap::Standard, "#[ab|]#c");
    let fx = feed(&mut e, "<C-c>");
    assert!(fx.contains(&Effect::SetClipboard("ab".into())));
    feed(&mut e, "<right>");
    e.set_clipboard(Some("ab".into()));
    feed(&mut e, "<C-v>");
    assert_eq!(state(&e), "abab#[|]#c");
}

#[test]
fn cut_and_paste_selection() {
    let mut e = editor_from(Keymap::Standard, "#[ab|]#cd");
    let fx = feed(&mut e, "<C-x>");
    assert!(fx.contains(&Effect::SetClipboard("ab".into())));
    assert_eq!(state(&e), "#[|]#cd");
    feed(&mut e, "<right><right>");
    e.set_clipboard(Some("ab".into()));
    feed(&mut e, "<C-v>");
    assert_eq!(state(&e), "cdab#[|]#");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "cd");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "abcd");
}

#[test]
fn copy_without_selection_takes_the_whole_line() {
    let mut e = editor_from(Keymap::Standard, "a#[|]#b\ncd\n");
    let fx = feed(&mut e, "<C-c>");
    assert!(fx.contains(&Effect::SetClipboard("ab\n".into())));
    feed(&mut e, "<down>");
    e.set_clipboard(Some("ab\n".into()));
    feed(&mut e, "<C-v>");
    // a whole-line copy pastes above the cursor line and the cursor keeps its place
    assert_eq!(e.text(), "ab\nab\ncd\n");
    assert_eq!(e.char_to_line(e.primary().head), 2);
}

#[test]
fn cut_without_selection_removes_the_line() {
    let mut e = editor_from(Keymap::Standard, "a\nb#[|]#\nc\n");
    let fx = feed(&mut e, "<C-x>");
    assert!(fx.contains(&Effect::SetClipboard("b\n".into())));
    assert_eq!(e.text(), "a\nc\n");
    let mut e = editor_from(Keymap::Standard, "a\nb#[|]#");
    feed(&mut e, "<C-x>");
    assert_eq!(e.text(), "a");
}

#[test]
fn paste_text_from_the_platform() {
    let mut e = editor_from(Keymap::Standard, "a#[|]#c");
    e.insert_text("b");
    assert_eq!(state(&e), "ab#[|]#c");
    let mut e = editor_from(Keymap::Standard, "#[a|]#c");
    e.insert_text("xyz");
    assert_eq!(state(&e), "xyz#[|]#c");
    let mut e = editor_from(Keymap::Standard, "#[|]#a\n#(|)#b");
    e.insert_text("1\n2");
    assert_eq!(e.text(), "1a\n2b");
}

#[test]
fn paste_distributes_lines_over_cursors() {
    let mut e = editor_from(Keymap::Standard, "#[|]#\n#(|)#\n#(|)#");
    e.insert_text("a\nb\nc");
    assert_eq!(e.text(), "a\nb\nc");
}

#[test]
fn undo_redo_keys() {
    let mut e = editor_from(Keymap::Standard, "#[|]#");
    feed(&mut e, "hello");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "");
    feed(&mut e, "<C-S-z>");
    assert_eq!(e.text(), "hello");
    feed(&mut e, "<C-z><C-y>");
    assert_eq!(e.text(), "hello");
    feed(&mut e, "<cmd-z>");
    assert_eq!(e.text(), "");
}

#[test]
fn typing_runs_are_grouped_per_word() {
    let mut e = editor_from(Keymap::Standard, "#[|]#");
    feed(&mut e, "ab cd ef");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "ab cd ");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "ab ");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "");
}

#[test]
fn movement_breaks_typing_runs() {
    let mut e = editor_from(Keymap::Standard, "#[|]#");
    feed(&mut e, "ab<left>x");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "ab");
    feed(&mut e, "<C-z>");
    assert_eq!(e.text(), "");
}

#[test]
fn backspace_and_delete() {
    st("ab#[|]#c", "<backspace>", "a#[|]#c");
    st("a#[|]#bc", "<del>", "a#[|]#c");
    st("a#[b|]#c", "<backspace>", "a#[|]#c");
    st("a#[b|]#c", "<del>", "a#[|]#c");
    st("#[|]#abc", "<backspace>", "#[|]#abc");
    st("abc#[|]#", "<del>", "abc#[|]#");
    st("ab#[|]#\nc", "<del>", "ab#[|]#c");
    st("a#[|]#b#(|)#c", "<backspace>", "#[|]#c");
}

#[test]
fn word_deletes() {
    st("foo bar#[|]#", "<C-backspace>", "foo #[|]#");
    st("foo bar #[|]#", "<C-backspace>", "foo #[|]#");
    st("#[|]#foo bar", "<C-del>", "#[|]# bar");
    st("foo#[|]# bar", "<C-del>", "foo#[|]#");
    st("foo.bar#[|]#", "<C-backspace>", "foo.#[|]#");
    st("a #[|]#b", "<cmd-backspace>", "#[|]#b");
}

#[test]
fn auto_pairs_in_standard() {
    st("#[|]#", "(", "(#[|]#)");
    st("#[|]#", "()", "()#[|]#");
    st("#[|]#", "[]", "[]#[|]#");
    st("#[|]#", "'a'", "'a'#[|]#");
    st("#[|]#", "(<backspace>", "#[|]#");
    st("#[|]#", "don't", "don't#[|]#");
    st("#[|]#a", "(", "(#[|]#a");
}

#[test]
fn typing_open_bracket_over_selection_wraps_it() {
    st("#[ab|]#c", "(", "(#[ab|]#)c");
    st("#[ab|]#c", "'", "'#[ab|]#'c");
    st("#[ab|]#c", "[", "[#[ab|]#]c");
}

#[test]
fn newline_auto_indent() {
    st("  a#[|]#", "<ret>x", "  a\n  x#[|]#");
    st("f(#[|]#", "<ret>x", "f(\n    x#[|]#");
    st("#[|]#", "(<ret>x", "(\n    x#[|]#\n)");
    st("ab#[|]#cd", "<ret>", "ab\n#[|]#cd");
    st("#[ab|]#", "<ret>", "\n#[|]#");
}

#[test]
fn ctrl_enter_never_inserts_a_newline() {
    let mut e = editor_from(Keymap::Standard, "a#[|]#");
    let fx = feed(&mut e, "<C-ret>");
    assert_eq!(e.text(), "a");
    assert!(fx.contains(&Effect::RunStatementAtCursor));
    let fx = feed(&mut e, "<cmd-ret>");
    assert_eq!(e.text(), "a");
    assert!(fx.contains(&Effect::RunStatementAtCursor));
    let mut e = editor_from(Keymap::Standard, "#[a|]#b");
    let fx = feed(&mut e, "<C-ret>");
    assert!(fx.contains(&Effect::RunSelection("a".into())));
}

#[test]
fn tab_and_shift_tab() {
    st("a#[|]#", "<tab>", "a    #[|]#");
    st("#[a\nb|]#", "<tab>", "    #[a\n    b|]#");
    st("    #[a|]#", "<S-tab>", "#[a|]#");
    st("    a#[|]#", "<S-tab>", "a#[|]#");
    st("    #[a\n    b|]#", "<S-tab>", "#[a\nb|]#");
    st("  a#[|]#", "<S-tab>", "a#[|]#");
    st("a\n#[|]#b", "<S-tab>", "a\n#[|]#b");
    let fx = effects_of(Keymap::Standard, "#[|]#", "<tab>");
    assert!(!fx.contains(&Effect::RequestCompletion));
}

#[test]
fn comment_toggle() {
    st("a#[|]#", "<C-/>", "-- a#[|]#");
    st("-- a#[|]#", "<C-/>", "a#[|]#");
    st("#[a\nb|]#", "<C-/>", "-- #[a\n-- b|]#");
    st("#[-- a\n-- b|]#", "<C-/>", "#[a\nb|]#");
    st("a#[|]#", "<cmd-/>", "-- a#[|]#");
}

#[test]
fn select_next_occurrence_ctrl_d() {
    st("foo bar foo#[|]# foo", "<C-d>", "foo bar #[foo|]# foo");
    st("#[|]#foo bar foo foo", "<C-d>", "#[foo|]# bar foo foo");
    st("#[|]#foo bar foo foo", "<C-d><C-d>", "#(foo|)# bar #[foo|]# foo");
    st("#[|]#foo bar foo foo", "<C-d><C-d><C-d>", "#(foo|)# bar #(foo|)# #[foo|]#");
    st("#[|]#foo bar foo foo", "<C-d><C-d><C-d><C-d>", "#(foo|)# bar #(foo|)# #[foo|]#");
    st("#[foo|]# x foo", "<C-d>", "#(foo|)# x #[foo|]#");
    st("x#[|]# foo", "<C-d>", "#[x|]# foo");
}

#[test]
fn select_all_occurrences() {
    st("foo#[|]# bar foo", "<C-S-l>", "#[foo|]# bar #(foo|)#");
    st("#[foo|]# bar foo foo", "<C-S-l>", "#[foo|]# bar #(foo|)# #(foo|)#");
}

#[test]
fn move_lines() {
    st("a\nb#[|]#\nc", "<A-up>", "b#[|]#\na\nc");
    st("a\nb#[|]#\nc", "<A-down>", "a\nc\nb#[|]#");
    st("a#[|]#\nb\nc", "<A-up>", "a#[|]#\nb\nc");
    st("a\nb\nc#[|]#", "<A-down>", "a\nb\nc#[|]#");
    st("a\n#[b\nc|]#\nd", "<A-up>", "#[b\nc|]#\na\nd");
    st("a\n#[b\nc|]#\nd", "<A-down>", "a\nd\n#[b\nc|]#");
    st("a\nb#[|]#", "<A-up>", "b#[|]#\na");
}

#[test]
fn duplicate_lines() {
    st("a#[|]#\nb", "<C-S-d>", "a\na#[|]#\nb");
    st("a\nb#[|]#", "<C-S-d>", "a\nb\nb#[|]#");
    st("#[a\nb|]#\nc", "<C-S-d>", "a\nb\n#[a\nb|]#\nc");
    st("a#[|]#\nb", "<A-S-down>", "a\na#[|]#\nb");
}

#[test]
fn delete_lines() {
    st("a\nb#[|]#\nc", "<C-S-k>", "a\n#[|]#c");
    st("a\nb#[|]#", "<C-S-k>", "a#[|]#");
    st("a#[|]#\nb", "<C-S-k>", "#[|]#b");
    st("#[a\nb|]#\nc", "<C-S-k>", "#[|]#c");
}

#[test]
fn select_line() {
    st("a#[|]#b\ncd", "<C-l>", "#[ab\n|]#cd");
    st("a#[|]#b\ncd", "<C-l><C-l>", "#[ab\ncd|]#");
}

#[test]
fn add_cursors_above_and_below() {
    st("ab#[|]#c\nabc\nabc", "<C-A-down>", "ab#(|)#c\nab#[|]#c\nabc");
    st("abc\nab#[|]#c\nabc", "<C-A-up>", "ab#[|]#c\nab#(|)#c\nabc");
    st("abc\nab#[|]#c", "<A-S-up>", "ab#[|]#c\nab#(|)#c");
    st("abc#[|]#\nab", "<C-A-down>", "abc#(|)#\nab#[|]#");
    st("ab#[|]#c\nabc\nabc", "<C-A-down><C-A-down>", "ab#(|)#c\nab#(|)#c\nab#[|]#c");
    st("ab#[|]#c", "<C-A-up>", "ab#[|]#c");
}

#[test]
fn multi_cursor_typing_and_escape() {
    st("ab#[|]#c\nabc", "<C-A-down>X", "abX#(|)#c\nabX#[|]#c");
    st("ab#[|]#c\nabc", "<C-A-down><esc>", "abc\nab#[|]#c");
    st("#[a|]#bc", "<esc>", "a#[|]#bc");
    st("a#[|]#bc", "<esc>", "a#[|]#bc");
    let fx = effects_of(Keymap::Standard, "a#[|]#", "<esc>");
    assert!(fx.contains(&Effect::DismissCompletion));
}

#[test]
fn search_prompt_and_f3() {
    let mut e = editor_from(Keymap::Standard, "#[|]#a foo b foo c foo");
    feed(&mut e, "<C-f>foo");
    assert_eq!(e.prompt().unwrap().prefix, "find:");
    assert_eq!(state(&e), "a #[foo|]# b foo c foo");
    feed(&mut e, "<ret>");
    assert!(e.prompt().is_none());
    assert_eq!(state(&e), "a #[foo|]# b foo c foo");
    feed(&mut e, "<F3>");
    assert_eq!(state(&e), "a foo b #[foo|]# c foo");
    feed(&mut e, "<F3>");
    assert_eq!(state(&e), "a foo b foo c #[foo|]#");
    feed(&mut e, "<F3>");
    assert_eq!(state(&e), "a #[foo|]# b foo c foo");
    feed(&mut e, "<S-F3>");
    assert_eq!(state(&e), "a foo b foo c #[foo|]#");
    assert_eq!(e.search_matches().len(), 3);
    feed(&mut e, "<C-f><esc>");
    assert_eq!(state(&e), "a foo b foo c #[foo|]#");
}

#[test]
fn ctrl_f_prefills_selection() {
    let mut e = editor_from(Keymap::Standard, "#[foo|]# foo");
    feed(&mut e, "<C-f>");
    assert_eq!(e.prompt().unwrap().text, "foo");
}

#[test]
fn goto_line_prompt() {
    st("#[|]#a\nb\nc", "<C-g>3<ret>", "a\nb\n#[|]#c");
    st("#[|]#a\nb\nc", "<C-g>2<ret>", "a\n#[|]#b\nc");
    st("#[|]#abc\nxyz", "<C-g>2:3<ret>", "abc\nxy#[|]#z");
    st("#[|]#a\nb", "<C-g>5<ret><C-g><esc>", "a\n#[|]#b");
}

#[test]
fn chords_and_shortcuts() {
    let cases: Vec<(&str, Effect)> = vec![
        ("<C-s>", Effect::Save),
        ("<C-w>", Effect::CloseTab),
        ("<C-n>", Effect::NewTab),
        ("<C-t>", Effect::NewTab),
        ("<C-tab>", Effect::NextTab),
        ("<C-S-tab>", Effect::PreviousTab),
        ("<C-S-p>", Effect::OpenCommandPalette),
        ("<F1>", Effect::OpenCommandPalette),
        ("<C-p>", Effect::OpenTablePicker),
        ("<C-S-i>", Effect::FormatDocument),
        ("<C-A-l>", Effect::FormatDocument),
        ("<C-b>", Effect::ToggleExplorer),
        ("<C-j>", Effect::ToggleBottomPanel),
        ("<C-S-e>", Effect::FocusExplorer),
        ("<C-k><C-t>", Effect::OpenThemePicker),
        ("<cmd-s>", Effect::Save),
        ("<cmd-p>", Effect::OpenTablePicker),
        // code intelligence (VS Code's keys)
        ("<F12>", Effect::GoToDefinition),
        ("<S-F12>", Effect::FindUsages),
        ("<F2>", Effect::RenameSymbol),
        ("<F8>", Effect::NextProblem),
        ("<S-F8>", Effect::PreviousProblem),
        ("<C-.>", Effect::CodeActions),
        ("<cmd-.>", Effect::CodeActions),
        ("<C-S-m>", Effect::ShowProblems),
    ];
    for (keys, want) in cases {
        let fx = effects_of(Keymap::Standard, "a#[|]#", keys);
        assert!(fx.contains(&want), "{keys}: {fx:?}");
    }
}

#[test]
fn chord_pending_state() {
    let mut e = editor_from(Keymap::Standard, "a#[|]#");
    feed(&mut e, "<C-k>");
    assert_eq!(e.pending_keys(), "ctrl-k");
    assert_eq!(e.key_hints().unwrap().0, "ctrl-k");
    feed(&mut e, "<esc>");
    assert_eq!(e.pending_keys(), "");
    assert!(e.key_hints().is_none());
    // an unrelated second key cancels the chord
    let fx = feed(&mut e, "<C-k>x");
    assert!(!fx.contains(&Effect::OpenThemePicker));
    assert_eq!(e.text(), "a");
}

#[test]
fn unbound_keys_are_reported() {
    let mut e = editor_from(Keymap::Standard, "a#[|]#");
    assert!(!e.handle_key(&KeyInput::parse("F9").unwrap()).handled);
    assert!(!e.handle_key(&KeyInput::parse("ctrl-q").unwrap()).handled);
    assert!(e.handle_key(&KeyInput::parse("a").unwrap()).handled);
    let mut h = editor_from(Keymap::Helix, "a#[|]#");
    assert!(!h.handle_key(&KeyInput::parse("F9").unwrap()).handled);
}

#[test]
fn mode_is_always_standard() {
    let mut e = editor_from(Keymap::Standard, "a#[|]#");
    assert_eq!(e.mode(), Mode::Standard);
    feed(&mut e, "ihello<esc>");
    assert_eq!(e.mode(), Mode::Standard);
    assert_eq!(e.text(), "aihello");
}

#[test]
fn switching_keymaps_keeps_text_and_selections() {
    let mut e = editor_from(Keymap::Standard, "ab#[|]#c");
    e.set_keymap(Keymap::Helix);
    assert_eq!(e.mode(), Mode::Normal);
    assert_eq!(e.text(), "abc");
    assert_eq!(e.primary().range(), 2..3);
    e.set_keymap(Keymap::Standard);
    assert_eq!(e.mode(), Mode::Standard);
    assert_eq!(e.text(), "abc");
}
