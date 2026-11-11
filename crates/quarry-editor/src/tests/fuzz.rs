//! Randomised key-sequence fuzzing with a fixed seed: no panics, selections always valid,
//! and undoing everything returns the original text.

use super::*;
use crate::Modifiers;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic across platforms, good enough for fuzzing
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const DOCS: &[&str] = &[
    "",
    "\n",
    "select 1;\nselect 2;\n",
    "SELECT a, b FROM t WHERE (x = 1 AND y = 'z') -- c\nORDER BY a;",
    "héllo wörld\n日本語 テスト\n🙂👨\u{200d}👩\u{200d}👧 e\u{0301}x\n",
    "a\r\nb\r\n\r\nc",
    "    indented\n\tTab\n  (\n   )\n",
    "word1 word2.word3, 'q' \"d\" `b` [1] {2} <3>\n\n\nlast line no newline",
];

fn helix_pool() -> Vec<KeyInput> {
    let mut v = Vec::new();
    for c in (0x20u8..0x7f).map(|b| b as char) {
        v.push(KeyInput::ch(c));
    }
    for name in [
        "esc",
        "ret",
        "tab",
        "backspace",
        "del",
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "pageup",
        "pagedown",
        "A-;",
        "A-:",
        "A-,",
        "A-.",
        "A-s",
        "A-x",
        "A-d",
        "A-c",
        "A-J",
        "A-K",
        "A-C",
        "A-minus",
        "A-_",
        "A-(",
        "A-)",
        "A-`",
        "A-*",
        "C-a",
        "C-x",
        "C-c",
        "C-d",
        "C-u",
        "C-f",
        "C-b",
        "C-o",
        "C-i",
        "C-s",
        "C-w",
        "C-h",
        "C-k",
        "C-space",
        "C-ret",
    ] {
        v.push(KeyInput::parse(name).unwrap());
    }
    v
}

fn standard_pool() -> Vec<KeyInput> {
    let mut v = Vec::new();
    for c in "abcdefghijklmnopqrstuvwxyz ()[]{}'\"`=,.;:-+*/_0123456789".chars() {
        v.push(KeyInput::ch(c));
    }
    for l in "acxvzydlkfgwnbjpeist/".chars() {
        v.push(KeyInput { key: Key::Char(l), modifiers: Modifiers { ctrl: true, ..Default::default() } });
        v.push(KeyInput {
            key: Key::Char(l.to_ascii_uppercase()),
            modifiers: Modifiers { ctrl: true, ..Default::default() },
        });
    }
    for name in [
        "esc",
        "ret",
        "tab",
        "S-tab",
        "backspace",
        "del",
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "pageup",
        "pagedown",
        "S-left",
        "S-right",
        "S-up",
        "S-down",
        "S-home",
        "S-end",
        "C-left",
        "C-right",
        "C-S-left",
        "C-S-right",
        "C-home",
        "C-end",
        "C-backspace",
        "C-del",
        "A-up",
        "A-down",
        "A-S-up",
        "A-S-down",
        "C-A-up",
        "C-A-down",
        "F3",
        "S-F3",
        "C-ret",
        "C-tab",
        "cmd-left",
        "cmd-backspace",
        "A-left",
        "A-right",
        "C-A-l",
        "F1",
        "F5",
    ] {
        v.push(KeyInput::parse(name).unwrap());
    }
    v
}

fn vim_pool() -> Vec<KeyInput> {
    let mut v = Vec::new();
    for c in (0x20u8..0x7f).map(|b| b as char) {
        // letters and digits get more weight: they start most commands
        v.push(KeyInput::ch(c));
        if c.is_ascii_alphanumeric() {
            v.push(KeyInput::ch(c));
        }
    }
    for l in "adefghjklmnopqrtuvwxyb[c ".chars() {
        v.push(KeyInput { key: Key::Char(l), modifiers: Modifiers { ctrl: true, ..Default::default() } });
    }
    for name in [
        "esc",
        "esc",
        "ret",
        "tab",
        "backspace",
        "del",
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "pageup",
        "pagedown",
        "C-ret",
        "C-S-ret",
        "F5",
        "C-space",
    ] {
        v.push(KeyInput::parse(name).unwrap());
    }
    v
}

/// A few whole commands, typed in one go: random single keys rarely form valid ones.
const VIM_WORDS: &[&str] = &[
    "dd",
    "yyp",
    "ciwx<esc>",
    "dap",
    "ci(y<esc>",
    "vipd",
    "<C-v>jjIx<esc>",
    "<C-v>jj$Ay<esc>",
    "Vjd",
    "viwp",
    "qaxjq@a",
    "3@a",
    "10.",
    "gUU",
    "g~w",
    ">>",
    "<lt><lt>",
    "==",
    "gcc",
    "J",
    "gJ",
    ":s/a/b/g<ret>",
    ":%s/\\(a\\)/[&]/g<ret>",
    ":g/e/d<ret>",
    ":g/x/normal Ax<ret>",
    ":sort<ret>",
    ":%norm Ix<ret>",
    ":2,3m0<ret>",
    ":1,2t$<ret>",
    ":noh<ret>",
    "/a<ret>",
    "?e<ret>",
    "*",
    "#",
    "n",
    "N",
    "ma",
    "'a",
    "`a",
    "gv",
    "<C-o>",
    "<C-i>",
    "ifoo<C-o>0bar<esc>",
    "Rab<bs><esc>",
    "ix<C-r>\"<esc>",
    "<C-a>",
    "5<C-x>",
    "p",
    "P",
    "gp",
    "u",
    "<C-r>",
    ".",
    "x",
    "~",
    "ra",
    "r<ret>",
    "cc<esc>",
    "S<esc>",
    "C<esc>",
    "D",
    "A<ret><esc>",
    "o<esc>",
    "O<esc>",
    "zz",
    "<C-d>",
    "<C-u>",
    "H",
    "M",
    "L",
];

fn check_invariants(e: &Editor, ctx: &str) {
    let len = e.len_chars();
    let sels = e.selections();
    assert!(!sels.is_empty(), "{ctx}: no selections");
    assert!(e.primary_index() < sels.len(), "{ctx}: primary out of range");
    let text = e.text();
    assert_eq!(e.line_count(), text.matches('\n').count() + 1, "{ctx}: line count");
    let mut prev_to = 0;
    for (i, s) in sels.iter().enumerate() {
        assert!(s.anchor <= len && s.head <= len, "{ctx}: selection {s:?} out of bounds (len {len})");
        // offsets are char indexes: converting to bytes must land on a char boundary
        for p in [s.anchor, s.head] {
            let b = e.char_to_byte(p);
            assert!(text.is_char_boundary(b), "{ctx}: {p} not a char boundary");
            assert_eq!(e.byte_to_char(b), p);
        }
        if i > 0 {
            assert!(s.range().start >= prev_to, "{ctx}: overlapping/unsorted selections {sels:?}");
        }
        prev_to = s.range().end;
    }
    // CRLF is never split: a boundary must not fall between \r and \n
    let chars: Vec<char> = text.chars().collect();
    for s in sels {
        for p in [s.anchor, s.head] {
            if p > 0 && p < chars.len() && chars[p - 1] == '\r' && chars[p] == '\n' {
                panic!("{ctx}: selection splits a CRLF at {p}");
            }
        }
    }
    if let Some(p) = e.prompt() {
        assert!(p.cursor <= p.text.chars().count(), "{ctx}: prompt cursor");
    }
    if e.keymap() == Keymap::Vim
        && e.prompt().is_none()
        && e.mode() == Mode::Normal
        && e.selections().len() == 1
    {
        // Normal mode never rests on the line end of a non-empty line
        let c = e.cursor_of(&e.primary());
        let l = e.char_to_line(c.min(len));
        let ls = e.line_to_char(l);
        let content = e.line(l).chars().count();
        if content > 0 && e.primary().len() <= 1 {
            assert!(
                c < ls + content,
                "{ctx}: Normal-mode cursor {c} is past the end of its line; text {:?} sels {:?}",
                e.text(),
                e.selections()
            );
        }
    }
}

fn run_fuzz(keymap: Keymap, seed: u64, iterations: usize, keys_per_run: usize) {
    let pool = match keymap {
        Keymap::Helix => helix_pool(),
        Keymap::Standard => standard_pool(),
        Keymap::Vim => vim_pool(),
    };
    let mut rng = Rng(seed);
    for iter in 0..iterations {
        let doc = *rng.pick(DOCS);
        let mut e = Editor::new(doc, keymap);
        e.set_viewport(0, 10);
        let mut log: Vec<String> = Vec::new();
        for step in 0..keys_per_run {
            let ctx_log = log.join(" ");
            let ctx = format!("{keymap:?} iter {iter} step {step} doc {doc:?} keys [{ctx_log}]");
            match rng.below(40) {
                0 => {
                    let len = e.len_chars();
                    let at = rng.below(len + 2);
                    log.push(format!("click({at})"));
                    e.click(at, rng.below(3) == 0, rng.below(4) == 0);
                }
                1 => {
                    let at = rng.below(e.len_chars() + 2);
                    log.push(format!("drag({at})"));
                    e.drag_to(at);
                }
                2 => {
                    let at = rng.below(e.len_chars() + 2);
                    log.push(format!("dbl({at})"));
                    e.double_click(at);
                }
                3 => {
                    let at = rng.below(e.len_chars() + 2);
                    log.push(format!("triple({at})"));
                    e.triple_click(at);
                }
                4 => {
                    let t = *rng.pick(&["x", "foo\nbar", "(", "日本", "\r\n", "e\u{0301}", ""]);
                    log.push(format!("text({t:?})"));
                    e.insert_text(t);
                }
                5 => {
                    let t = *rng.pick(&["clip", "a\nb\n", "", "x y"]);
                    log.push(format!("clipboard({t:?})"));
                    e.set_clipboard(Some(t.to_string()));
                }
                6 => {
                    let n = e.len_chars();
                    let (a, b) = (rng.below(n + 1), rng.below(n + 1));
                    let t = *rng.pick(&["", "Z", "é\n"]);
                    log.push(format!("replace({a}..{b}, {t:?})"));
                    e.replace_range(a.min(b)..a.max(b), t);
                }
                7 => {
                    let (first, visible) = (rng.below(5), 1 + rng.below(30));
                    log.push(format!("viewport({first},{visible})"));
                    e.set_viewport(first, visible);
                }
                8 | 9 if keymap == Keymap::Vim => {
                    let w = *rng.pick(VIM_WORDS);
                    log.push(w.to_string());
                    for k in parse_keys(w) {
                        e.handle_key(&k);
                        check_invariants(&e, &format!("{ctx} word {w}"));
                    }
                }
                _ => {
                    let k = rng.pick(&pool).clone();
                    log.push(k.display());
                    e.handle_key(&k);
                }
            }
            if std::env::var("QUARRY_FUZZ_TRACE").ok().and_then(|v| v.parse::<usize>().ok()) == Some(iter) {
                eprintln!(
                    "step {step}: {} -> mode {:?} sels {:?} text {:?}",
                    log.last().unwrap(),
                    e.mode(),
                    e.selections(),
                    e.text()
                );
            }
            check_invariants(&e, &ctx);
            let _ = e.pending_keys();
            let _ = e.key_hints();
            let _ = e.matching_bracket();
            let _ = e.search_matches();
            if step % 7 == 0 {
                let _ = e.highlights(rng.below(e.line_count()));
            }
        }
        // leave any prompt / insert mode, then undo everything
        for _ in 0..3 {
            e.handle_key(&KeyInput::parse("esc").unwrap());
        }
        let mut guard = 0;
        while e.undo() {
            guard += 1;
            assert!(guard < 100_000, "undo does not terminate");
        }
        assert_eq!(
            e.text(),
            doc,
            "{keymap:?} iter {iter}: undo-all must restore the original text; keys [{}]",
            log.join(" ")
        );
        // and redo-all then undo-all is stable
        let mut n = 0;
        while e.redo() {
            n += 1;
        }
        let after = e.text();
        for _ in 0..n {
            assert!(e.undo());
        }
        assert_eq!(e.text(), doc);
        let _ = after;
    }
}

/// `QUARRY_FUZZ_SEED` / `QUARRY_FUZZ_ITERS` let a developer run a much longer campaign locally.
fn campaign(default_seed: u64, default_iters: usize) -> (u64, usize) {
    let seed = std::env::var("QUARRY_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(default_seed);
    let iters = std::env::var("QUARRY_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(default_iters);
    (seed, iters)
}

#[test]
fn fuzz_helix() {
    let (seed, iters) = campaign(0x5EED_0001, 3000);
    run_fuzz(Keymap::Helix, seed, iters, 30);
}

#[test]
fn fuzz_standard() {
    let (seed, iters) = campaign(0x5EED_0002, 3000);
    run_fuzz(Keymap::Standard, seed, iters, 30);
}

#[test]
fn fuzz_vim() {
    let (seed, iters) = campaign(0x5EED_0003, 3000);
    run_fuzz(Keymap::Vim, seed, iters, 30);
}

#[test]
fn fuzz_vim_second_seed() {
    run_fuzz(Keymap::Vim, 0x7E57_BEEF, 2000, 40);
}

#[test]
fn fuzz_long_sessions() {
    run_fuzz(Keymap::Helix, 0xABCD_1234, 150, 400);
    run_fuzz(Keymap::Standard, 0xABCD_5678, 150, 400);
    run_fuzz(Keymap::Vim, 0xABCD_9ABC, 150, 400);
}
