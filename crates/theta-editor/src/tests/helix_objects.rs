use super::*;

#[test]
fn matching_bracket_mm() {
    hx("#[(|]#a (b) c)", "mm", "(a (b) c#[)|]#");
    hx("(a (b) c#[)|]#", "mm", "#[(|]#a (b) c)");
    hx("(a #[(|]#b) c)", "mm", "(a (b#[)|]# c)");
    hx("#[a|]# (b)", "mm", "a (b#[)|]#");
    hx("#[a|]# b", "mm", "#[a|]# b");
    hx("#[(|]#a)", "vmm", "#[(a)|]#");
}

#[test]
fn surround_add() {
    hx("#[hello|]# world", "ms(", "#[(hello)|]# world");
    hx("#[hello|]# world", "ms)", "#[(hello)|]# world");
    hx("#[hello|]# world", "ms[", "#[[hello]|]# world");
    hx("#[hello|]# world", "ms{", "#[{hello}|]# world");
    hx("#[hello|]# world", "ms<", "#[<hello>|]# world");
    hx("#[hello|]# world", "ms'", "#['hello'|]# world");
    hx("#[hello|]# world", "ms\"", "#[\"hello\"|]# world");
    hx("#[hello|]# world", "ms`", "#[`hello`|]# world");
    hx("#[h|]#ello #(w|)#orld", "ms(", "#[(h)|]#ello #((w)|)#orld");
    hx("#[hello|]#", "ms<ret>", "#[\nhello\n|]#");
}

#[test]
fn surround_delete() {
    hx("(a #[b|]# c)", "md(", "a #[b|]# c");
    hx("'ab#[c|]#'", "md'", "ab#[c|]#");
    hx("(a [b#[c|]# d] e)", "md[", "(a b#[c|]# d e)");
    hx("(a [b#[c|]# d] e)", "mdm", "(a b#[c|]# d e)");
    hx("(a (b#[c|]# d) e)", "2md(", "a (b#[c|]# d) e");
}

#[test]
fn surround_replace() {
    hx("(a #[b|]# c)", "mr([", "[a #[b|]# c]");
    hx("(a #[b|]# c)", "mr('", "'a #[b|]# c'");
    hx("'a #[b|]# c'", "mr'\"", "\"a #[b|]# c\"");
    hx("(a [b#[c|]# d] e)", "mrm{", "(a {b#[c|]# d} e)");
}

#[test]
fn text_objects_inner_around() {
    hx("(so [many {go#[o|]#d} text] here)", "mi{", "(so [many {#[good|]#} text] here)");
    hx("(so [many {go#[o|]#d} text] here)", "mi[", "(so [#[many {good} text|]#] here)");
    hx("(so [many {go#[o|]#d} text] here)", "mi(", "(#[so [many {good} text] here|]#)");
    hx("(so [many {go#[o|]#d} text] here)", "ma{", "(so [many #[{good}|]# text] here)");
    hx("(so [many {go#[o|]#d} text] here)", "mim", "(so [many {#[good|]#} text] here)");
    hx("(so [many {go#[o|]#d} text] here)", "2mim", "(so [#[many {good} text|]#] here)");
    hx("(so [many {go#[o|]#d} text] here)", "3mim", "(#[so [many {good} text] here|]#)");
    hx("(so [many {go#[o|]#d} text] here)", "mam", "(so [many #[{good}|]# text] here)");
    hx("some (#[t|]#ext) here", "mim", "some (#[text|]#) here");
    hx("so#[m|]#e (text) here", "mim", "so#[m|]#e (text) here");
    hx("some (text#[)|]# here", "mim", "some (#[text|]#) here");
}

#[test]
fn text_object_pairs_respect_the_whole_selection() {
    hx("(so (many (go#[od) text) |]#here)", "mim", "(#[so (many (good) text) here|]#)");
    hx("((so)((many) go#[o|]#d (text))(here))", "mim", "((so)(#[(many) good (text)|]#)(here))");
    hx("((so)((many) go#[o|]#d (text))(here))", "2mim", "(#[(so)((many) good (text))(here)|]#)");
}

#[test]
fn text_object_direction_is_preserved() {
    hx("(so [many {go#[|od]#} text] here)", "mi{", "(so [many {#[|good]#} text] here)");
    hx("(so [many {go#[|od]#} text] here)", "2mim", "(so [#[|many {good} text]#] here)");
}

#[test]
fn quote_objects() {
    hx("'so 'many 'go#[o|]#d' text' here'", "mi'", "'so 'many '#[good|]#' text' here'");
    hx("'so 'many 'go#[o|]#d' text' here'", "2mi'", "'so '#[many 'good' text|]#' here'");
    hx("'so \"many 'go#[o|]#d' text\" here'", "mi\"", "'so \"#[many 'good' text|]#\" here'");
    hx("say \"he#[l|]#lo\" now", "ma\"", "say #[\"hello\"|]# now");
    hx("say \"he#[l|]#lo\" now", "mi\"", "say \"#[hello|]#\" now");
    hx("x = 'a#[b|]#c';", "mi'", "x = '#[abc|]#';");
    hx("x = 'a#[b|]#c';", "mim", "x = '#[abc|]#';");
}

#[test]
fn word_objects() {
    hx("hello #[w|]#orld foo", "miw", "hello #[world|]# foo");
    hx("hello #[w|]#orld foo", "maw", "hello #[world |]#foo");
    hx("hello world #[f|]#oo", "maw", "hello world#[ foo|]#");
    hx("a.b#[|]#c", "miw", "a.#[bc|]#");
    hx("foo.b#[a|]#r baz", "miW", "#[foo.bar|]# baz");
    hx("foo.b#[a|]#r baz", "miw", "foo.#[bar|]# baz");
    hx("a  #[ |]# b", "miw", "a#[    |]#b");
}

#[test]
fn paragraph_objects() {
    hx("a\n#[b|]#\n\nc\n", "mip", "#[a\nb\n|]#\nc\n");
    hx("a\n#[b|]#\n\nc\n", "map", "#[a\nb\n\n|]#c\n");
    hx("a\n\n#[c|]#\n", "mip", "a\n\n#[c\n|]#");
}

#[test]
fn text_object_hints_are_shown_while_waiting() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    feed(&mut e, "m");
    let (title, hints) = e.key_hints().unwrap();
    assert_eq!(title, "Match");
    assert!(hints.iter().any(|h| h.key == "m"));
    feed(&mut e, "i");
    let (title, hints) = e.key_hints().unwrap();
    assert_eq!(title, "Match inside");
    assert!(hints.iter().any(|h| h.key == "w"));
    assert_eq!(e.pending_keys(), "");
    feed(&mut e, "<esc>");
    assert!(e.key_hints().is_none());
}
