//! Cases whose expected results were produced by real Vim (`tests/vim_oracle/`): each line of
//! `golden.tsv` is `text<TAB>keys<TAB>vim's text<TAB>line<TAB>col[<TAB>name=type:text]...`.
//! `text` carries the cursor marker (`[x]` or `‹x›`); `line`/`col` are Vim's 1-based line and
//! byte column of the cursor afterwards.

use super::*;

const GOLDEN: &str = include_str!("../../tests/vim_oracle/golden.tsv");

pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(d) => out.push(d),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// (line, byte column) of the cursor, 1-based like Vim.
pub(crate) fn cursor_pos(e: &Editor) -> (usize, usize) {
    let c = e.cursor_of(&e.primary());
    let l = e.char_to_line(c);
    let col = e.char_to_byte(c) - e.char_to_byte(e.line_to_char(l));
    (l + 1, col + 1)
}

#[test]
fn golden_cases_agree_with_real_vim() {
    let mut failures = Vec::new();
    let mut n = 0;
    for (i, line) in GOLDEN.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        n += 1;
        let f: Vec<&str> = line.split('\t').collect();
        let before = unescape(f[0]);
        let keys_owned = f[1].replace('\u{a6}', "");
        let keys = keys_owned.as_str();
        let (want_text, want_line, want_col): (String, usize, usize) =
            (unescape(f[2]), f[3].parse().unwrap(), f[4].parse().unwrap());
        let mut e = vim_editor(&before);
        if keys.starts_with(':') {
            feed(&mut e, &format!("{keys}<ret>"));
        } else {
            feed(&mut e, keys);
        }
        let got = e.text();
        let (gl, gc) = cursor_pos(&e);
        let mut bad = Vec::new();
        if got != want_text {
            bad.push(format!("text {got:?} != vim {want_text:?}"));
        }
        if (gl, gc) != (want_line, want_col) {
            bad.push(format!("cursor {gl}:{gc} != vim {want_line}:{want_col}"));
        }
        {
            for r in f.iter().skip(5) {
                let Some((name, rest)) = r.split_once('=') else { continue };
                let Some((kind, text)) = rest.split_once(':') else { continue };
                let name = name.chars().next().unwrap();
                let want = if text.is_empty() && kind.is_empty() {
                    None
                } else {
                    Some((unescape(text), kind.chars().next().unwrap_or('c')))
                };
                let got = vim_reg(&e, name);
                let want_kind = want.as_ref().map(|w| match w.1 {
                    'v' => 'c',
                    'V' => 'l',
                    _ => 'b',
                });
                let got_cmp = got.as_ref().map(|g| (g.0.clone(), g.1));
                let want_cmp = want.map(|w| (w.0, want_kind.unwrap()));
                // Vim reports an empty register as "" (charwise); here it may simply be unset
                let got_cmp = got_cmp.or_else(|| want_cmp.as_ref().filter(|w| w.0.is_empty()).cloned());
                if want_cmp.is_some() && got_cmp != want_cmp {
                    bad.push(format!("register {name}: {got_cmp:?} != vim {want_cmp:?}"));
                }
            }
        }
        if !bad.is_empty() {
            if let Ok(path) = std::env::var("ORACLE_REPORT") {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
                let esc = |s: &str| s.replace('\\', "\\\\").replace('\n', "\\n");
                writeln!(
                    f,
                    "{}\t{}\t{}\t{}\t{gl}:{gc}\t{want_line}:{want_col}\t{}",
                    esc(&before),
                    keys,
                    esc(&got),
                    esc(&want_text),
                    esc(&bad.join(" ; "))
                )
                .unwrap();
            }
            failures.push(format!(
                "line {}: {:?} keys {:?}\n    {}",
                i + 1,
                before,
                keys,
                bad.join("\n    ")
            ));
        }
    }
    assert!(n > 0, "golden file is empty");
    let shown = failures.iter().take(60).cloned().collect::<Vec<_>>().join("\n");
    assert!(
        failures.is_empty(),
        "{} of {n} oracle cases differ from Vim (first 60):\n{shown}",
        failures.len()
    );
}

/// `PROBE_TEXT='a [b]c' PROBE_KEYS='dw' cargo test probe -- --ignored --nocapture` shows what
/// the engine does with one case (compare with `tests/vim_oracle/oracle.py probe`).
#[test]
#[ignore]
fn probe() {
    let text = std::env::var("PROBE_TEXT").unwrap_or_default();
    let keys = std::env::var("PROBE_KEYS").unwrap_or_default();
    let mut e = vim_editor(&unescape(&text));
    let fx = feed(&mut e, &keys);
    let (l, c) = cursor_pos(&e);
    println!("text={:?} line={l} col={c} mode={:?} effects={fx:?}", e.text(), e.mode());
}
