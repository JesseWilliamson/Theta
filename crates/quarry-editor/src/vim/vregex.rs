//! Vim regular expressions on top of the `regex` crate.
//!
//! Vim's pattern syntax differs from Rust's mainly in what needs a backslash: in the default
//! "magic" mode `( ) | + ? { }` are literal and `\( \) \| \+ \? \{` are the operators. This module
//! translates the commonly used subset and reports everything else (back-references, `\zs`,
//! look-around ...) as an explicit error instead of silently matching something different.

use regex::{Regex, RegexBuilder};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Magic {
    VeryMagic,
    Magic,
    NoMagic,
    VeryNoMagic,
}

const SET_A: &str = "^$.*[~";
const SET_B: &str = "()|+?={<>@%&";

fn is_special(mode: Magic, c: char, escaped: bool) -> bool {
    let a = SET_A.contains(c);
    let b = SET_B.contains(c);
    match mode {
        Magic::VeryMagic => (a || b) != escaped,
        Magic::Magic => (a && !escaped) || (b && escaped) || (a && escaped && false),
        Magic::NoMagic => match c {
            '^' | '$' => !escaped,
            _ => (a || b) && escaped,
        },
        Magic::VeryNoMagic => (a || b) && escaped,
    }
}

fn has_uppercase(pat: &str) -> bool {
    let mut it = pat.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            it.next();
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

/// Escape `s` so Vim (and this translator) treat it literally in magic mode.
pub(crate) fn escape_literal(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, '\\' | '/' | '.' | '*' | '$' | '^' | '~' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn rust_lit(c: char) -> String {
    regex::escape(&c.to_string())
}

fn class_token(d: char) -> Option<&'static str> {
    Some(match d {
        's' => r"[ \t]",
        'S' => r"[^ \t]",
        'd' => "[0-9]",
        'D' => "[^0-9]",
        'w' => "[0-9A-Za-z_]",
        'W' => "[^0-9A-Za-z_]",
        'a' => "[A-Za-z]",
        'A' => "[^A-Za-z]",
        'l' => "[a-z]",
        'L' => "[^a-z]",
        'u' => "[A-Z]",
        'U' => "[^A-Z]",
        'x' => "[0-9A-Fa-f]",
        'X' => "[^0-9A-Fa-f]",
        'o' => "[0-7]",
        'O' => "[^0-7]",
        'h' => "[A-Za-z_]",
        'H' => "[^A-Za-z_]",
        'i' | 'k' => r"[0-9A-Za-z_\u{80}-\u{10FFFF}]",
        'I' | 'K' => r"[A-Za-z_\u{80}-\u{10FFFF}]",
        _ => return None,
    })
}

/// Translate a Vim pattern into `regex` syntax. Returns the pattern and an explicit case
/// request (`\c` => Some(true) meaning ignore case, `\C` => Some(false)).
pub(crate) fn translate(pat: &str) -> Result<(String, Option<bool>), String> {
    let cs: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    let mut mode = Magic::Magic;
    let mut ic: Option<bool> = None;
    let mut i = 0;
    // at the start of a branch: `^` is an anchor and `*` is literal
    let mut branch_start = true;
    let n = cs.len();
    let unsupported = |what: &str| Err(format!("unsupported Vim regex syntax: {what}"));
    while i < n {
        let c = cs[i];
        let (esc, ch) = if c == '\\' {
            if i + 1 >= n {
                out.push_str(r"\\");
                i += 1;
                branch_start = false;
                continue;
            }
            i += 1;
            (true, cs[i])
        } else {
            (false, c)
        };
        i += 1;
        let at_start = branch_start;
        branch_start = false;
        if esc {
            // escapes that are not mode dependent
            match ch {
                'v' => {
                    mode = Magic::VeryMagic;
                    branch_start = at_start;
                    continue;
                }
                'm' => {
                    mode = Magic::Magic;
                    branch_start = at_start;
                    continue;
                }
                'M' => {
                    mode = Magic::NoMagic;
                    branch_start = at_start;
                    continue;
                }
                'V' => {
                    mode = Magic::VeryNoMagic;
                    branch_start = at_start;
                    continue;
                }
                'c' => {
                    ic = Some(true);
                    branch_start = at_start;
                    continue;
                }
                'C' => {
                    ic = Some(false);
                    branch_start = at_start;
                    continue;
                }
                'z' => return unsupported(&format!("\\z{}", cs.get(i).copied().unwrap_or(' '))),
                'n' => {
                    out.push_str(r"\r?\n");
                    continue;
                }
                't' => {
                    out.push_str(r"\t");
                    continue;
                }
                'e' => {
                    out.push_str(r"\x1b");
                    continue;
                }
                'r' => {
                    out.push_str(r"\r");
                    continue;
                }
                'b' => {
                    out.push_str(r"\x08");
                    continue;
                }
                '1'..='9' => return unsupported(&format!("back-reference \\{ch}")),
                '_' => {
                    let d = cs.get(i).copied().unwrap_or(' ');
                    i += 1;
                    match d {
                        's' => out.push_str(r"[ \t\r\n]"),
                        '.' => out.push_str(r"(?s:.)"),
                        '^' => out.push('^'),
                        '$' => out.push('$'),
                        _ => return unsupported(&format!("\\_{d}")),
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(cl) = class_token(ch) {
                out.push_str(cl);
                continue;
            }
        }
        if !is_special(mode, ch, esc) {
            out.push_str(&rust_lit(ch));
            continue;
        }
        match ch {
            '^' => {
                if at_start {
                    out.push('^');
                    branch_start = true;
                } else {
                    out.push_str(r"\^");
                }
            }
            '$' => {
                let at_end = i >= n
                    || (cs[i] == '\\' && matches!(cs.get(i + 1), Some('|' | ')' | 'n' | '&')))
                    || (mode == Magic::VeryMagic && matches!(cs[i], '|' | ')'));
                if at_end {
                    out.push('$');
                } else {
                    out.push_str(r"\$");
                }
            }
            '.' => out.push('.'),
            '*' => {
                if at_start {
                    out.push_str(r"\*");
                } else {
                    out.push('*');
                }
            }
            '~' => return unsupported("~ (previous substitute string)"),
            '[' => match parse_collection(&cs, i) {
                Some((s, ni)) => {
                    out.push_str(&s);
                    i = ni;
                }
                None => out.push_str(r"\["),
            },
            '(' => {
                out.push('(');
                branch_start = true;
            }
            ')' => out.push(')'),
            '|' => {
                out.push('|');
                branch_start = true;
            }
            '+' => out.push('+'),
            '?' | '=' => out.push('?'),
            '<' => out.push_str(r"\b{start}"),
            '>' => out.push_str(r"\b{end}"),
            '{' => {
                // \{n,m} (the closing brace may be escaped)
                let mut j = i;
                let mut body = String::new();
                while j < n && cs[j] != '}' {
                    if cs[j] != '\\' {
                        body.push(cs[j]);
                    }
                    j += 1;
                }
                if j >= n {
                    return Err("E554: Syntax error in \\{...}".into());
                }
                i = j + 1;
                let (lazy, body) = match body.strip_prefix('-') {
                    Some(b) => (true, b.to_string()),
                    None => (false, body),
                };
                let q = match body.as_str() {
                    "" => "*".to_string(),
                    b if b.contains(',') => {
                        let (lo, hi) = b.split_once(',').unwrap();
                        format!("{{{},{}}}", if lo.is_empty() { "0" } else { lo }, hi)
                    }
                    b => format!("{{{b}}}"),
                };
                out.push_str(&q);
                if lazy {
                    out.push('?');
                }
            }
            '@' => return unsupported("\\@ (look-around)"),
            '%' => {
                let d = cs.get(i).copied().unwrap_or(' ');
                i += 1;
                match d {
                    '(' => {
                        out.push_str("(?:");
                        branch_start = true;
                    }
                    _ => return unsupported(&format!("\\%{d}")),
                }
            }
            '&' => return unsupported("\\& (concat)"),
            other => out.push_str(&rust_lit(other)),
        }
    }
    Ok((out, ic))
}

/// `[...]` starting after the `[`. Returns the Rust class and the index after the `]`.
fn parse_collection(cs: &[char], start: usize) -> Option<(String, usize)> {
    let n = cs.len();
    let mut i = start;
    let mut neg = false;
    if i < n && cs[i] == '^' {
        neg = true;
        i += 1;
    }
    let mut items = String::new();
    let mut first = true;
    let esc_item =
        |c: char| -> String { if c.is_ascii_punctuation() { format!("\\{c}") } else { c.to_string() } };
    // read one (possibly escaped) literal; returns (char, next index)
    let read = |i: usize| -> Option<(char, usize)> {
        let c = *cs.get(i)?;
        if c == '\\' {
            let d = *cs.get(i + 1)?;
            let v = match d {
                'e' => '\x1b',
                't' => '\t',
                'r' => '\r',
                'b' => '\x08',
                'n' => '\n',
                '\\' | ']' | '^' | '-' => d,
                _ => return Some(('\\', i + 1)),
            };
            Some((v, i + 2))
        } else {
            Some((c, i + 1))
        }
    };
    loop {
        let c = *cs.get(i)?;
        if c == ']' && !first {
            i += 1;
            break;
        }
        first = false;
        if c == '[' && cs.get(i + 1) == Some(&':') {
            // [:alpha:]
            if let Some(end) = (i + 2..n.saturating_sub(1)).find(|&j| cs[j] == ':' && cs[j + 1] == ']') {
                let name: String = cs[i + 2..end].iter().collect();
                let ok = matches!(
                    name.as_str(),
                    "alpha"
                        | "digit"
                        | "alnum"
                        | "upper"
                        | "lower"
                        | "space"
                        | "punct"
                        | "xdigit"
                        | "blank"
                        | "cntrl"
                        | "print"
                        | "graph"
                );
                if ok {
                    items.push_str(&format!("[:{name}:]"));
                    i = end + 2;
                    continue;
                }
            }
        }
        let (a, ni) = read(i)?;
        i = ni;
        if cs.get(i) == Some(&'-') && cs.get(i + 1).is_some_and(|&x| x != ']') {
            let (b, nj) = read(i + 1)?;
            items.push_str(&esc_item(a));
            items.push('-');
            items.push_str(&esc_item(b));
            i = nj;
        } else {
            items.push_str(&esc_item(a));
        }
    }
    if items.is_empty() {
        return None;
    }
    Some((format!("[{}{}]", if neg { "^" } else { "" }, items), i))
}

/// Compile with the editor's case options (`\c` / `\C` in the pattern win).
pub(crate) fn compile(pat: &str, ignorecase: bool, smartcase: bool) -> Result<Regex, String> {
    let (rs, force) = translate(pat)?;
    let ic = match force {
        Some(v) => v,
        None => ignorecase && !(smartcase && has_uppercase(pat)),
    };
    build(&rs, ic)
}

pub(crate) fn build(rs: &str, ic: bool) -> Result<Regex, String> {
    RegexBuilder::new(rs).case_insensitive(ic).multi_line(true).crlf(true).build().map_err(|e| {
        let s = e.to_string();
        format!("invalid pattern: {}", s.lines().last().unwrap_or("").trim())
    })
}

// ---- replacement strings --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rep {
    Lit(String),
    Group(usize),
    /// `\u` / `\l`: change the case of the next char
    One(bool),
    /// `\U` / `\L`: change the case until `\E` / `\e`
    Span(bool),
    EndSpan,
}

pub(crate) fn parse_replacement(s: &str) -> Vec<Rep> {
    let mut out: Vec<Rep> = Vec::new();
    let mut lit = String::new();
    let mut it = s.chars().peekable();
    let flush = |lit: &mut String, out: &mut Vec<Rep>| {
        if !lit.is_empty() {
            out.push(Rep::Lit(std::mem::take(lit)));
        }
    };
    while let Some(c) = it.next() {
        match c {
            '&' => {
                flush(&mut lit, &mut out);
                out.push(Rep::Group(0));
            }
            '\\' => match it.next() {
                Some(d @ '0'..='9') => {
                    flush(&mut lit, &mut out);
                    out.push(Rep::Group(d as usize - '0' as usize));
                }
                Some('n') => lit.push('\0'),
                Some('r') => lit.push('\n'),
                Some('t') => lit.push('\t'),
                Some('u') => {
                    flush(&mut lit, &mut out);
                    out.push(Rep::One(true));
                }
                Some('l') => {
                    flush(&mut lit, &mut out);
                    out.push(Rep::One(false));
                }
                Some('U') => {
                    flush(&mut lit, &mut out);
                    out.push(Rep::Span(true));
                }
                Some('L') => {
                    flush(&mut lit, &mut out);
                    out.push(Rep::Span(false));
                }
                Some('e') | Some('E') => {
                    flush(&mut lit, &mut out);
                    out.push(Rep::EndSpan);
                }
                Some(d) => lit.push(d),
                None => lit.push('\\'),
            },
            '\r' => lit.push('\n'),
            c => lit.push(c),
        }
    }
    flush(&mut lit, &mut out);
    out
}

pub(crate) fn expand(parts: &[Rep], caps: &regex::Captures) -> String {
    let mut out = String::new();
    let mut span: Option<bool> = None;
    let mut one: Option<bool> = None;
    let mut push = |s: &str, out: &mut String, span: Option<bool>, one: &mut Option<bool>| {
        for c in s.chars() {
            let up = one.take().or(span);
            match up {
                Some(true) => out.extend(c.to_uppercase()),
                Some(false) => out.extend(c.to_lowercase()),
                None => out.push(c),
            }
        }
    };
    for p in parts {
        match p {
            Rep::Lit(s) => push(s, &mut out, span, &mut one),
            Rep::Group(i) => {
                let g = caps.get(*i).map_or("", |m| m.as_str());
                push(g, &mut out, span, &mut one);
            }
            Rep::One(u) => one = Some(*u),
            Rep::Span(u) => span = Some(*u),
            Rep::EndSpan => {
                span = None;
                one = None;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(p: &str) -> String {
        translate(p).unwrap().0
    }

    #[test]
    fn magic_mode_needs_backslashes_for_operators() {
        assert_eq!(t("a(b)"), r"a\(b\)");
        assert_eq!(t(r"\(a\|b\)\+"), "(a|b)+");
        assert_eq!(t(r"a\{2,3}"), "a{2,3}");
        assert_eq!(t(r"a\{-1,}"), "a{1,}?");
        assert_eq!(t(r"\<foo\>"), r"\b{start}foo\b{end}");
        assert_eq!(t("a.b*"), "a.b*");
        assert_eq!(t("a+"), r"a\+");
    }

    #[test]
    fn very_magic_and_classes() {
        assert_eq!(t(r"\v(a|b)+"), "(a|b)+");
        assert_eq!(t(r"\v<x>"), r"\b{start}x\b{end}");
        assert_eq!(t(r"\d\+"), "[0-9]+");
        assert_eq!(t("[a-z_]"), r"[a-z\_]");
        assert_eq!(t(r"\V.*"), r"\.\*");
    }

    #[test]
    fn anchors_only_at_the_edges() {
        assert_eq!(t("^a$"), "^a$");
        assert_eq!(t("a^b"), r"a\^b");
        assert_eq!(t("a$b"), r"a\$b");
        assert_eq!(t("*a"), r"\*a");
    }

    #[test]
    fn unsupported_syntax_is_an_error() {
        assert!(translate(r"\(a\)\1").unwrap_err().contains("back-reference"));
        assert!(translate(r"foo\zsbar").unwrap_err().contains("\\z"));
        assert!(translate(r"a\@<=b").unwrap_err().contains("look-around"));
        assert!(translate("~").is_err());
    }

    #[test]
    fn case_options() {
        let re = compile("foo", true, true).unwrap();
        assert!(re.is_match("FOO"));
        let re = compile("Foo", true, true).unwrap();
        assert!(!re.is_match("foo"));
        let re = compile(r"Foo\c", true, true).unwrap();
        assert!(re.is_match("foo"));
        let re = compile("foo", false, false).unwrap();
        assert!(!re.is_match("FOO"));
    }

    #[test]
    fn replacement_expansion() {
        let re = Regex::new("(a)(b)").unwrap();
        let caps = re.captures("ab").unwrap();
        assert_eq!(expand(&parse_replacement(r"\2-\1[&]"), &caps), "b-a[ab]");
        assert_eq!(expand(&parse_replacement(r"\u\1\U\2\Ex"), &caps), "ABx");
        assert_eq!(expand(&parse_replacement(r"\&"), &caps), "&");
    }
}
