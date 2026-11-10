use super::*;

#[test]
fn delete_and_yank_paste() {
    hx("#[hello|]# world\n", "d", "#[ |]#world\n");
    hx("#[h|]#ello world\n", "d", "#[e|]#llo world\n");
    hx("#[hello|]# world\n", "dp", " #[hello|]#world\n");
    hx("#[hello|]# world\n", "yp", "hello#[hello|]# world\n");
    hx("#[hello|]# world\n", "yP", "#[hello|]#hello world\n");
    hx("#[h|]#ello\n", "y3p", "h#[hhh|]#ello\n");
    hx("#[ab|]#c\n", "ylp", "abc#[ab|]#\n");
}

#[test]
fn count_is_ignored_by_delete() {
    hx("#[a|]#bcdef\n", "3d", "#[b|]#cdef\n");
}

#[test]
fn delete_lines_with_x() {
    hx("#[a|]#\nb\nc\n", "xd", "#[b|]#\nc\n");
    hx("#[a|]#\nb\nc\n", "xxd", "#[c|]#\n");
    hx("#[a|]#\nb\nc\n", "2xd", "#[c|]#\n");
}

#[test]
fn linewise_yank_paste() {
    hx("#[a|]#\nb\n", "xyp", "a\n#[a\n|]#b\n");
    hx("a\n#[b|]#\n", "xyP", "a\n#[b\n|]#b\n");
    hx("#[a|]#\nb\n", "xyjp", "a\nb\n#[a\n|]#");
}

#[test]
fn change_selection() {
    hx("#[hello|]# world\n", "cbye<esc>", "bye#[ |]#world\n");
    hx("#[hello|]# world\n", "c<esc>", "#[ |]#world\n");
    // changing whole lines keeps an empty line to type on
    hx("#[a|]#\nb\n", "xcX<esc>", "X#[\n|]#b\n");
}

#[test]
fn change_yanks_but_alt_c_does_not() {
    hx("#[hello|]# world\n", "cx<esc>p", "x #[hello|]#world\n");
    hx("#[ab|]#c\n", "y<A-c>x<esc>p", "xc#[ab|]#\n");
}

#[test]
fn delete_noyank() {
    hx("#[a|]#b\n", "yl<A-d>p", "a\n#[a|]#");
}

#[test]
fn replace_char() {
    hx("#[hello|]# world\n", "rx", "#[xxxxx|]# world\n");
    hx("#[h|]#ello\n", "rx", "#[x|]#ello\n");
    hx("#[h|]#ello\n", "r<ret>", "#[\n|]#ello\n");
    hx("#[he|]#llo\n", "r-", "#[--|]#llo\n");
    hx("#[a|]# #(b|)#\n", "rz", "#[z|]# #(z|)#\n");
    hx("#[é̀|]#x\n", "rz", "#[z|]#x\n");
}

#[test]
fn case_switching() {
    hx("#[hello|]# World\n", "~", "#[HELLO|]# World\n");
    hx("#[Hello|]# World\n", "~", "#[hELLO|]# World\n");
    hx("#[Hello|]# World\n", "`", "#[hello|]# World\n");
    hx("#[hello|]# World\n", "<A-`>", "#[HELLO|]# World\n");
}

#[test]
fn indent_and_unindent() {
    hx("#[a|]#\nb\n", ">", "    #[a|]#\nb\n");
    hx("#[a\nb|]#\n", ">", "    #[a\n    b|]#\n");
    hx("#[a\n\nb|]#\n", ">", "    #[a\n\n    b|]#\n");
    hx("    #[a|]#\n", "<lt>", "#[a|]#\n");
    hx("  #[a|]#\n", "<lt>", "#[a|]#\n");
    hx("        #[a|]#\n", "<lt>", "    #[a|]#\n");
    hx("#[a|]#\n", "2>", "        #[a|]#\n");
}

#[test]
fn join_lines() {
    hx("#[a\nb|]#\n", "J", "#[a b|]#\n");
    hx("#[a|]#\nb\nc\n", "J", "#[a|]# b\nc\n");
    hx("#[a\nb\nc|]#\n", "J", "#[a b c|]#\n");
    hx("#[a|]#\n  b\n", "J", "#[a|]# b\n");
    hx("#[a|]#\n\nb\n", "J", "#[a|]#\nb\n");
    hx("#[a|]#\nb\n", "<A-J>", "a#[ |]#b\n");
}

#[test]
fn open_lines() {
    hx("#[a|]#\nb\n", "ox<esc>", "a\nx#[\n|]#b\n");
    hx("a\n#[b|]#\n", "Ox<esc>", "a\nx#[\n|]#b\n");
    hx("    #[a|]#\n", "ox<esc>", "    a\n    x#[\n|]#");
    hx("#[f|]#(\n", "ox<esc>", "f(\n    x#[\n|]#");
    hx("#[a|]#\n", "Ox<esc>", "x#[\n|]#a\n");
    hx("#[a|]#\n", "2ox<esc>", "a\nx#(\n|)#x#[\n|]#");
}

#[test]
fn undo_redo() {
    hx("#[hello|]# world\n", "du", "#[hello|]# world\n");
    hx("#[hello|]# world\n", "duU", "#[ |]#world\n");
    hx("#[a|]#bc\n", "ixy<esc>u", "#[a|]#bc\n");
    hx("#[a|]#bc\n", "ixy<esc>uU", "xy#[|a]#bc\n");
    hx("#[a|]#bc\n", "uuu", "#[a|]#bc\n");
    hx("#[a|]#bcd\n", "dddu", "#[c|]#d\n");
}

#[test]
fn undo_groups_insert_session() {
    hx("#[a|]#bc\n", "ix<esc>iy<esc>u", "x#[|a]#bc\n");
    hx("#[a|]#bc\n", "ix<esc>iy<esc>uu", "#[a|]#bc\n");
    hx("#[a|]#bc\n", "ixyz<esc>u", "#[a|]#bc\n");
    // `c` and the following typing are one change
    hx("#[ab|]#c\n", "cxyz<esc>u", "#[ab|]#c\n");
}

#[test]
fn collapse_flip_and_primary() {
    hx("#[hello|]# world\n", ";", "hell#[o|]# world\n");
    hx("#[hello|]# world\n", "<A-;>", "#[|hello]# world\n");
    hx("#[|hello]# world\n", "<A-:>", "#[hello|]# world\n");
    hx("#[a|]# #(b|)# #(c|)#\n", ",", "#[a|]# b c\n");
    hx("#[a|]# #(b|)# #(c|)#\n", "<A-,>", "a #[b|]# #(c|)#\n");
    hx("#[a|]# #(b|)# #(c|)#\n", ")", "#(a|)# #[b|]# #(c|)#\n");
    hx("#[a|]# #(b|)# #(c|)#\n", "(", "#(a|)# #(b|)# #[c|]#\n");
}

#[test]
fn select_all_and_lines() {
    hx("#[a|]#bc\ndef\n", "%", "#[abc\ndef\n|]#");
    hx("#[a|]#bc\ndef\n", "x", "#[abc\n|]#def\n");
    hx("#[a|]#bc\ndef\n", "xx", "#[abc\ndef\n|]#");
    hx("a#[b|]#c\ndef\n", "X", "#[abc\n|]#def\n");
    hx("#[ab|]#c\ndef\n", "vjX", "#[abc\ndef\n|]#");
    hx("#[abc\ndef\n|]#", "<A-x>", "#[abc\ndef\n|]#");
    hx("#[abc\ndef\nghi\n|]#", "<A-x>", "#[abc\ndef\nghi\n|]#");
}

#[test]
fn increment_decrement() {
    hx("#[1|]#2\n", "<C-a>", "#[13|]#\n");
    hx("#[1|]#2\n", "5<C-a>", "#[17|]#\n");
    hx("#[1|]#0\n", "<C-x>", "#[9|]#\n");
    hx("#[a|]# b 7\n", "<C-a>", "a b #[8|]#\n");
    hx("#[-|]#3\n", "<C-a>", "#[-2|]#\n");
    hx("#[0|]#x0f\n", "<C-a>", "#[0x10|]#\n");
    hx("#[9|]#9\n", "<C-a>", "#[100|]#\n");
    hx("#[x|]# 5\n", "<C-x>", "x #[4|]#\n");
}

#[test]
fn toggle_comments() {
    hx("#[select 1|]#\n", "<C-c>", "-- #[select 1|]#\n");
    hx("#[-- select 1|]#\n", "<C-c>", "#[select 1|]#\n");
    hx("#[a\nb|]#\n", "<C-c>", "-- #[a\n-- b|]#\n");
    hx("#[  a\n  b|]#\n", "<C-c>", "#[  -- a\n  -- b|]#\n");
    hx("#[a\n\nb|]#\n", "<C-c>", "-- #[a\n\n-- b|]#\n");
    hx("#[a|]#\n", "<space>c", "-- #[a|]#\n");
    hx("#[--a|]#\n", "<C-c>", "#[a|]#\n");
}

#[test]
fn format_effect() {
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "=");
    assert!(fx.contains(&Effect::FormatSelection));
}

#[test]
fn repeat_last_change() {
    hx("#[a|]#aaa\n", "d.", "#[a|]#a\n");
    hx("#[a|]#bcd\n", "rx", "#[x|]#bcd\n");
    hx("#[a|]#b\n", "ix<esc>.", "xx#[|a]#b\n");
    hx("#[a|]#\nb\nc\n", "xd.", "#[\n|]#c\n");
    hx("#[a|]#\nb\n", ">j.", "    a\n    b#[\n|]#");
    hx("#[a|]#bc\n", "rx.", "#[x|]#bc\n");
}

#[test]
fn macros() {
    hx("#[a|]#bcdef\n", "QdQ", "#[b|]#cdef\n");
    hx("#[a|]#bcdef\n", "QdQq", "#[c|]#def\n");
    hx("#[a|]#bcdef\n", "QdQ2q", "#[d|]#ef\n");
    hx("#[a|]#bcdef\n", "\"aQdQ\"aq", "#[c|]#def\n");
}

#[test]
fn registers() {
    hx("#[a|]#b\n", "\"qyl\"qp", "ab#[a|]#\n");
    hx("#[a|]#b\n", "\"qyl\"wy\"qp", "ab#[a|]#\n");
    hx("#[a|]#b\n", "\"ayl\"byl\"ap", "ab\n#[a|]#");
}

#[test]
fn clipboard_registers_and_effects() {
    let mut e = editor_from(Keymap::Helix, "#[hello|]# world\n");
    let fx = feed(&mut e, "y");
    assert!(fx.contains(&Effect::SetClipboard("hello".into())), "{fx:?}");
    let fx = feed(&mut e, "\"+y");
    assert!(fx.contains(&Effect::SetClipboard("hello".into())));
    e.set_clipboard(Some("EXT".into()));
    feed(&mut e, "\"+p");
    assert!(e.text().contains("EXT"));
    // default register picks up a clipboard changed by another application
    let mut e = editor_from(Keymap::Helix, "#[a|]#b\n");
    e.set_clipboard(Some("zz".into()));
    feed(&mut e, "p");
    assert_eq!(e.text(), "azzb\n");
    // but prefers our own yank while the clipboard still holds what we mirrored
    let mut e = editor_from(Keymap::Helix, "#[a|]#b\n");
    feed(&mut e, "y");
    e.set_clipboard(Some("a".into()));
    feed(&mut e, "p");
    assert_eq!(e.text(), "aab\n");
}

#[test]
fn space_clipboard_commands() {
    let mut e = editor_from(Keymap::Helix, "#[hello|]# world\n");
    let fx = feed(&mut e, "<space>y");
    assert!(fx.contains(&Effect::SetClipboard("hello".into())));
    e.set_clipboard(Some("X".into()));
    feed(&mut e, "<space>p");
    assert_eq!(e.text(), "helloX world\n");
    let mut e = editor_from(Keymap::Helix, "#[a|]#b\n");
    e.set_clipboard(Some("X".into()));
    feed(&mut e, "<space>P");
    assert_eq!(e.text(), "Xab\n");
}

#[test]
fn replace_with_yanked() {
    hx("#[a|]# b\n", "yw", "#[a |]#b\n");
}

#[test]
fn multi_selection_edits() {
    hx("#[a|]#b #(a|)#c #(a|)#d\n", "d", "#[b|]# #(c|)# #(d|)#\n");
    hx("#[a|]#b #(a|)#c\n", "ix<esc>", "x#[|a]#b x#(|a)#c\n");
    hx("#[a|]#b #(a|)#c\n", "yp", "a#[a|]#b a#(a|)#c\n");
    hx("#[a|]#b #(c|)#d\n", "dp", "b#[a|]# d#(c|)#\n");
}

#[test]
fn append_and_insert_commands() {
    hx("#[a|]#bc\n", "Ax<esc>", "abcx#[\n|]#");
    hx("  #[a|]#bc\n", "Ix<esc>", "  x#[a|]#bc\n");
    hx("#[foo|]#\n", "abar<esc>", "#[foobar|]#\n");
    hx("#[f|]#oo\n", "ax<esc>", "#[fx|]#oo\n");
    hx("#[f|]#oo\n", "a<esc>", "#[f|]#oo\n");
    hx("#[f|]#oo\n#(b|)#ar\n", "a<esc>", "#[f|]#oo\n#(b|)#ar\n");
    hx("#[f|]#oo\n", "ix<esc>", "x#[|f]#oo\n");
    hx("#[f|]#oo\n", "ib<esc>", "b#[|f]#oo\n");
    hx("#[\n|]#", "i<esc>", "#[|\n]#");
}
