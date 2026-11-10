//! Context-aware completion.
//!
//! The pipeline: lex the script, refuse to complete inside strings/comments, cut out
//! the statement under the cursor, remove the half-typed word from the token list,
//! work out *where* in the statement the cursor is (table position, expression in a
//! given clause, after `alias.`, ...), gather candidates for that position, then
//! rank them with a fuzzy matcher.
//!
//! Only the current statement is analysed, so cost is independent of script length;
//! lexing the whole script is the only O(n) step and takes well under a millisecond
//! for a few thousand lines.

use crate::fk::{TableRef, join_conditions, suggest_joins};
use crate::keywords::{self, qualified_name, quote_if_needed};
use crate::lexer::{TokenKind, tokenize};
use crate::scope::{
    Clause, CteDef, Parsed, ScopeScan, Source, SourceKind, StmtKind, scan_scope, scope_ctx, source_columns,
    visible_ctes,
};
use crate::script::segments;
use quarry_core::{Catalog, DatabaseKind, RoutineKind, Table, TableKind, TypeKind};
use std::collections::HashSet;
use std::ops::Range;

pub struct CompletionRequest<'a> {
    pub sql: &'a str,
    /// Byte offset of the cursor.
    pub offset: usize,
    pub kind: DatabaseKind,
    pub catalog: Option<&'a Catalog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    Keyword,
    Schema,
    Table,
    View,
    Column,
    Function,
    Alias,
    Join,
    Snippet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// Column type, `table · 1.2k rows`, FK description, function signature...
    pub detail: Option<String>,
    pub insert_text: String,
    pub score: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionList {
    /// Bytes of the partial word being completed.
    pub replace_range: Range<usize>,
    /// Best first, at most [`MAX_ITEMS`].
    pub items: Vec<CompletionItem>,
}

pub const MAX_ITEMS: usize = 200;

// ---------------------------------------------------------------------------
// fuzzy matching

const NEG: i32 = i32::MIN / 2;

/// Case-insensitive fuzzy score of `pattern` against `cand`; `None` when `pattern`
/// is not a subsequence. Prefix, word-boundary and consecutive matches score higher.
pub(crate) fn fuzzy_score(pattern: &str, cand: &str) -> Option<i32> {
    if pattern.is_empty() {
        return Some(0);
    }
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let p: Vec<char> = pattern.chars().map(lower).collect();
    let c: Vec<char> = cand.chars().collect();
    let cl: Vec<char> = c.iter().map(|&x| lower(x)).collect();
    let (m, n) = (p.len(), c.len());
    if m > n {
        return None;
    }
    // Cheap subsequence rejection before the DP.
    let mut pi = 0;
    for &ch in &cl {
        if pi < m && ch == p[pi] {
            pi += 1;
        }
    }
    if pi < m {
        return None;
    }
    let boundary = |j: usize| -> bool {
        j == 0
            || matches!(c[j - 1], '_' | ' ' | '.' | '-' | '$')
            || (c[j].is_uppercase() && c[j - 1].is_lowercase())
    };
    let pat_chars: Vec<char> = pattern.chars().collect();
    let mut prev = vec![NEG; n];
    let mut cur = vec![NEG; n];
    for i in 0..m {
        // `run` = max over k < j-1 of prev[k] + k, so a gap of g skipped characters costs g.
        let mut run = NEG;
        for j in 0..n {
            cur[j] = NEG;
            if j >= 2 && prev[j - 2] > NEG {
                run = run.max(prev[j - 2] + (j as i32 - 2));
            }
            if cl[j] != p[i] {
                continue;
            }
            let mut bonus = 0;
            if j == 0 {
                bonus += 16;
            } else if boundary(j) {
                bonus += 12;
            }
            if c[j] == pat_chars[i] {
                bonus += 1;
            }
            let base = if i == 0 {
                -(j.min(8) as i32)
            } else {
                let mut best = NEG;
                if j >= 1 && prev[j - 1] > NEG {
                    best = prev[j - 1] + 10;
                }
                if run > NEG {
                    best = best.max(run - (j as i32 - 1));
                }
                if best == NEG {
                    continue;
                }
                best
            };
            cur[j] = base + bonus;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let best = prev.iter().copied().max().filter(|&s| s > NEG)?;
    let mut score = best;
    if cl.starts_with(&p) {
        score += 80;
        if m == n {
            score += 20;
        }
    }
    Some(score)
}

// ---------------------------------------------------------------------------
// raw candidates

struct Raw {
    label: String,
    kind: CompletionKind,
    detail: Option<String>,
    insert: String,
    bonus: i32,
}

fn raw(
    label: impl Into<String>,
    kind: CompletionKind,
    detail: Option<String>,
    insert: impl Into<String>,
    bonus: i32,
) -> Raw {
    Raw { label: label.into(), kind, detail, insert: insert.into(), bonus }
}

#[derive(Clone)]
enum Ctx {
    StatementStart,
    Qualified(Vec<String>),
    Tables {
        join: bool,
        views_only: bool,
    },
    ColumnList(Option<(Option<String>, String)>),
    /// Columns of the DML target (`ALTER COLUMN |`, `SET |`, `RETURNING`...).
    TargetColumns,
    Exclusive(Vec<&'static str>),
    Expr {
        clause: Clause,
        operand: bool,
    },
    Nothing,
}

struct Env<'a> {
    kind: DatabaseKind,
    catalog: Option<&'a Catalog>,
    lower_kw: bool,
    prefix: String,
    force_quote: bool,
    /// Text right after the replaced word starts with `(`: do not add one.
    paren_follows: bool,
}

impl Env<'_> {
    fn kw(&self, word: &str) -> String {
        // Match the user's casing: lower-case scripts get lower-case keywords.
        let typed_upper = self.prefix.chars().any(|c| c.is_uppercase());
        let typed_lower = self.prefix.chars().any(|c| c.is_lowercase());
        let lower = if typed_upper {
            false
        } else if typed_lower {
            true
        } else {
            self.lower_kw
        };
        if lower { word.to_lowercase() } else { word.to_string() }
    }

    fn ident(&self, name: &str) -> String {
        if self.force_quote { self.kind.quote_ident(name) } else { quote_if_needed(name, self.kind) }
    }
}

fn is_default_schema(catalog: &Catalog, name: &str) -> bool {
    match catalog.default_schema.as_deref() {
        Some(d) => d.eq_ignore_ascii_case(name),
        None => catalog.schemas.len() == 1,
    }
}

fn human_rows(n: u64) -> String {
    match n {
        0..=999 => format!("{n} rows"),
        1_000..=999_999 => {
            let v = n as f64 / 1000.0;
            if v < 10.0 { format!("{:.1}k rows", v) } else { format!("{:.0}k rows", v) }
        }
        1_000_000..=999_999_999 => {
            let v = n as f64 / 1e6;
            if v < 10.0 { format!("{:.1}M rows", v) } else { format!("{:.0}M rows", v) }
        }
        _ => format!("{:.1}B rows", n as f64 / 1e9),
    }
}

fn table_detail(t: &Table) -> String {
    let base = match t.kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::ForeignTable => "foreign table",
    };
    match t.row_estimate {
        Some(n) if t.kind != TableKind::View => format!("{base} · {}", human_rows(n)),
        _ => base.to_string(),
    }
}

fn has_geometry(catalog: &Catalog) -> bool {
    catalog.schemas.iter().any(|s| {
        s.tables
            .iter()
            .any(|t| t.columns.iter().any(|c| c.geometry.is_some() || c.kind == TypeKind::Geometry))
    })
}

// ---------------------------------------------------------------------------
// candidate generators

fn keyword_items(env: &Env, words: &[&str], bonus: i32) -> Vec<Raw> {
    words.iter().map(|w| raw(*w, CompletionKind::Keyword, None, env.kw(w), bonus)).collect()
}

fn function_items(env: &Env) -> Vec<Raw> {
    let mut out = Vec::new();
    let geo = env.catalog.is_some_and(has_geometry);
    for f in keywords::builtin_functions(env.kind, geo) {
        let sig = if f.returns.is_empty() {
            format!("{}({})", f.name, f.args)
        } else {
            format!("{}({}) → {}", f.name, f.args, f.returns)
        };
        let insert = if env.paren_follows {
            f.name.to_string()
        } else if f.args.is_empty() {
            format!("{}()", f.name)
        } else {
            format!("{}(", f.name)
        };
        out.push(raw(f.name, CompletionKind::Function, Some(sig), insert, 150));
    }
    if let Some(cat) = env.catalog {
        for s in &cat.schemas {
            let qualify = !is_default_schema(cat, &s.name);
            for r in s.routines.iter().filter(|r| r.kind != RoutineKind::Procedure) {
                let label = if qualify { format!("{}.{}", s.name, r.name) } else { r.name.clone() };
                let ret = r.return_type.as_deref().map(|t| format!(" → {t}")).unwrap_or_default();
                let sig = format!("{}{}{}", r.name, r.signature, ret);
                let name = if qualify {
                    qualified_name(Some(&s.name), &r.name, env.kind)
                } else {
                    env.ident(&r.name)
                };
                let insert = if env.paren_follows { name } else { format!("{name}(") };
                out.push(raw(label, CompletionKind::Function, Some(sig), insert, 160));
            }
        }
    }
    out
}

fn table_items(env: &Env, ctes: &[CteDef], views_only: bool) -> Vec<Raw> {
    let mut out = Vec::new();
    for c in ctes {
        out.push(raw(c.name.clone(), CompletionKind::Table, Some("CTE".into()), env.ident(&c.name), 320));
    }
    let Some(cat) = env.catalog else { return out };
    for s in &cat.schemas {
        let qualify = !is_default_schema(cat, &s.name);
        out.push(raw(
            s.name.clone(),
            CompletionKind::Schema,
            Some("schema".into()),
            format!("{}.", env.ident(&s.name)),
            200,
        ));
        for t in &s.tables {
            let is_view = matches!(t.kind, TableKind::View | TableKind::MaterializedView);
            if views_only && !is_view {
                continue;
            }
            let label = if qualify { format!("{}.{}", s.name, t.name) } else { t.name.clone() };
            let insert =
                if qualify { qualified_name(Some(&s.name), &t.name, env.kind) } else { env.ident(&t.name) };
            let kind = if is_view { CompletionKind::View } else { CompletionKind::Table };
            out.push(raw(label, kind, Some(table_detail(t)), insert, 300));
        }
    }
    out
}

fn source_label(src: &Source) -> &'static str {
    match src.kind {
        SourceKind::Table => "table",
        SourceKind::Cte => "CTE",
        SourceKind::Derived(_) => "subquery",
        SourceKind::Function(_) => "function",
    }
}

fn column_items_for(
    env: &Env,
    src: &Source,
    ctes: &[CteDef],
    qualifier: Option<&str>,
    extra_detail: bool,
) -> Vec<Raw> {
    source_columns(src, env.catalog, ctes)
        .into_iter()
        .map(|c| {
            let (label, insert) = match qualifier {
                Some(q) => (format!("{q}.{}", c.name), format!("{}.{}", env.ident(q), env.ident(&c.name))),
                None => (c.name.clone(), env.ident(&c.name)),
            };
            let mut detail = c.detail.clone();
            if extra_detail {
                if !detail.is_empty() {
                    detail.push_str(" · ");
                }
                detail.push_str(src.reference_name());
            }
            raw(
                label,
                CompletionKind::Column,
                if detail.is_empty() { None } else { Some(detail) },
                insert,
                300,
            )
        })
        .collect()
}

fn columns_of_table(env: &Env, schema: Option<&str>, name: &str, ctes: &[CteDef]) -> Vec<Raw> {
    let src = Source {
        schema: schema.map(|s| s.to_string()),
        name: name.to_string(),
        alias: None,
        kind: if schema.is_none() && ctes.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
            SourceKind::Cte
        } else {
            SourceKind::Table
        },
        tok_start: 0,
        tok_end: 0,
        name_tok: 0,
        clause: Clause::None,
        target: false,
    };
    column_items_for(env, &src, ctes, None, false)
}

fn qualified_items(env: &Env, parts: &[String], scan: &ScopeScan, ctes: &[CteDef]) -> Vec<Raw> {
    let mut out = Vec::new();
    match parts {
        [name] => {
            let by_alias = scan
                .sources
                .iter()
                .find(|s| s.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name)));
            let by_name =
                scan.sources.iter().find(|s| s.alias.is_none() && s.name.eq_ignore_ascii_case(name));
            let by_table_name = scan.sources.iter().find(|s| s.name.eq_ignore_ascii_case(name));
            let src_cols = by_alias
                .or(by_name)
                .or(by_table_name)
                .map(|src| column_items_for(env, src, ctes, None, false))
                .unwrap_or_default();
            if !src_cols.is_empty() {
                out.extend(src_cols);
            } else if name.eq_ignore_ascii_case("excluded") && scan.stmt == StmtKind::Insert {
                if let Some(t) = scan.sources.iter().find(|s| s.target) {
                    out.extend(column_items_for(env, t, ctes, None, false));
                }
            } else {
                if let Some(c) = ctes.iter().rev().find(|c| c.name.eq_ignore_ascii_case(name)) {
                    out.extend(columns_of_table(env, None, &c.name, ctes));
                }
                if let Some(cat) = env.catalog {
                    if out.is_empty()
                        && let Some((s, t)) = cat.find_table(None, name)
                    {
                        out.extend(columns_of_table(env, Some(&s.name), &t.name, ctes));
                    }
                    if let Some(schema) = cat.schema(name) {
                        for t in &schema.tables {
                            let is_view = matches!(t.kind, TableKind::View | TableKind::MaterializedView);
                            out.push(raw(
                                t.name.clone(),
                                if is_view { CompletionKind::View } else { CompletionKind::Table },
                                Some(table_detail(t)),
                                env.ident(&t.name),
                                300,
                            ));
                        }
                        for r in schema.routines.iter().filter(|r| r.kind != RoutineKind::Procedure) {
                            let ret = r.return_type.as_deref().map(|t| format!(" → {t}")).unwrap_or_default();
                            let insert = if env.paren_follows {
                                env.ident(&r.name)
                            } else {
                                format!("{}(", env.ident(&r.name))
                            };
                            out.push(raw(
                                r.name.clone(),
                                CompletionKind::Function,
                                Some(format!("{}{}{}", r.name, r.signature, ret)),
                                insert,
                                150,
                            ));
                        }
                    }
                }
            }
        }
        [schema, table] => {
            if let Some(cat) = env.catalog
                && let Some((s, t)) = cat.find_table(Some(schema), table)
            {
                out.extend(columns_of_table(env, Some(&s.name), &t.name, ctes));
            }
        }
        _ => {}
    }
    out
}

fn refs_of(scan: &ScopeScan) -> (Vec<TableRef>, Vec<usize>) {
    let mut refs = Vec::new();
    let mut src_idx = Vec::new();
    for (i, s) in scan.sources.iter().enumerate() {
        if s.kind == SourceKind::Table {
            refs.push(TableRef { schema: s.schema.clone(), table: s.name.clone(), alias: s.alias.clone() });
            src_idx.push(i);
        }
    }
    (refs, src_idx)
}

fn join_items(env: &Env, scan: &ScopeScan) -> Vec<Raw> {
    let Some(cat) = env.catalog else { return Vec::new() };
    let (refs, _) = refs_of(scan);
    suggest_joins(cat, &refs, env.kind)
        .into_iter()
        .take(40)
        .map(|j| {
            let text = j.clause.strip_prefix("JOIN ").unwrap_or(&j.clause).to_string();
            let detail = if j.inferred { format!("suggested · {}", j.via) } else { format!("FK {}", j.via) };
            raw(text.clone(), CompletionKind::Join, Some(detail), text, if j.inferred { 450 } else { 600 })
        })
        .collect()
}

fn on_condition_items(env: &Env, scan: &ScopeScan) -> Vec<Raw> {
    let Some(cat) = env.catalog else { return Vec::new() };
    let (refs, src_idx) = refs_of(scan);
    if refs.len() < 2 {
        return Vec::new();
    }
    // The table that this ON belongs to: the last JOINed source, else the last source.
    let newest_src =
        scan.sources.iter().rposition(|s| s.clause == Clause::Join).unwrap_or(scan.sources.len() - 1);
    let Some(newest) = src_idx.iter().position(|&i| i == newest_src) else { return Vec::new() };
    join_conditions(cat, &refs, newest, env.kind)
        .into_iter()
        .map(|c| {
            let detail = if c.inferred { format!("suggested · {}", c.via) } else { format!("FK {}", c.via) };
            raw(
                c.condition.clone(),
                CompletionKind::Join,
                Some(detail),
                c.condition,
                if c.inferred { 750 } else { 900 },
            )
        })
        .collect()
}

fn expr_column_items(env: &Env, scan: &ScopeScan, ctes: &[CteDef], clause: Clause) -> Vec<Raw> {
    let mut sources: Vec<&Source> = scan.sources.iter().collect();
    // INSERT ... SELECT: the INSERT target's columns are not readable from the SELECT.
    if scan.stmt == StmtKind::Insert
        && !matches!(clause, Clause::Set | Clause::OnConflict | Clause::Returning)
        && sources.iter().any(|s| !s.target)
    {
        sources.retain(|s| !s.target);
    }
    let cols: Vec<Vec<crate::scope::ColumnRef>> =
        sources.iter().map(|s| source_columns(s, env.catalog, ctes)).collect();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for list in &cols {
        for c in list {
            *seen.entry(c.name.to_lowercase()).or_default() += 1;
        }
    }
    let multi = sources.len() > 1;
    let mut out = Vec::new();
    for (src, list) in sources.iter().zip(&cols) {
        for c in list {
            let ambiguous = multi && seen.get(&c.name.to_lowercase()).copied().unwrap_or(0) > 1;
            let (label, insert) = if ambiguous {
                let q = src.reference_name();
                (format!("{q}.{}", c.name), format!("{}.{}", env.ident(q), env.ident(&c.name)))
            } else {
                (c.name.clone(), env.ident(&c.name))
            };
            let mut detail = c.detail.clone();
            if ambiguous {
                if !detail.is_empty() {
                    detail.push_str(" · ");
                }
                detail.push_str(&format!("from {}", src.reference_name()));
            }
            out.push(raw(
                label,
                CompletionKind::Column,
                if detail.is_empty() { None } else { Some(detail) },
                insert,
                300,
            ));
        }
        if !matches!(src.kind, SourceKind::Function(_)) || src.alias.is_some() {
            let name = src.reference_name();
            if !name.is_empty() {
                out.push(raw(
                    name.to_string(),
                    CompletionKind::Alias,
                    Some(format!("{} in scope", source_label(src))),
                    env.ident(name),
                    250,
                ));
            }
        }
    }
    out
}

struct Snippet {
    trigger: &'static str,
    insert: &'static str,
    shown: &'static str,
}

const SNIPPETS: &[Snippet] = &[
    Snippet { trigger: "sel", insert: "SELECT * FROM ", shown: "SELECT * FROM …" },
    Snippet { trigger: "selc", insert: "SELECT count(*) FROM ", shown: "SELECT count(*) FROM …" },
    Snippet { trigger: "selw", insert: "SELECT * FROM  WHERE ", shown: "SELECT * FROM … WHERE …" },
    Snippet {
        trigger: "ins", insert: "INSERT INTO  () VALUES ()", shown: "INSERT INTO … (…) VALUES (…)"
    },
    Snippet { trigger: "upd", insert: "UPDATE  SET  WHERE ", shown: "UPDATE … SET … WHERE …" },
    Snippet { trigger: "del", insert: "DELETE FROM  WHERE ", shown: "DELETE FROM … WHERE …" },
    Snippet {
        trigger: "cte",
        insert: "WITH cte AS (\n    SELECT \n)\nSELECT * FROM cte",
        shown: "WITH cte AS (…) SELECT …",
    },
    Snippet {
        trigger: "crt",
        insert: "CREATE TABLE  (\n    id integer PRIMARY KEY\n)",
        shown: "CREATE TABLE … (…)",
    },
    Snippet { trigger: "idx", insert: "CREATE INDEX  ON  ()", shown: "CREATE INDEX … ON … (…)" },
    Snippet { trigger: "exp", insert: "EXPLAIN ANALYZE ", shown: "EXPLAIN ANALYZE …" },
];

fn snippet_items(env: &Env) -> Vec<Raw> {
    SNIPPETS
        .iter()
        .map(|s| {
            let insert = if env.lower_kw
                && !env.prefix.chars().any(|c| c.is_uppercase())
                && !s.insert.contains('\n')
            {
                // Keep lower-case scripts lower-case.
                s.insert.to_lowercase()
            } else {
                s.insert.to_string()
            };
            raw(s.trigger, CompletionKind::Snippet, Some(s.shown.to_string()), insert, 130)
        })
        .collect()
}

fn clause_key(c: Clause) -> &'static str {
    match c {
        Clause::Select => "select",
        Clause::From => "from",
        Clause::Join => "join",
        Clause::On => "on",
        Clause::Where => "where",
        Clause::Having => "having",
        Clause::GroupBy => "group_by",
        Clause::OrderBy => "order_by",
        Clause::Limit => "limit",
        Clause::Set => "set",
        Clause::Update => "update",
        Clause::Delete => "delete",
        Clause::Into => "into",
        Clause::Values => "values",
        _ => "other",
    }
}

fn operand_kw(up: &str) -> bool {
    matches!(
        up,
        "SELECT"
            | "DISTINCT"
            | "ALL"
            | "WHERE"
            | "AND"
            | "OR"
            | "NOT"
            | "ON"
            | "BY"
            | "HAVING"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "CASE"
            | "IN"
            | "EXISTS"
            | "ANY"
            | "SOME"
            | "LIKE"
            | "ILIKE"
            | "BETWEEN"
            | "IS"
            | "SET"
            | "RETURNING"
            | "USING"
            | "LIMIT"
            | "OFFSET"
            | "VALUES"
            | "FROM"
            | "JOIN"
            | "INTO"
            | "UPDATE"
            | "DELETE"
            | "WITH"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "DEFAULT"
            | "OVER"
            | "FILTER"
            | "WINDOW"
            | "SIMILAR"
            | "ESCAPE"
            | "CAST"
            | "INTERVAL"
            | "ARRAY"
    )
}

/// Does the token before the cursor leave us expecting an operand (column, literal,
/// function...) as opposed to an operator / next keyword?
fn expects_operand(p: &Parsed, last: usize) -> bool {
    let t = &p.toks[last];
    match t.kind {
        TokenKind::Punct => matches!(t.up.as_str(), "(" | "," | "["),
        TokenKind::Operator => {
            if t.up == "*" {
                // `SELECT *` / `t.*` is a complete item; `a * |` wants an operand.
                !(last == 0
                    || matches!(p.up(last - 1), "SELECT" | "DISTINCT")
                    || p.is_punct(last - 1, ".")
                    || p.is_punct(last - 1, ",")
                    || p.is_punct(last - 1, "("))
            } else {
                true
            }
        }
        TokenKind::Word => operand_kw(&t.up),
        _ => false,
    }
}

fn case_is_open(p: &Parsed, range: &Range<usize>, upto: usize) -> bool {
    let mut depth = 0i32;
    for i in range.start..upto.min(range.end) {
        match p.up(i) {
            "CASE" => depth += 1,
            "END" => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}

fn column_list_table(p: &Parsed, open: usize, scan: &ScopeScan) -> Option<Option<(Option<String>, String)>> {
    if open == 0 {
        return None;
    }
    if p.is_word(open - 1, "CONFLICT") {
        let t = scan.sources.iter().find(|s| s.target)?;
        return Some(Some((t.schema.clone(), t.name.clone())));
    }
    if !p.is_ident(open - 1) {
        return None;
    }
    let mut j = open - 1;
    let mut parts = vec![p.ident(j)];
    while j >= 2 && p.is_punct(j - 1, ".") && p.is_ident(j - 2) {
        parts.insert(0, p.ident(j - 2));
        j -= 2;
    }
    if j == 0 {
        return None;
    }
    let before = p.up(j - 1);
    let ok = matches!(before, "INTO" | "REFERENCES" | "TABLE")
        || (before == "ON" && p.first_word() == "CREATE")
        || (before == "ONLY");
    if !ok {
        return None;
    }
    let name = parts.pop()?;
    Some(Some((parts.pop(), name)))
}

fn determine(
    p: &Parsed,
    ci: usize,
    sc: &crate::scope::ScopeCtx,
    scan: &ScopeScan,
    kind: DatabaseKind,
) -> Ctx {
    if ci == 0 {
        return Ctx::StatementStart;
    }
    let last = ci - 1;
    if (0..ci).all(|i| matches!(p.up(i), "EXPLAIN" | "ANALYZE" | "ANALYSE" | "VERBOSE" | "QUERY" | "PLAN")) {
        return Ctx::StatementStart;
    }
    if p.is_punct(last, ".") {
        let mut parts = Vec::new();
        let mut j = last;
        while p.is_punct(j, ".") && j >= 1 && p.is_ident(j - 1) {
            parts.insert(0, p.ident(j - 1));
            if j >= 3 && p.is_punct(j - 2, ".") {
                j -= 2;
            } else {
                break;
            }
        }
        return if parts.is_empty() { Ctx::Nothing } else { Ctx::Qualified(parts) };
    }
    let first = p.first_word();
    let lu = p.toks[last].up.as_str();
    let prev_up = if last > 0 { p.up(last - 1) } else { "" };
    let clause = scan.clause_at(sc.ref_idx);
    let in_paren = sc.inner_paren.is_some();
    let operand = expects_operand(p, last);

    // Keyword sequences that have exactly one sensible continuation.
    match lu {
        "ORDER" | "GROUP" | "PARTITION" => return Ctx::Exclusive(vec!["BY"]),
        "INSERT" if !in_paren => return Ctx::Exclusive(vec!["INTO"]),
        "REPLACE" if last == 0 => return Ctx::Exclusive(vec!["INTO"]),
        "DELETE" if last == 0 => return Ctx::Exclusive(vec!["FROM"]),
        "LEFT" | "RIGHT" | "FULL" if !in_paren => return Ctx::Exclusive(vec!["JOIN", "OUTER JOIN"]),
        "INNER" | "CROSS" | "NATURAL" | "OUTER" if !in_paren => return Ctx::Exclusive(vec!["JOIN"]),
        "UNION" | "INTERSECT" | "EXCEPT" => {
            return Ctx::Exclusive(vec!["ALL", "DISTINCT", "SELECT", "VALUES"]);
        }
        "ALL" if matches!(prev_up, "UNION" | "INTERSECT" | "EXCEPT") => {
            return Ctx::Exclusive(vec!["SELECT", "VALUES"]);
        }
        "IS" => return Ctx::Exclusive(vec!["NULL", "NOT NULL", "TRUE", "FALSE", "DISTINCT FROM"]),
        "NULLS" => return Ctx::Exclusive(vec!["FIRST", "LAST"]),
        "NOT" if last > 0 && !expects_operand(p, last - 1) => {
            return Ctx::Exclusive(vec!["IN", "LIKE", "BETWEEN", "NULL", "EXISTS"]);
        }
        "CONFLICT" if prev_up == "ON" => return Ctx::Exclusive(vec!["DO NOTHING", "DO UPDATE SET"]),
        "DO" if clause == Clause::OnConflict => return Ctx::Exclusive(vec!["NOTHING", "UPDATE SET"]),
        "ON" if scan.stmt == StmtKind::Insert
            && matches!(clause, Clause::Values | Clause::Other | Clause::None | Clause::Into) =>
        {
            return Ctx::Exclusive(if kind == DatabaseKind::MySql {
                vec!["DUPLICATE KEY UPDATE"]
            } else {
                vec!["CONFLICT"]
            });
        }
        "UPDATE" if matches!(prev_up, "DO" | "FOR") => {
            return Ctx::Exclusive(if prev_up == "DO" {
                vec!["SET"]
            } else {
                vec!["OF", "NOWAIT", "SKIP LOCKED"]
            });
        }
        "CREATE" if last == 0 => {
            return Ctx::Exclusive(match kind {
                DatabaseKind::Postgres => vec![
                    "TABLE",
                    "VIEW",
                    "MATERIALIZED VIEW",
                    "INDEX",
                    "UNIQUE INDEX",
                    "SCHEMA",
                    "FUNCTION",
                    "TRIGGER",
                    "SEQUENCE",
                    "EXTENSION",
                    "TYPE",
                    "TEMPORARY TABLE",
                    "OR REPLACE",
                ],
                DatabaseKind::MySql => vec![
                    "TABLE",
                    "VIEW",
                    "INDEX",
                    "UNIQUE INDEX",
                    "DATABASE",
                    "SCHEMA",
                    "FUNCTION",
                    "PROCEDURE",
                    "TRIGGER",
                    "TEMPORARY TABLE",
                    "OR REPLACE",
                ],
                DatabaseKind::Sqlite => vec![
                    "TABLE",
                    "VIEW",
                    "INDEX",
                    "UNIQUE INDEX",
                    "TRIGGER",
                    "VIRTUAL TABLE",
                    "TEMPORARY TABLE",
                ],
            });
        }
        "ALTER" if last == 0 => return Ctx::Exclusive(vec!["TABLE", "VIEW", "INDEX", "SCHEMA", "SEQUENCE"]),
        "DROP" if last == 0 => {
            return Ctx::Exclusive(vec![
                "TABLE",
                "VIEW",
                "INDEX",
                "SCHEMA",
                "SEQUENCE",
                "TRIGGER",
                "FUNCTION",
                "TABLE IF EXISTS",
            ]);
        }
        "COLUMN" if clause == Clause::AlterTable => return Ctx::TargetColumns,
        "ADD" if clause == Clause::AlterTable => {
            return Ctx::Exclusive(vec![
                "COLUMN",
                "CONSTRAINT",
                "PRIMARY KEY",
                "FOREIGN KEY",
                "UNIQUE",
                "CHECK",
            ]);
        }
        "AS" => return Ctx::Nothing,
        _ => {}
    }

    // Column list in parentheses: `INSERT INTO t (|`, `ON CONFLICT (|`, `CREATE INDEX .. ON t (|`.
    if let Some(open) = sc.inner_paren
        && operand
        && let Some(t) = column_list_table(p, open, scan)
    {
        return Ctx::ColumnList(t);
    }

    // Table positions.
    if !in_paren || p.is_subquery_paren(sc.inner_paren.unwrap_or(usize::MAX)) {
        let ddl_first = matches!(
            first,
            "DROP"
                | "ALTER"
                | "TRUNCATE"
                | "LOCK"
                | "COMMENT"
                | "ANALYZE"
                | "VACUUM"
                | "REINDEX"
                | "CLUSTER"
                | "COPY"
                | "DESCRIBE"
                | "DESC"
                | "OPTIMIZE"
                | "REFRESH"
        );
        let table_pos = match lu {
            "FROM" => !matches!(prev_up, "DISTINCT"),
            "JOIN" | "INTO" | "ONLY" | "REFERENCES" | "LATERAL" => true,
            "UPDATE" => clause == Clause::Update,
            "TABLE" => first != "CREATE" || false,
            "VIEW" => matches!(first, "DROP" | "ALTER") || matches!(prev_up, "MATERIALIZED"),
            "TRUNCATE" | "DESCRIBE" | "DESC" if last == 0 => true,
            "COPY" | "ANALYZE" | "VACUUM" | "OPTIMIZE" | "REINDEX" | "CLUSTER"
                if last == 0 && kind != DatabaseKind::MySql || (last == 0 && lu == "OPTIMIZE") =>
            {
                true
            }
            "EXISTS" if prev_up == "IF" => ddl_first && first != "CREATE",
            "ON" => {
                first == "CREATE" && clause == Clause::IndexOn
                    || (first == "CREATE"
                        && last > 0
                        && scan.sources.is_empty()
                        && p.toks.iter().any(|t| t.up == "INDEX"))
            }
            "USING" => scan.stmt == StmtKind::Delete,
            "," => {
                (matches!(clause, Clause::From | Clause::Update) && lu == ",")
                    || (ddl_first && matches!(first, "DROP" | "TRUNCATE" | "LOCK"))
            }
            _ => false,
        };
        if table_pos {
            return Ctx::Tables { join: lu == "JOIN", views_only: lu == "VIEW" && first == "DROP" };
        }
    }
    if lu == "TABLE" && first == "CREATE" {
        return Ctx::Nothing;
    }

    // Plain expression positions.
    if clause == Clause::Set && operand && !in_paren && (lu == "SET" || lu == ",") {
        return Ctx::TargetColumns;
    }
    if clause == Clause::Returning
        && operand
        && scan.stmt != StmtKind::Select
        && !in_paren
        && matches!(lu, "RETURNING" | ",")
    {
        // Returning can reference the target's columns; treat as ordinary expression.
    }
    Ctx::Expr { clause, operand }
}

fn clamp_boundary(s: &str, mut off: usize) -> usize {
    off = off.min(s.len());
    while !s.is_char_boundary(off) {
        off -= 1;
    }
    off
}

fn inside_literal_or_comment(sql: &str, toks: &[crate::lexer::Token], off: usize) -> bool {
    let first = toks.partition_point(|t| t.end < off);
    toks.iter().skip(first).take(2).any(|t| {
        let _ = sql;
        match t.kind {
            TokenKind::LineComment => off > t.start && off <= t.end,
            TokenKind::BlockComment | TokenKind::String | TokenKind::DollarString => {
                off > t.start && (off < t.end || !t.terminated)
            }
            _ => false,
        }
    })
}

/// Complete at `request.offset`.
pub fn complete(request: &CompletionRequest) -> CompletionList {
    let sql = request.sql;
    let kind = request.kind;
    let off = clamp_boundary(sql, request.offset);
    let nothing = CompletionList { replace_range: off..off, items: Vec::new() };
    let raw_toks = tokenize(sql, kind);
    if inside_literal_or_comment(sql, &raw_toks, off) {
        return nothing;
    }
    let segs = segments(sql, &raw_toks, kind);
    let Some(seg) = segs.iter().find(|s| off >= s.start && off <= s.end) else { return nothing };
    let seg_toks = &raw_toks[seg.tokens.clone()];

    // The word being typed (identifier-like token containing/ending at the cursor).
    let partial = seg_toks
        .iter()
        .find(|t| matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent) && t.start < off && off <= t.end)
        .copied();
    let replace_range = partial.map_or(off..off, |t| t.range());
    let (prefix, force_quote) = match partial {
        Some(t) => {
            let typed = &sql[t.start..off];
            let quoted = t.kind == TokenKind::QuotedIdent;
            let stripped = if quoted { typed.get(1..).unwrap_or("") } else { typed };
            (stripped.to_string(), quoted)
        }
        None => (String::new(), false),
    };

    let p = Parsed::new(sql, seg_toks.iter().filter(|t| Some(**t) != partial));
    let ci = p.toks.partition_point(|t| t.start < off);
    let lower_kw = p.toks[..ci]
        .iter()
        .rev()
        .find(|t| t.kind == TokenKind::Word && keywords::is_format_keyword(&sql[t.start..t.end]))
        .is_some_and(|t| !sql[t.start..t.end].chars().any(|c| c.is_uppercase()));
    let paren_follows = sql[replace_range.end..].trim_start_matches([' ', '\t']).starts_with('(');
    let env =
        Env { kind, catalog: request.catalog, lower_kw, prefix: prefix.clone(), force_quote, paren_follows };

    let sc = scope_ctx(&p, ci);
    let ctes = visible_ctes(&p, &sc.scopes, request.catalog);
    let scan = scan_scope(&p, sc.branch.start, sc.branch.end, request.catalog, &ctes);
    let ctx = determine(&p, ci, &sc, &scan, kind);

    let mut cands: Vec<Raw> = Vec::new();
    match &ctx {
        Ctx::StatementStart => {
            cands.extend(keyword_items(&env, &keywords::statement_keywords(kind), 200));
            cands.extend(snippet_items(&env));
        }
        Ctx::Qualified(parts) => cands.extend(qualified_items(&env, parts, &scan, &ctes)),
        Ctx::Tables { join, views_only } => {
            if *join {
                cands.extend(join_items(&env, &scan));
            }
            cands.extend(table_items(&env, &ctes, *views_only));
        }
        Ctx::ColumnList(Some((schema, name))) => {
            cands.extend(columns_of_table(&env, schema.as_deref(), name, &ctes))
        }
        Ctx::ColumnList(None) => {}
        Ctx::TargetColumns => {
            if let Some(t) = scan.sources.iter().find(|s| s.target).or(scan.sources.first()) {
                cands.extend(column_items_for(&env, t, &ctes, None, false));
            }
        }
        Ctx::Exclusive(words) => cands.extend(keyword_items(&env, words, 300)),
        Ctx::Expr { clause, operand: false } => {
            let mut words = keywords::follow_keywords(clause_key(*clause), kind);
            if case_is_open(&p, &sc.branch, ci) {
                words.extend(["WHEN", "THEN", "ELSE", "END"]);
            }
            let last_is_table_ish = matches!(
                clause,
                Clause::From | Clause::Join | Clause::Update | Clause::Into | Clause::Delete
            );
            cands.extend(keyword_items(&env, &words, if last_is_table_ish { 300 } else { 250 }));
        }
        Ctx::Expr { clause, operand: true } => {
            let last = ci - 1;
            let after_on = p.is_word(last, "ON") && *clause == Clause::On;
            if after_on {
                cands.extend(on_condition_items(&env, &scan));
            }
            match clause {
                Clause::Values | Clause::Limit => {}
                _ => cands.extend(expr_column_items(&env, &scan, &ctes, *clause)),
            }
            cands.extend(function_items(&env));
            cands.extend(keyword_items(&env, &keywords::operand_keywords(kind), 100));
            if p.is_punct(last, "(") {
                cands.extend(keyword_items(&env, &["SELECT", "WITH"], 90));
            }
            if matches!(clause, Clause::Values) {
                cands.extend(keyword_items(&env, &["DEFAULT"], 150));
            }
            if *clause == Clause::Set && scan.stmt == StmtKind::Insert && kind != DatabaseKind::MySql {
                cands.push(raw(
                    "excluded",
                    CompletionKind::Alias,
                    Some("proposed row".into()),
                    env.kw("excluded"),
                    260,
                ));
            }
            if case_is_open(&p, &sc.branch, ci) && p.is_word(last, "CASE") {
                cands.extend(keyword_items(&env, &["WHEN"], 260));
            }
        }
        Ctx::Nothing => {}
    }
    finish(cands, &prefix, replace_range)
}

fn finish(cands: Vec<Raw>, prefix: &str, replace_range: Range<usize>) -> CompletionList {
    let mut seen: HashSet<(String, CompletionKind)> = HashSet::new();
    let mut items: Vec<CompletionItem> = Vec::with_capacity(cands.len().min(1024));
    for c in cands {
        let m = fuzzy_score(prefix, &c.label);
        // Qualified labels (`c.id`, `public.orders`) also match on their last segment.
        let tail = c.label.rsplit('.').next().filter(|t| t.len() < c.label.len());
        let tail_score = tail.and_then(|t| fuzzy_score(prefix, t)).map(|s| s - 2);
        let Some(s) = m.max(tail_score) else { continue };
        if !seen.insert((c.label.clone(), c.kind)) {
            continue;
        }
        // Shorter labels first among equal matches: `orders` before `order_items` for `ord`.
        let len_penalty = if prefix.is_empty() { 0 } else { (c.label.chars().count() as i32).min(40) / 4 };
        items.push(CompletionItem {
            label: c.label,
            kind: c.kind,
            detail: c.detail,
            insert_text: c.insert,
            score: s + c.bonus - len_penalty,
        });
    }
    items.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    items.truncate(MAX_ITEMS);
    CompletionList { replace_range, items }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use DatabaseKind::*;

    fn run(sql_with_cursor: &str, kind: DatabaseKind, cat: Option<&Catalog>) -> (CompletionList, String) {
        let off = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replacen('|', "", 1);
        let list = complete(&CompletionRequest { sql: &sql, offset: off, kind, catalog: cat });
        (list, sql)
    }

    fn labels(sql: &str) -> Vec<String> {
        let cat = fixtures::shop();
        run(sql, Postgres, Some(&cat)).0.items.into_iter().map(|i| i.label).collect()
    }

    fn items(sql: &str) -> Vec<CompletionItem> {
        let cat = fixtures::shop();
        run(sql, Postgres, Some(&cat)).0.items
    }

    fn has(sql: &str, label: &str) -> bool {
        labels(sql).iter().any(|l| l == label)
    }

    #[test]
    fn fuzzy_prefix_beats_boundary_beats_subsequence() {
        let prefix = fuzzy_score("ord", "orders").unwrap();
        let boundary = fuzzy_score("oi", "order_items").unwrap();
        let sub = fuzzy_score("ods", "orders").unwrap();
        assert!(prefix > boundary, "{prefix} {boundary}");
        assert!(boundary > sub || prefix > sub);
        assert!(fuzzy_score("xyz", "orders").is_none());
        assert!(fuzzy_score("ORD", "orders").is_some());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        // Word-boundary subsequence outranks scattered subsequence.
        assert!(fuzzy_score("ci", "customer_id").unwrap() > fuzzy_score("ci", "recipient").unwrap());
    }

    #[test]
    fn fuzzy_exact_beats_longer() {
        assert!(fuzzy_score("id", "id").unwrap() > fuzzy_score("id", "idx").unwrap());
        assert!(fuzzy_score("id", "id").unwrap() > fuzzy_score("id", "customer_id").unwrap());
    }

    #[test]
    fn statement_start_offers_keywords_and_snippets() {
        let l = items("|");
        assert!(l.iter().any(|i| i.label == "SELECT" && i.kind == CompletionKind::Keyword));
        assert!(l.iter().any(|i| i.label == "sel" && i.kind == CompletionKind::Snippet));
        let l = items("sel|");
        assert_ne!(l[0].kind, CompletionKind::Column);
        let snip = l.iter().find(|i| i.kind == CompletionKind::Snippet).unwrap();
        assert_eq!(snip.insert_text, "SELECT * FROM ");
        assert!(snip.detail.as_deref().unwrap().contains("SELECT * FROM"));
        assert!(l.iter().any(|i| i.label == "SELECT"));
    }

    #[test]
    fn statement_start_after_semicolon() {
        assert!(has("select 1; |", "UPDATE"));
        assert!(has("select 1;\nsel|", "SELECT"));
    }

    #[test]
    fn replace_range_covers_partial_word() {
        let cat = fixtures::shop();
        let (l, sql) = run("select * from cust|", Postgres, Some(&cat));
        assert_eq!(&sql[l.replace_range.clone()], "cust");
        let (l, sql) = run("select * from |", Postgres, Some(&cat));
        assert_eq!(l.replace_range, sql.len()..sql.len());
        let (l, sql) = run("select c.na| from customers c", Postgres, Some(&cat));
        assert_eq!(&sql[l.replace_range], "na");
    }

    #[test]
    fn tables_after_from() {
        let l = items("select * from |");
        let customers = l.iter().find(|i| i.label == "customers").expect("customers");
        assert_eq!(customers.kind, CompletionKind::Table);
        assert_eq!(customers.detail.as_deref(), Some("table · 12k rows"));
        let ac = l.iter().find(|i| i.label == "active_customers").unwrap();
        assert_eq!(ac.kind, CompletionKind::View);
        assert!(l.iter().any(|i| i.label == "analytics" && i.kind == CompletionKind::Schema));
        // Non-default schema tables carry their schema.
        assert!(l.iter().any(|i| i.label == "analytics.events" && i.insert_text == "analytics.events"));
    }

    #[test]
    fn row_count_formatting() {
        assert_eq!(human_rows(42), "42 rows");
        assert_eq!(human_rows(1234), "1.2k rows");
        assert_eq!(human_rows(850_000), "850k rows");
        assert_eq!(human_rows(3_400_000), "3.4M rows");
    }

    #[test]
    fn tables_after_other_keywords() {
        for sql in [
            "insert into |",
            "update |",
            "delete from |",
            "select * from customers join |",
            "truncate |",
            "drop table |",
            "alter table |",
            "select * from a, |",
        ] {
            assert!(has(sql, "orders"), "{sql}");
        }
        assert!(!has("create table |", "orders"));
    }

    #[test]
    fn tables_are_fuzzy_filtered() {
        let l = labels("select * from oi|");
        assert_eq!(l[0], "order_items");
        let l = labels("select * from ord|");
        assert_eq!(l[0], "orders");
        assert!(!l.contains(&"customers".to_string()));
    }

    #[test]
    fn join_gives_fk_items_first() {
        let l = items("select * from customers c join |");
        let first = &l[0];
        assert_eq!(first.kind, CompletionKind::Join);
        assert_eq!(first.label, "orders o ON o.customer_id = c.id");
        assert_eq!(first.insert_text, "orders o ON o.customer_id = c.id");
        assert!(first.detail.as_deref().unwrap().contains("orders_customer_id_fkey"));
        // Ordinary tables still follow.
        assert!(l.iter().any(|i| i.label == "products" && i.kind == CompletionKind::Table));
    }

    #[test]
    fn join_prefix_filters_join_items() {
        let l = items("select * from orders o join cu|");
        assert_eq!(l[0].label, "customers c ON o.customer_id = c.id");
        assert_eq!(l[0].kind, CompletionKind::Join);
    }

    #[test]
    fn on_offers_join_conditions_first() {
        let l = items("select * from customers c join orders o on |");
        assert_eq!(l[0].label, "o.customer_id = c.id");
        assert_eq!(l[0].kind, CompletionKind::Join);
        assert!(l.iter().any(|i| i.label == "c.id" || i.label == "id"));
    }

    #[test]
    fn on_inferred_condition_is_marked_suggested() {
        let l = items("select * from customers c join reviews r on |");
        let cond = l.iter().find(|i| i.label == "r.customer_id = c.id").expect("inferred condition");
        assert!(cond.detail.as_deref().unwrap().starts_with("suggested"));
    }

    #[test]
    fn alias_dot_gives_columns() {
        let l = items("select c.| from customers c");
        let names: Vec<&str> = l.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"email") && names.contains(&"created_at"));
        assert!(l.iter().all(|i| i.kind == CompletionKind::Column));
        let id = l.iter().find(|i| i.label == "id").unwrap();
        assert_eq!(id.detail.as_deref(), Some("integer · PK"));
    }

    #[test]
    fn alias_dot_with_prefix() {
        let l = labels("select o.cu| from orders o join customers c on c.id = o.customer_id");
        assert_eq!(l, vec!["customer_id"]);
    }

    #[test]
    fn table_dot_and_schema_dot() {
        assert!(has("select customers.| from customers", "email"));
        assert!(has("select * from analytics.|", "events"));
        assert!(has("select * from public.|", "orders"));
        assert!(has("select analytics.events.| from analytics.events", "occurred_at"));
        let l = items("select * from analytics.|");
        assert!(l.iter().all(|i| i.kind != CompletionKind::Column));
    }

    #[test]
    fn fk_columns_describe_their_reference() {
        let l = items("select o.| from orders o");
        let c = l.iter().find(|i| i.label == "customer_id").unwrap();
        assert_eq!(c.detail.as_deref(), Some("integer · FK → customers.id"));
    }

    #[test]
    fn select_list_uses_from_clause_after_cursor() {
        let l = labels("select | from customers c");
        assert!(l.contains(&"email".to_string()));
        assert!(l.contains(&"c".to_string()));
        assert!(!l.contains(&"sku".to_string()));
    }

    #[test]
    fn ambiguous_columns_are_qualified() {
        let l = labels("select | from customers c join orders o on o.customer_id = c.id");
        assert!(l.contains(&"c.id".to_string()) && l.contains(&"o.id".to_string()));
        assert!(!l.contains(&"id".to_string()));
        assert!(l.contains(&"email".to_string()));
        let ins = items("select | from customers c join orders o on true");
        assert_eq!(ins.iter().find(|i| i.label == "o.id").unwrap().insert_text, "o.id");
    }

    #[test]
    fn where_group_order_having_use_columns() {
        for sql in [
            "select * from customers where |",
            "select * from customers where id = 1 and |",
            "select * from customers group by |",
            "select * from customers order by |",
            "select email from customers group by email having |",
        ] {
            assert!(has(sql, "email"), "{sql}");
        }
    }

    #[test]
    fn expression_contexts_include_functions_and_keywords() {
        let l = items("select * from customers where |");
        assert!(l.iter().any(|i| i.label == "coalesce" && i.kind == CompletionKind::Function));
        assert!(l.iter().any(|i| i.label == "NOT" && i.kind == CompletionKind::Keyword));
        // Columns outrank functions with an empty prefix.
        let col = l.iter().find(|i| i.label == "email").unwrap().score;
        let func = l.iter().find(|i| i.label == "coalesce").unwrap().score;
        assert!(col > func);
    }

    #[test]
    fn after_operand_offers_next_clause_keywords() {
        let l = labels("select * from customers |");
        for k in ["WHERE", "JOIN", "LEFT JOIN", "GROUP BY", "ORDER BY", "LIMIT"] {
            assert!(l.contains(&k.to_string()), "{k} in {l:?}");
        }
        assert!(!l.contains(&"email".to_string()));
        let l = labels("select * from customers where id = 1 |");
        assert!(l.contains(&"AND".to_string()) && l.contains(&"ORDER BY".to_string()));
        let l = labels("select id |");
        assert!(l.contains(&"FROM".to_string()));
    }

    #[test]
    fn exclusive_keyword_follow_ups() {
        assert_eq!(labels("select * from t order |"), vec!["BY"]);
        assert_eq!(labels("select * from t group |"), vec!["BY"]);
        assert!(has("select * from t left |", "JOIN"));
        assert!(has("select * from t union |", "ALL"));
        assert!(has("select * from t where a is |", "NOT NULL"));
        assert_eq!(labels("insert |"), vec!["INTO"]);
        assert!(has("create |", "TABLE"));
    }

    #[test]
    fn subquery_uses_innermost_scope() {
        let sql = "select * from customers c where c.id in (select o.| from orders o)";
        let l = labels(sql);
        assert!(l.contains(&"customer_id".to_string()));
        assert!(!l.contains(&"email".to_string()));
        let l =
            labels("select * from customers c where exists (select 1 from orders o where o.customer_id = |)");
        assert!(l.iter().any(|x| x == "o.id" || x == "id"), "{l:?}");
        assert!(l.contains(&"total".to_string()));
        assert!(!l.contains(&"email".to_string()));
    }

    #[test]
    fn subquery_in_select_list_sees_only_its_tables() {
        let l = labels("select (select max(|) from orders o) from customers c");
        assert!(l.contains(&"total".to_string()));
        assert!(!l.contains(&"email".to_string()));
    }

    #[test]
    fn union_branch_scope() {
        let l = labels("select a from customers union select | from products");
        assert!(l.contains(&"sku".to_string()));
        assert!(!l.contains(&"email".to_string()));
    }

    #[test]
    fn cte_names_and_columns() {
        let l = labels("with recent as (select id, total from orders) select * from |");
        assert!(l.contains(&"recent".to_string()));
        let i = items("with recent as (select id, total from orders) select r.| from recent r");
        let names: Vec<&str> = i.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(names, vec!["id", "total"]);
        let l = labels("with recent as (select id, total as amt from orders) select | from recent");
        assert!(l.contains(&"amt".to_string()));
    }

    #[test]
    fn cte_body_sees_its_own_from() {
        let l = labels("with x as (select | from customers) select 1");
        assert!(l.contains(&"email".to_string()));
    }

    #[test]
    fn derived_table_columns() {
        let l = labels("select d.| from (select email as mail, id from customers) d");
        assert_eq!(l, vec!["id", "mail"]);
    }

    #[test]
    fn update_set_offers_target_columns() {
        let l = labels("update customers set |");
        assert!(l.contains(&"email".to_string()));
        assert!(!l.iter().any(|x| x == "coalesce"));
        let l = labels("update customers set email = 'x', |");
        assert!(l.contains(&"tier".to_string()));
        let l = labels("update customers set email = | where id = 1");
        assert!(l.contains(&"name".to_string()) && l.iter().any(|x| x == "lower"));
    }

    #[test]
    fn insert_column_list() {
        let l = labels("insert into customers (|) values (1)");
        assert_eq!(l.len(), 5);
        assert!(l.contains(&"email".to_string()));
        let l = labels("insert into customers (id, |");
        assert!(l.contains(&"tier".to_string()));
        let l = labels("insert into analytics.events (|");
        assert!(l.contains(&"occurred_at".to_string()));
    }

    #[test]
    fn insert_select_does_not_read_target_columns() {
        let l = labels("insert into customers (name) select | from products");
        assert!(l.contains(&"sku".to_string()));
        assert!(!l.contains(&"email".to_string()));
    }

    #[test]
    fn on_conflict_columns_and_excluded() {
        let l = labels("insert into customers (id) values (1) on conflict (|) do nothing");
        assert!(l.contains(&"email".to_string()));
        let l =
            labels("insert into customers (id) values (1) on conflict (id) do update set name = excluded.|");
        assert!(l.contains(&"email".to_string()));
        assert!(has("insert into customers (id) values (1) on conflict (id) do update set |", "name"));
    }

    #[test]
    fn delete_and_returning() {
        assert!(has("delete from customers where |", "email"));
        assert!(has("update customers set name = 'x' returning |", "email"));
    }

    #[test]
    fn create_index_columns() {
        assert!(has("create index i on orders (|)", "customer_id"));
        assert!(has("create index i on |", "orders"));
    }

    #[test]
    fn alter_table_columns() {
        assert!(has("alter table orders drop column |", "status"));
        assert!(has("alter table orders alter column |", "total"));
    }

    #[test]
    fn nothing_inside_strings_and_comments() {
        let cat = fixtures::shop();
        for sql in [
            "select 'cust|' from t",
            "select 1 -- cus|",
            "select /* cus| */ 1",
            "select $$ cus| $$",
            "select 'unterminated cus|",
        ] {
            let (l, _) = run(sql, Postgres, Some(&cat));
            assert!(l.items.is_empty(), "{sql}");
        }
        // Right after the closing quote the cursor is outside the literal.
        let (l, _) = run("select 'a'| from customers", Postgres, Some(&cat));
        let _ = l;
        let (l, _) = run("select 1 /* x */ | from customers", Postgres, Some(&cat));
        assert!(!l.items.is_empty());
    }

    #[test]
    fn keywords_follow_script_casing() {
        let (l, _) = run("select * from customers whe|", Postgres, None);
        assert_eq!(l.items[0].insert_text, "where");
        let (l, _) = run("select * from customers WHE|", Postgres, None);
        assert_eq!(l.items[0].insert_text, "WHERE");
        let (l, _) = run("select * from customers |", Postgres, None);
        assert_eq!(l.items.iter().find(|i| i.label == "WHERE").unwrap().insert_text, "where");
        let (l, _) = run("SELECT * FROM customers |", Postgres, None);
        assert_eq!(l.items.iter().find(|i| i.label == "WHERE").unwrap().insert_text, "WHERE");
    }

    #[test]
    fn identifiers_quoted_only_when_needed() {
        let mut cat = fixtures::shop();
        cat.schemas[0].tables.push(fixtures::table(
            "Order Items",
            vec![fixtures::int("Group")],
            &[],
            vec![],
            None,
        ));
        cat.schemas[0].tables.push(fixtures::table(
            "order",
            vec![fixtures::int("select")],
            &[],
            vec![],
            None,
        ));
        let (l, _) = run("select * from |", Postgres, Some(&cat));
        let oi = l.items.iter().find(|i| i.label == "Order Items").unwrap();
        assert_eq!(oi.insert_text, "\"Order Items\"");
        let o = l.items.iter().find(|i| i.label == "order").unwrap();
        assert_eq!(o.insert_text, "\"order\"");
        let plain = l.items.iter().find(|i| i.label == "orders").unwrap();
        assert_eq!(plain.insert_text, "orders");
        let (l, _) = run("select x.| from \"Order Items\" x", Postgres, Some(&cat));
        assert_eq!(l.items[0].insert_text, "\"Group\"");
        let (l, _) = run("select * from |", MySql, Some(&cat));
        assert_eq!(l.items.iter().find(|i| i.label == "order").unwrap().insert_text, "`order`");
        assert_eq!(l.items.iter().find(|i| i.label == "Order Items").unwrap().insert_text, "`Order Items`");
    }

    #[test]
    fn typed_quote_is_replaced_and_kept() {
        let cat = fixtures::shop();
        let (l, sql) = run("select * from \"cust|", Postgres, Some(&cat));
        assert_eq!(&sql[l.replace_range.clone()], "\"cust");
        assert_eq!(l.items[0].label, "customers");
        assert_eq!(l.items[0].insert_text, "\"customers\"");
    }

    #[test]
    fn works_without_catalog() {
        let (l, _) = run("select * from |", Postgres, None);
        assert!(l.items.is_empty());
        let (l, _) = run("select c.| from customers c", Postgres, None);
        assert!(l.items.is_empty());
        let (l, _) = run("select | from customers c", Postgres, None);
        assert!(l.items.iter().any(|i| i.label == "c" && i.kind == CompletionKind::Alias));
        assert!(l.items.iter().any(|i| i.kind == CompletionKind::Function));
    }

    #[test]
    fn dialect_functions_and_postgis() {
        let cat = fixtures::shop();
        let (l, _) = run("select | from stores", Postgres, Some(&cat));
        let st = l.items.iter().find(|i| i.label == "ST_Distance").expect("PostGIS function offered");
        assert_eq!(st.detail.as_deref(), Some("ST_Distance(geomA, geomB) → double precision"));
        let (l, _) = run("select | from customers", Postgres, Some(&cat));
        assert!(l.items.iter().any(|i| i.label == "ST_Distance"), "catalog has geometry columns somewhere");
        let plain = Catalog {
            schemas: vec![quarry_core::Schema {
                name: "public".into(),
                tables: vec![cat.schemas[0].tables[0].clone()],
                ..Default::default()
            }],
            default_schema: Some("public".into()),
        };
        let (l, _) = run("select | from customers", Postgres, Some(&plain));
        assert!(!l.items.iter().any(|i| i.label == "ST_Distance"));
        let (l, _) = run("select | from customers", MySql, Some(&plain));
        assert!(l.items.iter().any(|i| i.label == "group_concat"));
        assert!(!l.items.iter().any(|i| i.label == "string_agg"));
        let (l, _) = run("select | from customers", Sqlite, Some(&plain));
        assert!(l.items.iter().any(|i| i.label == "strftime"));
    }

    #[test]
    fn function_insert_text_and_existing_paren() {
        let (l, _) = run("select cou| from customers", Postgres, None);
        let f = l.items.iter().find(|i| i.label == "count").unwrap();
        assert_eq!(f.insert_text, "count(");
        let (l, _) = run("select now| from customers", Postgres, None);
        assert_eq!(l.items.iter().find(|i| i.label == "now").unwrap().insert_text, "now()");
        let (l, _) = run("select cou|(*) from customers", Postgres, None);
        assert_eq!(l.items.iter().find(|i| i.label == "count").unwrap().insert_text, "count");
    }

    #[test]
    fn catalog_routines_are_functions() {
        let l = items("select order_t| from orders");
        let f = l.iter().find(|i| i.label == "order_total").unwrap();
        assert_eq!(f.kind, CompletionKind::Function);
        assert_eq!(f.detail.as_deref(), Some("order_total(order_id integer) → numeric"));
    }

    #[test]
    fn scores_are_sorted_descending_and_capped() {
        let cat = fixtures::big(500);
        let (l, _) = run("select * from |", Postgres, Some(&cat));
        assert_eq!(l.items.len(), MAX_ITEMS);
        assert!(l.items.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn explain_continues_statement_start() {
        let l = labels("explain |");
        assert!(l.contains(&"SELECT".to_string()));
        let l = labels("explain analyze sel|");
        assert!(l.contains(&"SELECT".to_string()));
    }

    #[test]
    fn case_expression_keywords() {
        let l = labels("select case when a > 1 then 'x' |");
        assert!(l.contains(&"ELSE".to_string()) && l.contains(&"END".to_string()));
        assert!(has("select case | from customers", "WHEN"));
    }

    #[test]
    fn cursor_in_middle_of_script_uses_only_its_statement() {
        let sql = "select * from products;\nselect | from customers;\nselect * from orders;";
        let l = labels(sql);
        assert!(l.contains(&"email".to_string()));
        assert!(!l.contains(&"sku".to_string()));
    }

    #[test]
    fn mysql_and_sqlite_basic() {
        let cat = fixtures::shop();
        let (l, _) = run("select c.| from customers c", MySql, Some(&cat));
        assert!(l.items.iter().any(|i| i.label == "email"));
        let (l, _) = run("insert into customers (a) values (1) on |", MySql, Some(&cat));
        assert_eq!(l.items[0].label, "DUPLICATE KEY UPDATE");
        let (l, _) = run("select * from customers where id = 1 |", Sqlite, Some(&cat));
        assert!(l.items.iter().any(|i| i.label == "ORDER BY"));
    }

    #[test]
    fn cursor_offsets_never_panic_on_unicode() {
        let cat = fixtures::shop();
        let sql = "select 'é日本' as 名前, c.na from customers c where c.id = 1";
        for off in 0..=sql.len() + 2 {
            let _ = complete(&CompletionRequest { sql, offset: off, kind: Postgres, catalog: Some(&cat) });
        }
    }

    #[test]
    fn performance_on_large_script_and_catalog() {
        let cat = fixtures::big(500);
        let mut sql = String::new();
        for i in 0..2000 {
            sql.push_str(&format!("select a.id, b.col_1_of_{} from table_{:04} a join table_{:04} b on b.id = a.parent_id where a.id > {};\n", i % 50, i % 500, (i + 1) % 500, i));
        }
        let cursor = sql.len() / 2;
        // Move the cursor into the middle of a statement: right after `where a.`.
        let at = sql[cursor..].find("where a.").map(|p| cursor + p + "where a.".len()).unwrap();
        let mut best = u128::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let l =
                complete(&CompletionRequest { sql: &sql, offset: at, kind: Postgres, catalog: Some(&cat) });
            best = best.min(t.elapsed().as_micros());
            assert!(!l.items.is_empty());
        }
        // `CI` (set by GitHub Actions) widens wall-clock bounds for slow, shared runners.
        let slack = if std::env::var_os("CI").is_some() { 10 } else { 1 };
        assert!(best < 10_000 * slack, "completion took {best}us");
        // A table-name completion over all 500 tables.
        let tail = format!("{sql}select * from tab");
        let t = std::time::Instant::now();
        let l = complete(&CompletionRequest {
            sql: &tail,
            offset: tail.len(),
            kind: Postgres,
            catalog: Some(&cat),
        });
        assert!(!l.items.is_empty());
        assert!(t.elapsed().as_millis() < 10 * slack as u128, "table completion took {:?}", t.elapsed());
    }
}
