//! Per-key latency on a 20,000-line document (the workspace builds tests with opt-level 1).

use super::*;
use std::time::{Duration, Instant};

/// Wall-clock bounds are for a quiet developer machine. Shared CI runners (`CI` is set by GitHub
/// Actions) are several times slower and noisier, so the bounds widen there; an O(document)
/// regression is still orders of magnitude over even the widened budget.
fn slack() -> u32 {
    if std::env::var_os("CI").is_some() { 10 } else { 1 }
}

fn big_doc() -> String {
    (0..20_000)
        .map(|i| format!("SELECT col_{i}, other FROM table_{i} WHERE id = {i} AND name = 'x'; -- line {i}\n"))
        .collect()
}

/// Shared machines produce the odd scheduling spike, so allow a single key over the budget as
/// long as nothing is egregiously slow; a real O(document) regression would hit many keys.
fn assert_fast(mut times: Vec<(Duration, String)>, what: &str) {
    times.sort_by_key(|t| std::cmp::Reverse(t.0));
    let budget = Duration::from_millis(5) * slack();
    let over = times.iter().filter(|t| t.0 > budget).count();
    eprintln!("{what} worst: {:?} ({})", times[0].0, times[0].1);
    assert!(
        over <= 1 && times[0].0 < Duration::from_millis(40) * slack(),
        "{what}: slow keys {:?}",
        &times[..over.clamp(1, 4)]
    );
}

fn time_keys(e: &mut Editor, keys: &str) -> Duration {
    let mut worst = Duration::ZERO;
    for k in parse_keys(keys) {
        let t = Instant::now();
        e.handle_key(&k);
        worst = worst.max(t.elapsed());
    }
    worst
}

#[test]
fn helix_keys_are_fast_on_20k_lines() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Helix);
    e.set_viewport(10_000, 50);
    feed(&mut e, "10000G");
    let keys = [
        "j",
        "k",
        "w",
        "b",
        "e",
        "l",
        "h",
        "x",
        "xd",
        "u",
        "U",
        "ifoo<esc>",
        "A;<esc>",
        "o<esc>",
        "dd",
        "yp",
        "P",
        "gg",
        "ge",
        "10000G",
        "mm",
        "ms(",
        "mi(",
        "miw",
        ">",
        "<lt>",
        "J",
        "~",
        "<C-d>",
        "<C-u>",
        "<C-f>",
        "<C-b>",
        "f;",
        "tx",
        "<A-;>",
        ";",
        "C",
        ",",
        "<C-c>",
        "<C-a>",
        "vjjj<esc>",
        "xxx",
        "d",
        "i<ret><esc>",
        "i(<esc>",
        "zz",
        "gl",
        "gh",
        "gs",
    ];
    let times = keys.iter().map(|k| (time_keys(&mut e, k), k.to_string())).collect();
    assert_fast(times, "helix");
}

#[test]
fn typing_in_insert_mode_is_fast() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Helix);
    feed(&mut e, "10000Gi");
    let mut worst = Duration::ZERO;
    for c in "select * from users where id = 1".chars() {
        let t = Instant::now();
        e.handle_key(&KeyInput::ch(c));
        worst = worst.max(t.elapsed());
    }
    eprintln!("insert worst: {worst:?}");
    assert!(worst < Duration::from_millis(20) * slack(), "{worst:?}");
}

#[test]
fn standard_keys_are_fast_on_20k_lines() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Standard);
    e.set_viewport(10_000, 50);
    e.click(e.line_to_char(10_000) + 4, false, false);
    let keys = [
        "x",
        "abc def",
        "<ret>",
        "<backspace>",
        "<left>",
        "<right>",
        "<up>",
        "<down>",
        "<home>",
        "<end>",
        "<C-left>",
        "<C-right>",
        "<S-down>",
        "<C-z>",
        "<C-S-z>",
        "<tab>",
        "<S-tab>",
        "<C-/>",
        "<C-d>",
        "<A-up>",
        "<A-down>",
        "<C-S-d>",
        "<C-S-k>",
        "<C-l>",
        "<pagedown>",
        "<pageup>",
        "<C-A-down>",
        "<esc>",
        "<C-c>",
        "<C-x>",
        "<C-v>",
        "<C-home>",
        "<C-end>",
        "<C-a>",
        "<esc>",
    ];
    e.set_clipboard(Some("pasted".into()));
    let times = keys.iter().map(|k| (time_keys(&mut e, k), k.to_string())).collect();
    assert_fast(times, "standard");
}

#[test]
fn many_cursors_stay_responsive() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Helix);
    // a few hundred cursors, one per line
    feed(&mut e, "10000G");
    feed(&mut e, "200C");
    assert!(e.selections().len() > 150);
    let t = Instant::now();
    feed(&mut e, "ix");
    let typed = t.elapsed();
    feed(&mut e, "<esc>");
    assert!(typed < Duration::from_millis(50) * slack(), "{typed:?}");
    assert!(e.text().matches("xSELECT").count() >= 150);
}

#[test]
fn search_keys_on_20k_lines() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Helix);
    feed(&mut e, "10000G");
    let mut worst = (Duration::ZERO, String::new());
    for keys in [
        "/table_15",
        "<ret>",
        "n",
        "N",
        "*",
        "n",
        "/zzzznotthere",
        "<esc>",
        "?other",
        "<esc>",
        "/id = 1",
        "<ret>",
        "n",
    ] {
        let mut keys_worst = Duration::ZERO;
        for k in parse_keys(keys) {
            let t = Instant::now();
            e.handle_key(&k);
            keys_worst = keys_worst.max(t.elapsed());
        }
        eprintln!("{keys:>16}: {keys_worst:?}");
        if keys_worst > worst.0 {
            worst = (keys_worst, keys.to_string());
        }
    }
    assert!(worst.0 < Duration::from_millis(20) * slack(), "{worst:?}");
    // the highlight cache keeps repeated asks cheap
    let t = Instant::now();
    e.search_matches();
    e.search_matches();
    eprintln!("search_matches x2: {:?}", t.elapsed());
}

/// Every Vim key on a 20k-line buffer, including the scans that could walk the whole document
/// (`}`, `)`, `ip`, `*`, searches) and undo/redo/dot after big edits. Whole-file ex commands are
/// excluded on purpose: `:%s` is O(document) by definition.
#[test]
fn vim_keys_are_fast_on_20k_lines() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Vim);
    e.set_viewport(10_000, 50);
    feed(&mut e, "10000G");
    let keys = [
        "j",
        "k",
        "w",
        "b",
        "e",
        "ge",
        "W",
        "B",
        "E",
        "l",
        "h",
        "0",
        "^",
        "$",
        "x",
        ".",
        "u",
        "<C-r>",
        "dd",
        "yyp",
        "P",
        "p",
        "J",
        "~",
        ">>",
        "<lt><lt>",
        "==",
        "gg",
        "G",
        "10000G",
        "%",
        "}",
        "{",
        ")",
        "(",
        "]]",
        "[[",
        "H",
        "M",
        "L",
        "zz",
        "<C-d>",
        "<C-u>",
        "<C-f>",
        "<C-b>",
        "<C-e>",
        "<C-y>",
        "*",
        "#",
        "n",
        "N",
        "/table_5<ret>",
        "?table_9<ret>",
        "fx",
        "tx",
        ";",
        ",",
        "ma",
        "'a",
        "`a",
        "<C-o>",
        "<C-i>",
        "ciw<esc>",
        "dap",
        "yip",
        "vipy",
        "Vjjd",
        "<C-v>jjIx<esc>",
        "viw<esc>",
        "gv",
        "ifoo<esc>",
        "A;<esc>",
        "o<esc>",
        "O<esc>",
        "3dd",
        "qaxjq",
        "@a",
        "yi'",
        "ci(x<esc>",
        "gUU",
        "g~w",
        "<C-a>",
        "gcc",
        ":5,8s/col/c/<ret>",
        ":10,20d<ret>",
        ":noh<ret>",
        ":5,9m12<ret>",
    ];
    let times = keys.iter().map(|k| (time_keys(&mut e, k), k.to_string())).collect();
    assert_fast(times, "vim");
}

/// A document without blank lines or sentence ends is the worst case for `)`, `}` and `ip`.
#[test]
fn vim_paragraph_and_sentence_scans_are_bounded() {
    let doc: String = (0..20_000).map(|i| format!("word{i} and more words here\n")).collect();
    let mut e = Editor::new(&doc, Keymap::Vim);
    e.set_viewport(0, 50);
    let keys = ["}", ")", "ip", "ap", "vip<esc>", "is", "as", "G", "{", "(", "dip", "u"];
    let times = keys.iter().map(|k| (time_keys(&mut e, k), k.to_string())).collect();
    assert_fast(times, "vim scans");
}
