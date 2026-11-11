//! The Vim test DSL.
//!
//! ```text
//! vim("fo[o] bar", "dw", "fo[b]ar")      // [x] is the block cursor ON the char x
//! vim("a\n[]\nb", "dd", "a\n[b]")         // [] is the cursor on an empty line / at the end of an empty buffer
//! vim("‹a›[1]", ...)                       // when the text itself contains [ ], use ‹x› for the cursor
//! vim("[a]b", "i", "‸ab")                  // ‸ marks the bar cursor while in Insert / Replace mode
//! vis("fo[o] bar", "vl", "fo«o[ ]»bar")    // Visual states: «...» is the highlighted range of every
//!                                          // selection (one per row in block mode), [x] the cursor
//! ```
//! Keys are the usual DSL (`<esc> <ret> <C-v> <lt> <space> ...`). Tables of cases use
//! `vim_cases! { name: "before", "keys" => "after"; ... }` so a failure names the behaviour.

use super::*;

fn markers(s: &str) -> (char, char) {
    if s.contains('‹') { ('‹', '›') } else { ('[', ']') }
}

/// Split `before` into text and cursor char index (first marker pair; no marker = 0).
pub fn parse_vim(before: &str) -> (String, usize) {
    let (o, c) = markers(before);
    let cs: Vec<char> = before.chars().collect();
    let Some(i) = cs.iter().position(|&x| x == o) else { return (before.to_string(), 0) };
    let j = cs[i..]
        .iter()
        .position(|&x| x == c)
        .map(|j| i + j)
        .unwrap_or_else(|| panic!("unterminated cursor marker in {before:?}"));
    let mut text: String = cs[..i].iter().collect();
    text.extend(cs[i + 1..j].iter());
    text.extend(cs[j + 1..].iter());
    (text, i)
}

pub fn vim_editor(before: &str) -> Editor {
    let (text, cursor) = parse_vim(before);
    let mut e = Editor::new(&text, Keymap::Vim);
    e.set_viewport(0, 24);
    e.set_selections(vec![Selection::point(cursor)], 0);
    e
}

/// The editor state in DSL form (cursor only).
pub fn render_vim(e: &Editor, like: &str) -> String {
    let (o, c) = markers(like);
    let text = e.text();
    let cs: Vec<char> = text.chars().collect();
    let cur = e.cursor_of(&e.primary());
    let mut out = String::new();
    for i in 0..=cs.len() {
        if i == cur {
            match e.mode() {
                Mode::Insert | Mode::Replace => out.push('‸'),
                _ => {
                    if i < cs.len() && cs[i] != '\n' && cs[i] != '\r' {
                        out.push(o);
                        out.push(cs[i]);
                        out.push(c);
                        continue;
                    }
                    out.push(o);
                    out.push(c);
                }
            }
        }
        if i < cs.len() {
            out.push(cs[i]);
        }
    }
    out
}

#[track_caller]
pub fn vim(before: &str, keys: &str, after: &str) {
    let mut e = vim_editor(before);
    feed(&mut e, keys);
    let got = render_vim(&e, if after.contains('‹') { "‹" } else { before });
    assert_eq!(got, after, "\nbefore: {before:?}\nkeys:   {keys:?}\nmode:   {:?}", e.mode());
}

/// Like `vim` but also checks the Mode.
#[track_caller]
pub fn vim_mode(before: &str, keys: &str, after: &str, mode: Mode) {
    let mut e = vim_editor(before);
    feed(&mut e, keys);
    let got = render_vim(&e, if after.contains('‹') { "‹" } else { before });
    assert_eq!(got, after, "\nbefore: {before:?}\nkeys:   {keys:?}");
    assert_eq!(e.mode(), mode, "\nbefore: {before:?}\nkeys:   {keys:?}");
}

/// Render selections + cursor.
pub fn render_vis(e: &Editor) -> String {
    let text = e.text();
    let cs: Vec<char> = text.chars().collect();
    let cur = e.cursor_of(&e.primary());
    let mut marks: Vec<(usize, u8, &str)> = Vec::new();
    for s in e.selections() {
        marks.push((s.from(), 2, "«"));
        marks.push((s.to(), 1, "»"));
    }
    if cur < cs.len() && cs[cur] != '\n' && cs[cur] != '\r' {
        marks.push((cur, 3, "["));
        marks.push((cur + 1, 0, "]"));
    } else {
        marks.push((cur, 3, "[]"));
    }
    marks.sort_by_key(|m| (m.0, m.1));
    let mut out = String::new();
    let mut it = marks.into_iter().peekable();
    for i in 0..=cs.len() {
        while let Some(m) = it.peek() {
            if m.0 == i {
                out.push_str(m.2);
                it.next();
            } else {
                break;
            }
        }
        if i < cs.len() {
            out.push(cs[i]);
        }
    }
    out
}

#[track_caller]
pub fn vis(before: &str, keys: &str, expected: &str) {
    let mut e = vim_editor(before);
    feed(&mut e, keys);
    assert_eq!(render_vis(&e), expected, "\nbefore: {before:?}\nkeys:   {keys:?}\nmode:   {:?}", e.mode());
}

#[track_caller]
pub fn vis_mode(before: &str, keys: &str, expected: &str, mode: Mode) {
    let mut e = vim_editor(before);
    feed(&mut e, keys);
    assert_eq!(render_vis(&e), expected, "\nbefore: {before:?}\nkeys:   {keys:?}");
    assert_eq!(e.mode(), mode, "\nbefore: {before:?}\nkeys:   {keys:?}");
}

/// Text only (cursor ignored).
#[track_caller]
pub fn vim_text(before: &str, keys: &str, after: &str) {
    let mut e = vim_editor(before);
    feed(&mut e, keys);
    assert_eq!(e.text(), after, "\nbefore: {before:?}\nkeys:   {keys:?}");
}

/// Effects produced by `keys`.
pub fn vim_effects(before: &str, keys: &str) -> Vec<Effect> {
    let mut e = vim_editor(before);
    feed(&mut e, keys)
}

/// Contents of a register as (text, kind) with kind `c`, `l` or `b`.
pub fn vim_reg(e: &Editor, name: char) -> Option<(String, char)> {
    e.vim_reg_dump(name)
}

macro_rules! vim_cases {
    ($($name:ident: $before:expr, $keys:expr => $after:expr;)*) => {
        $(
            #[test]
            fn $name() {
                $crate::tests::vim_dsl::vim($before, $keys, $after);
            }
        )*
    };
}

macro_rules! vim_text_cases {
    ($($name:ident: $before:expr, $keys:expr => $after:expr;)*) => {
        $(
            #[test]
            fn $name() {
                $crate::tests::vim_dsl::vim_text($before, $keys, $after);
            }
        )*
    };
}

macro_rules! vis_cases {
    ($($name:ident: $before:expr, $keys:expr => $after:expr;)*) => {
        $(
            #[test]
            fn $name() {
                $crate::tests::vim_dsl::vis($before, $keys, $after);
            }
        )*
    };
}
pub(crate) use {vim_cases, vim_text_cases, vis_cases};

#[test]
fn dsl_parses_markers() {
    assert_eq!(parse_vim("fo[o] bar"), ("foo bar".to_string(), 2));
    assert_eq!(parse_vim("a\n[]\nb"), ("a\n\nb".to_string(), 2));
    assert_eq!(parse_vim("‹a›[1]"), ("a[1]".to_string(), 0));
    let e = vim_editor("fo[o] bar");
    assert_eq!(render_vim(&e, "["), "fo[o] bar");
    assert_eq!(render_vis(&vim_editor("[a]b")), "«[a]»b");
}
