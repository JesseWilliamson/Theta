use super::*;

#[test]
fn horizontal() {
    hx("#[h|]#ello", "l", "h#[e|]#llo");
    hx("h#[e|]#llo", "h", "#[h|]#ello");
    hx("#[h|]#ello", "3l", "hel#[l|]#o");
    hx("#[h|]#ello", "100l", "hello#[|]#");
    hx("#[h|]#ello", "<right>", "h#[e|]#llo");
    hx("h#[e|]#llo", "<left>", "#[h|]#ello");
    hx("#[h|]#ello", "h", "#[h|]#ello");
}

#[test]
fn vertical_keeps_goal_column() {
    hx("hel#[l|]#o\nab\nhello\n", "j", "hello\nab#[\n|]#hello\n");
    hx("hel#[l|]#o\nab\nhello\n", "jj", "hello\nab\nhel#[l|]#o\n");
    hx("hello\nab\nhel#[l|]#o\n", "kk", "hel#[l|]#o\nab\nhello\n");
    hx("a#[b|]#c\ndef\n", "j", "abc\nd#[e|]#f\n");
    hx("a#[b|]#c\ndef\nghi\n", "2j", "abc\ndef\ng#[h|]#i\n");
    hx("a#[b|]#c\ndef\nghi\n", "<down>", "abc\nd#[e|]#f\nghi\n");
}

#[test]
fn word_motions() {
    hx("#[h|]#ello world", "w", "#[hello |]#world");
    hx("hello #[w|]#orld foo", "w", "hello #[world |]#foo");
    hx("#[h|]#ello world", "e", "#[hello|]# world");
    hx("hello#[ |]#world", "b", "#[|hello ]#world");
    hx("hello wor#[l|]#d", "b", "hello #[|worl]#d");
    hx("#[h|]#ello world foo", "2w", "hello #[world |]#foo");
    hx("#[f|]#oo.bar baz", "w", "#[foo|]#.bar baz");
    hx("#[f|]#oo.bar baz", "W", "#[foo.bar |]#baz");
    hx("#[f|]#oo.bar baz", "E", "#[foo.bar|]# baz");
    hx("foo.bar #[b|]#az", "B", "#[|foo.bar ]#baz");
}

#[test]
fn word_motion_crosses_lines() {
    hx("#[h|]#ello\nworld", "ww", "hello\n#[world|]#");
    hx("hello#[\n|]#world", "w", "hello\n#[world|]#");
    hx("#[a|]#\n\nb", "w", "a\n\n#[b|]#");
}

#[test]
fn word_motions_extend_in_select_mode() {
    hx("#[h|]#ello world foo", "vw", "#[hello |]#world foo");
    hx("#[h|]#ello world foo", "vww", "#[hello world |]#foo");
    hx("#[h|]#ello world", "ve", "#[hello|]# world");
}

#[test]
fn find_char() {
    hx("#[h|]#ello world", "fo", "#[hello|]# world");
    hx("#[h|]#ello world", "to", "#[hell|]#o world");
    hx("#[h|]#ello world", "2fo", "#[hello wo|]#rld");
    hx("hello wor#[l|]#d", "Fo", "hello w#[|orl]#d");
    hx("hello wor#[l|]#d", "To", "hello wo#[|rl]#d");
    hx("#[h|]#ello world", "fz", "#[h|]#ello world");
    hx("#[h|]#ello world", "fo<A-.>", "hell#[o wo|]#rld");
    hx("#[h|]#ello\nworld", "fw", "#[hello\nw|]#orld");
}

#[test]
fn line_motions() {
    hx("  he#[l|]#lo\n", "gh", "#[ |]# hello\n");
    hx("  he#[l|]#lo\n", "gl", "  hell#[o|]#\n");
    hx("  he#[l|]#lo\n", "gs", "  #[h|]#ello\n");
    hx("  he#[l|]#lo\n", "<home>", "#[ |]# hello\n");
    hx("  he#[l|]#lo\n", "<end>", "  hell#[o|]#\n");
    hx("hel#[l|]#o\n\nx\n", "jgl", "hello\n#[\n|]#x\n");
}

#[test]
fn goto_file() {
    hx("a\nb\n#[c|]#\n", "gg", "#[a|]#\nb\nc\n");
    hx("#[a|]#\nb\nc\n", "ge", "a\nb\n#[c|]#\n");
    hx("#[a|]#\nb\nc\n", "3G", "a\nb\n#[c|]#\n");
    hx("#[a|]#\nb\nc\n", "2gg", "a\n#[b|]#\nc\n");
    hx("#[a|]#\nb\nc\n", "99G", "a\nb\n#[c|]#\n");
    hx("#[a|]#\nb\nc\n", "G", "#[a|]#\nb\nc\n");
    hx("#[a|]#bcdef\n", "4g|", "abc#[d|]#ef\n");
}

#[test]
fn select_mode_extends_goto() {
    hx("#[a|]#bc\ndef\n", "vgl", "#[abc|]#\ndef\n");
    hx("#[a|]#bc\ndef\nghi\n", "vG", "#[a|]#bc\ndef\nghi\n");
    hx("#[a|]#bc\ndef\nghi\n", "v3G", "#[abc\ndef\ng|]#hi\n");
    hx("a#[b|]#c\n", "vgh", "#[|ab]#c\n");
}

#[test]
fn paragraphs() {
    hx("#[a|]#\nb\n\nc\n\nd\n", "]p", "a\nb\n\n#[c|]#\n\nd\n");
    hx("a\nb\n\n#[c|]#\n\nd\n", "[p", "#[a|]#\nb\n\nc\n\nd\n");
}

#[test]
fn page_motions_use_viewport() {
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut e = crate::Editor::new(&text, crate::Keymap::Helix);
    e.set_viewport(0, 20);
    feed(&mut e, "<C-d>");
    assert_eq!(e.char_to_line(e.primary().head), 10);
    feed(&mut e, "<C-u>");
    assert_eq!(e.char_to_line(e.primary().head), 0);
    let fx = feed(&mut e, "<C-f>");
    assert_eq!(e.char_to_line(e.primary().head), 20);
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::Lines(20))));
    feed(&mut e, "<pagedown>");
    assert_eq!(e.char_to_line(e.primary().head), 40);
    feed(&mut e, "<pageup>");
    assert_eq!(e.char_to_line(e.primary().head), 20);
}

#[test]
fn window_gotos() {
    let text: String = (0..50).map(|i| format!("l{i}\n")).collect();
    let mut e = crate::Editor::new(&text, crate::Keymap::Helix);
    e.set_viewport(10, 20);
    feed(&mut e, "gt");
    assert_eq!(e.char_to_line(e.primary().head), 10);
    feed(&mut e, "gc");
    assert_eq!(e.char_to_line(e.primary().head), 20);
    feed(&mut e, "gb");
    assert_eq!(e.char_to_line(e.primary().head), 29);
}

#[test]
fn scroll_effects() {
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "zz");
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::CursorCenter)));
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "zt");
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::CursorTop)));
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "zb");
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::CursorBottom)));
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "3zj");
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::Lines(3))));
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "zk");
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::Lines(-1))));
    let fx = effects_of(Keymap::Helix, "#[a|]#\n", "Zz");
    assert!(fx.contains(&Effect::Scroll(crate::ScrollRequest::CursorCenter)));
}

#[test]
fn jumplist() {
    hx("#[a|]#\nb\nc\n", "Ggg", "#[a|]#\nb\nc\n");
    hx("#[a|]#\nb\nc\n", "3G<C-o>", "#[a|]#\nb\nc\n");
    hx("#[a|]#\nb\nc\n", "3G<C-o><C-i>", "a\nb\n#[c|]#\n");
    hx("#[a|]#\nb\nc\n", "2jgg<C-o>", "a\nb\n#[c|]#\n");
}

#[test]
fn navigation_keys_in_normal_mode() {
    // Home / End are `gh` / `gl`.
    hx("  he#[l|]#lo\nx\n", "<home>", "#[ |]# hello\nx\n");
    hx("  he#[l|]#lo\nx\n", "<end>", "  hell#[o|]#\nx\n");
    // ctrl-Home / ctrl-End are `gg` / `ge`.
    hx("a\nb\n#[c|]#\n", "<C-home>", "#[a|]#\nb\nc\n");
    hx("#[a|]#\nb\nc\n", "<C-end>", "a\nb\n#[c|]#\n");
    // Arrows move like hjkl.
    hx("a#[b|]#c\ndef\n", "<right>", "ab#[c|]#\ndef\n");
    hx("a#[b|]#c\ndef\n", "<left>", "#[a|]#bc\ndef\n");
    hx("a#[b|]#c\ndef\n", "<down>", "abc\nd#[e|]#f\n");
    hx("abc\nd#[e|]#f\n", "<up>", "a#[b|]#c\ndef\n");
}

#[test]
fn navigation_keys_extend_in_select_mode() {
    hx("  he#[l|]#lo\nx\n", "v<home>", "#[|  hel]#lo\nx\n");
    hx("  he#[l|]#lo\nx\n", "v<end>", "  he#[llo|]#\nx\n");
    hx("a\nb\n#[c|]#\n", "v<C-home>", "#[|a\nb\nc]#\n");
    hx("#[a|]#\nb\nc\n", "v<C-end>", "#[a\nb\nc|]#\n");
    hx("a#[b|]#c\ndef\n", "v<right>", "a#[bc|]#\ndef\n");
    hx("a#[b|]#c\ndef\n", "v<left>", "#[|ab]#c\ndef\n");
    hx("a#[b|]#c\ndef\n", "v<down>", "a#[bc\nde|]#f\n");
    hx("abc\nd#[e|]#f\n", "v<up>", "a#[|bc\nde]#f\n");
}

#[test]
fn page_keys_extend_in_select_mode() {
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut e = crate::Editor::new(&text, crate::Keymap::Helix);
    e.set_viewport(0, 20);
    feed(&mut e, "v<pagedown>");
    assert_eq!(e.char_to_line(e.primary().head), 20);
    assert_eq!(e.primary().anchor, 0, "select mode keeps the anchor");
    feed(&mut e, "<pageup>");
    assert_eq!(e.char_to_line(e.primary().head), 0);
    assert_eq!(e.primary().anchor, 0);
    // In normal mode the same keys just move.
    feed(&mut e, "<esc><pagedown>");
    assert_eq!(e.char_to_line(e.primary().head), 20);
    assert_eq!(e.primary().head - e.primary().anchor, 1, "a plain move leaves a one-char block cursor");
}
