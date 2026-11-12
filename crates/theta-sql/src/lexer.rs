//! A hand-written, *tolerant* SQL tokenizer shared by the splitter, formatter,
//! completion engine and linter.
//!
//! Design constraints:
//! * It never fails and never panics: unterminated strings/comments become tokens
//!   flagged `terminated == false`, because the editor feeds us half-typed SQL on
//!   every keystroke.
//! * Tokens tile the input exactly (`concat(tokens) == sql`), so callers can map
//!   token ranges straight back to editor offsets.
//! * Scanning is byte-oriented. Every delimiter we stop at is ASCII, and bytes
//!   `>= 0x80` are always treated as identifier characters, so token boundaries
//!   always fall on UTF-8 character boundaries.

use std::ops::Range;
use theta_core::DatabaseKind;
use theta_core::Family;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Whitespace,
    /// `-- ...` (and `# ...` in MySQL), up to but excluding the newline.
    LineComment,
    BlockComment,
    /// Bare word: keyword or unquoted identifier.
    Word,
    /// `"x"`, `` `x` `` or SQLite `[x]`.
    QuotedIdent,
    /// `'x'` including prefixed forms (`E'..'`, `X'..'`, `N'..'`) and, in MySQL, `"x"`.
    String,
    /// Postgres `$tag$ ... $tag$`.
    DollarString,
    Number,
    /// `$1`, `?`, `:name`, `@var`.
    Param,
    Operator,
    /// One of `( ) , ; . [ ]`.
    Punct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    /// False for a string / quoted identifier / block comment / dollar string that
    /// runs to the end of input without its closing delimiter.
    pub terminated: bool,
}

impl Token {
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.start..self.end]
    }

    pub fn is_trivia(&self) -> bool {
        matches!(self.kind, TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment)
    }

    pub fn is_comment(&self) -> bool {
        matches!(self.kind, TokenKind::LineComment | TokenKind::BlockComment)
    }

    /// Case-insensitive keyword test; only bare words qualify.
    pub fn is_kw(&self, sql: &str, kw: &str) -> bool {
        self.kind == TokenKind::Word && self.text(sql).eq_ignore_ascii_case(kw)
    }

    pub fn is_punct(&self, sql: &str, p: &str) -> bool {
        self.kind == TokenKind::Punct && self.text(sql) == p
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_part(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// PostgreSQL's lexical rules (dollar quoting, nested block comments, `E''` strings, `?` and `@` as
/// operators). DuckDB deliberately copies them, and so do the Postgres-wire products.
fn pg_like(kind: DatabaseKind) -> bool {
    matches!(kind.family(), Family::Postgres | Family::DuckDb)
}

/// Strings that treat a backslash as an escape: MySQL, and ClickHouse (`'it\'s'`).
fn backslash_strings(kind: DatabaseKind) -> bool {
    matches!(kind.family(), Family::MySql | Family::ClickHouse)
}

fn is_op_char(b: u8, kind: DatabaseKind) -> bool {
    match b {
        b'+' | b'-' | b'*' | b'/' | b'<' | b'>' | b'=' | b'~' | b'!' | b'%' | b'^' | b'&' | b'|' => true,
        b'@' | b'?' => pg_like(kind),
        b'#' => !matches!(kind.family(), Family::MySql | Family::ClickHouse),
        b'`' => kind.family() == Family::Postgres,
        _ => false,
    }
}

/// Split `sql` into tokens that exactly tile the input.
pub fn tokenize(sql: &str, kind: DatabaseKind) -> Vec<Token> {
    let b = sql.as_bytes();
    let n = b.len();
    let mut out = Vec::with_capacity(n / 4 + 4);
    let mut i = 0;
    while i < n {
        let start = i;
        let c = b[i];
        let (tk, end, terminated) = match c {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => {
                while i < n && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
                    i += 1;
                }
                (TokenKind::Whitespace, i, true)
            }
            b'-' if i + 1 < n && b[i + 1] == b'-' && line_comment_ok(b, i, kind) => {
                (TokenKind::LineComment, line_end(b, i), true)
            }
            b'#' if matches!(kind.family(), Family::MySql | Family::ClickHouse) => {
                (TokenKind::LineComment, line_end(b, i), true)
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                let (e, t) = block_comment_end(b, i, pg_like(kind));
                (TokenKind::BlockComment, e, t)
            }
            b'\'' => {
                let (e, t) = quoted_end(b, i, b'\'', backslash_strings(kind));
                (TokenKind::String, e, t)
            }
            b'"' => {
                if kind.family() == Family::MySql {
                    let (e, t) = quoted_end(b, i, b'"', true);
                    (TokenKind::String, e, t)
                } else {
                    let (e, t) = quoted_end(b, i, b'"', false);
                    (TokenKind::QuotedIdent, e, t)
                }
            }
            // PostgreSQL treats the backtick as an operator character, so `` `abc `` is a prefix operator, not an
            // unterminated quoted identifier. (DuckDB, which copied PostgreSQL's parser, accepts it as a quote.)
            b'`' if kind.family() != Family::Postgres => {
                let (e, t) = quoted_end(b, i, b'`', false);
                (TokenKind::QuotedIdent, e, t)
            }
            b'[' if matches!(kind.family(), Family::Sqlite | Family::SqlServer) => {
                let mut j = i + 1;
                // SQLite's own tokenizer lets `[...]` span lines; T-SQL's `[a b]` is the same shape.
                while j < n && b[j] != b']' {
                    j += 1;
                }
                if j < n && b[j] == b']' {
                    (TokenKind::QuotedIdent, j + 1, true)
                } else {
                    (TokenKind::Punct, i + 1, true)
                }
            }
            b'$' if pg_like(kind) => lex_dollar(b, i),
            b'0'..=b'9' => (TokenKind::Number, number_end(b, i), true),
            b'.' if i + 1 < n && b[i + 1].is_ascii_digit() && !prev_is_operand_end(b, i) => {
                (TokenKind::Number, number_end(b, i), true)
            }
            b'(' | b')' | b',' | b';' | b'.' | b'[' | b']' => (TokenKind::Punct, i + 1, true),
            b':' => {
                if i + 1 < n && b[i + 1] == b':' {
                    (TokenKind::Operator, i + 2, true)
                } else if i + 1 < n && is_ident_start(b[i + 1]) && !prev_is_operand_end(b, i) {
                    let mut j = i + 1;
                    while j < n && is_ident_part(b[j]) {
                        j += 1;
                    }
                    (TokenKind::Param, j, true)
                } else {
                    (TokenKind::Operator, i + 1, true)
                }
            }
            b'?' if !pg_like(kind) => {
                let mut j = i + 1;
                while j < n && b[j].is_ascii_digit() {
                    j += 1;
                }
                (TokenKind::Param, j, true)
            }
            b'@' if !pg_like(kind) => {
                let mut j = i + 1;
                if j < n && b[j] == b'@' {
                    j += 1;
                }
                if j < n && b[j] == b'`' {
                    let (e, _) = quoted_end(b, j, b'`', false);
                    j = e;
                } else {
                    while j < n && (is_ident_part(b[j]) || b[j] == b'.') {
                        j += 1;
                    }
                }
                (TokenKind::Param, j, true)
            }
            _ if is_ident_start(c) || (c == b'$' && !pg_like(kind)) => {
                let mut j = i + 1;
                while j < n && is_ident_part(b[j]) {
                    j += 1;
                }
                if j < n && b[j] == b'\'' && string_prefix(&b[i..j], kind) {
                    let backslash = backslash_strings(kind) || matches!(b[i], b'e' | b'E');
                    let (e, t) = quoted_end(b, j, b'\'', backslash);
                    (TokenKind::String, e, t)
                } else if pg_like(kind)
                    && j + 1 < n
                    && (b[i] == b'U' || b[i] == b'u')
                    && j == i + 1
                    && b[j] == b'&'
                    && b[j + 1] == b'\''
                {
                    let (e, t) = quoted_end(b, j + 1, b'\'', false);
                    (TokenKind::String, e, t)
                } else {
                    (TokenKind::Word, j, true)
                }
            }
            _ if is_op_char(c, kind) => {
                let mut j = i;
                while j < n && is_op_char(b[j], kind) {
                    if b[j] == b'-' && j + 1 < n && b[j + 1] == b'-' && j > i {
                        break;
                    }
                    if b[j] == b'/' && j + 1 < n && b[j + 1] == b'*' && j > i {
                        break;
                    }
                    j += 1;
                }
                // `a=-1` must lex as `=` `-`: a multi-char operator may only end in
                // +/- when it contains one of ~!@#%^&|? (Postgres' own rule).
                let special = |s: &[u8]| s.iter().any(|x| b"~!@#%^&|?".contains(x));
                while j - i > 1 && matches!(b[j - 1], b'+' | b'-') && !special(&b[i..j]) {
                    j -= 1;
                }
                (TokenKind::Operator, j, true)
            }
            _ => {
                // Any other ASCII byte (`\`, `{`, `}`, stray `$`, ...). Advance a whole
                // char so we never split UTF-8 (non-ASCII is handled as ident above).
                (TokenKind::Operator, i + 1, true)
            }
        };
        i = end;
        out.push(Token { kind: tk, start, end, terminated });
    }
    out
}

fn line_comment_ok(b: &[u8], i: usize, kind: DatabaseKind) -> bool {
    // MySQL requires whitespace/control after `--`; otherwise `5--1` is arithmetic.
    kind.family() != Family::MySql || i + 2 >= b.len() || b[i + 2].is_ascii_whitespace() || b[i + 2] < 0x20
}

fn line_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    // Keep a preceding '\r' out of the comment so CRLF scripts round-trip.
    if i > 0 && i <= b.len() && i - 1 < b.len() && b[i - 1] == b'\r' {
        i -= 1;
    }
    i
}

fn block_comment_end(b: &[u8], start: usize, nested: bool) -> (usize, bool) {
    let n = b.len();
    let mut depth = 1;
    let mut i = start + 2;
    while i < n {
        if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return (i, true);
            }
        } else if nested && b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    (n, false)
}

/// End of a quoted run starting at `start` (which holds the opening quote).
/// A doubled quote is an escaped quote; `backslash` additionally enables `\x`.
fn quoted_end(b: &[u8], start: usize, q: u8, backslash: bool) -> (usize, bool) {
    let n = b.len();
    let mut i = start + 1;
    while i < n {
        let c = b[i];
        if backslash && c == b'\\' {
            i += 2;
            continue;
        }
        if c == q {
            if i + 1 < n && b[i + 1] == q {
                i += 2;
                continue;
            }
            return (i + 1, true);
        }
        i += 1;
    }
    (n, false)
}

fn string_prefix(p: &[u8], kind: DatabaseKind) -> bool {
    if p.len() == 1 {
        return match kind.family() {
            Family::Postgres | Family::DuckDb => {
                matches!(p[0], b'e' | b'E' | b'b' | b'B' | b'x' | b'X' | b'n' | b'N')
            }
            Family::MySql => matches!(p[0], b'b' | b'B' | b'x' | b'X' | b'n' | b'N'),
            Family::Sqlite => matches!(p[0], b'x' | b'X'),
            // N'..' is T-SQL's national string; ClickHouse has no string prefixes.
            Family::SqlServer => matches!(p[0], b'n' | b'N'),
            Family::ClickHouse => false,
        };
    }
    // MySQL charset introducers: _utf8'..', _binary'..'
    kind.family() == Family::MySql && p[0] == b'_'
}

fn prev_is_operand_end(b: &[u8], i: usize) -> bool {
    i > 0 && (is_ident_part(b[i - 1]) || matches!(b[i - 1], b')' | b']' | b'"' | b'`'))
}

fn number_end(b: &[u8], start: usize) -> usize {
    let n = b.len();
    let mut i = start;
    if b[i] == b'0'
        && i + 1 < n
        && matches!(b[i + 1], b'x' | b'X')
        && i + 2 < n
        && b[i + 2].is_ascii_hexdigit()
    {
        i += 2;
        while i < n && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        return i;
    }
    while i < n && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i < n
        && b[i] == b'.'
        && (i + 1 >= n || b[i + 1].is_ascii_digit() || !(is_ident_start(b[i + 1]) || b[i + 1] == b'.'))
    {
        i += 1;
        while i < n && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < n && matches!(b[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < n && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        if j < n && b[j].is_ascii_digit() {
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    i
}

/// `$1` parameter, `$tag$...$tag$` string, or a lone `$` operator.
fn lex_dollar(b: &[u8], i: usize) -> (TokenKind, usize, bool) {
    let n = b.len();
    let mut j = i + 1;
    if j < n && b[j].is_ascii_digit() {
        while j < n && b[j].is_ascii_digit() {
            j += 1;
        }
        return (TokenKind::Param, j, true);
    }
    if j < n && is_ident_start(b[j]) {
        while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) {
            j += 1;
        }
    }
    if j < n && b[j] == b'$' {
        let tag = &b[i..=j];
        let mut k = j + 1;
        while k + tag.len() <= n {
            if &b[k..k + tag.len()] == tag {
                return (TokenKind::DollarString, k + tag.len(), true);
            }
            k += 1;
        }
        return (TokenKind::DollarString, n, false);
    }
    (TokenKind::Operator, i + 1, true)
}

/// Unquote a quoted identifier token's text (`"a""b"` -> `a"b`); bare words pass through.
pub fn unquote_ident(text: &str) -> String {
    let b = text.as_bytes();
    match b.first() {
        Some(b'"') | Some(b'`') => {
            let q = b[0] as char;
            let inner = &text[1..];
            let inner = inner.strip_suffix(q).unwrap_or(inner);
            inner.replace(&format!("{q}{q}"), &q.to_string())
        }
        Some(b'[') => {
            let inner = &text[1..];
            inner.strip_suffix(']').unwrap_or(inner).to_string()
        }
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    fn kinds(sql: &str, k: DatabaseKind) -> Vec<(TokenKind, String)> {
        tokenize(sql, k)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .map(|t| (t.kind, t.text(sql).to_string()))
            .collect()
    }

    #[test]
    fn tokens_tile_the_input() {
        let sql = "SELECT a::int, 'x''y' /* c */ -- t\n FROM \"T\" WHERE x->>'k' = $1;";
        let toks = tokenize(sql, Postgres);
        let joined: String = toks.iter().map(|t| t.text(sql)).collect();
        assert_eq!(joined, sql);
    }

    #[test]
    fn doubled_quote_escapes() {
        assert_eq!(kinds("'it''s'", Postgres), vec![(TokenKind::String, "'it''s'".into())]);
        assert_eq!(kinds("\"a\"\"b\"", Postgres), vec![(TokenKind::QuotedIdent, "\"a\"\"b\"".into())]);
        assert_eq!(kinds("`a``b`", MySql), vec![(TokenKind::QuotedIdent, "`a``b`".into())]);
    }

    #[test]
    fn mysql_backslash_escape_and_double_quote_string() {
        assert_eq!(kinds(r"'a\'b'", MySql).len(), 1);
        assert_eq!(kinds(r#""a\"b""#, MySql), vec![(TokenKind::String, r#""a\"b""#.into())]);
        // In Postgres a backslash is an ordinary character.
        // (`b'` then opens a bit-string, hence two tokens.)
        assert_eq!(kinds(r"'a\'b'", Postgres).len(), 2);
    }

    #[test]
    fn postgres_escape_string() {
        assert_eq!(kinds(r"E'a\'b'", Postgres), vec![(TokenKind::String, r"E'a\'b'".into())]);
    }

    #[test]
    fn dollar_quoting() {
        let sql = "$fn$ select ';' $fn$ x";
        let t = kinds(sql, Postgres);
        assert_eq!(t[0], (TokenKind::DollarString, "$fn$ select ';' $fn$".into()));
        assert_eq!(kinds("$$a$$", Postgres)[0].0, TokenKind::DollarString);
        assert_eq!(kinds("$1", Postgres)[0].0, TokenKind::Param);
        let un = tokenize("$$ abc", Postgres);
        assert!(!un[0].terminated);
    }

    #[test]
    fn comments() {
        assert_eq!(kinds("a -- hi\nb", Postgres)[1].0, TokenKind::LineComment);
        assert_eq!(kinds("a # hi\nb", MySql)[1].0, TokenKind::LineComment);
        assert_ne!(kinds("a # b", Postgres)[1].0, TokenKind::LineComment);
        // MySQL needs whitespace after `--`.
        assert_eq!(kinds("5--1", MySql).len(), 4);
        let t = kinds("/* a /* b */ c */ x", Postgres);
        assert_eq!(t[0], (TokenKind::BlockComment, "/* a /* b */ c */".into()));
        let t = kinds("/* a /* b */ c */ x", MySql);
        assert_eq!(t[0], (TokenKind::BlockComment, "/* a /* b */".into()));
    }

    #[test]
    fn unterminated_tokens_are_flagged() {
        assert!(!tokenize("'abc", Postgres)[0].terminated);
        assert!(!tokenize("/* abc", Postgres)[0].terminated);
        assert!(!tokenize("\"abc", Postgres)[0].terminated);
    }

    #[test]
    fn operators_and_casts() {
        let t = kinds("a::int ->> 'k' <> 1 || b", Postgres);
        let texts: Vec<_> = t.iter().map(|x| x.1.as_str()).collect();
        assert_eq!(texts, ["a", "::", "int", "->>", "'k'", "<>", "1", "||", "b"]);
        let t = kinds("a=-1", Postgres);
        assert_eq!(t.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["a", "=", "-", "1"]);
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("1.5e-3", Postgres), vec![(TokenKind::Number, "1.5e-3".into())]);
        assert_eq!(kinds(".5", Postgres)[0].0, TokenKind::Number);
        let t = kinds("t.col", Postgres);
        assert_eq!(t.len(), 3);
        assert_eq!(kinds("0xFF", MySql)[0].0, TokenKind::Number);
    }

    #[test]
    fn params() {
        assert_eq!(kinds(":name", Postgres)[0].0, TokenKind::Param);
        assert_eq!(kinds("?", MySql)[0].0, TokenKind::Param);
        assert_eq!(kinds("@v := 1", MySql)[0].0, TokenKind::Param);
        assert_eq!(kinds("a ? b", Postgres)[1].0, TokenKind::Operator);
    }

    #[test]
    fn sqlite_bracket_identifier() {
        assert_eq!(kinds("[my col]", Sqlite), vec![(TokenKind::QuotedIdent, "[my col]".into())]);
        assert_eq!(kinds("a[1]", Postgres).len(), 4);
        assert_eq!(kinds("[a\nb]", Sqlite), vec![(TokenKind::QuotedIdent, "[a\nb]".into())]);
        assert_eq!(kinds("[abc", Sqlite)[0].0, TokenKind::Punct);
    }

    #[test]
    fn non_ascii_never_splits_chars() {
        let sql = "select 'é日本' as 日本語, 😀";
        for k in DatabaseKind::ALL {
            let toks = tokenize(sql, *k);
            for t in toks {
                assert!(sql.is_char_boundary(t.start) && sql.is_char_boundary(t.end));
            }
        }
    }

    #[test]
    fn unquote() {
        assert_eq!(unquote_ident("\"a\"\"b\""), "a\"b");
        assert_eq!(unquote_ident("`x`"), "x");
        assert_eq!(unquote_ident("[x y]"), "x y");
        assert_eq!(unquote_ident("abc"), "abc");
        assert_eq!(unquote_ident("\"unterminated"), "unterminated");
    }
}
