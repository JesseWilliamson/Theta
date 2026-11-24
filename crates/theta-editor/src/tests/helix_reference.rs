//! Cases from the Helix research report (section 5) plus the ones read straight from Helix's source.
//!
//! Source labels: H-unit = `helix-core/src/movement.rs` unit tests, H-test = `helix-term/tests/test/*.rs`,
//! Zed-test = `crates/vim/src/helix.rs` in Zed, Port = a Python port of `word_move` (checked against the
//! H-unit cases), Src = read from Helix's source only. "Src" cases that disagree with the source are noted
//! next to the case: Helix's source wins.
//!
//! Notation: Helix's own (`#[abc|]#` forward, `#[|abc]#` backward, `#(..)#` secondary).

use super::*;

#[test]
fn word_start_end_back() {
    // 1 H-unit: "Basic forward motion stops at the first space" (0,0)->(0,6)
    hx("#[h|]#ello world", "w", "#[hello |]#world");
    // 2 Port
    hx("#[hello |]#world", "w", "hello #[world|]#");
    // 3 Port
    hx("hell#[o|]# world", "w", "hell#[o |]#world");
    // 4 H-unit: (0,0)->(0,5)
    hx("#[h|]#ello world", "e", "#[hello|]# world");
    // 5 Port: from the end of a word, `e` selects the gap and the next word
    hx("#[hello|]# world", "e", "hello#[ world|]#");
    // 6 Port
    hx("hello #[w|]#orld", "b", "#[|hello ]#world");
    // 7 Port, same shape as the H-unit (3,3)->(4,0)
    hx("hello wo#[r|]#ld", "b", "hello #[|wor]#ld");
}

#[test]
fn word_vs_long_word() {
    // 8 Port; W matches the H-unit "punctuation are not treated any differently"
    hx("#[f|]#oo.bar baz", "w", "#[foo|]#.bar baz");
    hx("#[f|]#oo.bar baz", "W", "#[foo.bar |]#baz");
    // 9 Port
    hx("#[f|]#oo.bar baz", "e", "#[foo|]#.bar baz");
    hx("#[f|]#oo.bar baz", "E", "#[foo.bar|]# baz");
    // 15 Port (several spaces)
    hx("#[S|]#ELECT  id", "w", "#[SELECT  |]#id");
    hx("#[S|]#ELECT  id", "e", "#[SELECT|]#  id");
}

#[test]
fn word_counts() {
    // 10 Port + H-unit "Multiple motions at once": the anchor is the start of the last step's word
    hx("#[o|]#ne two three", "2w", "one #[two |]#three");
    // H-unit "Excessive motions are performed partially": 999 stops at the end
    hx("#[o|]#ne two three", "999w", "one two #[three|]#");
}

#[test]
fn word_motions_across_lines() {
    // 11 Port + H-unit "Jumping\n    into starting whitespace selects the spaces before 'into'"
    hx("a#[b|]#\n  cd", "w", "ab\n#[  |]#cd");
    // 12 Port
    hx("a#[b|]#\n  cd", "e", "ab\n#[  cd|]#");
    // 13 H-unit "Newlines\n\nare bridged seamlessly." (0,8) -> (10,14)
    hx("#[Newlines|]#\n\nare bridged", "w", "Newlines\n\n#[are |]#bridged");
    // H-unit "Jumping\n\n\n\n\n\n   from newlines to whitespace selects whitespace." (0,9)->(13,16)
    hx("#[Jumping\n\n|]#\n\n\n\n   from", "w", "Jumping\n\n\n\n\n\n#[   |]#from");
}

#[test]
fn word_motion_edges() {
    // 14 Port + H-unit "A failed motion does not modify the range": at the end of the text
    hx("hello worl#[d|]#", "w", "hello worl#[d|]#");
    // 16 H-unit: empty text and all-newline text
    hx("#[|]#", "w", "#[|]#");
    hx("\n\n\n\n\n#[|]#", "w", "\n\n\n\n\n#[|]#");
    // H-unit "Starting from a boundary advances the anchor"
    hx("#[ |]#Starting next", "w", " #[Starting |]#next");
    // H-unit "Starting from whitespace moves to last space in sequence"
    hx("#[ |]#   Starting", "w", "#[    |]#Starting");
    // H-unit "Identifiers_with_underscores are considered a single word"
    hx("#[I|]#dent_ifier next", "w", "#[Ident_ifier |]#next");
}

#[test]
fn head_inside_a_selection_moves_from_the_block_cursor() {
    // 17 Zed-test `test_word_motions`: Th«e quiˇ»ck brown, w -> The qu«ick ˇ»brown
    hx("Th#[e qui|]#ck brown", "w", "The qu#[ick |]#brown");
}

#[test]
fn select_mode_extends_words() {
    // 18 Port (with put_cursor ported)
    hx("#[h|]#ello world foo", "vww", "#[hello world |]#foo");
    // 19 Port: the anchor character `f` stays selected when the head crosses it
    hx("hello world #[f|]#oo", "vb", "hello #[|world f]#oo");
}

#[test]
fn select_mode_flip_and_full_cursor() {
    // 20 H-test `insert_to_normal_mode_cursor_position` (the select half)
    hx("#[f|]#oo", "vll<A-;><esc>", "#[|foo]#");
    // 21 Zed-test `test_helix_full_cursor_selection`: ll v hhh keeps the anchor character selected
    hx("#[o|]#ne two three", "llvhhh", "#[|one]# two three");
}

#[test]
fn extend_line_with_x() {
    // 22 H-test `test_extend_line`
    hx("#[l|]#orem\nipsum\ndolor\n\n", "x2x", "#[lorem\nipsum\ndolor\n|]#\n");
    // 23 H-test `test_extend_line`
    hx("#[l|]#orem\nipsum\n\n", "2x", "#[lorem\nipsum\n|]#\n");
    // 24 Src; Zed `test_helix_select_lines` asserts the same: an empty line is already a full line
    hx("ab\n#[\n|]#cd\nef", "x", "ab\n#[\ncd\n|]#ef");
    // 25 Src: last line without a trailing newline
    hx("ab\n#[c|]#d", "x", "ab\n#[cd|]#");
    hx("ab\n#[c|]#d", "xx", "ab\n#[cd|]#");
    // 26 Zed-test: partial reversed selection over two lines becomes both full lines, forward
    hx("li#[|ne one\nli]#ne two\nline three", "x", "#[line one\nline two\n|]#line three");
}

#[test]
fn x_shapes_from_helix_source() {
    // `extend_line_impl`: a selection that already is its line span extends by `count` lines
    hx("#[a\n|]#b\nc\n", "x", "#[a\nb\n|]#c\n");
    // `X` (extend_to_line_bounds) keeps the direction and adds no line
    hx("a#[b|]#c\ndef\n", "X", "#[abc\n|]#def\n");
    hx("a#[|bc\nd]#ef\nghi\n", "X", "#[|abc\ndef\n]#ghi\n");
    // `A-x` (shrink_to_line_bounds)
    hx("#[ab\ncd\n|]#ef\n", "<A-x>", "#[ab\ncd\n|]#ef\n");
    hx("a#[b\ncd\ne|]#f\n", "<A-x>", "ab\n#[cd\n|]#ef\n");
}

#[test]
fn delete_leaves_a_one_char_cursor() {
    // 27 Src (delete_by_selection + min_width_1); Zed's equivalent: The qu«ick ˇ»brown, d
    hx("#[hello |]#world", "d", "#[w|]#orld");
    // 28 Zed-test `test_delete`
    hx("The qu#[b|]#rown", "d", "The qu#[r|]#own");
    // 29 Zed-test `test_delete_character_end_of_line`: the newline is the character
    hx("brown#[\n|]#fox", "d", "brown#[f|]#ox");
}

#[test]
fn change_selection_and_linewise_change() {
    // 30 Src: the insert point is where the deleted text began
    hx("#[hello |]#world", "cX<esc>", "Xworld");
    let mut e = editor_from(Keymap::Helix, "#[hello |]#world");
    feed(&mut e, "c");
    assert_eq!(e.mode(), Mode::Insert);
    assert_eq!(e.text(), "world");
    // 31 Src (`selection_is_linewise` -> `open(Above)`): changing whole lines leaves one empty line
    let mut e = editor_from(Keymap::Helix, "#[ab\n|]#cd\n");
    feed(&mut e, "c");
    assert_eq!(e.mode(), Mode::Insert);
    assert_eq!(e.text(), "\ncd\n");
    feed(&mut e, "X<esc>");
    assert_eq!(e.text(), "X\ncd\n");
    // `x` then `c` on the middle line keeps the line count
    hx("a\n#[b|]#\nc\n", "xcX<esc>", "a\nX#[\n|]#c\n");
}

#[test]
fn insert_and_append_cursor_positions() {
    // 32 H-test `insert_mode_cursor_position`: `i` flips so the head is at the start
    hx("#[\n|]#", "i", "#[|\n]#");
    // 33 H-test: `a` extends past the last character of every selection
    hx("#[f|]#oo\n#(b|)#ar", "a", "#[fo|]#o\n#(ba|)#r");
    // 34 H-test: `a<esc>` with nothing typed restores the selection
    hx("#[f|]#oo\n#(b|)#ar", "a<esc>", "#[f|]#oo\n#(b|)#ar");
    // H-test: `i` does not collapse, typing goes before the selection, `<esc>` keeps it
    hx("#[hello|]# world", "iX<esc>", "X#[|hello]# world");
}

#[test]
fn multi_selection_paste_selects_pasted_text() {
    // 35 H-test `test_multi_selection_paste`
    hx(
        "#[|lorem]#\n#(|ipsum)#\n#(|dolor)#\n",
        "yp",
        "lorem#[|lorem]#\nipsum#(|ipsum)#\ndolor#(|dolor)#\n",
    );
}

#[test]
fn paste_after_and_before_char_wise() {
    // 36 Src (`paste_impl`): char-wise `p` pastes at range.to(), `P` at range.from()
    let mut e = editor_from(Keymap::Helix, "#[h|]#ello");
    e.registers.insert('a', vec!["XY".to_string()]);
    feed(&mut e, "\"ap");
    assert_eq!(state(&e), "h#[XY|]#ello");
    let mut e = editor_from(Keymap::Helix, "#[h|]#ello");
    e.registers.insert('a', vec!["XY".to_string()]);
    feed(&mut e, "\"aP");
    assert_eq!(state(&e), "#[XY|]#hello");
    // line-wise paste goes below / above the selection's lines and selects the pasted lines
    hx("a\n#[b|]#\nc\n", "xyp", "a\nb\n#[b\n|]#c\n");
    hx("a\n#[b|]#\nc\n", "xyP", "a\n#[b\n|]#b\nc\n");
}

#[test]
fn join_lines() {
    // 37 H-test `test_join_selections`
    hx("#[a|]#bc\ndef\n", "J", "#[a|]#bc def\n");
    // 38 H-test: joining with an empty line and with additional space in the next line
    hx("#[a|]#bc\n\ndef\n", "JJ", "#[a|]#bc def\n");
    hx("#[a|]#bc\n    def\n", "J", "#[a|]#bc def\n");
    // H-test `test_join_selections_space`: A-J selects the inserted spaces
    hx("#[a|]#bc\ndef\n", "<A-J>", "abc#[ |]#def\n");
}

#[test]
fn copy_selection_to_next_line() {
    // 39 H-test `test_selection_duplication`: primary is the last one
    hx("#[lo|]#rem\nipsum\ndolor\n", "CC", "#(lo|)#rem\n#(ip|)#sum\n#[do|]#lor\n");
    // backward ranges stay backward
    hx("#[|lo]#rem\nipsum\ndolor\n", "CC", "#(|lo)#rem\n#(|ip)#sum\n#[|do]#lor\n");
}

#[test]
fn replace_and_case() {
    // 40 Src
    hx("#[hello|]#", "rx", "#[xxxxx|]#");
    // 41 Src
    hx("#[Hello|]#", "~", "#[hELLO|]#");
}

#[test]
fn collapse_and_flip() {
    // 42 Src
    hx("#[hello |]#world", ";", "hello#[ |]#world");
    hx("#[hello |]#world", "<A-;>", "#[|hello ]#world");
}

#[test]
fn find_char_cases() {
    // 43 H-test `find_char`
    hx("he#[l|]#lo\nhello", "fl", "he#[ll|]#o\nhello");
    // 44 H-test: a second `fl` crosses the line
    hx("hel#[l|]#o\nhello", "fl", "hel#[lo\nhel|]#lo");
    // 45 H-test
    hx("he#[l|]#lo\nhello", "tl", "he#[llo\nhe|]#llo");
    // 46 H-test
    hx("hello\nhel#[l|]#o", "Fl", "hello\nhe#[|ll]#o");
    // 47 H-test: a count beyond the matches is a no-op
    hx("#[h|]#ello\nhello", "9fl", "#[h|]#ello\nhello");
    // 48 H-test `find_char_line_ending`
    hx("on#[e|]#\ntwo\n", "f<ret>", "on#[e\n|]#two\n");
}

#[test]
fn goto_motions() {
    // 49 H-test `search_selection_detect_word_boundaries_at_eof` (no panic at the end of the text)
    hx("#[o|]#ne\ntwo\nthree\n", "gej*h", "one\ntwo\nthree#[\n|]#");
    // 50 Src; Zed-test `test_g_l_end_of_line`: `gl` lands on the last character, not the newline
    hx("#[h|]#ello\n", "gl", "hell#[o|]#\n");
    // 51 Src: `ge` goes to the last line that has text
    hx("#[a|]#\nb\n", "ge", "a\n#[b|]#\n");
    // 52 Src: `G` without a count does nothing
    hx("a\n#[b|]#\nc\n", "G", "a\n#[b|]#\nc\n");
    hx("a\nb\n#[c|]#\n", "2G", "a\n#[b|]#\nc\n");
    // `gg` with a count is a line number too
    hx("a\nb\n#[c|]#\n", "2gg", "a\n#[b|]#\nc\n");
    // `gh` / `gs`
    hx("  ab#[c|]#\n", "gh", "#[ |]# abc\n");
    hx("  ab#[c|]#\n", "gs", "  #[a|]#bc\n");
}

#[test]
fn star_and_n() {
    // 53 Src: `*` does not move; `n` then selects the next match and replaces the primary
    hx("#[foo|]# bar foo", "*", "#[foo|]# bar foo");
    hx("#[foo|]# bar foo", "*n", "foo bar #[foo|]#");
    // 54 Src; Zed `do_helix_select`: in Select mode `n` adds the match
    hx("#[foo|]# bar foo", "v*n", "#(foo|)# bar #[foo|]#");
}

// ---------------------------------------------------------------------------------------------
// Ported from Helix's own integration tests (helix-term/tests/test/movement.rs and commands.rs).
// Generated mechanically: indoc! blocks are unindented and, like Helix's `LineFeedHandling::Native`,
// a trailing newline is appended to the input and the expectation when the text lacks one.
// ---------------------------------------------------------------------------------------------

/// Run every case, collect every mismatch (or panic) and fail once with the full list.
#[track_caller]
fn run_upstream(source: &str, cases: &[(&str, &str, &str)]) {
    let mut bad = Vec::new();
    for (before, keys, after) in cases {
        let r = std::panic::catch_unwind(|| {
            let mut e = editor_from(Keymap::Helix, before);
            feed(&mut e, keys);
            let got = if after.contains("#[") { state(&e) } else { e.text() };
            (got == *after).then_some(()).ok_or(got)
        });
        match r {
            Ok(Ok(())) => {}
            Ok(Err(got)) => bad.push(format!(
                "  keys {keys:?}\n    before {before:?}\n    want   {after:?}\n    got    {got:?}"
            )),
            Err(_) => bad.push(format!("  keys {keys:?}\n    before {before:?}\n    PANIC")),
        }
    }
    assert!(
        bad.is_empty(),
        "{source}: {} of {} cases differ from Helix\n{}",
        bad.len(),
        cases.len(),
        bad.join("\n")
    );
}

#[test]
fn upstream_insert_mode_cursor_position() {
    // source: helix-term/tests/test/movement.rs, fn insert_mode_cursor_position
    run_upstream("movement.rs::insert_mode_cursor_position", &[
        (r####"#[
|]#"####, r####"i"####, r####"#[|
]#"####),
        (r####"#[
|]#"####, r####"i<esc>"####, r####"#[|
]#"####),
        (r####"#[
|]#"####, r####"i<esc>i"####, r####"#[|
]#"####),
    ]);
}

#[test]
fn upstream_insert_to_normal_mode_cursor_position() {
    // source: helix-term/tests/test/movement.rs, fn insert_to_normal_mode_cursor_position
    run_upstream("movement.rs::insert_to_normal_mode_cursor_position", &[
        (r####"#[f|]#oo
"####, r####"vll<A-;><esc>"####, r####"#[|foo]#
"####),
        (r####"#[f|]#oo
#(b|)#ar
"####, r####"vll<A-;><esc>"####, r####"#[|foo]#
#(|bar)#
"####),
        (r####"#[f|]#oo
#(b|)#ar
"####, r####"a"####, r####"#[fo|]#o
#(ba|)#r
"####),
        (r####"#[f|]#oo
#(b|)#ar
"####, r####"a<esc>"####, r####"#[f|]#oo
#(b|)#ar
"####),
    ]);
}

#[test]
fn upstream_surround_by_character() {
    // source: helix-term/tests/test/movement.rs, fn surround_by_character
    run_upstream("movement.rs::surround_by_character", &[
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"mi{"####, r####"(so [many {#[good|]#} text] here)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"mi["####, r####"(so [#[many {good} text|]#] here)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"mi("####, r####"(#[so [many {good} text] here|]#)
"####),
        (r####"'so 'many 'go#[o|]#d' text' here'
"####, r####"mi'"####, r####"'so 'many '#[good|]#' text' here'
"####),
        (r####"'so 'many 'go#[o|]#d' text' here'
"####, r####"2mi'"####, r####"'so '#[many 'good' text|]#' here'
"####),
        (r####"'so "many 'go#[o|]#d' text" here'
"####, r####"mi""####, r####"'so "#[many 'good' text|]#" here'
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"mi{"####, r####"(so [many {#[|good]#} text] here)
"####),
    ]);
}

#[test]
fn upstream_surround_inside_pair() {
    // source: helix-term/tests/test/movement.rs, fn surround_inside_pair
    run_upstream("movement.rs::surround_inside_pair", &[
        (r####"some (#[t|]#ext) here
"####, r####"mim"####, r####"some (#[text|]#) here
"####),
        (r####"some (text#[)|]# here
"####, r####"mim"####, r####"some (#[text|]#) here
"####),
        (r####"so#[m|]#e (text) here
"####, r####"mim"####, r####"so#[m|]#e (text) here
"####),
        (r####"(so (many (go#[o|]#d) text) here)
"####, r####"1mim"####, r####"(so (many (#[good|]#) text) here)
"####),
        (r####"(so (many (go#[o|]#d) text) here)
"####, r####"2mim"####, r####"(so (#[many (good) text|]#) here)
"####),
        (r####"(so (many (go#[o|]#d) text) here)
"####, r####"3mim"####, r####"(#[so (many (good) text) here|]#)
"####),
        (r####"((so)((many) go#[o|]#d (text))(here))
"####, r####"mim"####, r####"((so)(#[(many) good (text)|]#)(here))
"####),
        (r####"((so)((many) go#[o|]#d (text))(here))
"####, r####"2mim"####, r####"(#[(so)((many) good (text))(here)|]#)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"mim"####, r####"(so [many {#[good|]#} text] here)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"2mim"####, r####"(so [#[many {good} text|]#] here)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"3mim"####, r####"(#[so [many {good} text] here|]#)
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"mim"####, r####"(so [many {#[|good]#} text] here)
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"2mim"####, r####"(so [#[|many {good} text]#] here)
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"3mim"####, r####"(#[|so [many {good} text] here]#)
"####),
        (r####"(so (many (go#[od) |]#text) here)
"####, r####"mim"####, r####"(so (#[many (good) text|]#) here)
"####),
        (r####"(so (many#[ (go|]#od) text) here)
"####, r####"mim"####, r####"(so (#[many (good) text|]#) here)
"####),
        (r####"(so#[ (many (go|]#od) text) here)
"####, r####"mim"####, r####"(#[so (many (good) text) here|]#)
"####),
        (r####"(so (many (go#[od) text) |]#here)
"####, r####"mim"####, r####"(#[so (many (good) text) here|]#)
"####),
        (r####"(so (many (good) text) #[he|]#re
so (many (good) text) #(|he)#re)
"####, r####"mim"####, r####"(#[so (many (good) text) here
so (many (good) text) here|]#)
"####),
    ]);
}

#[test]
fn upstream_surround_around_pair() {
    // source: helix-term/tests/test/movement.rs, fn surround_around_pair
    run_upstream("movement.rs::surround_around_pair", &[
        (r####"some (#[t|]#ext) here
"####, r####"mam"####, r####"some #[(text)|]# here
"####),
        (r####"some (text#[)|]# here
"####, r####"mam"####, r####"some #[(text)|]# here
"####),
        (r####"so#[m|]#e (text) here
"####, r####"mam"####, r####"so#[m|]#e (text) here
"####),
        (r####"(so (many (go#[o|]#d) text) here)
"####, r####"1mam"####, r####"(so (many #[(good)|]# text) here)
"####),
        (r####"(so (many (go#[o|]#d) text) here)
"####, r####"2mam"####, r####"(so #[(many (good) text)|]# here)
"####),
        (r####"(so (many (go#[o|]#d) text) here)
"####, r####"3mam"####, r####"#[(so (many (good) text) here)|]#
"####),
        (r####"((so)((many) go#[o|]#d (text))(here))
"####, r####"mam"####, r####"((so)#[((many) good (text))|]#(here))
"####),
        (r####"((so)((many) go#[o|]#d (text))(here))
"####, r####"2mam"####, r####"#[((so)((many) good (text))(here))|]#
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"mam"####, r####"(so [many #[{good}|]# text] here)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"2mam"####, r####"(so #[[many {good} text]|]# here)
"####),
        (r####"(so [many {go#[o|]#d} text] here)
"####, r####"3mam"####, r####"#[(so [many {good} text] here)|]#
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"mam"####, r####"(so [many #[|{good}]# text] here)
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"2mam"####, r####"(so #[|[many {good} text]]# here)
"####),
        (r####"(so [many {go#[|od]#} text] here)
"####, r####"3mam"####, r####"#[|(so [many {good} text] here)]#
"####),
        (r####"(so (many (go#[od) |]#text) here)
"####, r####"mam"####, r####"(so #[(many (good) text)|]# here)
"####),
        (r####"(so (many#[ (go|]#od) text) here)
"####, r####"mam"####, r####"(so #[(many (good) text)|]# here)
"####),
        (r####"(so#[ (many (go|]#od) text) here)
"####, r####"mam"####, r####"#[(so (many (good) text) here)|]#
"####),
        (r####"(so (many (go#[od) text) |]#here)
"####, r####"mam"####, r####"#[(so (many (good) text) here)|]#
"####),
        (r####"(so (many (good) text) #[he|]#re
so (many (good) text) #(|he)#re)
"####, r####"mam"####, r####"#[(so (many (good) text) here
so (many (good) text) here)|]#
"####),
    ]);
}

#[test]
fn upstream_cursor_position_append_eof() {
    // source: helix-term/tests/test/movement.rs, fn cursor_position_append_eof
    run_upstream("movement.rs::cursor_position_append_eof", &[
        (r####"#[foo|]#
"####, r####"abar<esc>"####, r####"#[foobar|]#
"####),
        (r####"#[|foo]#
"####, r####"abar<esc>"####, r####"#[foobar|]#
"####),
    ]);
}

#[test]
fn upstream_find_char() {
    // source: helix-term/tests/test/movement.rs, fn find_char
    run_upstream("movement.rs::find_char", &[
        (r####"he#[l|]#lo
hello
"####, r####"fl"####, r####"he#[ll|]#o
hello
"####),
        (r####"hel#[l|]#o
hello
"####, r####"fl"####, r####"hel#[lo
hel|]#lo
"####),
        (r####"hel#[l|]#o
hello
"####, r####"fx"####, r####"hel#[l|]#o
hello
"####),
        (r####"he#[l|]#lo
hello
"####, r####"2fl"####, r####"he#[llo
hel|]#lo
"####),
        (r####"#[h|]#ello
hello
"####, r####"9fl"####, r####"#[h|]#ello
hello
"####),
        (r####"h#[e|]#llo
hello
"####, r####"tl"####, r####"h#[el|]#lo
hello
"####),
        (r####"he#[l|]#lo
hello
"####, r####"tl"####, r####"he#[llo
he|]#llo
"####),
        (r####"hel#[l|]#o
hello
"####, r####"tl"####, r####"hel#[lo
he|]#llo
"####),
        (r####"hel#[l|]#o
hello
"####, r####"tx"####, r####"hel#[l|]#o
hello
"####),
        (r####"he#[l|]#lo
hello
"####, r####"2tl"####, r####"he#[llo
hel|]#lo
"####),
        (r####"#[h|]#ello
hello
"####, r####"9tl"####, r####"#[h|]#ello
hello
"####),
        (r####"hello
hel#[l|]#o
"####, r####"Fl"####, r####"hello
he#[|ll]#o
"####),
        (r####"hello
he#[l|]#lo
"####, r####"Fl"####, r####"hel#[|lo
hel]#lo
"####),
        (r####"hello
#[h|]#ello
"####, r####"Fx"####, r####"hello
#[h|]#ello
"####),
        (r####"hello
hel#[l|]#o
"####, r####"2Fl"####, r####"hel#[|lo
hell]#o
"####),
        (r####"hello
hell#[o|]#
"####, r####"9Fl"####, r####"hello
hell#[o|]#
"####),
        (r####"hello
hell#[o|]#
"####, r####"Tl"####, r####"hello
hel#[|lo]#
"####),
        (r####"hello
hel#[l|]#o
"####, r####"Tl"####, r####"hell#[|o
hell]#o
"####),
        (r####"hello
he#[l|]#lo
"####, r####"Tl"####, r####"hell#[|o
hel]#lo
"####),
        (r####"hello
#[h|]#ello
"####, r####"Tx"####, r####"hello
#[h|]#ello
"####),
        (r####"hello
hel#[l|]#o
"####, r####"2Tl"####, r####"hel#[|lo
hell]#o
"####),
        (r####"hello
hell#[o|]#
"####, r####"9Tl"####, r####"hello
hell#[o|]#
"####),
    ]);
}

#[test]
fn upstream_find_char_line_ending() {
    // source: helix-term/tests/test/movement.rs, fn find_char_line_ending
    run_upstream("movement.rs::find_char_line_ending", &[
        (r####"on#[e|]#
two
"####, r####"f<ret>"####, r####"on#[e
|]#two
"####),
        (r####"one#[
|]#two
"####, r####"f<ret>"####, r####"one#[
two
|]#"####),
        (r####"one
#[t|]#wo
"####, r####"f<ret>"####, r####"one
#[two
|]#"####),
        (r####"one#[
|]#"####, r####"f<ret>"####, r####"one#[
|]#"####),
        (r####"#[o|]#ne
two
"####, r####"2f<ret>"####, r####"#[one
two
|]#"####),
        (r####"#[o|]#ne
two
"####, r####"9f<ret>"####, r####"#[o|]#ne
two
"####),
        (r####"o#[n|]#e
two
"####, r####"t<ret>"####, r####"o#[ne|]#
two
"####),
        (r####"on#[e|]#
two
"####, r####"t<ret>"####, r####"on#[e
two|]#
"####),
        (r####"one#[
|]#two
"####, r####"t<ret>"####, r####"one#[
two|]#
"####),
        (r####"one#[
|]#"####, r####"t<ret>"####, r####"one#[
|]#"####),
        (r####"on#[e|]#
"####, r####"t<ret>"####, r####"on#[e|]#
"####),
        (r####"#[o|]#ne
two
"####, r####"2t<ret>"####, r####"#[one
two|]#
"####),
        (r####"#[o|]#ne
two
"####, r####"9t<ret>"####, r####"#[o|]#ne
two
"####),
        (r####"one
two
#[t|]#hree
"####, r####"F<ret>"####, r####"one
two#[|
t]#hree
"####),
        (r####"one
two#[
|]#three
"####, r####"F<ret>"####, r####"one#[|
two
]#three
"####),
        (r####"one
tw#[o|]#
three
"####, r####"F<ret>"####, r####"one#[|
two]#
three
"####),
        (r####"o#[n|]#e
"####, r####"F<ret>"####, r####"o#[n|]#e
"####),
        (r####"#[o|]#ne
"####, r####"F<ret>"####, r####"#[o|]#ne
"####),
        (r####"one
two
th#[r|]#ee
"####, r####"2F
"####, r####"one#[|
two
thr]#ee
"####),
        (r####"one
two
th#[r|]#ee
"####, r####"9F
"####, r####"one
two
th#[r|]#ee
"####),
        (r####"one
two
th#[r|]#ee
"####, r####"T<ret>"####, r####"one
two
#[|thr]#ee
"####),
        (r####"one
two
#[t|]#hree
"####, r####"T<ret>"####, r####"one
#[|two
t]#hree
"####),
        (r####"one
two#[
|]#three
"####, r####"T<ret>"####, r####"one
#[|two
]#three
"####),
        (r####"o#[n|]#e
"####, r####"T<ret>"####, r####"o#[n|]#e
"####),
        (r####"#[o|]#ne
"####, r####"T<ret>"####, r####"#[o|]#ne
"####),
        (r####"one
two
th#[r|]#ee
"####, r####"2T
"####, r####"one
#[|two
thr]#ee
"####),
        (r####"one
two
th#[r|]#ee
"####, r####"9T
"####, r####"one
two
th#[r|]#ee
"####),
        (r####"one
#[|t]#wo
three
"####, r####"T<ret>gll2f<ret>"####, r####"one
two#[
|]#three
"####),
        (r####"#[|o]#ne
two
three
"####, r####"f<ret>2t<ret>ghT<ret>F<ret>"####, r####"one#[|
t]#wo
three
"####),
    ]);
}

#[test]
fn upstream_repeat_find_char() {
    // source: helix-term/tests/test/movement.rs, fn repeat_find_char
    run_upstream("movement.rs::repeat_find_char", &[
        (r####"#[o|]#ne two
one two
"####, r####"ft<A-.>"####, r####"one #[two
one t|]#wo
"####),
        (r####"#[o|]#ne two
one two
"####, r####"f<ret><A-.>"####, r####"one two#[
one two
|]#"####),
        (r####"#[o|]#ne two
one two
"####, r####"ftf<ret><A-.>"####, r####"one two#[
one two
|]#"####),
    ]);
}

#[test]
fn upstream_surround_replace() {
    // source: helix-term/tests/test/movement.rs, fn test_surround_replace
    run_upstream("movement.rs::test_surround_replace", &[
        (r####"(#[|a]#)
"####, r####"mrm{"####, r####"{#[|a]#}
"####),
        (r####"(#[a|]#)
"####, r####"mrm{"####, r####"{#[a|]#}
"####),
        (r####"{{

#(}|)#
#[}|]#
"####, r####"mrm)"####, r####"((

#()|)#
#[)|]#
"####),
    ]);
}

#[test]
fn upstream_surround_delete() {
    // source: helix-term/tests/test/movement.rs, fn test_surround_delete
    run_upstream("movement.rs::test_surround_delete", &[
        (r####"(#[|a]#)
"####, r####"mdm"####, r####"#[|a]#
"####),
        (r####"(#[a|]#)
"####, r####"mdm"####, r####"#[a|]#
"####),
        (r####"{{

#(}|)#
#[}|]#
"####, r####"mdm"####, r####"

#(
|)##[
|]#"####),
    ]);
}

#[test]
fn upstream_search_selection_detect_word_boundaries_at_eof() {
    // source: helix-term/tests/test/commands.rs, fn search_selection_detect_word_boundaries_at_eof
    run_upstream("commands.rs::search_selection_detect_word_boundaries_at_eof", &[
        (r####"#[o|]#ne
two
three
"####, r####"gej*h"####, r####"one
two
three#[
|]#"####),
    ]);
}

#[test]
fn upstream_selection_duplication() {
    // source: helix-term/tests/test/commands.rs, fn test_selection_duplication
    run_upstream("commands.rs::test_selection_duplication", &[
        (r####"#[lo|]#rem
ipsum
dolor
"####, r####"CC"####, r####"#(lo|)#rem
#(ip|)#sum
#[do|]#lor
"####),
        (r####"#[|lo]#rem
ipsum
dolor
"####, r####"CC"####, r####"#(|lo)#rem
#(|ip)#sum
#[|do]#lor
"####),
        (r####"test
#[testitem|]#
"####, r####"<A-C>"####, r####"test
#[testitem|]#
"####),
        (r####"test
#[test|]#
"####, r####"<A-C>"####, r####"#[test|]#
#(test|)#
"####),
        (r####"#[testitem|]#
test
"####, r####"C"####, r####"#[testitem|]#
test
"####),
        (r####"#[test|]#
test
"####, r####"C"####, r####"#(test|)#
#[test|]#
"####),
    ]);
}

#[test]
fn upstream_multi_selection_paste() {
    // source: helix-term/tests/test/commands.rs, fn test_multi_selection_paste
    run_upstream("commands.rs::test_multi_selection_paste", &[
        (r####"#[|lorem]#
#(|ipsum)#
#(|dolor)#
"####, r####"yp"####, r####"lorem#[|lorem]#
ipsum#(|ipsum)#
dolor#(|dolor)#
"####),
    ]);
}

// commands.rs `test_multi_selection_shell_commands` not ported: needs a shell: `|`, `!` and `$` are not implemented in a SQL console (see COVERAGE.md).

#[test]
fn upstream_undo_redo() {
    // source: helix-term/tests/test/commands.rs, fn test_undo_redo
    run_upstream("commands.rs::test_undo_redo", &[
        (r####"#[|]#"####, r####"2[<space><C-s>u<C-o><C-i>"####, r####"#[|]#"####),
        (r####"#[|]#"####, r####"[<space><C-s>kduU<C-o><C-i>"####, r####"#[|]#"####),
        (r####"#[|]#"####, r####"[<space>u[<space>u"####, r####"#[|]#"####),
    ]);
}

// commands.rs `test_extend_line` not ported: the indoc! literals put one more newline after the selection than before it, so the expectation cannot be read mechanically; the same inputs are in `extend_line_with_x` above.

#[test]
fn upstream_delete_char_backward() {
    // source: helix-term/tests/test/commands.rs, fn test_delete_char_backward
    run_upstream("commands.rs::test_delete_char_backward", &[
        (r####"#(x|)# #[x|]#
"####, r####"c<space><backspace><esc>"####, r####"#[
|]#"####),
        (r####"#( |)##( |)#a#( |)#axx#[x|]#a
"####, r####"li<backspace><esc>"####, r####"#(a|)##(|a)#xx#[|a]#
"####),
    ]);
}

#[test]
fn upstream_delete_word_backward() {
    // source: helix-term/tests/test/commands.rs, fn test_delete_word_backward
    run_upstream("commands.rs::test_delete_word_backward", &[
        (r####"fo#[o|]#ba#(r|)#
"####, r####"a<C-w><esc>"####, r####"#[
|]#"####),
    ]);
}

#[test]
fn upstream_delete_word_forward() {
    // source: helix-term/tests/test/commands.rs, fn test_delete_word_forward
    run_upstream("commands.rs::test_delete_word_forward", &[
        (r####"fo#[o|]#b#(|ar)#
"####, r####"i<A-d><esc>"####, r####"fo#[
|]#"####),
    ]);
}

#[test]
fn upstream_delete_char_forward() {
    // source: helix-term/tests/test/commands.rs, fn test_delete_char_forward
    run_upstream("commands.rs::test_delete_char_forward", &[
        (r####"#[abc|]#def
#(abc|)#ef
#(abc|)#f
#(abc|)#
"####, r####"a<del><esc>"####, r####"#[abc|]#ef
#(abc|)#f
#(abc|)#
#(abc|)#
"####),
    ]);
}

// commands.rs `test_open_below_at_block_end_stays_in_block` not ported: needs tree-sitter language indentation.

#[test]
fn upstream_join_selections() {
    // source: helix-term/tests/test/commands.rs, fn test_join_selections
    run_upstream("commands.rs::test_join_selections", &[
        (r####"#[a|]#bc
def
"####, r####"J"####, r####"#[a|]#bc def
"####),
        (r####"#[a|]#bc

def
"####, r####"JJ"####, r####"#[a|]#bc def
"####),
        (r####"#[a|]#bc

    def
"####, r####"JJ"####, r####"#[a|]#bc def
"####),
    ]);
}

#[test]
fn upstream_join_selections_space() {
    // source: helix-term/tests/test/commands.rs, fn test_join_selections_space
    run_upstream("commands.rs::test_join_selections_space", &[
        (r####"#[a

b

c

d

e|]#
"####, r####"<A-J>"####, r####"a#[ |]#b#( |)#c#( |)#d#( |)#e
"####),
        (r####"#[a|]#bc
def
"####, r####"<A-J>"####, r####"abc#[ |]#def
"####),
        (r####"#[a|]#bc

def
"####, r####"<A-J>"####, r####"#[a|]#bc
def
"####),
        (r####"#[a|]#bc

    def
"####, r####"<A-J><A-J>"####, r####"abc#[ |]#def
"####),
        (r####"#[aaa   

bb  

c |]#
"####, r####"<A-J>"####, r####"aaa   #[ |]#bb  #( |)#c 
"####),
    ]);
}

// commands.rs `test_join_selections_comment` not ported: needs a language with comment tokens (`:lang rust`).

// commands.rs `test_toggle_comments_inside_comment_injection` not ported: needs tree-sitter injections.

#[test]
fn upstream_surround_delete_commands() {
    // source: helix-term/tests/test/commands.rs, fn surround_delete
    run_upstream("commands.rs::surround_delete", &[
        (r####"(#[|  ]#)
"####, r####"mdm"####, r####"#[|  ]#
"####),
        (r####"(#[|  ]#)
"####, r####"md("####, r####"#[|  ]#
"####),
    ]);
}

// commands.rs `surround_replace_ts` not ported: needs tree-sitter.

// commands.rs `global_search_with_multibyte_chars` not ported: project-wide search picker.

// commands.rs `test_reflow_selections_in_one_paragraph` not ported: :reflow is not implemented (n/a).

// ---------------------------------------------------------------------------------------------
// Syntax-node commands. Helix uses tree-sitter; Theta has a structural scanner (`tree.rs`), so the
// node boundaries are Theta's own, but the command semantics are Helix's (`helix-core/src/object.rs`):
// expand keeps the direction, shrink gives back what expand started from, next/prev sibling are
// forward/backward selections, and `alt-e` / `alt-b` move to the edge of the parent node.
// ---------------------------------------------------------------------------------------------

#[test]
fn alt_o_expands_to_the_parent_node_even_in_unfinished_sql() {
    // The report: `select a, abs(from time(` and alt-o appeared to do nothing.
    hx("select a, #[a|]#bs(from time(", "<A-o>", "select a, #[abs|]#(from time(");
    hx("select a, #[abs|]#(from time(", "<A-o>", "select a, #[abs(from time(|]#");
    hx("select a, #[abs(from time(|]#", "<A-o>", "select #[a, abs(from time(|]#");
    hx("select #[a, abs(from time(|]#", "<A-o>", "#[select a, abs(from time(|]#");
    hx("select a, #[a|]#bs(from time(", "<A-o><A-o><A-o><A-o><A-o>", "#[select a, abs(from time(|]#");
}

#[test]
fn alt_o_keeps_direction_and_alt_i_gives_the_selection_back() {
    hx("select a, #[|a]#bs(1)", "<A-o>", "select a, #[|abs]#(1)");
    hx("select a, #[a|]#bs(1)", "<A-o><A-o><A-i>", "select a, #[abs|]#(1)");
    hx("select a, #[a|]#bs(1)", "<A-o><A-o><A-i><A-i>", "select a, #[a|]#bs(1)");
    // with nothing to give back, shrink goes to the first child
    hx("select #[a, abs(1)|]#", "<A-i>", "select #[a|]#, abs(1)");
    // an edit makes the remembered selections stale
    hx("select a, #[a|]#bs(1)", "<A-o>cx<esc><A-i>", "select a, x(#[1|]#)");
}

#[test]
fn alt_p_alt_n_and_alt_a_walk_the_siblings() {
    hx("select #[a|]#, b, c", "<A-n>", "select a, #[b|]#, c");
    hx("select #[a|]#, b, c", "<A-n><A-n>", "select a, b, #[c|]#");
    hx("select a, b, #[c|]#", "<A-p>", "select a, #[|b]#, c");
    hx("select a, #[b|]#, c", "<A-a>", "select #[a|]#, #(b|)#, #(c|)#");
    hx("select #[a, b, c|]#", "<A-I>", "select #[a|]#, #(b|)#, #(c|)#");
    hx("f(#[x|]#, y)", "<A-right>", "f(x, #[y|]#)");
    hx("f(x, #[y|]#)", "<A-left>", "f(#[|x]#, y)");
}

#[test]
fn alt_e_and_alt_b_move_to_the_edges_of_the_parent() {
    // object.rs `move_parent_node_end`: the cursor lands on the character after the node
    hx("select #[a|]#bc, d", "<A-e>", "select abc#[,|]# d");
    hx("select ab#[c|]#, d", "<A-b>", "select #[a|]#bc, d");
    // in Select mode they extend
    hx("select #[a|]#bc, d", "v<A-e>", "select #[abc,|]# d");
}

#[test]
fn g_dot_goes_to_the_last_edit() {
        hx("one\ntwo\nthree\n#[|]#", "ggix<esc>jjg.", "x#[o|]#ne\ntwo\nthree\n");
}

// ---------------------------------------------------------------------------------------------
// Keys read from `helix-term/src/keymap/default.rs` and `commands.rs`.
// ---------------------------------------------------------------------------------------------

#[test]
fn trim_selections_follows_helix() {
    // `trim_selections`: whitespace-only and empty selections go, the rest is trimmed
    hx("#[  ab  |]# #(   |)#", "_", "  #[ab|]#      ");
    // nothing left: collapse and keep the primary one (the cursor stays on its block)
    hx("#[ |]#x#( |)#y", "_", "#[ |]#x y");
    // the primary is the first survivor that overlaps the old one, else the last
    hx("#( a |)# #[ b |]# #( c |)#", "_", " #(a|)#   #[b|]#   #(c|)# ");
}

#[test]
fn alt_u_and_alt_shift_u_travel_the_history() {
    // Theta's history is linear, so `earlier` / `later` are undo / redo
    hx("#[a|]#bc", "xd<A-u>", "#[abc|]#");
    hx("#[a|]#bc", "xd<A-u><A-U>", "");
}

#[test]
fn sticky_z_menu_stays_open_until_escape() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#\nb\nc\n");
    let fx = feed(&mut e, "Zjj");
    assert_eq!(fx.iter().filter(|x| matches!(x, Effect::Scroll(_))).count(), 2, "{fx:?}");
    let fx = feed(&mut e, "<esc>zj");
    assert_eq!(fx.iter().filter(|x| matches!(x, Effect::Scroll(_))).count(), 1);
    // after Escape the menu is closed again: `j` moves down
    feed(&mut e, "Zj<esc>j");
    assert_eq!(state(&e), "a\n#[b|]#\nc\n");
}

#[test]
fn g_n_and_g_p_switch_consoles() {
    let fx = effects_of(Keymap::Helix, "#[a|]#", "gn");
    assert_eq!(fx, vec![Effect::NextTab]);
    let fx = effects_of(Keymap::Helix, "#[a|]#", "gp");
    assert_eq!(fx, vec![Effect::PreviousTab]);
}

#[test]
fn insert_mode_keys_from_the_default_keymap() {
    // C-w / A-backspace: delete word backward; A-d / A-del: delete word forward
    hx("foo bar#[ |]#", "i<C-w><esc>", "foo #[| ]#");
    hx("foo bar#[ |]#", "i<A-backspace><esc>", "foo #[| ]#");
    hx("#[f|]#oo bar", "i<A-d><esc>", "#[b|]#ar");
    hx("#[f|]#oo bar", "i<A-del><esc>", "#[b|]#ar");
    // C-u: kill to line start; C-k: kill to line end
    hx("ab#[c|]#d", "a<C-u><esc>", "#[d|]#");
    hx("a#[b|]#cd", "i<C-k><esc>", "a#[|]#");
    // C-h = backspace, C-d = delete forward, C-j = enter
    hx("ab#[c|]#", "a<C-h><esc>", "ab");
    hx("#[a|]#bc", "i<C-d><esc>", "#[b|]#c");
    hx("a#[b|]#", "a<C-j>x<esc>", "ab\nx");
}

#[test]
fn insert_mode_ctrl_r_pastes_a_register_and_ctrl_s_splits_the_undo_step() {
    let mut e = editor_from(Keymap::Helix, "#[a|]#");
    e.registers.insert('q', vec!["XY".to_string()]);
    feed(&mut e, "i<C-r>q<esc>");
    assert_eq!(e.text(), "XYa");
    // C-s: two undo steps for one insert session
    hx("#[a|]#", "ix<C-s>y<esc>u", "xa");
    hx("#[a|]#", "ix<C-s>y<esc>uu", "a");
}
