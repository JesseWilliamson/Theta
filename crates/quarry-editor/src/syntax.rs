//! A hand-rolled SQL lexer used for highlighting.
//!
//! Why not tree-sitter: highlighting must degrade gracefully on half-typed statements and stay
//! cheap on 20k-line scripts. A line lexer with a tiny carry-over state (block comment depth,
//! open quote, dollar-quote tag) does both: every token is classified locally, so a typo never
//! poisons the rest of the file beyond what SQL's own quoting rules imply.
//!
//! Line start states are cached and invalidated from the first edited line downwards, so an
//! edit costs one re-lex of the lines between the edit and the line being painted.

use crate::text::{self, Text};
use ropey::Rope;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum HighlightKind {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Operator,
    Punctuation,
    Identifier,
    /// NULL TRUE FALSE
    Constant,
    /// $1 :name ?
    Parameter,
    Error,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HighlightSpan {
    /// char offsets within the requested line
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum LexState {
    #[default]
    Normal,
    Block(u32),
    /// Inside a quoted run that crossed a line break: `'` string, `"`/`` ` `` identifier.
    Quote(char),
    Dollar(String),
}

const KEYWORDS: &[&str] = &[
    "add", "all", "alter", "analyze", "and", "any", "as", "asc", "begin", "between", "by", "cascade", "case",
    "cast", "check", "collate", "column", "commit", "constraint", "copy", "create", "cross", "current_date",
    "current_time", "current_timestamp", "current_user", "database", "default", "delete", "desc", "describe",
    "distinct", "do", "drop", "else", "end", "escape", "except", "exists", "explain", "extract", "fetch",
    "filter", "first", "following", "for", "foreign", "from", "full", "grant", "group", "grouping", "having",
    "if", "ilike", "in", "index", "inner", "insert", "intersect", "into", "is", "isnull", "join", "key",
    "lateral", "left", "like", "limit", "materialized", "natural", "not", "notnull", "nulls", "offset", "on",
    "only", "or", "order", "outer", "over", "partition", "preceding", "primary", "range", "recursive",
    "references", "replace", "restrict", "returning", "revoke", "right", "rollback", "row", "rows", "savepoint",
    "schema", "select", "set", "show", "similar", "some", "table", "tablesample", "temp", "temporary", "then",
    "to", "truncate", "union", "unique", "unbounded", "update", "use", "using", "vacuum", "values", "view",
    "when", "where", "window", "with", "within", "without", "zone", "unlock", "lock", "trigger", "function",
    "procedure", "returns", "return", "language", "declare", "execute", "call", "loop", "while", "elseif",
    "engine", "auto_increment", "unsigned", "zerofill", "comment", "extension", "sequence", "owner", "tablespace",
    "using", "xor", "div", "mod", "regexp", "rlike", "binary", "interval", "array", "at", "time", "type", "domain",
    "rename", "column", "enable", "disable", "inherit", "inherits", "include", "overlaps", "ordinality", "unnest",
    "force", "ignore", "straight_join", "duplicate", "conflict", "nothing", "do", "pragma", "attach", "detach",
    "explain", "storage", "generated", "always", "identity", "stored", "virtual", "deferrable", "deferred",
    "initially", "immediate", "match", "partial", "simple", "action", "no", "cycle", "minvalue", "maxvalue",
    "start", "increment", "cache", "owned", "none", "current", "of", "role", "user", "password", "login",
    "superuser", "createdb", "grant", "option", "privileges", "public", "usage", "connect", "monitor",
];

const TYPES: &[&str] = &[
    "int", "integer", "bigint", "smallint", "tinyint", "mediumint", "serial", "bigserial", "smallserial",
    "decimal", "numeric", "real", "double", "float", "boolean", "bool", "char", "varchar", "character",
    "nchar", "nvarchar", "text", "tinytext", "mediumtext", "longtext", "bytea", "blob", "tinyblob",
    "mediumblob", "longblob", "date", "timestamp", "timestamptz", "timetz", "datetime", "year", "uuid",
    "json", "jsonb", "xml", "money", "bit", "varbit", "varbinary", "enum", "inet", "cidr", "macaddr", "point",
    "polygon", "circle", "geometry", "geography", "tsvector", "tsquery", "oid", "int2", "int4", "int8",
    "float4", "float8", "citext", "hstore", "ltree", "regclass", "bpchar", "name", "box2d", "box3d",
];

/// Keywords that are really function names when followed by `(`.
const FUNC_KEYWORDS: &[&str] = &["left", "right", "replace", "if", "mod", "extract", "cast", "row", "values", "filter", "any", "some", "unnest", "interval", "binary", "array", "set", "char", "date", "time", "timestamp"];

fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w)
}

fn is_type(w: &str) -> bool {
    TYPES.contains(&w)
}

fn ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Lex one line (without its terminator). Returns spans and the state at the line's end.
pub fn lex_line(line: &str, mut state: LexState) -> (Vec<HighlightSpan>, LexState) {
    let cs: Vec<char> = line.chars().collect();
    let n = cs.len();
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut i = 0;
    let mut after_cast = false;
    let push = |spans: &mut Vec<HighlightSpan>, a: usize, b: usize, kind| {
        if b > a {
            spans.push(HighlightSpan { range: a..b, kind });
        }
    };
    while i < n {
        // Continuations from the previous line.
        match &state {
            LexState::Block(depth) => {
                let mut d = *depth;
                let start = i;
                while i < n {
                    if cs[i] == '*' && i + 1 < n && cs[i + 1] == '/' {
                        i += 2;
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    } else if cs[i] == '/' && i + 1 < n && cs[i + 1] == '*' {
                        i += 2;
                        d += 1;
                    } else {
                        i += 1;
                    }
                }
                push(&mut spans, start, i, HighlightKind::Comment);
                state = if d == 0 { LexState::Normal } else { LexState::Block(d) };
                continue;
            }
            LexState::Quote(q) => {
                let q = *q;
                let start = i;
                let mut closed = false;
                while i < n {
                    if cs[i] == q {
                        if i + 1 < n && cs[i + 1] == q {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                let kind = if q == '\'' { HighlightKind::String } else { HighlightKind::Identifier };
                push(&mut spans, start, i, kind);
                state = if closed { LexState::Normal } else { LexState::Quote(q) };
                continue;
            }
            LexState::Dollar(tag) => {
                let close: Vec<char> = format!("${tag}$").chars().collect();
                let start = i;
                let mut closed = false;
                while i < n {
                    if cs[i] == '$' && cs[i..].starts_with(&close) {
                        i += close.len();
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                push(&mut spans, start, i, HighlightKind::String);
                if closed {
                    state = LexState::Normal;
                }
                continue;
            }
            LexState::Normal => {}
        }

        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let was_cast = std::mem::take(&mut after_cast);
        let prev_dot = spans.last().is_some_and(|s| s.range.end <= i && cs[s.range.end - 1] == '.' && s.kind == HighlightKind::Punctuation);
        // comments
        if c == '-' && i + 1 < n && cs[i + 1] == '-' {
            push(&mut spans, i, n, HighlightKind::Comment);
            break;
        }
        if c == '/' && i + 1 < n && cs[i + 1] == '*' {
            let start = i;
            i += 2;
            // hand over to the Block arm, but keep the opening in the same span
            let mut d = 1u32;
            while i < n {
                if cs[i] == '*' && i + 1 < n && cs[i + 1] == '/' {
                    i += 2;
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                } else if cs[i] == '/' && i + 1 < n && cs[i + 1] == '*' {
                    i += 2;
                    d += 1;
                } else {
                    i += 1;
                }
            }
            push(&mut spans, start, i, HighlightKind::Comment);
            state = if d == 0 { LexState::Normal } else { LexState::Block(d) };
            continue;
        }
        // strings and quoted identifiers
        if c == '\'' || c == '"' || c == '`' {
            let start = i;
            i += 1;
            let mut closed = false;
            while i < n {
                if cs[i] == c {
                    if i + 1 < n && cs[i + 1] == c {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            let kind = if c == '\'' { HighlightKind::String } else { HighlightKind::Identifier };
            push(&mut spans, start, i, kind);
            if !closed {
                state = LexState::Quote(c);
            }
            continue;
        }
        if c == '$' {
            // $1 parameter or $tag$ ... $tag$ dollar quoting
            if i + 1 < n && cs[i + 1].is_ascii_digit() {
                let start = i;
                i += 1;
                while i < n && cs[i].is_ascii_digit() {
                    i += 1;
                }
                push(&mut spans, start, i, HighlightKind::Parameter);
                continue;
            }
            let mut j = i + 1;
            if j < n && ident_start(cs[j]) {
                while j < n && ident_part(cs[j]) && cs[j] != '$' {
                    j += 1;
                }
            }
            if j < n && cs[j] == '$' {
                let tag: String = cs[i + 1..j].iter().collect();
                let start = i;
                i = j + 1;
                let close: Vec<char> = format!("${tag}$").chars().collect();
                let mut closed = false;
                while i < n {
                    if cs[i] == '$' && cs[i..].starts_with(&close) {
                        i += close.len();
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                push(&mut spans, start, i, HighlightKind::String);
                if !closed {
                    state = LexState::Dollar(tag);
                }
                continue;
            }
            push(&mut spans, i, i + 1, HighlightKind::Error);
            i += 1;
            continue;
        }
        // numbers
        let prev_word = i > 0 && ident_part(cs[i - 1]);
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && cs[i + 1].is_ascii_digit() && !prev_word) {
            let start = i;
            if c == '0' && i + 1 < n && (cs[i + 1] == 'x' || cs[i + 1] == 'X') {
                i += 2;
                while i < n && cs[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < n && (cs[i].is_ascii_digit() || cs[i] == '_') {
                    i += 1;
                }
                if i < n && cs[i] == '.' && !(i + 1 < n && cs[i + 1] == '.') {
                    i += 1;
                    while i < n && cs[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < n && (cs[i] == 'e' || cs[i] == 'E') {
                    let mut j = i + 1;
                    if j < n && (cs[j] == '+' || cs[j] == '-') {
                        j += 1;
                    }
                    if j < n && cs[j].is_ascii_digit() {
                        while j < n && cs[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    }
                }
            }
            push(&mut spans, start, i, HighlightKind::Number);
            continue;
        }
        // parameters
        if c == '?' {
            push(&mut spans, i, i + 1, HighlightKind::Parameter);
            i += 1;
            continue;
        }
        if (c == ':' || c == '@') && i + 1 < n && (ident_start(cs[i + 1]) || cs[i + 1] == '@') && !prev_word && !(i > 0 && cs[i - 1] == ':') {
            let start = i;
            i += 1;
            while i < n && (ident_part(cs[i]) || cs[i] == '@') {
                i += 1;
            }
            push(&mut spans, start, i, HighlightKind::Parameter);
            continue;
        }
        // words
        if ident_start(c) {
            let start = i;
            while i < n && ident_part(cs[i]) {
                i += 1;
            }
            // string prefixes: E'..' B'..' X'..' N'..' U&'..'
            if i < n && cs[i] == '\'' && i - start == 1 && matches!(cs[start], 'e' | 'E' | 'b' | 'B' | 'x' | 'X' | 'n' | 'N') {
                // lex as a string starting at the prefix
                let sstart = start;
                i += 1;
                let mut closed = false;
                while i < n {
                    if cs[i] == '\'' {
                        if i + 1 < n && cs[i + 1] == '\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        closed = true;
                        break;
                    }
                    if cs[i] == '\\' && matches!(cs[start], 'e' | 'E') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                let i2 = i.min(n);
                push(&mut spans, sstart, i2, HighlightKind::String);
                i = i2;
                if !closed {
                    state = LexState::Quote('\'');
                }
                continue;
            }
            let word: String = cs[start..i].iter().collect::<String>().to_lowercase();
            let mut j = i;
            while j < n && cs[j].is_whitespace() {
                j += 1;
            }
            let call = j < n && cs[j] == '(';
            let kind = if prev_dot {
                if call { HighlightKind::Function } else { HighlightKind::Identifier }
            } else if matches!(word.as_str(), "null" | "true" | "false" | "unknown") {
                HighlightKind::Constant
            } else if was_cast && (is_type(&word) || !is_keyword(&word)) {
                HighlightKind::Type
            } else if call && FUNC_KEYWORDS.contains(&word.as_str()) && !matches!(word.as_str(), "values" | "set" | "row" | "any" | "some") {
                HighlightKind::Function
            } else if is_type(&word) {
                HighlightKind::Type
            } else if is_keyword(&word) {
                HighlightKind::Keyword
            } else if call {
                HighlightKind::Function
            } else {
                HighlightKind::Identifier
            };
            push(&mut spans, start, i, kind);
            continue;
        }
        if matches!(c, '(' | ')' | ',' | ';' | '.' | '[' | ']' | '{' | '}') {
            push(&mut spans, i, i + 1, HighlightKind::Punctuation);
            i += 1;
            continue;
        }
        if "+-*/%=<>!|&^~#@:".contains(c) {
            let start = i;
            while i < n && "+-*/%=<>!|&^~#@:".contains(cs[i]) {
                // stop before a comment opener so `a--b` style typos still show the comment
                if i > start && ((cs[i] == '-' && i + 1 < n && cs[i + 1] == '-') || (cs[i] == '/' && i + 1 < n && cs[i + 1] == '*')) {
                    break;
                }
                i += 1;
            }
            push(&mut spans, start, i, HighlightKind::Operator);
            if cs[start..i].ends_with(&[':', ':']) {
                after_cast = true;
            }
            continue;
        }
        push(&mut spans, i, i + 1, HighlightKind::Error);
        i += 1;
    }
    // A line ending inside a single-line construct needs no carry-over except the multi-line ones.
    (spans, state)
}

#[derive(Default)]
pub struct Syntax {
    /// `starts[i]` is the lexer state at the start of line `i`; always holds at least line 0.
    starts: Vec<LexState>,
    cache_revision: u64,
    cache: HashMap<usize, Vec<HighlightSpan>>,
}

impl Syntax {
    pub fn new() -> Self {
        Syntax { starts: vec![LexState::Normal], cache_revision: u64::MAX, cache: HashMap::new() }
    }

    /// The text changed starting at `line`: later start states are no longer trustworthy.
    pub fn note_edit(&mut self, line: usize) {
        self.starts.truncate(line + 1);
        if self.starts.is_empty() {
            self.starts.push(LexState::Normal);
        }
    }

    fn state_at(&mut self, t: Text, line: usize) -> LexState {
        while self.starts.len() <= line {
            let i = self.starts.len() - 1;
            let (_, end) = lex_line(&text::line_content(t, i), self.starts[i].clone());
            self.starts.push(end);
        }
        self.starts[line].clone()
    }

    pub fn highlights(&mut self, rope: &Rope, revision: u64, line: usize) -> Vec<HighlightSpan> {
        if self.cache_revision != revision {
            self.cache.clear();
            self.cache_revision = revision;
        }
        if let Some(s) = self.cache.get(&line) {
            return s.clone();
        }
        let t = rope.slice(..);
        if line >= t.len_lines() {
            return Vec::new();
        }
        let state = self.state_at(t, line);
        let (spans, _) = lex_line(&text::line_content(t, line), state);
        if self.cache.len() > 4096 {
            self.cache.clear();
        }
        self.cache.insert(line, spans.clone());
        spans
    }
}
