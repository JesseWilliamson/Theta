use super::*;

#[test]
fn typing_and_escape() {
    hx("#[|]#", "ihello<esc>", "hello#[|]#");
    hx("#[a|]#bc", "iX<esc>", "X#[|a]#bc");
    hx("#[a|]#bc\n", "iXY<esc>", "XY#[|a]#bc\n");
    hx("ab#[c|]#\n", "aX<esc>", "ab#[cX|]#\n");
    hx("#[a|]#b\n#(c|)#d\n", "iX<esc>", "X#[|a]#b\nX#(|c)#d\n");
}

#[test]
fn append_at_end_of_document_without_newline() {
    hx("ab#[c|]#", "aX<esc>", "abc#[|X]#");
    hx("abc#[|]#", "iX<esc>", "abcX#[|]#");
}

#[test]
fn enter_auto_indents() {
    hx("  #[a|]#b\n", "A<ret>x<esc>", "  ab\n  x#[\n|]#");
    hx("#[a|]#b\n", "A<ret>x<esc>", "ab\nx#[\n|]#");
    hx("f(#[|]#", "i<ret>x<esc>", "f(\n    x#[|]#");
    hx("  #[|]#", "i<ret>", "  \n  ");
}

#[test]
fn enter_between_brackets_opens_a_block() {
    let mut e = editor_from(Keymap::Helix, "#[|]#");
    feed(&mut e, "i(<ret>x");
    assert_eq!(e.text(), "(\n    x\n)");
    let mut e = editor_from(Keymap::Helix, "#[|]#");
    feed(&mut e, "i{<ret>");
    assert_eq!(e.text(), "{\n    \n}");
    let mut e = editor_from(Keymap::Helix, "  #[a|]#\n");
    feed(&mut e, "A(<ret>x");
    assert_eq!(e.text(), "  a(\n      x\n  )\n");
}

#[test]
fn tab_inserts_four_spaces_without_completion() {
    let mut e = editor_from(Keymap::Helix, "#[|]#");
    let fx = feed(&mut e, "i<tab>");
    assert_eq!(e.text(), "    ");
    assert!(!fx.contains(&Effect::RequestCompletion));
}

#[test]
fn backspace_variants() {
    hx("ab#[c|]#\n", "i<backspace><esc>", "a#[|c]#\n");
    let mut e = editor_from(Keymap::Helix, "abc#[|]#");
    feed(&mut e, "i<backspace>");
    assert_eq!(e.text(), "ab");
    // whole indent level in leading whitespace
    let mut e = editor_from(Keymap::Helix, "        #[|]#");
    feed(&mut e, "i<backspace>");
    assert_eq!(e.text(), "    ");
    let mut e = editor_from(Keymap::Helix, "      #[|]#");
    feed(&mut e, "i<backspace>");
    assert_eq!(e.text(), "    ");
    let mut e = editor_from(Keymap::Helix, "  x   #[|]#");
    feed(&mut e, "i<backspace>");
    assert_eq!(e.text(), "  x  ");
    let mut e = editor_from(Keymap::Helix, "#[|]#");
    feed(&mut e, "i<backspace>");
    assert_eq!(e.text(), "");
}

#[test]
fn delete_forward_and_ctrl_d() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#bc");
    feed(&mut e, "i<del>");
    assert_eq!(e.text(), "bc");
    let mut e = editor_from(Keymap::Helix, "#[a|]#bc");
    feed(&mut e, "i<C-d>");
    assert_eq!(e.text(), "bc");
}

#[test]
fn word_and_line_kills() {
    let mut e = editor_from(Keymap::Helix, "hello world#[|]#");
    feed(&mut e, "i<C-w>");
    assert_eq!(e.text(), "hello ");
    let mut e = editor_from(Keymap::Helix, "hello world#[|]#");
    feed(&mut e, "i<A-backspace>");
    assert_eq!(e.text(), "hello ");
    let mut e = editor_from(Keymap::Helix, "#[h|]#ello world");
    feed(&mut e, "i<A-d>");
    assert_eq!(e.text(), "world");
    let mut e = editor_from(Keymap::Helix, "ab cd#[|]#\nx");
    feed(&mut e, "i<C-u>");
    assert_eq!(e.text(), "\nx");
    let mut e = editor_from(Keymap::Helix, "#[a|]#b cd\nx");
    feed(&mut e, "i<C-k>");
    assert_eq!(e.text(), "\nx");
    let mut e = editor_from(Keymap::Helix, "ab#[\n|]#x");
    feed(&mut e, "i<C-k>");
    assert_eq!(e.text(), "abx");
    let mut e = editor_from(Keymap::Helix, "ab#[c|]#");
    feed(&mut e, "i<C-h>");
    assert_eq!(e.text(), "ac");
}

#[test]
fn insert_arrows_home_end() {
    let mut e = editor_from(Keymap::Helix, "ab#[c|]#\ndef\n");
    feed(&mut e, "i<left>X<right><right>Y<home>Z<end>W");
    assert_eq!(e.text(), "ZaXbcYW\ndef\n");
}

#[test]
fn ctrl_x_requests_completion_and_ctrl_space() {
    let fx = effects_of(Keymap::Helix, "#[|]#", "i<C-x>");
    assert!(fx.contains(&Effect::RequestCompletion));
    let fx = effects_of(Keymap::Helix, "#[|]#", "i<C-space>");
    assert!(fx.contains(&Effect::RequestCompletion));
    let fx = effects_of(Keymap::Helix, "#[a|]#", "<C-space>");
    assert!(fx.contains(&Effect::RequestCompletion));
}

#[test]
fn auto_pairs() {
    let t = |keys: &str| {
        let mut e = editor_from(Keymap::Helix, "#[|]#");
        feed(&mut e, &format!("i{keys}"));
        e.text()
    };
    assert_eq!(t("("), "()");
    assert_eq!(t("[x"), "[x]");
    assert_eq!(t("{"), "{}");
    assert_eq!(t("'"), "''");
    assert_eq!(t("\""), "\"\"");
    assert_eq!(t("`"), "``");
    assert_eq!(t("()"), "()");
    assert_eq!(t("(a)"), "(a)");
    assert_eq!(t("'a'"), "'a'");
    assert_eq!(t("((x))"), "((x))");
    assert_eq!(t("f(a, g(b))"), "f(a, g(b))");
    assert_eq!(t("a'"), "a'");
    assert_eq!(t("don't"), "don't");
    assert_eq!(t("(<backspace>"), "");
    assert_eq!(t("'<backspace>"), "");
    assert_eq!(t("(a<backspace>"), "()");
    assert_eq!(t(")"), ")");
}

#[test]
fn auto_pair_not_before_word_char() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#bc");
    feed(&mut e, "i(");
    assert_eq!(e.text(), "(abc");
    let mut e = editor_from(Keymap::Helix, "#[a|]#bc");
    feed(&mut e, "i'");
    assert_eq!(e.text(), "'abc");
}

#[test]
fn auto_pair_cursor_lands_between() {
    let mut e = editor_from(Keymap::Helix, "#[|]#");
    feed(&mut e, "i(");
    feed(&mut e, "x");
    assert_eq!(e.text(), "(x)");
    assert_eq!(e.cursor_of(&e.primary()), 2);
}

#[test]
fn multi_cursor_insert_pairs() {
    let mut e = editor_from(Keymap::Helix, "#[\n|]##(\n|)#");
    feed(&mut e, "i(x");
    assert_eq!(e.text(), "(x)\n(x)\n");
    let mut e = editor_from(Keymap::Helix, "#[a|]# #(b|)#");
    feed(&mut e, "i(");
    assert_eq!(e.text(), "(a (b");
}

#[test]
fn insert_text_pastes_verbatim() {
    let mut e = editor_from(Keymap::Helix, "#[|]#");
    feed(&mut e, "i");
    e.insert_text("(a\nb");
    assert_eq!(e.text(), "(a\nb");
    // Normal mode ignores platform text input
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    let out = e.insert_text("zzz");
    assert!(!out.handled);
    assert_eq!(e.text(), "a");
}

#[test]
fn crlf_documents_stay_crlf() {
    let mut e = editor_from(Keymap::Helix, "a#[|]#\r\nb\r\n");
    feed(&mut e, "i<ret>x");
    assert_eq!(e.text(), "a\r\nx\r\nb\r\n");
    let mut e = editor_from(Keymap::Helix, "#[a|]#\r\nb\r\n");
    feed(&mut e, "ox<esc>");
    assert_eq!(e.text(), "a\r\nx\r\nb\r\n");
    // h/l never split a CRLF
    let mut e = editor_from(Keymap::Helix, "#[a|]#\r\nb");
    feed(&mut e, "l");
    assert_eq!(e.primary().range(), 1..3);
    feed(&mut e, "l");
    assert_eq!(e.primary().range(), 3..4);
    feed(&mut e, "h");
    assert_eq!(e.primary().range(), 1..3);
}

#[test]
fn escape_emits_dismiss_completion() {
    let fx = effects_of(Keymap::Helix, "#[|]#", "i<esc>");
    assert!(fx.contains(&Effect::DismissCompletion));
}

#[test]
fn modes() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    assert_eq!(e.mode(), Mode::Normal);
    feed(&mut e, "i");
    assert_eq!(e.mode(), Mode::Insert);
    feed(&mut e, "<esc>v");
    assert_eq!(e.mode(), Mode::Select);
    feed(&mut e, "v");
    assert_eq!(e.mode(), Mode::Normal);
    feed(&mut e, "v<esc>");
    assert_eq!(e.mode(), Mode::Normal);
    feed(&mut e, "vd");
    assert_eq!(e.mode(), Mode::Normal);
}

#[test]
fn ctrl_home_and_end_jump_to_the_document_ends() {
    hx("a\nb#[|]#c\nd\n", "i<C-home>X<esc>", "X#[a|]#\nbc\nd\n");
    hx("a\nb#[|]#c\nd\n", "i<C-end>X<esc>", "a\nbc\nd\nX#[|]#");
    // Plain Home / End stay on the line.
    hx("a\nb#[|]#c\nd\n", "i<home>X<esc>", "a\nX#[b|]#c\nd\n");
}
