pub use crate::{Editor, Effect, Key, KeyInput, Keymap, Mode, Selection};

/// Parse `before` into (text, selections, primary).
pub fn parse_marked(s: &str) -> (String, Vec<Selection>, usize) {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut n = 0usize;
    let mut sels = Vec::new();
    let mut primary = 0usize;
    let mut i = 0;
    while i < cs.len() {
        if i + 1 < cs.len() && cs[i] == '#' && (cs[i + 1] == '[' || cs[i + 1] == '(') {
            let is_primary = cs[i + 1] == '[';
            let close = if is_primary { ']' } else { ')' };
            i += 2;
            let start = n;
            let mut bar = None;
            while i < cs.len() && !(cs[i] == close && cs.get(i + 1) == Some(&'#')) {
                if cs[i] == '|' && bar.is_none() {
                    bar = Some(n);
                } else {
                    out.push(cs[i]);
                    n += 1;
                }
                i += 1;
            }
            assert!(i < cs.len(), "unterminated selection marker in {s:?}");
            i += 2;
            let end = n;
            let bar = bar.unwrap_or_else(|| panic!("selection without `|` in {s:?}"));
            let (anchor, head) = if bar == end { (start, end) } else { (end, start) };
            if is_primary {
                primary = sels.len();
            }
            sels.push(Selection { anchor, head });
        } else {
            out.push(cs[i]);
            n += 1;
            i += 1;
        }
    }
    (out, sels, primary)
}

/// Render text with selections back into the marked form.
pub fn render_marked(text: &str, sels: &[Selection], primary: usize) -> String {
    let cs: Vec<char> = text.chars().collect();
    let mut inserts: Vec<(usize, u8, String)> = Vec::new(); // (pos, order, text)
    for (i, s) in sels.iter().enumerate() {
        let (open, close) = if i == primary { ("#[", "]#") } else { ("#(", ")#") };
        let (from, to) = (s.anchor.min(s.head), s.anchor.max(s.head));
        if s.anchor == s.head {
            inserts.push((from, 1, format!("{open}|{close}")));
        } else if s.head > s.anchor {
            inserts.push((from, 1, open.to_string()));
            inserts.push((to, 0, format!("|{close}")));
        } else {
            inserts.push((from, 1, format!("{open}|")));
            inserts.push((to, 0, close.to_string()));
        }
    }
    inserts.sort_by_key(|(p, o, _)| (*p, *o));
    let mut out = String::new();
    let mut it = inserts.into_iter().peekable();
    for pos in 0..=cs.len() {
        while let Some((p, _, t)) = it.peek() {
            if *p == pos {
                out.push_str(t);
                it.next();
            } else {
                break;
            }
        }
        if pos < cs.len() {
            out.push(cs[pos]);
        }
    }
    out
}

/// Parse a key string like `ihello<esc>d<C-d>`.
pub fn parse_keys(s: &str) -> Vec<KeyInput> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '<' {
            if let Some(j) = cs[i + 1..].iter().position(|&c| c == '>').filter(|&j| j > 0 && j <= 14) {
                let name: String = cs[i + 1..i + 1 + j].iter().collect();
                if let Some(k) = KeyInput::parse(&name) {
                    out.push(k);
                    i += j + 2;
                    continue;
                }
            }
        }
        out.push(KeyInput { key: Key::Char(cs[i]), modifiers: Default::default() });
        i += 1;
    }
    out
}

pub fn editor_from(keymap: Keymap, before: &str) -> Editor {
    let (text, sels, primary) = parse_marked(before);
    let mut e = Editor::new(&text, keymap);
    if !sels.is_empty() {
        e.set_selections(sels, primary);
    }
    e
}

/// Feed keys, collecting effects (without the ever-present RevealCursor).
pub fn feed(e: &mut Editor, keys: &str) -> Vec<Effect> {
    let mut effects = Vec::new();
    for k in parse_keys(keys) {
        let out = e.handle_key(&k);
        effects.extend(out.effects.into_iter().filter(|x| *x != Effect::RevealCursor));
    }
    effects
}

pub fn state(e: &Editor) -> String {
    render_marked(&e.text(), e.selections(), e.primary_index())
}

#[track_caller]
pub fn check(keymap: Keymap, before: &str, keys: &str, after: &str) {
    let mut e = editor_from(keymap, before);
    feed(&mut e, keys);
    let got = if after.contains("#[") { state(&e) } else { e.text() };
    assert_eq!(got, after, "\nbefore: {before:?}\nkeys:   {keys:?}");
}

/// Helix keymap check.
#[track_caller]
pub fn hx(before: &str, keys: &str, after: &str) {
    check(Keymap::Helix, before, keys, after);
}

/// Standard keymap check.
#[track_caller]
pub fn st(before: &str, keys: &str, after: &str) {
    check(Keymap::Standard, before, keys, after);
}

/// Run keys and return effects for inspection.
pub fn effects_of(keymap: Keymap, before: &str, keys: &str) -> Vec<Effect> {
    let mut e = editor_from(keymap, before);
    feed(&mut e, keys)
}

#[test]
fn dsl_roundtrip() {
    for s in ["#[h|]#ello", "#[|hello]# #(w|)#orld", "abc#[|]#", "#(a|)# #[b|]# #(|c)#"] {
        let (t, sels, p) = parse_marked(s);
        assert_eq!(render_marked(&t, &sels, p), s);
    }
}
