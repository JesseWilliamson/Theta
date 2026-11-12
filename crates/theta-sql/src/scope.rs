//! Light-weight, error-tolerant structural analysis of one statement: which
//! tables/CTEs/derived tables are visible, which clause a token sits in, and what
//! the enclosing subquery scopes are. Completion, linting and `tables_in_scope`
//! all build on this so they agree about what "in scope" means.
//!
//! It is deliberately not a parser. It scans a flat token list, uses parenthesis
//! matching to skip nested expressions, and recognises the handful of keywords that
//! introduce table references. Half-typed statements simply yield fewer facts.

use crate::lexer::{Token, TokenKind, unquote_ident};
use std::ops::Range;
use theta_core::{Catalog, DatabaseKind};

#[derive(Debug, Clone)]
pub(crate) struct Tok {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    /// Upper-cased text for words, exact text for punctuation/operators, empty otherwise.
    pub up: String,
}

pub(crate) struct Parsed<'a> {
    pub sql: &'a str,
    pub toks: Vec<Tok>,
    /// For `(` tokens: index of the matching `)`, or `toks.len()` if unclosed.
    pub close: Vec<usize>,
}

impl<'a> Parsed<'a> {
    pub fn new<'t>(sql: &'a str, raw: impl Iterator<Item = &'t Token>) -> Self {
        let toks: Vec<Tok> = raw
            .filter(|t| !t.is_trivia())
            .map(|t| {
                let up = match t.kind {
                    TokenKind::Word => t.text(sql).to_ascii_uppercase(),
                    TokenKind::Punct | TokenKind::Operator => t.text(sql).to_string(),
                    _ => String::new(),
                };
                Tok { kind: t.kind, start: t.start, end: t.end, up }
            })
            .collect();
        let mut close = vec![usize::MAX; toks.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (i, t) in toks.iter().enumerate() {
            if t.kind == TokenKind::Punct {
                if t.up == "(" {
                    stack.push(i);
                } else if t.up == ")"
                    && let Some(o) = stack.pop()
                {
                    close[o] = i;
                }
            }
        }
        for o in stack {
            close[o] = toks.len();
        }
        Parsed { sql, toks, close }
    }

    pub fn from_sql(sql: &'a str, kind: DatabaseKind) -> Self {
        let raw = crate::lexer::tokenize(sql, kind);
        Parsed::new(sql, raw.iter())
    }

    pub fn len(&self) -> usize {
        self.toks.len()
    }

    pub fn is_punct(&self, i: usize, p: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.kind == TokenKind::Punct && t.up == p)
    }

    pub fn is_op(&self, i: usize, p: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.kind == TokenKind::Operator && t.up == p)
    }

    pub fn is_word(&self, i: usize, w: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.kind == TokenKind::Word && t.up == w)
    }

    pub fn up(&self, i: usize) -> &str {
        match self.toks.get(i) {
            Some(t) if t.kind == TokenKind::Word => &t.up,
            _ => "",
        }
    }

    pub fn text(&self, i: usize) -> &str {
        &self.sql[self.toks[i].start..self.toks[i].end]
    }

    pub fn is_ident(&self, i: usize) -> bool {
        self.toks.get(i).is_some_and(|t| matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent))
    }

    pub fn ident(&self, i: usize) -> String {
        unquote_ident(self.text(i))
    }

    pub fn range(&self, i: usize) -> Range<usize> {
        self.toks[i].start..self.toks[i].end
    }

    pub fn first_word(&self) -> &str {
        self.up(0)
    }

    /// `(` followed by SELECT/WITH/VALUES (or another paren that starts a subquery).
    pub fn is_subquery_paren(&self, open: usize) -> bool {
        if !self.is_punct(open, "(") {
            return false;
        }
        let mut j = open + 1;
        while self.is_punct(j, "(") {
            j += 1;
        }
        matches!(self.up(j), "SELECT" | "WITH" | "VALUES")
    }

    pub fn closing(&self, open: usize) -> usize {
        self.close[open].min(self.toks.len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Clause {
    None,
    With,
    Select,
    From,
    Join,
    On,
    JoinUsing,
    Where,
    GroupBy,
    Having,
    Window,
    OrderBy,
    Limit,
    Set,
    Values,
    Returning,
    Into,
    Update,
    Delete,
    OnConflict,
    AlterTable,
    IndexOn,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StmtKind {
    Select,
    Insert,
    Update,
    Delete,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SourceKind {
    Table,
    Cte,
    Derived(Vec<String>),
    Function(Vec<String>),
}

#[derive(Debug, Clone)]
pub(crate) struct Source {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
    pub kind: SourceKind,
    /// Token index range of the whole factor (name through alias).
    pub tok_start: usize,
    pub tok_end: usize,
    /// Token index of the table-name token (last part of a qualified name).
    #[allow(dead_code)]
    pub name_tok: usize,
    pub clause: Clause,
    /// DML target (`UPDATE t`, `INSERT INTO t`, `DELETE FROM t`, `ALTER TABLE t`).
    pub target: bool,
}

impl Source {
    pub fn reference_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CteDef {
    pub name: String,
    pub columns: Vec<String>,
}

pub(crate) struct ScopeScan {
    pub sources: Vec<Source>,
    /// (token index of the keyword, clause that starts there), in order.
    pub clause_starts: Vec<(usize, Clause)>,
    pub stmt: StmtKind,
}

impl ScopeScan {
    /// Clause in effect for a cursor sitting before token `idx`.
    pub fn clause_at(&self, idx: usize) -> Clause {
        self.clause_starts.iter().rev().find(|(pos, _)| *pos < idx).map_or(Clause::None, |(_, c)| *c)
    }
}

pub(crate) struct ColumnRef {
    pub name: String,
    pub detail: String,
}

const ALIAS_STOP: &[&str] = &[
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "FETCH",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "CROSS",
    "NATURAL",
    "OUTER",
    "ON",
    "USING",
    "SET",
    "VALUES",
    "SELECT",
    "WITH",
    "RETURNING",
    "WINDOW",
    "FOR",
    "LATERAL",
    "STRAIGHT_JOIN",
    "TABLESAMPLE",
    "AS",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "INTO",
    "FROM",
    "FORCE",
    "IGNORE",
    "USE",
    "INDEXED",
    "PARTITION",
    "CONFLICT",
    "DO",
    "ASC",
    "DESC",
    "BY",
    "DEFAULT",
    "ADD",
    "DROP",
    "ALTER",
    "RENAME",
    "OWNER",
    "LIKE",
    "BETWEEN",
    "CASE",
    "EXISTS",
];

fn is_alias_word(p: &Parsed, i: usize) -> bool {
    match p.toks.get(i) {
        Some(t) if t.kind == TokenKind::QuotedIdent => true,
        Some(t) if t.kind == TokenKind::Word => !ALIAS_STOP.contains(&t.up.as_str()),
        _ => false,
    }
}

/// Parse one table factor at `i` (within `..to`). Returns the source (if any) and
/// the index after the factor.
fn parse_factor(
    p: &Parsed,
    mut i: usize,
    to: usize,
    clause: Clause,
    catalog: Option<&Catalog>,
    ctes: &[CteDef],
) -> (Option<Source>, usize) {
    let start = i;
    while matches!(p.up(i), "LATERAL" | "ONLY") && i + 1 < to {
        i += 1;
    }
    if i >= to {
        return (None, i);
    }
    let mut src;
    if p.is_punct(i, "(") {
        let c = p.closing(i).min(to);
        if !p.is_subquery_paren(i) {
            // Parenthesised join tree or table list: not modelled.
            return (None, (c + 1).min(to.max(i + 1)));
        }
        let cols = derive_columns(p, i + 1, c, catalog, ctes);
        src = Source {
            schema: None,
            name: String::new(),
            alias: None,
            kind: SourceKind::Derived(cols),
            tok_start: start,
            tok_end: c + 1,
            name_tok: i,
            clause,
            target: false,
        };
        i = (c + 1).min(to);
    } else if p.is_ident(i)
        && !(p.toks[i].kind == TokenKind::Word && ALIAS_STOP.contains(&p.toks[i].up.as_str()))
    {
        let mut parts = vec![p.ident(i)];
        let mut name_tok = i;
        i += 1;
        while i + 1 < to && p.is_punct(i, ".") && p.is_ident(i + 1) {
            parts.push(p.ident(i + 1));
            name_tok = i + 1;
            i += 2;
        }
        if p.is_punct(i, ".") && i + 1 < to && p.is_op(i + 1, "*") {
            // `schema.*` in a column list is not a table factor.
            return (None, i + 2);
        }
        let name = parts.pop().unwrap_or_default();
        let schema = if parts.is_empty() { None } else { parts.pop() };
        // After INTO/UPDATE/DELETE/ALTER TABLE/ON, `name (` opens a column list, not a call.
        let call_ok = !matches!(
            clause,
            Clause::Into | Clause::IndexOn | Clause::AlterTable | Clause::Update | Clause::Delete
        );
        if p.is_punct(i, "(") && i < to && call_ok {
            let c = p.closing(i).min(to);
            src = Source {
                schema,
                name,
                alias: None,
                kind: SourceKind::Function(Vec::new()),
                tok_start: start,
                tok_end: c + 1,
                name_tok,
                clause,
                target: false,
            };
            i = (c + 1).min(to);
            if p.is_word(i, "WITH") && p.is_word(i + 1, "ORDINALITY") {
                i += 2;
            }
        } else {
            let is_cte = schema.is_none() && ctes.iter().rev().any(|c| c.name.eq_ignore_ascii_case(&name));
            src = Source {
                schema,
                name,
                alias: None,
                kind: if is_cte { SourceKind::Cte } else { SourceKind::Table },
                tok_start: start,
                tok_end: i,
                name_tok,
                clause,
                target: false,
            };
        }
    } else {
        return (None, i);
    }
    // Alias: `[AS] alias [(col, ...)]`
    let mut j = i;
    let had_as = p.is_word(j, "AS");
    if had_as {
        j += 1;
    }
    if j < to
        && is_alias_word(p, j)
        && (had_as || p.toks[j].kind != TokenKind::Word || !p.is_punct(j + 1, "."))
    {
        src.alias = Some(p.ident(j));
        j += 1;
        if p.is_punct(j, "(") && matches!(src.kind, SourceKind::Derived(_) | SourceKind::Function(_)) {
            let c = p.closing(j).min(to);
            let names: Vec<String> = (j + 1..c).filter(|&k| p.is_ident(k)).map(|k| p.ident(k)).collect();
            if !names.is_empty() {
                match &mut src.kind {
                    SourceKind::Derived(cols) | SourceKind::Function(cols) => *cols = names,
                    _ => {}
                }
            }
            j = (c + 1).min(to);
        }
        i = j;
    }
    src.tok_end = i;
    (Some(src), i)
}

/// Parse a `WITH` list starting at the `WITH` token. Returns the CTEs and the
/// index just after the list.
pub(crate) fn parse_with(p: &Parsed, i: usize, to: usize, catalog: Option<&Catalog>) -> (Vec<CteDef>, usize) {
    let mut ctes: Vec<CteDef> = Vec::new();
    let mut j = i + 1;
    if p.is_word(j, "RECURSIVE") {
        j += 1;
    }
    while j < to && p.is_ident(j) {
        let name = p.ident(j);
        j += 1;
        let mut explicit: Option<Vec<String>> = None;
        if p.is_punct(j, "(") {
            let c = p.closing(j);
            explicit = Some((j + 1..c).filter(|&k| p.is_ident(k)).map(|k| p.ident(k)).collect());
            j = (c + 1).min(to);
        }
        if !p.is_word(j, "AS") {
            ctes.push(CteDef { name, columns: explicit.unwrap_or_default() });
            break;
        }
        j += 1;
        while p.is_word(j, "NOT") || p.is_word(j, "MATERIALIZED") {
            j += 1;
        }
        if !p.is_punct(j, "(") {
            ctes.push(CteDef { name, columns: explicit.unwrap_or_default() });
            break;
        }
        let c = p.closing(j).min(to);
        let columns = explicit.unwrap_or_else(|| derive_columns(p, j + 1, c, catalog, &ctes));
        ctes.push(CteDef { name, columns });
        j = (c + 1).min(to);
        if p.is_punct(j, ",") {
            j += 1;
        } else {
            break;
        }
    }
    (ctes, j)
}

fn find_depth0(p: &Parsed, from: usize, to: usize, pred: impl Fn(&Parsed, usize) -> bool) -> Option<usize> {
    let mut i = from;
    while i < to {
        if p.is_punct(i, "(") {
            i = p.closing(i) + 1;
            continue;
        }
        if pred(p, i) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Output column names of the query in `from..to`, when they are statically obvious.
pub(crate) fn derive_columns(
    p: &Parsed,
    from: usize,
    to: usize,
    catalog: Option<&Catalog>,
    ctes: &[CteDef],
) -> Vec<String> {
    derive_columns_ex(p, from, to, catalog, ctes).into_iter().map(|(n, _)| n).collect()
}

/// Like [`derive_columns`], flagging which names are explicit aliases.
fn derive_columns_ex(
    p: &Parsed,
    from: usize,
    to: usize,
    catalog: Option<&Catalog>,
    ctes: &[CteDef],
) -> Vec<(String, bool)> {
    let mut from = from;
    let mut local_ctes: Vec<CteDef> = ctes.to_vec();
    if p.is_word(from, "WITH") {
        let (found, next) = parse_with(p, from, to, catalog);
        local_ctes.extend(found);
        from = next;
    }
    while p.is_punct(from, "(") && from < to {
        from += 1;
    }
    if p.is_word(from, "VALUES") {
        let first_row_end = if p.is_punct(from + 1, "(") { p.closing(from + 1).min(to) } else { from + 1 };
        let mut n = 1;
        let mut i = from + 2;
        while i < first_row_end {
            if p.is_punct(i, "(") {
                i = p.closing(i) + 1;
                continue;
            }
            if p.is_punct(i, ",") {
                n += 1;
            }
            i += 1;
        }
        return (1..=n).map(|k| (format!("column{k}"), false)).collect();
    }
    let Some(sel) = find_depth0(p, from, to, |p, i| p.is_word(i, "SELECT")) else { return Vec::new() };
    let mut i = sel + 1;
    if p.is_word(i, "DISTINCT") {
        i += 1;
        if p.is_word(i, "ON") && p.is_punct(i + 1, "(") {
            i = p.closing(i + 1) + 1;
        }
    } else if p.is_word(i, "ALL") {
        i += 1;
    }
    let list_end = find_depth0(p, i, to, |p, k| {
        (p.is_word(k, "FROM") && !p.is_word(k.wrapping_sub(1), "DISTINCT"))
            || matches!(
                p.up(k),
                "UNION" | "INTERSECT" | "EXCEPT" | "WHERE" | "GROUP" | "ORDER" | "LIMIT" | "HAVING" | "INTO"
            )
    })
    .unwrap_or(to);
    // Split items on depth-0 commas.
    let mut items: Vec<Range<usize>> = Vec::new();
    let mut s = i;
    let mut k = i;
    while k < list_end {
        if p.is_punct(k, "(") {
            k = p.closing(k) + 1;
            continue;
        }
        if p.is_punct(k, ",") {
            items.push(s..k);
            s = k + 1;
        }
        k += 1;
    }
    if s < list_end {
        items.push(s..list_end);
    }
    let mut scan: Option<ScopeScan> = None;
    let mut out: Vec<(String, bool)> = Vec::new();
    for it in items {
        let (a, b) = (it.start, it.end.min(p.len()));
        if b <= a {
            continue;
        }
        let star_qual: Option<Option<String>> = if b - a == 1 && p.is_op(a, "*") {
            Some(None)
        } else if b - a == 3 && p.is_ident(a) && p.is_punct(a + 1, ".") && p.is_op(a + 2, "*") {
            Some(Some(p.ident(a)))
        } else {
            None
        };
        if let Some(qual) = star_qual {
            let sc = scan.get_or_insert_with(|| scan_scope(p, from, to, catalog, &local_ctes));
            for src in &sc.sources {
                if qual.as_ref().is_none_or(|q| src.reference_name().eq_ignore_ascii_case(q)) {
                    out.extend(
                        source_columns(src, catalog, &local_ctes).into_iter().map(|c| (c.name, false)),
                    );
                }
            }
            continue;
        }
        let last = b - 1;
        if b - a >= 2 && p.is_word(last - 1, "AS") && p.is_ident(last) {
            out.push((p.ident(last), true));
        } else if (b - a == 1 && p.is_ident(last))
            || (b - a >= 3 && p.is_ident(last) && p.is_punct(last - 1, "."))
        {
            // A plain column reference: the output column takes its name.
            out.push((p.ident(last), false));
        } else if b - a >= 2
            && is_alias_word(p, last)
            && (p.is_ident(last - 1)
                || matches!(p.toks[last - 1].kind, TokenKind::Number | TokenKind::String)
                || p.is_punct(last - 1, ")")
                || p.is_punct(last - 1, "]"))
            && !p.is_punct(last - 1, ".")
        {
            out.push((p.ident(last), true));
        } else if p.is_ident(a) && p.is_punct(a + 1, "(") && p.closing(a + 1) + 1 == b {
            out.push((p.ident(a).to_ascii_lowercase(), false));
        } else if p.is_word(a, "CASE") {
            out.push(("case".into(), false));
        }
    }
    out
}

fn first_clause_keyword<'p>(p: &'p Parsed<'_>) -> &'p str {
    p.first_word()
}

/// Scan the depth-0 tokens of `from..to` and collect table sources and clause starts.
pub(crate) fn scan_scope(
    p: &Parsed,
    from: usize,
    to: usize,
    catalog: Option<&Catalog>,
    ctes: &[CteDef],
) -> ScopeScan {
    let to = to.min(p.len());
    let mut sources: Vec<Source> = Vec::new();
    let mut starts: Vec<(usize, Clause)> = Vec::new();
    let mut stmt = StmtKind::Other;
    let mut stmt_decided = false;
    let mut clause = Clause::None;
    let mut local_ctes: Vec<CteDef> = ctes.to_vec();
    let create_index = first_clause_keyword(p) == "CREATE";
    let alter = first_clause_keyword(p) == "ALTER";
    let mut i = from;

    macro_rules! set_clause {
        ($idx:expr, $c:expr) => {{
            clause = $c;
            starts.push(($idx, $c));
        }};
    }
    macro_rules! take_factors {
        ($start:expr, $multi:expr, $clause:expr, $target_first:expr) => {{
            let mut k = $start;
            let mut first = true;
            loop {
                let (src, next) = parse_factor(p, k, to, $clause, catalog, &local_ctes);
                let advanced = next > k;
                if let Some(mut s) = src {
                    s.target = $target_first && first;
                    sources.push(s);
                }
                first = false;
                k = next.max(k);
                if $multi && p.is_punct(k, ",") && advanced {
                    k += 1;
                    continue;
                }
                break;
            }
            k
        }};
    }

    while i < to {
        if p.is_punct(i, "(") {
            i = p.closing(i) + 1;
            continue;
        }
        if p.is_punct(i, ",") && clause == Clause::On {
            // `JOIN b ON a.x = b.x, c`: the comma starts a new FROM-list item.
            i = take_factors!(i + 1, true, Clause::From, false);
            continue;
        }
        if p.toks[i].kind != TokenKind::Word {
            i += 1;
            continue;
        }
        let up = p.toks[i].up.as_str();
        match up {
            "WITH" if !stmt_decided && sources.is_empty() && clause == Clause::None => {
                let (found, next) = parse_with(p, i, to, catalog);
                local_ctes.extend(found);
                set_clause!(i, Clause::With);
                i = next;
                continue;
            }
            "SELECT" => {
                if !stmt_decided {
                    stmt = StmtKind::Select;
                    stmt_decided = true;
                }
                set_clause!(i, Clause::Select);
            }
            "INSERT" | "REPLACE" if !stmt_decided => {
                stmt = StmtKind::Insert;
                stmt_decided = true;
                set_clause!(i, Clause::Other);
            }
            "INTO" => {
                set_clause!(i, Clause::Into);
                i = take_factors!(i + 1, false, Clause::Into, stmt == StmtKind::Insert);
                continue;
            }
            "UPDATE" if !stmt_decided => {
                stmt = StmtKind::Update;
                stmt_decided = true;
                set_clause!(i, Clause::Update);
                i = take_factors!(i + 1, true, Clause::Update, true);
                continue;
            }
            "DELETE" if !stmt_decided => {
                stmt = StmtKind::Delete;
                stmt_decided = true;
                set_clause!(i, Clause::Delete);
            }
            "FROM" if !p.is_word(i.wrapping_sub(1), "DISTINCT") => {
                let target = stmt == StmtKind::Delete && clause == Clause::Delete;
                set_clause!(i, Clause::From);
                i = take_factors!(i + 1, true, Clause::From, target);
                continue;
            }
            "JOIN" => {
                set_clause!(i, Clause::Join);
                i = take_factors!(i + 1, false, Clause::Join, false);
                continue;
            }
            "ON" => {
                if p.is_word(i + 1, "CONFLICT") {
                    set_clause!(i, Clause::OnConflict);
                } else if p.is_word(i + 1, "DUPLICATE") {
                    set_clause!(i, Clause::Set);
                } else if create_index && !stmt_decided && sources.is_empty() {
                    set_clause!(i, Clause::IndexOn);
                    i = take_factors!(i + 1, false, Clause::IndexOn, true);
                    continue;
                } else if matches!(clause, Clause::Join | Clause::From) {
                    set_clause!(i, Clause::On);
                }
            }
            "USING" => {
                if p.is_punct(i + 1, "(") {
                    set_clause!(i, Clause::JoinUsing);
                } else if stmt == StmtKind::Delete {
                    set_clause!(i, Clause::From);
                    i = take_factors!(i + 1, true, Clause::From, false);
                    continue;
                }
            }
            "WHERE" => set_clause!(i, Clause::Where),
            "GROUP" if p.is_word(i + 1, "BY") => set_clause!(i, Clause::GroupBy),
            "HAVING" => set_clause!(i, Clause::Having),
            "WINDOW" => set_clause!(i, Clause::Window),
            "ORDER" if p.is_word(i + 1, "BY") => set_clause!(i, Clause::OrderBy),
            "LIMIT" | "OFFSET" | "FETCH" => set_clause!(i, Clause::Limit),
            "SET" if matches!(stmt, StmtKind::Update | StmtKind::Insert) || clause == Clause::AlterTable => {
                set_clause!(i, Clause::Set)
            }
            "VALUES" => set_clause!(i, Clause::Values),
            "RETURNING" => set_clause!(i, Clause::Returning),
            "UNION" | "INTERSECT" | "EXCEPT" => set_clause!(i, Clause::None),
            "TABLE" if alter && i == from + 1 => {
                set_clause!(i, Clause::AlterTable);
                if p.is_word(i + 1, "IF") && p.is_word(i + 2, "EXISTS") {
                    i += 2;
                }
                i = take_factors!(i + 1, false, Clause::AlterTable, true);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    ScopeScan { sources, clause_starts: starts, stmt }
}

/// Columns a source exposes, with a short detail string (type and key markers).
pub(crate) fn source_columns(src: &Source, catalog: Option<&Catalog>, ctes: &[CteDef]) -> Vec<ColumnRef> {
    match &src.kind {
        SourceKind::Derived(c) | SourceKind::Function(c) => {
            c.iter().map(|n| ColumnRef { name: n.clone(), detail: String::new() }).collect()
        }
        SourceKind::Cte => ctes
            .iter()
            .rev()
            .find(|c| c.name.eq_ignore_ascii_case(&src.name))
            .map(|c| {
                c.columns.iter().map(|n| ColumnRef { name: n.clone(), detail: "CTE column".into() }).collect()
            })
            .unwrap_or_default(),
        SourceKind::Table => {
            let Some((_, t)) = catalog.and_then(|c| c.find_table(src.schema.as_deref(), &src.name)) else {
                return Vec::new();
            };
            t.columns
                .iter()
                .map(|col| {
                    let mut d = col.data_type.clone();
                    if t.primary_key.iter().any(|k| k.eq_ignore_ascii_case(&col.name)) {
                        d.push_str(" · PK");
                    }
                    if let Some(fk) = t
                        .foreign_keys
                        .iter()
                        .find(|f| f.columns.iter().any(|c| c.eq_ignore_ascii_case(&col.name)))
                    {
                        let idx =
                            fk.columns.iter().position(|c| c.eq_ignore_ascii_case(&col.name)).unwrap_or(0);
                        let rc = fk.referenced_columns.get(idx).map_or("", |s| s.as_str());
                        d.push_str(&format!(" · FK → {}.{}", fk.referenced_table, rc));
                    }
                    if !col.nullable && !t.primary_key.iter().any(|k| k.eq_ignore_ascii_case(&col.name)) {
                        d.push_str(" · NOT NULL");
                    }
                    ColumnRef { name: col.name.clone(), detail: d }
                })
                .collect()
        }
    }
}

/// Open-paren token indexes enclosing a cursor that sits before token `idx`
/// (outermost first).
pub(crate) fn enclosing_parens(p: &Parsed, idx: usize) -> Vec<usize> {
    let mut stack = Vec::new();
    for i in 0..idx.min(p.len()) {
        if p.is_punct(i, "(") {
            stack.push(i);
        } else if p.is_punct(i, ")") {
            stack.pop();
        }
    }
    stack
}

/// The innermost query scope around `idx`, narrowed to the set-operation branch
/// (`UNION` etc.) containing it, plus the index to use for clause lookup and the
/// outer scopes (for CTE visibility).
pub(crate) struct ScopeCtx {
    /// Full token range of every enclosing query scope, outermost first.
    pub scopes: Vec<Range<usize>>,
    /// Branch of the innermost scope containing the cursor.
    pub branch: Range<usize>,
    /// Where to look up the clause: the outermost non-subquery paren inside the
    /// innermost scope, or the cursor position itself.
    pub ref_idx: usize,
    /// Innermost enclosing paren that is *not* a subquery (function call, list...).
    pub inner_paren: Option<usize>,
}

pub(crate) fn scope_ctx(p: &Parsed, idx: usize) -> ScopeCtx {
    let chain = enclosing_parens(p, idx);
    let mut scopes: Vec<Range<usize>> = std::iter::once(0..p.len()).collect();
    let mut last_sub_pos: Option<usize> = None;
    for (pos, &o) in chain.iter().enumerate() {
        if p.is_subquery_paren(o) {
            scopes.push(o + 1..p.closing(o));
            last_sub_pos = Some(pos);
        }
    }
    let nested_start = last_sub_pos.map_or(0, |x| x + 1);
    let nested = &chain[nested_start..];
    let ref_idx = nested.first().copied().unwrap_or(idx);
    let inner_paren = nested.last().copied();
    let inner = scopes.last().cloned().unwrap_or(0..p.len());
    // Branch narrowing: find depth-0 set operators relative to the scope.
    let mut start = inner.start;
    let mut end = inner.end;
    let mut i = inner.start;
    while i < inner.end {
        if p.is_punct(i, "(") {
            i = p.closing(i) + 1;
            continue;
        }
        if matches!(p.up(i), "UNION" | "INTERSECT" | "EXCEPT") {
            if i < ref_idx {
                start = i + 1;
                if matches!(p.up(start), "ALL" | "DISTINCT") {
                    start += 1;
                }
            } else {
                end = i;
                break;
            }
        }
        i += 1;
    }
    ScopeCtx { scopes, branch: start..end.max(start), ref_idx, inner_paren }
}

/// All CTEs visible from the given scopes (outer definitions first, so later ones shadow).
pub(crate) fn visible_ctes(p: &Parsed, scopes: &[Range<usize>], catalog: Option<&Catalog>) -> Vec<CteDef> {
    let mut out = Vec::new();
    for s in scopes {
        let mut i = s.start;
        while p.is_punct(i, "(") && i < s.end {
            i += 1;
        }
        if p.is_word(i, "WITH") {
            let (found, _) = parse_with(p, i, s.end, catalog);
            out.extend(found);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use DatabaseKind::Postgres;

    type Found = (Option<String>, String, Option<String>);

    fn scan(sql: &str) -> (Vec<Found>, StmtKind) {
        let p = Parsed::from_sql(sql, Postgres);
        let s = scan_scope(&p, 0, p.len(), None, &[]);
        (s.sources.iter().map(|x| (x.schema.clone(), x.name.clone(), x.alias.clone())).collect(), s.stmt)
    }

    #[test]
    fn from_join_aliases() {
        let (s, k) =
            scan("select * from public.customers as c join orders o on o.customer_id = c.id, stores");
        assert_eq!(k, StmtKind::Select);
        assert_eq!(
            s,
            vec![
                (Some("public".into()), "customers".into(), Some("c".into())),
                (None, "orders".into(), Some("o".into())),
                (None, "stores".into(), None),
            ]
        );
    }

    #[test]
    fn clause_keywords_are_not_aliases() {
        let (s, _) = scan("select * from customers where id = 1");
        assert_eq!(s[0].2, None);
        let (s, _) = scan("select * from customers left join orders on true");
        assert_eq!(s[0].2, None);
        assert_eq!(s[1].2, None);
    }

    #[test]
    fn dml_targets() {
        let (s, k) = scan("update orders o set status = 'x' from customers c where c.id = o.customer_id");
        assert_eq!(k, StmtKind::Update);
        assert_eq!(s.len(), 2);
        let (s, k) = scan("insert into orders (id) select id from customers");
        assert_eq!(k, StmtKind::Insert);
        assert_eq!(s.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["orders", "customers"]);
        let (s, k) = scan("delete from orders o using customers c where true");
        assert_eq!(k, StmtKind::Delete);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn on_conflict_do_update_is_not_a_new_update() {
        let (s, k) = scan("insert into t (a) values (1) on conflict (a) do update set a = 2");
        assert_eq!(k, StmtKind::Insert);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn derived_table_and_cte_columns() {
        let p = Parsed::from_sql(
            "with x as (select id, name as n, count(*) from t) select * from x join (select a, b from y) d on true",
            Postgres,
        );
        let (ctes, _) = parse_with(&p, 0, p.len(), None);
        assert_eq!(ctes[0].columns, vec!["id", "n", "count"]);
        let s = scan_scope(&p, 0, p.len(), None, &ctes);
        assert_eq!(s.sources[0].kind, SourceKind::Cte);
        assert_eq!(s.sources[1].kind, SourceKind::Derived(vec!["a".into(), "b".into()]));
        assert_eq!(s.sources[1].alias.as_deref(), Some("d"));
    }

    #[test]
    fn explicit_cte_column_list_and_star_expansion() {
        let cat = fixtures::shop();
        let p = Parsed::from_sql(
            "with c(a, b) as (select 1, 2), d as (select * from customers) select 1",
            Postgres,
        );
        let (ctes, _) = parse_with(&p, 0, p.len(), Some(&cat));
        assert_eq!(ctes[0].columns, vec!["a", "b"]);
        assert_eq!(ctes[1].columns, vec!["id", "email", "name", "tier", "created_at"]);
    }

    #[test]
    fn function_source_with_alias_columns() {
        let p = Parsed::from_sql("select * from generate_series(1, 3) as g(n)", Postgres);
        let s = scan_scope(&p, 0, p.len(), None, &[]);
        assert_eq!(s.sources[0].kind, SourceKind::Function(vec!["n".into()]));
        assert_eq!(s.sources[0].alias.as_deref(), Some("g"));
    }

    #[test]
    fn clause_tracking() {
        let p = Parsed::from_sql("select a from t where x = 1 group by a order by a limit 3", Postgres);
        let s = scan_scope(&p, 0, p.len(), None, &[]);
        let at = |kw: &str| p.toks.iter().position(|t| t.up == kw).unwrap();
        assert_eq!(s.clause_at(at("FROM")), Clause::Select);
        assert_eq!(s.clause_at(at("FROM") + 1), Clause::From);
        assert_eq!(s.clause_at(at("GROUP") + 2), Clause::GroupBy);
        assert_eq!(s.clause_at(at("LIMIT") + 1), Clause::Limit);
    }

    #[test]
    fn scope_chain_and_branches() {
        let sql = "select * from a where id in (select x from b union select y from c where z)";
        let p = Parsed::from_sql(sql, Postgres);
        let idx = p.toks.iter().position(|t| p.sql[t.start..t.end] == *"z").unwrap();
        let ctx = scope_ctx(&p, idx);
        assert_eq!(ctx.scopes.len(), 2);
        let s = scan_scope(&p, ctx.branch.start, ctx.branch.end, None, &[]);
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.sources[0].name, "c");
    }

    #[test]
    fn nested_function_paren_uses_outer_clause_position() {
        let sql = "select count(a, b) from t";
        let p = Parsed::from_sql(sql, Postgres);
        let ctx = scope_ctx(&p, 4); // before `b`
        assert_eq!(ctx.inner_paren, Some(2));
        assert_eq!(ctx.ref_idx, 2);
    }

    #[test]
    fn unclosed_parens_do_not_panic() {
        let p = Parsed::from_sql("select * from (select a from ", Postgres);
        let s = scan_scope(&p, 0, p.len(), None, &[]);
        assert_eq!(s.sources.len(), 1);
        let p = Parsed::from_sql("with a as (select 1", Postgres);
        let _ = scan_scope(&p, 0, p.len(), None, &[]);
    }

    #[test]
    fn values_columns_and_distinct() {
        let p = Parsed::from_sql("select * from (values (1, 2, 3)) v", Postgres);
        let s = scan_scope(&p, 0, p.len(), None, &[]);
        assert_eq!(
            s.sources[0].kind,
            SourceKind::Derived(vec!["column1".into(), "column2".into(), "column3".into()])
        );
        let p = Parsed::from_sql("select distinct on (a) a, b from t", Postgres);
        assert_eq!(derive_columns(&p, 0, p.len(), None, &[]), vec!["a", "b"]);
    }
}
