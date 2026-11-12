//! The Vim keymap's public surface: modes, labels, cursor styles, highlights of Visual modes,
//! pending keys and hints, the leader menu, clipboard registers, effects and the host-facing
//! `:` commands. Behaviour that real Vim can decide lives in the generated `vim_*_cases` files.

use super::*;
use crate::CursorStyle;

fn ed(before: &str) -> Editor {
    vim_editor(before)
}

fn fx(before: &str, keys: &str) -> Vec<Effect> {
    vim_effects(before, keys)
}

fn status_of(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Status(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

// ---- modes, labels, cursor styles --------------------------------------------------------------------------

#[test]
fn modes_labels_and_cursor_styles() {
    let mut e = ed("fo[o] bar\nbaz\n");
    assert_eq!((e.mode(), e.mode_label(), e.cursor_style()), (Mode::Normal, "NORMAL", CursorStyle::Block));
    for (keys, mode, label, style) in [
        ("i", Mode::Insert, "INSERT", CursorStyle::Bar),
        ("<esc>R", Mode::Replace, "REPLACE", CursorStyle::Underline),
        ("<esc>v", Mode::Visual, "VISUAL", CursorStyle::Block),
        ("<esc>V", Mode::VisualLine, "V-LINE", CursorStyle::Block),
        ("<esc><C-v>", Mode::VisualBlock, "V-BLOCK", CursorStyle::Block),
        ("<esc>", Mode::Normal, "NORMAL", CursorStyle::Block),
    ] {
        feed(&mut e, keys);
        assert_eq!((e.mode(), e.mode_label(), e.cursor_style()), (mode, label, style), "after {keys}");
    }
}

#[test]
fn labels_and_styles_for_the_other_keymaps() {
    let mut h = Editor::new("abc", Keymap::Helix);
    assert_eq!((h.mode_label(), h.cursor_style()), ("NOR", CursorStyle::Block));
    feed(&mut h, "i");
    assert_eq!((h.mode_label(), h.cursor_style()), ("INS", CursorStyle::Bar));
    feed(&mut h, "<esc>v");
    assert_eq!((h.mode_label(), h.cursor_style()), ("SEL", CursorStyle::Block));
    let s = Editor::new("abc", Keymap::Standard);
    assert_eq!((s.mode_label(), s.cursor_style()), ("STD", CursorStyle::Bar));
}

#[test]
fn switching_keymaps_resets_to_normal() {
    let mut e = Editor::new("hello world", Keymap::Helix);
    feed(&mut e, "i");
    e.set_keymap(Keymap::Vim);
    assert_eq!((e.keymap(), e.mode()), (Keymap::Vim, Mode::Normal));
    feed(&mut e, "ix<esc>");
    assert_eq!(e.text(), "xhello world");
    feed(&mut e, "vl");
    e.set_keymap(Keymap::Helix);
    assert_eq!(e.mode(), Mode::Normal);
    e.set_keymap(Keymap::Vim);
    assert_eq!(e.mode(), Mode::Normal);
    assert!(e.selected_text().is_none());
}

// ---- highlight selections of the Visual modes ---------------------------------------------------------------

vis_cases! {
    vis_char_forward: "fo[o] bar\n", "vl" => "fo«o[ ]»bar\n";
    vis_char_backward: "foo [b]ar\n", "vh" => "foo«[ ]b»ar\n";
    vis_single_char: "fo[o] bar\n", "v" => "fo«[o]» bar\n";
    vis_o_swaps_ends: "fo[o] bar\n", "vlo" => "fo«[o] »bar\n";
    vis_multi_line: "ab[c]\ndef\n", "vj" => "ab«c\nde[f]»\n";
    vis_includes_the_newline_on_dollar: "a[b]c\ndef\n", "v$" => "a«bc[]\n»def\n";
    vis_word_object: "foo [b]ar baz\n", "viw" => "foo «ba[r]» baz\n";
    vis_aw_object: "foo [b]ar baz\n", "vaw" => "foo «bar[ ]»baz\n";
    vis_extend_iw_twice: "foo [b]ar baz\n", "viwiw" => "foo «bar[ ]»baz\n";
    vis_paren_object: "f(a, [b])\n", "vi(" => "f(«a, [b]»)\n";
    vis_paren_object_twice: "((a[b]))\n", "vi(i(" => "(«(ab[)]»)\n";
    vis_line: "a\nb[b]\nc\n", "V" => "a\n«b[b]\n»c\n";
    vis_line_down: "a[a]\nbb\ncc\n", "Vj" => "«aa\nb[b]\n»cc\n";
    vis_line_up: "aa\nb[b]\ncc\n", "Vk" => "«a[a]\nbb\n»cc\n";
    vis_line_paragraph_object: "a\nb\n\nc\n", "vip" => "«a\n[b]\n»\nc\n";
    vis_block_two_rows: "ab[c]def\nghijkl\n", "<C-v>jl" => "ab«cd»ef\ngh«i[j]»kl\n";
    vis_block_skips_short_rows: "abc[d]\nx\nabcd\n", "<C-v>jj" => "abc«d»\nx\nabc«[d]»\n";
    vis_block_dollar_goes_to_the_line_ends: "a[b]c\ndefgh\n", "<C-v>j$" => "a«bc»\nd«efgh»[]\n";
    vis_block_corner_swap: "ab[c]d\nefgh\n", "<C-v>jlO" => "ab«cd»\nef«[g]h»\n";
    vis_gv_restores: "foo [b]ar\n", "vl<esc>gv" => "foo «b[a]»r\n";
    vis_v_then_shift_v_switches_kind: "a[a]\nbb\n", "vjV" => "«aa\nb[b]\n»";
}

#[test]
fn visual_selected_text_and_cursor() {
    let mut e = ed("foo [b]ar baz\n");
    assert_eq!(e.selected_text(), None, "Normal mode has no selection");
    feed(&mut e, "viw");
    assert_eq!(e.selected_text().as_deref(), Some("bar"));
    assert_eq!(e.cursor_of(&e.primary()), 6);
    feed(&mut e, "o");
    assert_eq!(e.cursor_of(&e.primary()), 4, "after `o` the cursor is at the other end");
    assert_eq!(e.primary().range(), 4..7);
    feed(&mut e, "<esc>");
    assert_eq!(e.selected_text(), None);
}

#[test]
fn block_visual_has_one_selection_per_row_and_the_primary_on_the_cursor_row() {
    let mut e = ed("abcd\nefgh\nijkl\n");
    feed(&mut e, "l<C-v>jjl");
    let sels = e.selections();
    assert_eq!(sels.len(), 3);
    assert_eq!(sels.iter().map(|s| e.slice(s.range())).collect::<Vec<_>>(), vec!["bc", "fg", "jk"]);
    assert_eq!(e.primary_index(), 2);
    assert_eq!(e.cursor_of(&e.primary()), 12);
    feed(&mut e, "kk");
    assert_eq!(e.primary_index(), 0);
}

// ---- Normal mode keeps the cursor on a character --------------------------------------------------------------

#[test]
fn normal_cursor_never_rests_past_the_last_character() {
    let mut e = ed("ab[c]\nxy\n");
    feed(&mut e, "l");
    assert_eq!(e.cursor_of(&e.primary()), 2, "l stops on the last char");
    feed(&mut e, "A");
    assert_eq!(e.cursor_of(&e.primary()), 3, "Insert mode may stand after it");
    feed(&mut e, "<esc>");
    assert_eq!(e.cursor_of(&e.primary()), 2, "Esc moves left");
    feed(&mut e, "$");
    assert_eq!(e.cursor_of(&e.primary()), 2);
    // the highlight is the block over the character
    assert_eq!(e.primary().range(), 2..3);
}

#[test]
fn empty_line_cursor_and_highlight() {
    let e = ed("a\n[]\nb\n");
    assert_eq!(e.cursor_of(&e.primary()), 2);
    assert_eq!(e.primary().range(), 2..3, "the block covers the line break");
}

// ---- global bindings -------------------------------------------------------------------------------------------

#[test]
fn global_run_bindings_work_in_every_mode() {
    for (setup, expect_selection) in
        [("", false), ("i", false), ("R", false), ("viw", true), ("V", true), ("<C-v>", true)]
    {
        let mut e = ed("[s]elect 1;\n");
        feed(&mut e, setup);
        let out = feed(&mut e, "<C-ret>");
        if expect_selection {
            assert!(matches!(out.as_slice(), [Effect::RunSelection(_)]), "{setup}: {out:?}");
        } else {
            assert_eq!(out, vec![Effect::RunStatementAtCursor], "{setup}");
        }
        assert_eq!(feed(&mut e, "<C-S-ret>"), vec![Effect::RunAll], "{setup}");
        assert_eq!(feed(&mut e, "<F5>"), vec![Effect::RunAll], "{setup}");
        assert_eq!(feed(&mut e, "<C-space>"), vec![Effect::RequestCompletion], "{setup}");
        assert_eq!(feed(&mut e, "<cmd-ret>").len(), 1, "{setup}");
    }
}

#[test]
fn run_selection_carries_the_visual_text() {
    assert_eq!(fx("[s]elect 1;\n", "viw<C-ret>"), vec![Effect::RunSelection("select".into())]);
    assert_eq!(fx("[a]\nb\n", "Vj<C-ret>"), vec![Effect::RunSelection("a\nb\n".into())]);
}

#[test]
fn escape_dismisses_completion_and_clears_pending_state() {
    assert!(fx("[a]\n", "<esc>").contains(&Effect::DismissCompletion));
    assert!(fx("[a]\n", "i<esc>").contains(&Effect::DismissCompletion));
    let mut e = ed("[a]bc\n");
    feed(&mut e, "2d");
    assert_eq!(e.pending_keys(), "2d");
    feed(&mut e, "<esc>");
    assert_eq!(e.pending_keys(), "");
    feed(&mut e, "x");
    assert_eq!(e.text(), "bc\n", "the cancelled operator did not linger");
}

#[test]
fn ctrl_c_and_ctrl_bracket_are_escape() {
    vim_mode("[a]bc\n", "ix<C-c>", "[x]abc\n", Mode::Normal);
    vim_mode("[a]bc\n", "ix<C-[>", "[x]abc\n", Mode::Normal);
    vim_mode("[a]bc\n", "vl<C-c>", "a[b]c\n", Mode::Normal);
    vim_mode("[a]bc\n", "Rx<C-[>", "[x]bc\n", Mode::Normal);
}

// ---- pending keys and hints -----------------------------------------------------------------------------------------

#[test]
fn pending_keys_show_the_command_being_typed() {
    let mut e = ed("[a]bc def\n");
    for (keys, shown) in [
        ("d", "d"),
        ("<esc>2d", "2d"),
        ("<esc>2d3", "2d3"),
        ("<esc>\"a", "\"a"),
        ("<esc>\"ay", "\"ay"),
        ("<esc>g", "g"),
        ("<esc>dg", "dg"),
        ("<esc>ci", "ci"),
        ("<esc>3", "3"),
        ("<esc>f", "f"),
        ("<esc>z", "z"),
        ("<esc>m", "m"),
        ("<esc>'", "'"),
        ("<esc>q", "q"),
        ("<esc>@", "@"),
        ("<esc>r", "r"),
    ] {
        feed(&mut e, keys);
        assert_eq!(e.pending_keys(), shown, "after {keys}");
    }
    feed(&mut e, "<esc>i");
    assert_eq!(e.pending_keys(), "", "Insert mode shows nothing");
}

#[test]
fn key_hints_for_prefixes() {
    let mut e = ed("[a]bc\n");
    assert!(e.key_hints().is_none());
    feed(&mut e, "g");
    let (title, hints) = e.key_hints().unwrap();
    assert_eq!(title, "Goto");
    assert!(hints.iter().any(|h| h.key == "g" && h.description.contains("file start")));
    feed(&mut e, "<esc>\"");
    assert_eq!(e.key_hints().unwrap().0, "Register");
    feed(&mut e, "<esc>m");
    assert_eq!(e.key_hints().unwrap().0, "Set mark");
    feed(&mut e, "<esc>'");
    assert_eq!(e.key_hints().unwrap().0, "Jump to mark line");
    feed(&mut e, "<esc>`");
    assert_eq!(e.key_hints().unwrap().0, "Jump to mark");
    feed(&mut e, "<esc>z");
    assert_eq!(e.key_hints().unwrap().0, "View");
    feed(&mut e, "<esc>d");
    let (title, hints) = e.key_hints().unwrap();
    assert!(title.starts_with("d"), "{title}");
    assert!(hints.iter().any(|h| h.key == "i") && hints.iter().any(|h| h.key == "a"));
    feed(&mut e, "i");
    let (title, hints) = e.key_hints().unwrap();
    assert_eq!(title, "Inside");
    assert!(hints.iter().any(|h| h.key == "w") && hints.iter().any(|h| h.key.contains('(')));
    feed(&mut e, "<esc>ca");
    assert_eq!(e.key_hints().unwrap().0, "Around");
    feed(&mut e, "<esc>vi");
    assert_eq!(e.key_hints().unwrap().0, "Inside");
    feed(&mut e, "<esc><esc>i");
    assert!(e.key_hints().is_none());
    feed(&mut e, "<C-r>");
    assert_eq!(e.key_hints().unwrap().0, "Insert register");
}

#[test]
fn leader_menu_hints_mirror_the_helix_space_menu() {
    let mut e = ed("[a]bc\n");
    feed(&mut e, "<space>");
    let (title, hints) = e.key_hints().unwrap();
    assert_eq!(title, "Space");
    for key in [
        "f", "b", "? space", "r", "R", "e", "E", "k", "t", "y", "p", "c", "x", "g", "d", "T", "j", "a", "n",
        "D", "s", "h", "C", "v", "V",
    ] {
        assert!(hints.iter().any(|h| h.key == key), "missing {key}");
    }
    assert_eq!(e.pending_keys(), "space");
}

// ---- the leader menu actions ---------------------------------------------------------------------------------------

#[test]
fn leader_actions() {
    let cases: Vec<(&str, Effect)> = vec![
        ("<space>f", Effect::OpenTablePicker),
        ("<space>/", Effect::OpenTablePicker),
        ("<space>b", Effect::OpenConnectionPicker),
        ("<space>?", Effect::OpenCommandPalette),
        ("<space><space>", Effect::OpenCommandPalette),
        ("<space>r", Effect::RunStatementAtCursor),
        ("<space>R", Effect::RunAll),
        ("<space>e", Effect::ExplainStatement { analyze: false }),
        ("<space>E", Effect::ExplainStatement { analyze: true }),
        ("<space>k", Effect::RequestCompletion),
        ("<space>t", Effect::OpenThemePicker),
        ("<space>x", Effect::CancelQuery),
        ("<space>g", Effect::FocusResults),
        ("<space>d", Effect::FocusExplorer),
        ("<space>T", Effect::ToggleExplorer),
        ("<space>j", Effect::ToggleBottomPanel),
        ("<space>s", Effect::HostCommand("search".into())),
        ("<space>h", Effect::HostCommand("lhistory".into())),
        ("<space>C", Effect::HostCommand("chart".into())),
        ("<space>v", Effect::HostCommand("cell".into())),
        ("<space>V", Effect::HostCommand("cell copy".into())),
        // code intelligence
        ("gd", Effect::GoToDefinition),
        ("gr", Effect::FindUsages),
        ("[d", Effect::PreviousProblem),
        ("]d", Effect::NextProblem),
        ("<space>a", Effect::CodeActions),
        ("<space>n", Effect::RenameSymbol),
        ("<space>D", Effect::ShowProblems),
        ("<F12>", Effect::GoToDefinition),
        ("<S-F12>", Effect::FindUsages),
        ("<F2>", Effect::RenameSymbol),
        ("<C-.>", Effect::CodeActions),
    ];
    for (keys, effect) in cases {
        assert!(fx("[s]elect 1\n", keys).contains(&effect), "{keys}");
    }
    assert!(
        matches!(fx("[s]elect 1\n", "viw<space>r").as_slice(), [Effect::RunSelection(s)] if s == "select")
    );
}

#[test]
fn leader_y_is_an_operator_into_the_clipboard() {
    let out = fx("[a]bc def\n", "<space>yiw");
    assert!(out.contains(&Effect::SetClipboard("abc".into())), "{out:?}");
    let out = fx("[a]bc def\n", "<space>yy");
    assert!(out.contains(&Effect::SetClipboard("abc def\n".into())), "{out:?}");
    let out = fx("[a]bc def\n", "viw<space>y");
    assert!(out.contains(&Effect::SetClipboard("abc".into())), "{out:?}");
    let out = fx("[a]bc def\n", "<space>Y");
    assert!(out.contains(&Effect::SetClipboard("abc def\n".into())), "{out:?}");
}

#[test]
fn leader_p_pastes_the_clipboard() {
    let mut e = ed("[a]bc\n");
    e.set_clipboard(Some("XY".into()));
    feed(&mut e, "<space>p");
    assert_eq!(e.text(), "aXYbc\n");
    feed(&mut e, "<space>P");
    assert_eq!(e.text(), "aXXYYbc\n");
    let mut e = ed("[a]bc def\n");
    e.set_clipboard(Some("Z".into()));
    feed(&mut e, "viw<space>p");
    assert_eq!(e.text(), "Z def\n");
}

#[test]
fn leader_c_toggles_sql_comments() {
    vim_text("[s]elect 1\nselect 2\n", "<space>c", "-- select 1\nselect 2\n");
    vim_text("[-]- select 1\nselect 2\n", "<space>c", "select 1\nselect 2\n");
    vim_text("[a]\nb\nc\n", "Vj<space>c", "-- a\n-- b\nc\n");
}

// ---- clipboard registers ----------------------------------------------------------------------------------------------

#[test]
fn plus_register_goes_through_the_host_clipboard() {
    let mut e = ed("[a]bc def\n");
    let out = feed(&mut e, "\"+yiw");
    assert_eq!(out, vec![Effect::SetClipboard("abc".into())]);
    // a plain yank does not touch the clipboard (unlike the Helix keymap)
    let out = feed(&mut e, "wyiw");
    assert!(!out.iter().any(|x| matches!(x, Effect::SetClipboard(_))), "{out:?}");
    e.set_clipboard(Some("from outside".into()));
    feed(&mut e, "\"+P");
    assert_eq!(e.text(), "abc from outsidedef\n");
    feed(&mut e, "u\"*p");
    assert!(e.text().contains("from outside"));
    // a clipboard ending in a newline is pasted linewise
    e.set_clipboard(Some("line\n".into()));
    feed(&mut e, "\"+p");
    assert!(e.text().ends_with("line\n"), "{:?}", e.text());
}

#[test]
fn unnamed_clipboard_option_mirrors_yanks_deletes_and_puts() {
    let mut e = ed("[a]bc def\nline\n");
    e.set_vim_clipboard_unnamed(true);
    assert_eq!(feed(&mut e, "yiw"), vec![Effect::SetClipboard("abc".into())]);
    assert_eq!(feed(&mut e, "wdw"), vec![Effect::SetClipboard("def".into())]);
    // a delete into the black hole or a named register leaves the clipboard alone
    let out = feed(&mut e, "\"_dd");
    assert!(!out.iter().any(|x| matches!(x, Effect::SetClipboard(_))), "{out:?}");
    // the host reports the clipboard it last received: the internal register (with its kind) wins
    e.set_clipboard(Some("def".into()));
    feed(&mut e, "p");
    assert_eq!(e.text(), "ldefine\n");
    // another application copied something: plain `p` pastes that
    e.set_clipboard(Some("from outside".into()));
    feed(&mut e, "0p");
    assert_eq!(e.text(), "lfrom outsidedefine\n");
}

#[test]
fn unnamed_clipboard_keeps_linewise_registers_linewise() {
    let mut e = ed("[o]ne\ntwo\n");
    e.set_vim_clipboard_unnamed(true);
    let out = feed(&mut e, "yy");
    assert_eq!(out, vec![Effect::SetClipboard("one\n".into())]);
    e.set_clipboard(Some("one\n".into()));
    feed(&mut e, "jp");
    assert_eq!(e.text(), "one\ntwo\none\n");
}

#[test]
fn unnamed_clipboard_is_off_by_default() {
    let out = fx("[a]bc\n", "yl");
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn star_register_is_the_same_clipboard() {
    let out = fx("[a]bc\n", "\"*yl");
    assert_eq!(out, vec![Effect::SetClipboard("a".into())]);
}

#[test]
fn read_only_registers() {
    let mut e = ed("[a]bc foo\n");
    feed(&mut e, "/foo<ret>");
    feed(&mut e, "0\"/P");
    assert_eq!(e.text(), "foo".to_string() + "abc foo\n");
    let mut e = ed("[a]bc\n");
    feed(&mut e, "ixy<esc>$\".p");
    assert_eq!(e.text(), "xyabcxy\n");
    let mut e = ed("[a]bc\n");
    feed(&mut e, ":s/b/B/<ret>\":p");
    assert!(e.text().contains("s/b/B/"), "{:?}", e.text());
}

#[test]
fn registers_command_lists_them() {
    let mut e = ed("[a]bc\ndef\n");
    feed(&mut e, "yy\"ayw");
    let out = feed(&mut e, ":registers<ret>");
    let msg = status_of(&out).join("\n");
    assert!(msg.contains("\"a"), "{msg}");
    assert!(msg.contains("\"0"), "{msg}");
}

// ---- format and comment operators ---------------------------------------------------------------------------------------

#[test]
fn equals_operator_formats() {
    assert!(fx("[s]elect 1;\nselect 2;\n", "==").contains(&Effect::FormatSelection));
    assert!(fx("[s]elect 1;\n", "gg=G").contains(&Effect::FormatDocument));
    assert!(fx("[s]elect 1;\nselect 2;\n", "=G").contains(&Effect::FormatDocument));
    let mut e = ed("a\n[b]\nc\n");
    let out = feed(&mut e, "==");
    assert!(out.contains(&Effect::FormatSelection));
    // the host formats the selection it finds: the line is selected when the effect is returned
    assert_eq!(e.selected_text(), None, "Normal mode has no selection to run");
    assert_eq!(e.primary().range(), 2..4);
    feed(&mut e, "l");
    assert_eq!(e.primary().range().len(), 1, "the next key collapses it again");
    assert!(fx("[a]\nb\nc\n", "Vj=").contains(&Effect::FormatSelection));
    assert!(fx("[a]\nb\n", "Vj=").contains(&Effect::FormatDocument));
}

#[test]
fn gc_toggles_line_comments() {
    vim_text("[s]elect 1\nselect 2\n\nx\n", "gcc", "-- select 1\nselect 2\n\nx\n");
    vim_text("[-]- select 1\n", "gcc", "select 1\n");
    vim_text("[a]\nb\n\nc\n", "gcip", "-- a\n-- b\n\nc\n");
    vim_text("[a]\n  b\n", "gcj", "-- a\n--   b\n");
    vim_text("[a]\nb\nc\n", "Vjgc", "-- a\n-- b\nc\n");
    vim_text("[a]\nb\nc\n", "gcG", "-- a\n-- b\n-- c\n");
    vim_text("[a]\nb\nc\n", "2gcc", "-- a\n-- b\nc\n");
    vim_text("[-]- a\nb\n", "gcj", "-- -- a\n-- b\n");
}

// ---- the command line -------------------------------------------------------------------------------------------------------

#[test]
fn host_commands_match_the_helix_table() {
    let cases: Vec<(&str, Vec<Effect>)> = vec![
        (":w", vec![Effect::Save]),
        (":write", vec![Effect::Save]),
        (":q", vec![Effect::CloseTab]),
        (":quit", vec![Effect::CloseTab]),
        (":q!", vec![Effect::CloseTab]),
        (":qa", vec![Effect::Quit]),
        (":wq", vec![Effect::Save, Effect::CloseTab]),
        (":x", vec![Effect::Save, Effect::CloseTab]),
        (":run", vec![Effect::RunStatementAtCursor]),
        (":run-all", vec![Effect::RunAll]),
        (":explain", vec![Effect::ExplainStatement { analyze: false }]),
        (":analyze", vec![Effect::ExplainStatement { analyze: true }]),
        (":fmt", vec![Effect::FormatDocument]),
        (":format", vec![Effect::FormatDocument]),
        (":theme", vec![Effect::OpenThemePicker]),
        (":theme dark", vec![Effect::HostCommand("theme dark".into())]),
        (":connect", vec![Effect::OpenConnectionPicker]),
        (":reopen", vec![Effect::HostCommand("reopen".into())]),
        (":cell copy", vec![Effect::HostCommand("cell copy".into())]),
        (":grep needle", vec![Effect::HostCommand("grep needle".into())]),
        (":search", vec![Effect::HostCommand("search".into())]),
        (":chart", vec![Effect::HostCommand("chart".into())]),
        (":lhistory", vec![Effect::HostCommand("lhistory".into())]),
        (":localhistory", vec![Effect::HostCommand("localhistory".into())]),
        (":new", vec![Effect::NewTab]),
        (":cancel", vec![Effect::CancelQuery]),
        (":help", vec![Effect::HostCommand("help".into())]),
        (":help s", vec![Effect::HostCommand("help s".into())]),
        (":frobnicate x", vec![Effect::HostCommand("frobnicate x".into())]),
    ];
    for (cmd, want) in cases {
        let mut e = ed("[a]\nb\n");
        let out = feed(&mut e, &format!("{cmd}<ret>"));
        assert_eq!(out, want, "{cmd}");
        // the Helix keymap maps the shared commands the same way
        if matches!(
            cmd,
            ":w" | ":q"
                | ":qa"
                | ":wq"
                | ":run"
                | ":run-all"
                | ":explain"
                | ":analyze"
                | ":fmt"
                | ":theme"
                | ":connect"
                | ":reopen"
                | ":cancel"
                | ":new"
        ) {
            let mut h = Editor::new("a\nb\n", Keymap::Helix);
            assert_eq!(feed(&mut h, &format!("{cmd}<ret>")), want, "helix {cmd}");
        }
    }
}

#[test]
fn colon_number_goes_to_the_line() {
    vim("[a]\nb\nc\n", ":2<ret>", "a\n[b]\nc\n");
    vim("[a]\n  b\nc\n", ":2<ret>", "a\n  [b]\nc\n");
    vim("[a]\nb\nc\n", ":99<ret>", "a\nb\n[c]\n");
    vim("[a]\nb\nc\n", ":$<ret>", "a\nb\n[c]\n");
    vim("[a]\nb\nc\n", "G:1<ret>", "[a]\nb\nc\n");
}

#[test]
fn visual_colon_prefills_the_range() {
    let mut e = ed("[a]\nb\nc\n");
    feed(&mut e, "Vj:");
    let p = e.prompt().unwrap();
    assert_eq!((p.prefix, p.text), (":", "'<,'>"));
    assert_eq!(e.mode(), Mode::Normal);
    feed(&mut e, "d<ret>");
    assert_eq!(e.text(), "c\n");
}

#[test]
fn count_colon_prefills_a_range() {
    let mut e = ed("[a]\nb\nc\n");
    feed(&mut e, "3:");
    assert_eq!(e.prompt().unwrap().text, ".,.+2");
}

#[test]
fn unsupported_regex_syntax_reports_an_error() {
    let out = fx("[a]a\n", ":s/\\(a\\)\\1/x/<ret>");
    let msg = status_of(&out).join("|");
    assert!(msg.contains("unsupported") && msg.contains("back-reference"), "{msg}");
    let out = fx("[a]a\n", "/a\\@<=b<ret>");
    assert!(status_of(&out).join("|").contains("look-around"));
    let out = fx("[a]a(\n", ":s/(/x/<ret>");
    assert!(status_of(&out).is_empty(), "a bare paren is literal in magic mode: {out:?}");
}

#[test]
fn substitute_errors_and_flags() {
    assert!(status_of(&fx("[a]\n", ":s/zzz/x/<ret>")).join("|").contains("E486"));
    assert!(status_of(&fx("[a]\n", ":s/zzz/x/e<ret>")).is_empty());
    vim_text("[a]aa\n", ":s/a/b/<ret>", "baa\n");
    vim_text("[a]aa\n", ":s/a/b/g<ret>", "bbb\n");
    vim_text("[A]aa\n", ":s/a/b/gi<ret>", "bbb\n");
    vim_text("[A]aa\n", ":s/a/b/gI<ret>", "Abb\n");
    vim_text("[a]aa\n", ":s/A/b/<ret>", "aaa\n");
    vim_text("[a]aa\n", ":s/A/b/i<ret>", "baa\n");
}

#[test]
fn set_options_control_the_search_case() {
    vim_text("[a] Foo foo\n", "/foo<ret>x", "a oo foo\n");
    vim_text("[a] foo Foo\n", "/Foo<ret>x", "a foo oo\n");
    vim_text("[a] Foo foo\n", ":set noic<ret>/foo<ret>x", "a Foo oo\n");
    vim_text("[a] foo Foo\n", ":set noic<ret>/foo<ret>x", "a oo Foo\n");
    vim_text("[a] Foo Foo\n", ":set scs ic<ret>/foo<ret>x", "a oo Foo\n");
    vim_text("[a] foo Foo\n", ":set noscs<ret>/Foo<ret>x", "a oo Foo\n");
    vim_text("[a] Foo foo\n", "/\\cFOO<ret>x", "a oo foo\n");
    vim_text("[a] Foo foo\n", "/\\Cfoo<ret>x", "a Foo oo\n");
    let out = fx("[a]\n", ":set ic?<ret>");
    assert!(status_of(&out).join("").contains("ignorecase"));
    let out = fx("[a]\n", ":set noic<ret>:set ic?<ret>");
    assert!(status_of(&out).join("").contains("noignorecase"));
    let out = fx("[a]\n", ":set wrap<ret>");
    assert_eq!(out, vec![Effect::HostCommand("set wrap".into())]);
}

#[test]
fn search_messages_and_highlights() {
    let mut e = ed("[a]bc abc\nabc\n");
    let out = feed(&mut e, "/zzz<ret>");
    assert!(status_of(&out).join("").contains("E486: Pattern not found: zzz"));
    feed(&mut e, "/abc<ret>");
    assert_eq!(e.search_matches().len(), 3);
    let out = feed(&mut e, "nn");
    assert!(status_of(&out).join("").contains("search hit BOTTOM"), "{out:?}");
    let out = feed(&mut e, "N");
    assert!(status_of(&out).join("").contains("search hit TOP"), "{out:?}");
    feed(&mut e, ":noh<ret>");
    assert!(e.search_matches().is_empty());
    feed(&mut e, "n");
    assert_eq!(e.search_matches().len(), 3, "searching again highlights again");
    feed(&mut e, ":set nohls<ret>");
    assert!(e.search_matches().is_empty());
    feed(&mut e, ":set hls<ret>");
    assert_eq!(e.search_matches().len(), 3);
    // typing a search previews its matches
    feed(&mut e, "/bc");
    assert_eq!(e.search_matches().len(), 3);
    feed(&mut e, "<esc>");
}

#[test]
fn search_history_and_prompt_editing() {
    let mut e = ed("[a]bc abc\n");
    feed(&mut e, "/bc<ret>/b<up><ret>");
    assert_eq!(e.cursor_of(&e.primary()), 5);
    feed(&mut e, "/zz<C-u>c<ret>");
    assert_eq!(e.cursor_of(&e.primary()), 6, "the prompt is editable: ctrl-u cleared the first text");
}

#[test]
fn command_line_completes_vim_command_names() {
    let mut e = ed("[a]\n");
    feed(&mut e, ":sub<tab>");
    assert_eq!(e.prompt().unwrap().text, "substitute");
    feed(&mut e, "<esc>:glo<tab>");
    assert_eq!(e.prompt().unwrap().text, "global");
    feed(&mut e, "<esc>:run<tab>");
    assert!(e.prompt().unwrap().text.starts_with("run"));
}

#[test]
fn marks_command_lists_marks() {
    let mut e = ed("[a]bc\ndef\n");
    feed(&mut e, "majmb");
    let msg = status_of(&feed(&mut e, ":marks<ret>")).join("|");
    assert!(msg.contains("a 1:1") && msg.contains("b 2:1"), "{msg}");
}

// ---- insert mode ------------------------------------------------------------------------------------------------------------------

#[test]
fn insert_auto_pairs_and_indent_follow_the_other_keymaps() {
    vim("[x]\n", "A(a)<esc>", "x(a[)]\n");
    vim("[x]\n", "A[a]<esc>", "x[a‹]›\n");
    vim("[x]\n", "A<lt>a>", "x<a>‸\n");
}

#[test]
fn insert_enter_between_braces_opens_a_block() {
    vim_text("[x]\n", "A{<ret>y<esc>", "x{\n    y\n}\n");
    vim_text("[ ] a(\n", "A<ret>b<esc>", "  a(\n      b\n");
    vim_text("    [a]\n", "A<ret>b<esc>", "    a\n    b\n");
}

#[test]
fn insert_tab_aligns_to_the_next_stop() {
    vim_text("[a]\n", "A<tab>x<esc>", "a   x\n");
    vim_text("[a]b\n", "i<tab>x<esc>", "    xab\n");
    vim_text("ab[c]d\n", "i<tab>x<esc>", "ab  xcd\n");
}

#[test]
fn insert_backspace_and_delete() {
    vim_text("a[b]c\n", "i<bs><esc>", "bc\n");
    vim_text("a[b]c\n", "i<del><esc>", "ac\n");
    vim_text("a[(]b)\n", "a<bs><esc>", "ab)\n");
    vim_text("[x]\n", "A(<bs><esc>", "x\n");
}

#[test]
fn insert_ctrl_n_and_p_request_completion() {
    assert!(fx("[a]\n", "i<C-n>").contains(&Effect::RequestCompletion));
    assert!(fx("[a]\n", "i<C-p>").contains(&Effect::RequestCompletion));
}

#[test]
fn insert_ctrl_o_runs_one_command() {
    vim_text("[a]bc def\n", "A<C-o>0X<esc>", "Xabc def\n");
    vim_text("[a]bc def\n", "i<C-o>wX<esc>", "abc Xdef\n");
    vim_text("[a]bc def\n", "A<C-o>b!<esc>", "abc !def\n");
    vim_mode("[a]bc\n", "i<C-o>", "[a]bc\n", Mode::Normal);
    vim_mode("[a]bc\n", "i<C-o>l", "a‸bc\n", Mode::Insert);
}

#[test]
fn insert_text_from_the_host_is_literal() {
    let mut e = ed("[a]\n");
    feed(&mut e, "i");
    let out = e.insert_text("(x\r\ny");
    assert!(out.handled);
    assert_eq!(e.text(), "(x\nya\n", "pasted text is not auto-paired and CRLF becomes LF");
    feed(&mut e, "<esc>");
    let out = e.insert_text("zzz");
    assert!(!out.handled, "Normal mode leaves pastes to the host");
}

#[test]
fn replace_mode_overwrites_and_restores() {
    vim_text("[a]bcdef\n", "Rxyz<esc>", "xyzdef\n");
    vim_text("[a]bcdef\n", "Rxyz<bs><bs><esc>", "xbcdef\n");
    vim_text("[a]bc\n", "Rxyzw<esc>", "xyzw\n");
    vim_text("[a]bc\n", "Rx<ret>y<esc>", "x\nyc\n");
    vim_text("[a]bc\n", "R<left>x<esc>", "xbc\n");
}

// ---- scrolling and the viewport ------------------------------------------------------------------------------------------------

fn tall() -> Editor {
    let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    let mut e = Editor::new(&text, Keymap::Vim);
    e.set_viewport(0, 10);
    e
}

#[test]
fn half_page_scrolling_moves_cursor_and_requests_a_scroll() {
    let mut e = tall();
    let out = feed(&mut e, "<C-d>");
    assert!(out.contains(&Effect::Scroll(crate::ScrollRequest::Lines(5))), "{out:?}");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 5);
    let out = feed(&mut e, "<C-u>");
    assert!(out.contains(&Effect::Scroll(crate::ScrollRequest::Lines(-5))));
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 0);
    feed(&mut e, "3<C-d>");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 3, "a count sets the scroll amount");
    feed(&mut e, "<C-d>");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 6, "and is remembered");
}

#[test]
fn full_page_scrolling() {
    let mut e = tall();
    let out = feed(&mut e, "<C-f>");
    assert!(out.contains(&Effect::Scroll(crate::ScrollRequest::Lines(8))), "{out:?}");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 8);
    feed(&mut e, "<C-b>");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 0);
}

#[test]
fn line_scrolling_keeps_the_cursor_on_screen() {
    let mut e = tall();
    let out = feed(&mut e, "<C-e>");
    assert!(out.contains(&Effect::Scroll(crate::ScrollRequest::Lines(1))));
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 1, "the cursor line scrolled off");
    feed(&mut e, "5<C-e>");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 6);
    feed(&mut e, "3<C-y>");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 6, "still on screen");
}

#[test]
fn z_commands_request_scrolls() {
    assert!(fx("[a]\n", "zz").contains(&Effect::Scroll(crate::ScrollRequest::CursorCenter)));
    assert!(fx("[a]\n", "zt").contains(&Effect::Scroll(crate::ScrollRequest::CursorTop)));
    assert!(fx("[a]\n", "zb").contains(&Effect::Scroll(crate::ScrollRequest::CursorBottom)));
    assert!(fx("[a]\n", "z.").contains(&Effect::Scroll(crate::ScrollRequest::CursorCenter)));
}

#[test]
fn screen_line_motions_use_the_viewport() {
    let mut e = tall();
    e.set_viewport(20, 10);
    feed(&mut e, "H");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 20);
    feed(&mut e, "L");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 29);
    feed(&mut e, "M");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 24);
    feed(&mut e, "3H");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 22);
    feed(&mut e, "2L");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 28);
    feed(&mut e, "dH");
    assert_eq!(e.line_count(), 101 - 9, "dH deletes from the top line to the cursor");
}

// ---- mouse ------------------------------------------------------------------------------------------------------------------------------

#[test]
fn mouse_clicks_and_drags() {
    let mut e = ed("[h]ello world\nsecond\n");
    e.click(7, false, false);
    assert_eq!(e.cursor_of(&e.primary()), 7);
    assert_eq!(e.mode(), Mode::Normal);
    e.click(100, false, false);
    assert_eq!(
        e.cursor_of(&e.primary()),
        17,
        "a click past the text lands on the last character of the last line"
    );
    e.click(5, false, false);
    e.drag_to(8);
    assert_eq!(e.mode(), Mode::Visual);
    assert_eq!(e.selected_text().as_deref(), Some(" wor"));
    feed(&mut e, "<esc>");
    e.click(2, false, false);
    e.click(6, true, false);
    assert_eq!(e.mode(), Mode::Visual);
    assert_eq!(e.selected_text().as_deref(), Some("llo w"));
}

#[test]
fn double_and_triple_click_select() {
    let mut e = ed("[h]ello world\nsecond\n");
    e.double_click(8);
    assert_eq!(e.mode(), Mode::Visual);
    assert_eq!(e.selected_text().as_deref(), Some("world"));
    e.triple_click(15);
    assert_eq!(e.mode(), Mode::VisualLine);
    assert_eq!(e.selected_text().as_deref(), Some("second\n"));
}

#[test]
fn clicking_in_insert_mode_keeps_inserting() {
    let mut e = ed("[h]ello\n");
    feed(&mut e, "i");
    e.click(3, false, false);
    assert_eq!(e.mode(), Mode::Insert);
    feed(&mut e, "X<esc>");
    assert_eq!(e.text(), "helXlo\n");
}

#[test]
fn host_set_selections_collapse_to_one_cursor() {
    let mut e = ed("hello world\n");
    e.set_selections(vec![Selection::new(2, 5), Selection::new(7, 9)], 1);
    assert_eq!(e.selections().len(), 1);
    assert_eq!(e.cursor_of(&e.primary()), 8);
    feed(&mut e, "x");
    assert_eq!(e.text(), "hello wold\n", "the cursor is on the last char of the host's primary selection");
}

// ---- undo grouping, counts, robustness -----------------------------------------------------------------------------------------------------

#[test]
fn an_insert_session_is_one_undo_step() {
    vim_text("[a]bc\n", "ifoo<esc>u", "abc\n");
    vim_text("[a]bc\n", "3ifoo<esc>u", "abc\n");
    vim_text("[a]bc\n", "ofoo<ret>bar<esc>u", "abc\n");
    vim_text("[a]bc\n", "cwfoo<esc>u", "abc\n");
    vim_text("[a]bc\n", "ifoo<esc>ibar<esc>u", "fooabc\n");
    vim_text("[a]bc\n", "ifoo<esc>ibar<esc>uu<C-r>", "fooabc\n");
}

#[test]
fn a_dot_repeat_is_one_undo_step() {
    vim_text("[a]bcdefgh\n", "3x.u", "defgh\n");
    vim_text("[a]b\nab\n", "Ax<esc>j.u", "abx\nab\n");
}

#[test]
fn absurd_counts_do_not_hang() {
    let mut e = ed("[a]\nb\n");
    let out = feed(&mut e, "99999999999999999999x");
    assert!(out.len() < 10);
    feed(&mut e, "99999999j");
    assert_eq!(e.char_to_line(e.cursor_of(&e.primary())), 1);
}

#[test]
fn unbound_chords_are_left_to_the_host() {
    let mut e = ed("[a]bc\n");
    let k = KeyInput::parse("ctrl-s").unwrap();
    assert!(!e.handle_key(&k).handled);
    let k = KeyInput::parse("alt-x").unwrap();
    assert!(!e.handle_key(&k).handled);
    let k = KeyInput::parse("q").unwrap();
    assert!(e.handle_key(&k).handled, "plain keys are always consumed");
}

#[test]
fn zz_and_friends_quit_commands() {
    assert_eq!(fx("[a]\n", "ZZ"), vec![Effect::Save, Effect::CloseTab]);
    assert_eq!(fx("[a]\n", "ZQ"), vec![Effect::CloseTab]);
}

#[test]
fn macro_registers_hold_keys_as_text() {
    let mut e = ed("[a]bc\n\n");
    feed(&mut e, "qaIx<esc>q");
    assert_eq!(e.vim_reg_dump('a'), Some(("Ix\u{1b}".to_string(), 'c')));
    feed(&mut e, "j\"ap");
    assert_eq!(e.line(1), "Ix\u{1b}");
    // any register can be run as a macro: `x` then a newline (which moves down)
    let mut e = ed("[x]\ny\n");
    feed(&mut e, "\"ayy@a");
    assert_eq!(e.text(), "\ny\n");
}

#[test]
fn at_colon_repeats_the_last_command_line() {
    vim_text("[a]aaa\n", ":s/a/b/<ret>@:", "bbaa\n");
    vim_text("[a]aaa\n", ":s/a/b/<ret>@:@@", "bbba\n");
}

#[test]
fn recording_register_follows_q() {
    let mut e = ed("[a]bc\n");
    assert_eq!(e.recording_register(), None);
    feed(&mut e, "qa");
    assert_eq!(e.recording_register(), Some('a'));
    feed(&mut e, "xq");
    assert_eq!(e.recording_register(), None);
}

#[test]
fn ctrl_shift_letters_are_host_chords_not_vim_keys() {
    // ctrl-shift-f / -e / -n / -t are application shortcuts (format, focus explorer, new
    // connection, reopen tab); they must not act as ctrl-f / ctrl-e / ctrl-n / ctrl-t.
    for (mode_keys, text) in [("", "[a]\nb\nc\nd\n"), ("i", "[a]bc\n")] {
        for key in
            ["ctrl-shift-f", "ctrl-shift-e", "ctrl-shift-n", "ctrl-shift-t", "ctrl-shift-d", "ctrl-shift-r"]
        {
            let mut e = ed(text);
            feed(&mut e, mode_keys);
            let before = (e.text(), e.selections().to_vec(), e.mode());
            let out = e.handle_key(&KeyInput::parse(key).unwrap());
            assert!(!out.handled, "{key} after {mode_keys:?}");
            assert!(
                out.effects.iter().all(|f| matches!(f, Effect::RevealCursor)),
                "{key}: {:?}",
                out.effects
            );
            assert_eq!((e.text(), e.selections().to_vec(), e.mode()), before, "{key} changed the editor");
        }
    }
    // the plain chords still work
    let mut e = ed("[a]bc\n");
    feed(&mut e, "i<C-t>");
    assert_eq!(e.text(), "    abc\n");
}
