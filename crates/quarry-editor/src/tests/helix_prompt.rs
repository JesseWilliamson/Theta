use super::*;

#[test]
fn search_forward_and_repeat() {
    hx("#[a|]# foo bar foo", "/foo<ret>", "a #[foo|]# bar foo");
    hx("#[a|]# foo bar foo", "/foo<ret>n", "a foo bar #[foo|]#");
    hx("#[a|]# foo bar foo", "/foo<ret>nn", "a #[foo|]# bar foo");
    hx("#[a|]# foo bar foo", "/foo<ret>N", "a foo bar #[foo|]#");
    hx("a foo bar #[f|]#oo", "?foo<ret>", "a #[foo|]# bar foo");
    hx("#[a|]# foo bar foo", "/o+<ret>", "a f#[oo|]# bar foo");
    hx("#[a|]# foo bar foo", "/foo<ret>2n", "a #[foo|]# bar foo");
}

#[test]
fn search_smart_case() {
    hx("#[a|]# Foo foo", "/foo<ret>", "a #[Foo|]# foo");
    hx("#[a|]# Foo foo", "/Foo<ret>", "a #[Foo|]# foo");
    hx("#[a|]# foo Foo", "/Foo<ret>", "a foo #[Foo|]#");
}

#[test]
fn search_escape_restores_selection() {
    hx("#[a|]# foo", "/foo<esc>", "#[a|]# foo");
    hx("#[a|]# foo", "/foo<ret>gg", "#[a|]# foo");
}

#[test]
fn search_no_match_keeps_selection_and_reports() {
    let mut e = editor_from(Keymap::Helix, "#[a|]# foo");
    let fx = feed(&mut e, "/zzz<ret>");
    assert!(fx.iter().any(|x| matches!(x, Effect::Status(s) if s.contains("no matches"))), "{fx:?}");
    assert_eq!(state(&e), "#[a|]# foo");
    let fx = feed(&mut e, "/(<ret>");
    assert!(fx.iter().any(|x| matches!(x, Effect::Status(s) if s.contains("invalid regex"))), "{fx:?}");
}

#[test]
fn search_prompt_state_and_live_preview() {
    let mut e = editor_from(Keymap::Helix, "#[a|]# foo bar");
    feed(&mut e, "/");
    let p = e.prompt().unwrap();
    assert_eq!((p.prefix, p.text, p.cursor), ("/", "", 0));
    feed(&mut e, "ba");
    assert_eq!(e.prompt().unwrap().text, "ba");
    assert_eq!(e.prompt().unwrap().cursor, 2);
    // preview moved the selection
    assert_eq!(state(&e), "a foo #[ba|]#r");
    feed(&mut e, "<backspace><backspace>");
    assert_eq!(state(&e), "#[a|]# foo bar");
    feed(&mut e, "<esc>");
    assert!(e.prompt().is_none());
}

#[test]
fn prompt_editing_keys() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    feed(&mut e, ":abc def");
    feed(&mut e, "<left><left><left>X");
    assert_eq!(e.prompt().unwrap().text, "abc Xdef");
    feed(&mut e, "<C-a>Y");
    assert_eq!(e.prompt().unwrap().text, "Yabc Xdef");
    feed(&mut e, "<C-e><C-w>");
    assert_eq!(e.prompt().unwrap().text, "Yabc ");
    feed(&mut e, "<C-u>");
    assert_eq!(e.prompt().unwrap().text, "");
    feed(&mut e, "xyz<home><del>");
    assert_eq!(e.prompt().unwrap().text, "yz");
    feed(&mut e, "<end><C-k>");
    feed(&mut e, "<backspace><backspace><backspace>");
    assert!(e.prompt().is_none(), "backspace on an empty prompt closes it");
}

#[test]
fn search_history_with_up_down() {
    let mut e = editor_from(Keymap::Helix, "#[a|]# foo bar");
    feed(&mut e, "/foo<ret>/bar<ret>/<up>");
    assert_eq!(e.prompt().unwrap().text, "bar");
    feed(&mut e, "<up>");
    assert_eq!(e.prompt().unwrap().text, "foo");
    feed(&mut e, "<down>");
    assert_eq!(e.prompt().unwrap().text, "bar");
    feed(&mut e, "<down>");
    assert_eq!(e.prompt().unwrap().text, "");
}

#[test]
fn star_sets_search_register() {
    hx("#[f|]#oo bar foo", "*n", "foo bar #[foo|]#");
    hx("#[foo|]# foobar foo", "*n", "foo foobar #[foo|]#");
    hx("#[foo bar|]# x foo bar", "*n", "foo bar x #[foo bar|]#");
}

#[test]
fn select_mode_n_adds_selections() {
    hx("#[a|]# foo bar foo", "/foo<ret>vn", "a #(foo|)# bar #[foo|]#");
}

#[test]
fn select_regex_in_selection() {
    hx("#[a1 b22 c3|]#", "s\\d+<ret>", "a#[1|]# b#(22|)# c#(3|)#");
    hx("#[a1 b22 c3|]#", "s[a-c]<ret>", "#[a|]#1 #(b|)#22 #(c|)#3");
    hx("#[a1|]# b2", "sb<ret>", "#[a1|]# b2");
    hx("#[ab|]#\n#(cd|)#", "s.<ret>", "#[a|]##(b|)#\n#(c|)##(d|)#");
}

#[test]
fn select_regex_esc_restores() {
    hx("#[a1 b2|]#", "s\\d<esc>", "#[a1 b2|]#");
}

#[test]
fn split_selection() {
    hx("#[a,b,c|]#", "S,<ret>", "#[a|]#,#(b|)#,#(c|)#");
    hx("#[a, b|]#", "S,\\s*<ret>", "#[a|]#, #(b|)#");
    hx("#[abc|]#", "S,<ret>", "#[abc|]#");
}

#[test]
fn split_on_newlines() {
    hx("#[a\nb\nc|]#", "<A-s>", "#[a|]#\n#(b|)#\n#(c|)#");
    hx("#[a\n\nb|]#", "<A-s>", "#[a|]#\n\n#(b|)#");
}

#[test]
fn keep_and_remove_matching() {
    hx("#[a1|]# #(b|)# #(c3|)#", "K\\d<ret>", "#[a1|]# b #(c3|)#");
    hx("#[a1|]# #(b|)# #(c3|)#", "<A-K>\\d<ret>", "a1 #[b|]# c3");
}

#[test]
fn trim_and_align_and_merge() {
    hx("#[ a |]#", "_", " #[a|]# ");
    hx("#[a|]# #(b|)# #(c|)#", "<A-minus>", "#[a b c|]#");
    hx("#[a|]##(b|)# #(c|)#", "<A-_>", "#[ab|]# #(c|)#");
    hx("#[a|]#\n#(bbb|)#", "<A-minus>", "#[a\nbbb|]#");
}

#[test]
fn align_selections() {
    hx("#[a|]# x\n#(bbb|)# x\n", "&", "  #[a|]# x\n#(bbb|)# x\n");
}

#[test]
fn copy_selection_to_lines() {
    hx("ab#[c|]#\nabc\nabc\n", "C", "ab#(c|)#\nab#[c|]#\nabc\n");
    hx("abc\nab#[c|]#\nabc\n", "<A-C>", "ab#[c|]#\nab#(c|)#\nabc\n");
    hx("abc\nab#[c|]#\nabc\n", "2C", "abc\nab#(c|)#\nab#[c|]#\n");
}

#[test]
fn command_line_effects() {
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":w<ret>");
    assert!(fx.contains(&Effect::Save));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":write<ret>");
    assert!(fx.contains(&Effect::Save));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":q<ret>");
    assert!(fx.contains(&Effect::CloseTab));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":qa<ret>");
    assert!(fx.contains(&Effect::Quit));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":wq<ret>");
    assert_eq!(fx, vec![Effect::Save, Effect::CloseTab]);
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":run<ret>");
    assert!(fx.contains(&Effect::RunStatementAtCursor));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":r<ret>");
    assert!(fx.contains(&Effect::RunStatementAtCursor));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":run-all<ret>");
    assert!(fx.contains(&Effect::RunAll));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":explain<ret>");
    assert!(fx.contains(&Effect::ExplainStatement { analyze: false }));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":analyze<ret>");
    assert!(fx.contains(&Effect::ExplainStatement { analyze: true }));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":format<ret>");
    assert!(fx.contains(&Effect::FormatDocument));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":fmt<ret>");
    assert!(fx.contains(&Effect::FormatDocument));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":connect<ret>");
    assert!(fx.contains(&Effect::OpenConnectionPicker));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":theme<ret>");
    assert!(fx.contains(&Effect::OpenThemePicker));
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":theme nord<ret>");
    assert_eq!(fx, vec![Effect::HostCommand("theme nord".into())]);
    let fx = effects_of(Keymap::Helix, "#[a|]#", ":frobnicate 1 2<ret>");
    assert_eq!(fx, vec![Effect::HostCommand("frobnicate 1 2".into())]);
}

#[test]
fn command_line_goto() {
    hx("#[a|]#\nb\nc\n", ":3<ret>", "a\nb\n#[c|]#\n");
    hx("#[a|]#\nb\nc\n", ":goto 2<ret>", "a\n#[b|]#\nc\n");
    hx("#[a|]#\nb\nc\n", ":99<ret>", "a\nb\n#[c|]#\n");
}

#[test]
fn command_line_sort() {
    hx("#[c|]# #(a|)# #(b|)#", ":sort<ret>", "#[a|]# #(b|)# #(c|)#");
    hx("#[c\na\nb|]#\n", ":sort<ret>", "#[a\nb\nc|]#\n");
}

#[test]
fn command_line_tab_completion_and_history() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    feed(&mut e, ":wr<tab>");
    assert!(e.prompt().unwrap().text.starts_with("write"), "{}", e.prompt().unwrap().text);
    feed(&mut e, "<esc>:run<tab>");
    assert!(e.prompt().unwrap().text.starts_with("run"));
    let first = e.prompt().unwrap().text.to_string();
    feed(&mut e, "<tab>");
    assert_ne!(e.prompt().unwrap().text, first, "tab cycles candidates");
    feed(&mut e, "<esc>:w<ret>:<up>");
    assert_eq!(e.prompt().unwrap().text, "w");
}

#[test]
fn pending_keys_for_status_bar() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    feed(&mut e, "2g");
    assert_eq!(e.pending_keys(), "2g");
    feed(&mut e, "<esc>");
    assert_eq!(e.pending_keys(), "");
    feed(&mut e, "\"a");
    assert_eq!(e.pending_keys(), "\"a");
    feed(&mut e, "<esc>");
    feed(&mut e, "\"");
    assert_eq!(e.pending_keys(), "\"");
    feed(&mut e, "<esc>");
    assert_eq!(e.pending_keys(), "");
}

#[test]
fn which_key_hints_for_sub_menus() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    for (key, title) in [
        ("g", "Goto"),
        ("<space>", "Space"),
        ("z", "View"),
        ("[", "Left bracket"),
        ("]", "Right bracket"),
        ("m", "Match"),
    ] {
        feed(&mut e, key);
        let (t, hints) = e.key_hints().unwrap_or_else(|| panic!("hints for {key}"));
        assert_eq!(t, title);
        assert!(!hints.is_empty());
        feed(&mut e, "<esc>");
    }
    feed(&mut e, "\"");
    assert_eq!(e.key_hints().unwrap().0, "Register");
    feed(&mut e, "<esc>");
    assert!(e.key_hints().is_none());
}

#[test]
fn space_menu_effects() {
    let cases: Vec<(&str, Effect)> = vec![
        ("<space>f", Effect::OpenTablePicker),
        ("<space>b", Effect::OpenConnectionPicker),
        ("<space>?", Effect::OpenCommandPalette),
        ("<space><space>", Effect::OpenCommandPalette),
        ("<space>r", Effect::RunStatementAtCursor),
        ("<space>R", Effect::RunAll),
        ("<space>e", Effect::ExplainStatement { analyze: false }),
        ("<space>E", Effect::ExplainStatement { analyze: true }),
        ("<space>k", Effect::RequestCompletion),
        ("<space>t", Effect::OpenThemePicker),
    ];
    for (keys, want) in cases {
        let fx = effects_of(Keymap::Helix, "#[a|]#", keys);
        assert!(fx.contains(&want), "{keys}: {fx:?}");
    }
    let fx = effects_of(Keymap::Helix, "#[sel|]#ect", "<space>r");
    assert!(!fx.contains(&Effect::RunStatementAtCursor));
    let fx = effects_of(Keymap::Helix, "#[select|]# 1", "<space>r");
    assert_eq!(fx, vec![Effect::RunSelection("select".into())]);
}

#[test]
fn global_run_keys() {
    for keymap in [Keymap::Helix, Keymap::Standard] {
        let fx = effects_of(keymap, "a#[|]#", "<C-ret>");
        assert!(fx.contains(&Effect::RunStatementAtCursor), "{keymap:?} {fx:?}");
        let fx = effects_of(keymap, "a#[|]#", "<cmd-ret>");
        assert!(fx.contains(&Effect::RunStatementAtCursor));
        let fx = effects_of(keymap, "a#[|]#", "<C-S-ret>");
        assert!(fx.contains(&Effect::RunAll));
        let fx = effects_of(keymap, "a#[|]#", "<F5>");
        assert!(fx.contains(&Effect::RunAll));
        let fx = effects_of(keymap, "a#[|]#", "<C-space>");
        assert!(fx.contains(&Effect::RequestCompletion));
        let fx = effects_of(keymap, "a#[|]#", "<C-ret>");
        assert!(!fx.contains(&Effect::RunAll));
    }
    let fx = effects_of(Keymap::Helix, "a#[|]#", "i<C-ret>");
    assert!(fx.contains(&Effect::RunStatementAtCursor));
}
