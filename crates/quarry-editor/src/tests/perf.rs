//! Per-key latency on a 20,000-line document (the workspace builds tests with opt-level 1).

use super::*;
use std::time::{Duration, Instant};

fn big_doc() -> String {
    (0..20_000).map(|i| format!("SELECT col_{i}, other FROM table_{i} WHERE id = {i} AND name = 'x'; -- line {i}\n")).collect()
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
        "j", "k", "w", "b", "e", "l", "h", "x", "xd", "u", "U", "ifoo<esc>", "A;<esc>", "o<esc>", "dd", "yp", "P", "gg", "ge", "10000G",
        "mm", "ms(", "mi(", "miw", ">", "<lt>", "J", "~", "<C-d>", "<C-u>", "<C-f>", "<C-b>", "f;", "tx", "<A-;>", ";", "C", ",", "<C-c>", "<C-a>",
        "vjjj<esc>", "xxx", "d", "i<ret><esc>", "i(<esc>", "zz", "gl", "gh", "gs",
    ];
    let mut worst = (Duration::ZERO, "");
    for k in keys {
        let d = time_keys(&mut e, k);
        if d > worst.0 {
            worst = (d, k);
        }
    }
    eprintln!("helix worst: {:?} ({})", worst.0, worst.1);
    assert!(worst.0 < Duration::from_millis(5), "slow key {:?} took {:?}", worst.1, worst.0);
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
    assert!(worst < Duration::from_millis(5), "{worst:?}");
}

#[test]
fn standard_keys_are_fast_on_20k_lines() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Standard);
    e.set_viewport(10_000, 50);
    e.click(e.line_to_char(10_000) + 4, false, false);
    let keys = [
        "x", "abc def", "<ret>", "<backspace>", "<left>", "<right>", "<up>", "<down>", "<home>", "<end>", "<C-left>", "<C-right>", "<S-down>",
        "<C-z>", "<C-S-z>", "<tab>", "<S-tab>", "<C-/>", "<C-d>", "<A-up>", "<A-down>", "<C-S-d>", "<C-S-k>", "<C-l>", "<pagedown>",
        "<pageup>", "<C-A-down>", "<esc>", "<C-c>", "<C-x>", "<C-v>", "<C-home>", "<C-end>", "<C-a>", "<esc>",
    ];
    e.set_clipboard(Some("pasted".into()));
    let mut worst = (Duration::ZERO, "");
    for k in keys {
        let d = time_keys(&mut e, k);
        if d > worst.0 {
            worst = (d, k);
        }
    }
    eprintln!("standard worst: {:?} ({})", worst.0, worst.1);
    assert!(worst.0 < Duration::from_millis(5), "slow key {:?} took {:?}", worst.1, worst.0);
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
    assert!(typed < Duration::from_millis(50), "{typed:?}");
    assert!(e.text().matches("xSELECT").count() >= 150);
}

#[test]
fn search_keys_on_20k_lines() {
    let doc = big_doc();
    let mut e = Editor::new(&doc, Keymap::Helix);
    feed(&mut e, "10000G");
    let mut worst = (Duration::ZERO, String::new());
    for keys in ["/table_15", "<ret>", "n", "N", "*", "n", "/zzzznotthere", "<esc>", "?other", "<esc>", "/id = 1", "<ret>", "n"] {
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
    assert!(worst.0 < Duration::from_millis(20), "{worst:?}");
    // the highlight cache keeps repeated asks cheap
    let t = Instant::now();
    e.search_matches();
    e.search_matches();
    eprintln!("search_matches x2: {:?}", t.elapsed());
}
