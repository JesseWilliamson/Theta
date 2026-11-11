//! Diagnostics for a script: lexical and structural syntax problems, dangerous
//! statements, suspicious predicates and (given a catalog) unknown names.
//!
//! The linter is intentionally conservative. A false positive in a database IDE is
//! worse than a missed hint, so name checks only fire when the reference is
//! unambiguous (a qualified column on a catalog table, or an unqualified column in
//! a query whose every source is a known table), and structural checks look only
//! for errors that are errors in every dialect.

use crate::keywords::{is_keyword_any, is_known_function, is_statement_start, is_type_name};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::scope::{
    Clause, CteDef, Parsed, ScopeScan, Source, SourceKind, StmtKind, scan_scope, select_aliases, visible_ctes,
};
use crate::script::{Segment, segment_content, segments};
use std::collections::HashSet;
use std::ops::Range;
use theta_core::Family;
use theta_core::{Catalog, DatabaseKind, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}

/// Whether `d` only says "this statement is not finished": what an editor should hold back
/// while the user is still typing it. Server errors and dangerous-statement warnings are not
/// in this class.
pub fn is_incomplete_while_typing(d: &Diagnostic) -> bool {
    const PREFIXES: &[&str] = &[
        "Expected ",
        "Incomplete expression",
        "Trailing comma",
        "Unexpected comma",
        "Double comma",
        "Unclosed '('",
        "Unterminated ",
        "JOIN requires an ON",
        "UPDATE statement has no SET",
        "IN () needs",
        "Unknown table",
        "Unknown column",
        "Column '",
    ];
    PREFIXES.iter().any(|p| d.message.starts_with(p))
}

fn diag(range: Range<usize>, severity: Severity, message: impl Into<String>) -> Diagnostic {
    Diagnostic { range, severity, message: message.into() }
}

const CLAUSE_STARTERS: &[&str] = &[
    "FROM",
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "RETURNING",
    "JOIN",
];

pub fn lint(sql: &str, kind: DatabaseKind, catalog: Option<&Catalog>) -> Vec<Diagnostic> {
    let raw = tokenize(sql, kind);
    let segs = segments(sql, &raw, kind);
    let mut out = Vec::new();
    let created = created_tables(sql, &raw, &segs);
    for seg in &segs {
        if segment_content(&raw, seg).is_none() {
            // Comment-only: still report an unterminated block comment.
            lexical(sql, &raw[seg.tokens.clone()], &mut out);
            continue;
        }
        lint_segment(sql, kind, catalog, &raw, seg, &created, &mut out);
    }
    out.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.severity.cmp(&a.severity)));
    out.dedup();
    out
}

fn lexical(sql: &str, toks: &[Token], out: &mut Vec<Diagnostic>) {
    for t in toks.iter().filter(|t| !t.terminated) {
        let (what, closer) = match t.kind {
            TokenKind::String => ("string literal", "'"),
            TokenKind::QuotedIdent => ("quoted identifier", "the closing quote"),
            TokenKind::BlockComment => ("block comment", "*/"),
            TokenKind::DollarString => ("dollar-quoted string", "the closing tag"),
            _ => continue,
        };
        let _ = sql;
        out.push(diag(t.range(), Severity::Critical, format!("Unterminated {what}: missing {closer}")));
    }
}

/// Names created by `CREATE [TEMP] TABLE|VIEW name` anywhere in the script, so that
/// later references are not reported as unknown.
fn created_tables(sql: &str, raw: &[Token], segs: &[Segment]) -> HashSet<String> {
    let mut set = HashSet::new();
    for seg in segs {
        let p = Parsed::new(sql, raw[seg.tokens.clone()].iter());
        if p.first_word() != "CREATE" {
            continue;
        }
        let mut i = 1;
        while i < p.len()
            && matches!(
                p.up(i),
                "OR" | "REPLACE"
                    | "TEMP"
                    | "TEMPORARY"
                    | "GLOBAL"
                    | "LOCAL"
                    | "UNLOGGED"
                    | "MATERIALIZED"
                    | "RECURSIVE"
                    | "VIRTUAL"
            )
        {
            i += 1;
        }
        if matches!(p.up(i), "TABLE" | "VIEW") {
            i += 1;
            if p.is_word(i, "IF") && p.is_word(i + 1, "NOT") && p.is_word(i + 2, "EXISTS") {
                i += 3;
            }
            let mut j = i;
            while p.is_ident(j) && p.is_punct(j + 1, ".") && p.is_ident(j + 2) {
                j += 2;
            }
            if p.is_ident(j) {
                set.insert(p.ident(j).to_lowercase());
            }
        }
    }
    set
}

fn lint_segment(
    sql: &str,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
    raw: &[Token],
    seg: &Segment,
    created: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    let seg_toks = &raw[seg.tokens.clone()];
    lexical(sql, seg_toks, out);
    let p = Parsed::new(sql, seg_toks.iter());
    if p.len() == 0 {
        return;
    }
    // Parentheses.
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..p.len() {
        if p.is_punct(i, "(") {
            stack.push(i);
        } else if p.is_punct(i, ")") && stack.pop().is_none() {
            out.push(diag(p.range(i), Severity::Critical, "Unmatched ')'"));
        }
    }
    for o in stack {
        out.push(diag(p.range(o), Severity::Critical, "Unclosed '(' — missing ')'"));
    }

    let first = p.first_word();
    // Stored-routine bodies, procedural blocks and client commands are not analysed
    // beyond lexing: their grammar is a different language.
    if matches!(first, "DO" | "CALL" | "DELIMITER" | "SOURCE" | "HELP") || is_routine_definition(&p) {
        return;
    }
    if p.toks[0].kind == TokenKind::Word && !is_statement_start(&p.toks[0].up) {
        out.push(diag(p.range(0), Severity::Critical, format!("Unknown statement keyword '{}'", p.text(0))));
        return;
    }
    if !matches!(p.toks[0].kind, TokenKind::Word | TokenKind::QuotedIdent) && !p.is_punct(0, "(") {
        out.push(diag(
            p.range(0),
            Severity::Critical,
            format!("Unexpected '{}' at the start of a statement", p.text(0)),
        ));
        return;
    }

    syntax_checks(&p, kind, out);
    dangerous_statements(&p, out);

    let cmap = ClauseMap::build(&p, catalog);
    null_comparisons(&p, &cmap, out);
    let ctes = visible_ctes(&p, std::slice::from_ref(&(0..p.len())), catalog);
    let mut walker = Walker { p: &p, kind, catalog, created, cmap: &cmap, out, ctes };
    walker.scope(0..p.len(), &[]);
}

fn is_routine_definition(p: &Parsed) -> bool {
    if p.first_word() != "CREATE" {
        return false;
    }
    for i in 1..p.len().min(14) {
        match p.up(i) {
            "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT" => return true,
            "TABLE" | "VIEW" | "INDEX" | "SCHEMA" | "DATABASE" | "SEQUENCE" | "TYPE" | "EXTENSION"
            | "DOMAIN" | "ROLE" | "USER" => return false,
            _ => {}
        }
    }
    false
}

fn syntax_checks(p: &Parsed, kind: DatabaseKind, out: &mut Vec<Diagnostic>) {
    let n = p.len();
    for i in 0..n {
        let t = &p.toks[i];
        if p.is_punct(i, ",") {
            let next = p.toks.get(i + 1);
            let prev_open = i > 0 && p.is_punct(i - 1, "(");
            let msg = match next {
                None => Some("Trailing comma at the end of the statement".to_string()),
                Some(nx) if nx.kind == TokenKind::Punct && nx.up == "," => Some("Double comma".to_string()),
                Some(nx) if nx.kind == TokenKind::Punct && (nx.up == ")" || nx.up == ";") => {
                    Some("Trailing comma before ')'".to_string())
                }
                Some(nx) if nx.kind == TokenKind::Word && CLAUSE_STARTERS.contains(&nx.up.as_str()) => {
                    Some(format!("Unexpected comma before {}", nx.up))
                }
                _ if prev_open => Some("Unexpected comma after '('".to_string()),
                _ => None,
            };
            if let Some(m) = msg {
                out.push(diag(p.range(i), Severity::Critical, m));
            }
            continue;
        }
        // `PRAGMA x = on`, `CREATE INDEX .. ON t`, `SET x = on` use these words differently.
        let dml_like = matches!(
            p.first_word(),
            "SELECT"
                | "WITH"
                | "INSERT"
                | "UPDATE"
                | "DELETE"
                | "MERGE"
                | "REPLACE"
                | "VALUES"
                | "EXPLAIN"
                | ""
        );
        if t.kind != TokenKind::Word || !dml_like {
            continue;
        }
        let next_up = p.toks.get(i + 1).map(|x| x.up.as_str());
        let next_is_end = match p.toks.get(i + 1) {
            None => true,
            Some(nx) => {
                (nx.kind == TokenKind::Punct && matches!(nx.up.as_str(), ")" | ";"))
                    || (nx.kind == TokenKind::Word
                        && CLAUSE_STARTERS.contains(&nx.up.as_str())
                        && nx.up != "JOIN")
            }
        };
        let needs_operand = matches!(t.up.as_str(), "WHERE" | "HAVING" | "AND" | "OR" | "SET" | "JOIN" | "BY" | "FROM" | "ON" | "USING" | "INTO" | "WHEN" | "THEN" | "ELSE")
            // `JOIN` directly followed by a clause keyword other than ON is also an error.
            || (t.up == "NOT" && next_up.is_none());
        if needs_operand && next_is_end {
            // `FROM` followed by `)`: `EXTRACT(x FROM )` is equally broken.
            let what = match t.up.as_str() {
                "FROM" | "JOIN" | "INTO" => "table name",
                "SET" => "assignment",
                "BY" => "column or expression",
                "USING" | "ON" => "join condition",
                _ => "expression",
            };
            let tail = next_up
                .map(|u| format!(" before {u}"))
                .unwrap_or_else(|| " at the end of the statement".into());
            out.push(diag(p.range(i), Severity::Critical, format!("Expected {what} after {}{tail}", t.up)));
        }
    }
    // Statement ends in the middle of an expression.
    if let Some(last) = p.toks.last()
        && last.kind == TokenKind::Operator
        && matches!(
            last.up.as_str(),
            "=" | "<>" | "!=" | "<" | ">" | "<=" | ">=" | "+" | "-" | "/" | "%" | "||" | "->" | "->>" | "::"
        )
    {
        out.push(diag(
            last.start..last.end,
            Severity::Critical,
            format!("Incomplete expression: statement ends after '{}'", last.up),
        ));
    }
    if let Some(i) = (0..n).rev().find(|&i| p.toks[i].kind == TokenKind::Word).filter(|&i| i == n - 1)
        && matches!(p.toks[i].up.as_str(), "AND" | "OR" | "IS" | "IN" | "LIKE" | "ILIKE" | "BETWEEN" | "AS")
    {
        out.push(diag(
            p.range(i),
            Severity::Critical,
            format!("Incomplete expression: statement ends after {}", p.toks[i].up),
        ));
    }
    // IN () is a syntax error in Postgres and MySQL.
    for i in 0..n.saturating_sub(2) {
        if p.is_word(i, "IN")
            && p.is_punct(i + 1, "(")
            && p.is_punct(i + 2, ")")
            && kind.family() != Family::Sqlite
        {
            out.push(diag(
                p.toks[i].start..p.toks[i + 2].end,
                Severity::Critical,
                "IN () needs at least one value",
            ));
        }
    }
    // Postgres requires ON/USING after a non-natural, non-cross JOIN.
    if kind.family().pg_like() {
        join_conditions(p, out);
    }
    // UPDATE without SET, DELETE without FROM (all but MySQL).
    if p.first_word() == "UPDATE" && !(0..n).any(|i| p.is_word(i, "SET")) && n > 1 {
        out.push(diag(p.range(0), Severity::Critical, "UPDATE statement has no SET clause"));
    }
    if p.first_word() == "DELETE"
        && !matches!(kind.family(), Family::MySql | Family::SqlServer)
        && n > 1
        && !p.is_word(1, "FROM")
    {
        out.push(diag(p.range(0), Severity::Critical, "Expected FROM after DELETE"));
    }
}

fn join_conditions(p: &Parsed, out: &mut Vec<Diagnostic>) {
    let n = p.len();
    let mut i = 0;
    while i < n {
        if p.is_punct(i, "(") {
            // Recurse by simply scanning inside as well; nesting is shallow in practice.
            i += 1;
            continue;
        }
        if p.is_word(i, "JOIN") {
            let modifier = (0..i)
                .rev()
                .take(3)
                .map(|k| p.up(k))
                .find(|w| matches!(*w, "CROSS" | "NATURAL" | "LEFT" | "RIGHT" | "FULL" | "INNER" | "OUTER"))
                .unwrap_or("");
            if matches!(modifier, "CROSS" | "NATURAL") {
                i += 1;
                continue;
            }
            // Walk to the next depth-0 JOIN-level keyword; look for ON/USING on the way.
            let mut j = i + 1;
            let mut depth = 0i32;
            let mut found = false;
            while j < n {
                if p.is_punct(j, "(") {
                    depth += 1;
                } else if p.is_punct(j, ")") {
                    depth -= 1;
                    if depth < 0 {
                        break;
                    }
                } else if depth == 0 && p.toks[j].kind == TokenKind::Word {
                    match p.toks[j].up.as_str() {
                        "ON" | "USING" => {
                            found = true;
                            break;
                        }
                        "JOIN" | "WHERE" | "GROUP" | "ORDER" | "HAVING" | "LIMIT" | "UNION" | "INTERSECT"
                        | "EXCEPT" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL"
                        | "RETURNING" | "WINDOW" | "SET" => break,
                        _ => {}
                    }
                } else if depth == 0 && p.is_punct(j, ",") {
                    break;
                }
                j += 1;
            }
            if !found && i + 1 < n {
                out.push(diag(
                    p.range(i),
                    Severity::Critical,
                    "JOIN requires an ON or USING condition (use CROSS JOIN for a cross product)",
                ));
            }
        }
        i += 1;
    }
}

/// UPDATE/DELETE statements (also inside CTEs) without a WHERE clause.
fn dangerous_statements(p: &Parsed, out: &mut Vec<Diagnostic>) {
    let n = p.len();
    for i in 0..n {
        if !(p.is_word(i, "UPDATE") || p.is_word(i, "DELETE")) {
            continue;
        }
        let prev_ok = i == 0
            || p.is_punct(i - 1, "(")
            || p.is_punct(i - 1, ")")
            || (0..i).all(|k| {
                matches!(p.up(k), "EXPLAIN" | "ANALYZE" | "VERBOSE")
                    || p.is_punct(k, "(")
                    || p.is_punct(k, ")")
                    || p.toks[k].kind == TokenKind::Word && matches!(p.up(k), "TRUE" | "FALSE")
            });
        if !prev_ok {
            continue;
        }
        let end = if i > 0 && p.is_punct(i - 1, "(") { p.closing(i - 1) } else { n };
        let mut j = i + 1;
        let mut has_where = false;
        while j < end {
            if p.is_punct(j, "(") {
                j = p.closing(j) + 1;
                continue;
            }
            if p.is_word(j, "WHERE") {
                has_where = true;
                break;
            }
            j += 1;
        }
        if !has_where {
            let (verb, what) =
                if p.is_word(i, "UPDATE") { ("UPDATE", "modify") } else { ("DELETE", "delete") };
            out.push(diag(
                p.range(i),
                Severity::Critical,
                format!("{verb} without WHERE will {what} every row in the table"),
            ));
        }
    }
}

/// Clause in effect at every token, honouring subquery scopes and set operations.
struct ClauseMap {
    clauses: Vec<Clause>,
}

impl ClauseMap {
    fn build(p: &Parsed, catalog: Option<&Catalog>) -> ClauseMap {
        let mut m = ClauseMap { clauses: vec![Clause::None; p.len()] };
        m.fill(p, 0..p.len(), catalog, &[]);
        m
    }

    fn fill(&mut self, p: &Parsed, range: Range<usize>, catalog: Option<&Catalog>, ctes: &[CteDef]) {
        for branch in branches(p, &range) {
            let scan = scan_scope(p, branch.start, branch.end, catalog, ctes);
            let mut i = branch.start;
            while i < branch.end {
                self.clauses[i] = scan.clause_at(i);
                if p.is_subquery_paren(i) {
                    let close = p.closing(i);
                    self.fill(p, i + 1..close, catalog, ctes);
                    if close < branch.end {
                        self.clauses[close] = scan.clause_at(i);
                    }
                    i = close + 1;
                    continue;
                }
                i += 1;
            }
        }
    }
}

/// Split a range at depth-0 set operators.
fn branches(p: &Parsed, range: &Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = range.start;
    let mut i = range.start;
    while i < range.end {
        if p.is_punct(i, "(") {
            i = p.closing(i) + 1;
            continue;
        }
        if matches!(p.up(i), "UNION" | "INTERSECT" | "EXCEPT") {
            out.push(start..i);
            start = i + 1;
            if matches!(p.up(start), "ALL" | "DISTINCT") {
                start += 1;
            }
        }
        i += 1;
    }
    out.push(start..range.end.max(start));
    out
}

fn null_comparisons(p: &Parsed, cmap: &ClauseMap, out: &mut Vec<Diagnostic>) {
    for i in 0..p.len().saturating_sub(1) {
        let t = &p.toks[i];
        if t.kind != TokenKind::Operator
            || !matches!(t.up.as_str(), "=" | "<>" | "!=")
            || !p.is_word(i + 1, "NULL")
        {
            continue;
        }
        if !matches!(
            cmap.clauses[i],
            Clause::Where
                | Clause::Having
                | Clause::On
                | Clause::Join
                | Clause::Select
                | Clause::OrderBy
                | Clause::GroupBy
        ) {
            continue;
        }
        let neg = t.up != "=";
        out.push(diag(
            t.start..p.toks[i + 1].end,
            Severity::Warning,
            if neg {
                "Comparing with NULL using <> is never true; use IS NOT NULL"
            } else {
                "Comparing with NULL using = is never true; use IS NULL"
            },
        ));
    }
}

struct Walker<'a, 'o> {
    p: &'a Parsed<'a>,
    kind: DatabaseKind,
    catalog: Option<&'a Catalog>,
    created: &'a HashSet<String>,
    cmap: &'a ClauseMap,
    out: &'o mut Vec<Diagnostic>,
    ctes: Vec<CteDef>,
}

fn is_system_name(schema: Option<&str>, name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    schema.is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "information_schema" | "pg_catalog" | "mysql" | "performance_schema" | "sys"
        )
    }) || n.starts_with("pg_")
        || n.starts_with("sqlite_")
        || n == "dual"
        || n == "information_schema"
}

fn table_of<'c>(cat: Option<&'c Catalog>, s: &Source) -> Option<&'c Table> {
    cat?.find_table(s.schema.as_deref(), &s.name).map(|(_, t)| t)
}

impl Walker<'_, '_> {
    fn scope(&mut self, range: Range<usize>, outer: &[Source]) {
        for branch in branches(self.p, &range) {
            let scan = scan_scope(self.p, branch.start, branch.end, self.catalog, &self.ctes);
            self.check_sources(&scan);
            self.check_cross_join(&scan);
            self.check_columns(&branch, &scan, outer);
            // Recurse into subqueries directly inside this branch.
            let mut visible: Vec<Source> = scan.sources.clone();
            visible.extend(outer.iter().cloned());
            let mut i = branch.start;
            while i < branch.end {
                if self.p.is_punct(i, "(") {
                    let close = self.p.closing(i);
                    if self.p.is_subquery_paren(i) {
                        self.scope(i + 1..close, &visible);
                    } else {
                        self.scope_nested_parens(i + 1..close, &visible);
                    }
                    i = close + 1;
                    continue;
                }
                i += 1;
            }
        }
    }

    /// Subqueries nested inside ordinary parentheses (`IN ((select ...))`, `f((select ..))`).
    fn scope_nested_parens(&mut self, range: Range<usize>, visible: &[Source]) {
        let mut i = range.start;
        while i < range.end {
            if self.p.is_punct(i, "(") {
                let close = self.p.closing(i);
                if self.p.is_subquery_paren(i) {
                    self.scope(i + 1..close, visible);
                } else {
                    self.scope_nested_parens(i + 1..close, visible);
                }
                i = close + 1;
                continue;
            }
            i += 1;
        }
    }

    fn check_sources(&mut self, scan: &ScopeScan) {
        let Some(cat) = self.catalog else { return };
        for s in &scan.sources {
            if s.kind != SourceKind::Table {
                continue;
            }
            if is_system_name(s.schema.as_deref(), &s.name) || self.created.contains(&s.name.to_lowercase()) {
                continue;
            }
            let range = self.p.range(s.name_tok);
            match s.schema.as_deref() {
                Some(sch) => {
                    // An unknown schema is more likely a partially loaded catalog than a typo.
                    if cat.schema(sch).is_some() && cat.find_table(Some(sch), &s.name).is_none() {
                        self.out.push(diag(
                            range,
                            Severity::Warning,
                            format!("Unknown table '{}.{}'", sch, s.name),
                        ));
                    }
                }
                None => {
                    if cat.find_table(None, &s.name).is_none() {
                        self.out.push(diag(range, Severity::Warning, format!("Unknown table '{}'", s.name)));
                    }
                }
            }
        }
    }

    fn check_cross_join(&mut self, scan: &ScopeScan) {
        if scan.stmt == StmtKind::Delete {
            return;
        }
        let from: Vec<&Source> = scan.sources.iter().filter(|s| s.clause == Clause::From).collect();
        if from.len() < 2 {
            return;
        }
        let has_where = scan.clause_starts.iter().any(|(_, c)| *c == Clause::Where);
        if has_where {
            return;
        }
        // Table functions and LATERAL subqueries legitimately correlate without a predicate.
        let lateral = from.iter().any(|s| s.tok_start > 0 && self.p.is_word(s.tok_start, "LATERAL"));
        if lateral || from.iter().any(|s| matches!(s.kind, SourceKind::Function(_))) {
            return;
        }
        // Only comma-separated neighbours count; sources joined with JOIN carry their own ON.
        let comma_joined: Vec<&&Source> =
            from.iter().filter(|s| s.tok_start > 0 && self.p.is_punct(s.tok_start - 1, ",")).collect();
        if comma_joined.is_empty() {
            return;
        }
        let first = from[0];
        let last = from[from.len() - 1];
        let names: Vec<String> = from
            .iter()
            .map(|s| match (&s.alias, s.name.is_empty()) {
                (Some(a), _) => a.clone(),
                (None, true) => "(subquery)".to_string(),
                (None, false) => s.name.clone(),
            })
            .collect();
        let start = self.p.toks[first.tok_start].start;
        let end = self.p.toks[last.tok_end.saturating_sub(1).max(last.tok_start)].end;
        self.out.push(diag(
            start..end,
            Severity::Warning,
            format!(
                "Implicit cross join of {} without a join predicate: every row is paired with every row",
                names.join(", ")
            ),
        ));
    }

    fn resolve<'s>(&self, q: &str, own: &'s [Source], outer: &'s [Source]) -> Option<&'s Source> {
        own.iter().chain(outer.iter()).find(|s| match &s.alias {
            Some(a) => a.eq_ignore_ascii_case(q),
            None => s.name.eq_ignore_ascii_case(q),
        })
    }

    fn check_columns(&mut self, branch: &Range<usize>, scan: &ScopeScan, outer: &[Source]) {
        if self.catalog.is_none() {
            return;
        }
        let p = self.p;
        // Token ranges that belong to a table factor (`public.customers c`) are not column references.
        let in_factor = |i: usize| scan.sources.iter().any(|s| i >= s.tok_start && i < s.tok_end);
        let mut i = branch.start;
        while i < branch.end {
            if p.is_punct(i, "(") && p.is_subquery_paren(i) {
                i = p.closing(i) + 1;
                continue;
            }
            // Qualified column: `q.col`
            if p.is_ident(i)
                && p.is_punct(i + 1, ".")
                && i + 2 < branch.end
                && p.is_ident(i + 2)
                && !(i > 0 && p.is_punct(i - 1, "."))
                && !in_factor(i)
                && !p.is_punct(i + 3, "(")
                && !p.is_punct(i + 3, ".")
            {
                let q = p.ident(i);
                if let Some(src) = self.resolve(&q, &scan.sources, outer)
                    && src.kind == SourceKind::Table
                    && let Some(t) = table_of(self.catalog, src)
                {
                    let col = p.ident(i + 2);
                    if !t.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&col)) && !is_pseudo_column(&col)
                    {
                        self.out.push(diag(
                            p.range(i + 2),
                            Severity::Warning,
                            format!("Column '{col}' not found in table '{}'", t.name),
                        ));
                    }
                }
                i += 3;
                continue;
            }
            i += 1;
        }
        self.check_unqualified(branch, scan, outer, &in_factor);
        self.check_insert_columns(scan);
    }

    fn check_insert_columns(&mut self, scan: &ScopeScan) {
        let p = self.p;
        for s in scan.sources.iter().filter(|s| s.clause == Clause::Into && s.target) {
            let open = s.tok_end;
            if !p.is_punct(open, "(") {
                continue;
            }
            let Some(t) = table_of(self.catalog, s) else { continue };
            let close = p.closing(open);
            let mut j = open + 1;
            while j < close {
                if p.is_ident(j)
                    && (p.is_punct(j + 1, ",") || j + 1 == close)
                    && (j == open + 1 || p.is_punct(j - 1, ","))
                {
                    let col = p.ident(j);
                    if !t.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&col)) {
                        self.out.push(diag(
                            p.range(j),
                            Severity::Warning,
                            format!("Column '{col}' not found in table '{}'", t.name),
                        ));
                    }
                }
                j += 1;
            }
        }
    }

    fn check_unqualified(
        &mut self,
        branch: &Range<usize>,
        scan: &ScopeScan,
        outer: &[Source],
        in_factor: &dyn Fn(usize) -> bool,
    ) {
        let p = self.p;
        // Only when every visible source is a known catalog table can "no such column" be certain.
        let mut tables: Vec<&Table> = Vec::new();
        for s in scan.sources.iter().chain(outer.iter()) {
            match (&s.kind, table_of(self.catalog, s)) {
                (SourceKind::Table, Some(t)) => tables.push(t),
                _ => return,
            }
        }
        if tables.is_empty() || scan.stmt == StmtKind::Other {
            return;
        }
        let aliases: HashSet<String> = select_aliases(p, branch.start, branch.end, self.catalog, &self.ctes)
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect();
        // Words following AS anywhere are aliases / type names.
        let mut i = branch.start;
        while i < branch.end {
            if p.is_punct(i, "(") && p.is_subquery_paren(i) {
                i = p.closing(i) + 1;
                continue;
            }
            let clause = self.cmap.clauses[i];
            let checkable = matches!(
                clause,
                Clause::Where
                    | Clause::Having
                    | Clause::GroupBy
                    | Clause::OrderBy
                    | Clause::Select
                    | Clause::On
                    | Clause::Set
                    | Clause::Returning
            );
            let t = &p.toks[i];
            let is_word = matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent);
            if checkable && is_word && !in_factor(i) {
                let prev_blocks = i > 0
                    && (p.is_punct(i - 1, ".")
                        || p.is_op(i - 1, "::")
                        || p.is_word(i - 1, "AS")
                        || p.is_word(i - 1, "INTERVAL")
                        || p.is_word(i - 1, "ON") && p.is_word(i, "CONFLICT"));
                let next_blocks = p.is_punct(i + 1, "(") || p.is_punct(i + 1, ".");
                let name = p.ident(i);
                let lname = name.to_lowercase();
                let reserved = t.kind == TokenKind::Word
                    && (is_keyword_any(&name, self.kind) || is_known_function(&name) || is_type_name(&name));
                // `SET col = ...` is only a column at assignment position; elsewhere defer to the generic rule.
                if !prev_blocks
                    && !next_blocks
                    && !reserved
                    && !is_pseudo_column(&name)
                    && !aliases.contains(&lname)
                    && !matches!(lname.as_str(), "excluded" | "new" | "old")
                    && !self.is_lambda_or_special(i)
                    && !tables.iter().any(|tb| tb.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&name)))
                {
                    // OVER (PARTITION BY ...) window names and named windows are not columns.
                    if !(p.is_word(i.wrapping_sub(1), "WINDOW") || p.is_word(i.wrapping_sub(1), "OVER")) {
                        self.out.push(diag(
                            p.range(i),
                            Severity::Warning,
                            format!("Unknown column '{name}'"),
                        ));
                    }
                }
            }
            i += 1;
        }
    }

    fn is_lambda_or_special(&self, i: usize) -> bool {
        // Inside EXTRACT(field FROM ...) / TRIM(BOTH ...) / SUBSTRING(... FROM ... FOR ...) the
        // first word is a keyword-like field name; also skip `AT TIME ZONE x`.
        let p = self.p;
        i > 0
            && (p.is_word(i - 1, "ZONE")
                || p.is_word(i - 1, "ESCAPE")
                || p.is_word(i - 1, "COLLATE")
                || p.is_word(i - 1, "OF")
                || p.is_word(i - 1, "NAME"))
    }
}

fn is_pseudo_column(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "ctid"
            | "oid"
            | "xmin"
            | "xmax"
            | "cmin"
            | "cmax"
            | "tableoid"
            | "rowid"
            | "_rowid_"
            | "level"
            | "rownum"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use DatabaseKind::*;

    fn run(sql: &str, kind: DatabaseKind) -> Vec<Diagnostic> {
        lint(sql, kind, None)
    }

    fn with_cat(sql: &str) -> Vec<Diagnostic> {
        lint(sql, Postgres, Some(&fixtures::shop()))
    }

    fn msgs(d: &[Diagnostic]) -> Vec<String> {
        d.iter().map(|d| d.message.clone()).collect()
    }

    fn snippet<'a>(sql: &'a str, d: &Diagnostic) -> &'a str {
        &sql[d.range.clone()]
    }

    #[test]
    fn valid_corpus_has_no_diagnostics() {
        let pg = [
            "select 1",
            "select c.id, count(*) as n from customers c left join orders o on o.customer_id = c.id where c.tier = 'gold' group by c.id having count(*) > 1 order by n desc limit 10",
            "with recent as (select id, total from orders where placed_at > now() - interval '7 days') select r.id from recent r where r.total > 10",
            "insert into customers (id, email) values (1, 'a@b.c') on conflict (id) do update set email = excluded.email returning id",
            "update orders set status = 'paid' where id = 7",
            "delete from orders where id in (select order_id from order_items where quantity = 0)",
            "select status->>'k', total::int, extract(year from placed_at) from orders",
            "select * from orders o join customers c using (id)",
            "select a, sum(b) over (partition by a order by c rows between unbounded preceding and current row) from t",
            "create function f() returns int as $$ begin update foo set a = 1; return 1; end; $$ language plpgsql",
            "select case when total > 10 then 'big' else 'small' end from orders",
            "select * from generate_series(1, 3) g, orders",
            "select * from orders o, lateral (select 1) x",
            "select count(*) filter (where status = 'x') from orders",
            "explain analyze select * from orders where id = 1",
            "select * from orders for update",
            "select e.* from (values (1, 2)) as e(a, b)",
            "select ctid, * from orders",
            "alter table orders add column note text",
            "create index idx on orders (customer_id)",
            "select * from orders where placed_at between '2024-01-01' and '2024-02-01'",
            "select distinct on (customer_id) customer_id from orders order by customer_id, placed_at desc",
            "with d as (delete from orders where id = 1 returning *) select * from d",
        ];
        for sql in pg {
            assert!(run(sql, Postgres).is_empty(), "{sql}: {:?}", run(sql, Postgres));
            // Statements over placeholder tables (`t`, `a`) cannot be checked against the shop catalog.
            if !sql.contains(" t") && !sql.contains("from a") && !sql.contains("generate_series") {
                let d = with_cat(sql);
                assert!(d.is_empty(), "{sql}: {:?}", msgs(&d));
            }
        }
        let mysql = [
            "select `order`.id from `order` where name = 'x'",
            "select ifnull(a, 0), date_format(now(), '%Y') from t limit 5, 10",
            "insert into t (a, b) values (1, 2) on duplicate key update b = values(b)",
            "update t join u on t.id = u.id set t.a = u.a where u.b = 1",
            "delete t from t join u on t.id = u.id where u.x = 1",
            "select * from t where a = \"x\" # comment",
            "create procedure p() begin update t set a = 1; select 1; end",
            "select group_concat(a separator ',') from t",
        ];
        for sql in mysql {
            assert!(run(sql, MySql).is_empty(), "{sql}: {:?}", run(sql, MySql));
        }
        let sqlite = [
            "select * from t where a = ?1 and b = :name",
            "insert or replace into t (a) values (1)",
            "select [my col] from t",
            "pragma foreign_keys = on",
            "create trigger tr after insert on t begin update u set a = 1; end",
            "select strftime('%Y', d) from t where x glob 'a*'",
            "select * from t join u on t.a = u.a",
        ];
        for sql in sqlite {
            assert!(run(sql, Sqlite).is_empty(), "{sql}: {:?}", run(sql, Sqlite));
        }
    }

    #[test]
    fn unterminated_literals_and_comments() {
        let sql = "select 'abc from t";
        let d = run(sql, Postgres);
        assert!(
            d.iter().any(|d| d.message.contains("Unterminated string") && snippet(sql, d).starts_with('\''))
        );
        assert!(run("select 1 /* open", Postgres).iter().any(|d| d.message.contains("block comment")));
        assert!(run("select \"abc", Postgres).iter().any(|d| d.message.contains("quoted identifier")));
        assert!(run("/* only a comment", Postgres).iter().any(|d| d.message.contains("block comment")));
    }

    #[test]
    fn parentheses_are_balanced() {
        let sql = "select (1 + 2)) from t";
        let d = run(sql, Postgres);
        assert!(d.iter().any(|d| d.message == "Unmatched ')'"
            && d.range == (sql.rfind(')').unwrap()..sql.rfind(')').unwrap() + 1)));
        let sql = "select count(a from t";
        let d = run(sql, Postgres);
        assert!(d.iter().any(|d| d.message.starts_with("Unclosed '('") && snippet(sql, d) == "("));
    }

    #[test]
    fn commas() {
        let sql = "select a, b, from t";
        let d = run(sql, Postgres);
        assert!(d.iter().any(|d| d.message == "Unexpected comma before FROM" && snippet(sql, d) == ","));
        assert!(run("select a,, b from t", Postgres).iter().any(|d| d.message == "Double comma"));
        assert!(
            run("insert into t (a, b,) values (1, 2)", Postgres)
                .iter()
                .any(|d| d.message.contains("before ')'"))
        );
        assert!(run("select a,", Postgres).iter().any(|d| d.message.contains("Trailing comma")));
        assert!(run("select limit 5, 10 from t", MySql).iter().all(|d| !d.message.contains("comma")));
    }

    #[test]
    fn missing_operands() {
        let d = run("select * from t where", Postgres);
        assert!(d.iter().any(|d| d.message.starts_with("Expected expression after WHERE")), "{d:?}");
        let d = run("select * from where a = 1", Postgres);
        assert!(d.iter().any(|d| d.message.starts_with("Expected table name after FROM before WHERE")));
        let d = run("select * from t where a = 1 and", Postgres);
        assert!(
            d.iter()
                .any(|d| d.message.contains("after AND") || d.message.contains("statement ends after AND"))
        );
        assert!(
            run("select * from t where a =", Postgres).iter().any(|d| d.message.contains("ends after '='"))
        );
        assert!(run("select * from t order by", Postgres).iter().any(|d| d.message.contains("after BY")));
        assert!(run("select * from t join", Postgres).iter().any(|d| d.message.contains("after JOIN")));
    }

    #[test]
    fn unknown_statement_keyword() {
        let sql = "selec * from t";
        let d = run(sql, Postgres);
        assert_eq!(d.len(), 1);
        assert_eq!(snippet(sql, &d[0]), "selec");
        assert_eq!(d[0].severity, Severity::Critical);
        assert!(run("42 + 1", Postgres)[0].message.contains("Unexpected '42'"));
        assert!(
            run("show tables; use x; pragma x; analyze; vacuum; call p()", MySql)
                .iter()
                .all(|d| !d.message.contains("Unknown statement"))
        );
    }

    #[test]
    fn update_and_delete_without_where() {
        let sql = "update orders set status = 'x'";
        let d = run(sql, Postgres);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Critical);
        assert_eq!(snippet(sql, &d[0]), "update");
        assert!(d[0].message.contains("every row"));
        let d = run("delete from orders", Postgres);
        assert!(d[0].message.starts_with("DELETE without WHERE"));
        assert!(run("delete from orders where id = 1", Postgres).is_empty());
        assert!(run("update t set a = 1 where b in (select b from u)", Postgres).is_empty());
        // In a CTE.
        let d = run("with x as (delete from orders returning *) select * from x", Postgres);
        assert!(d.iter().any(|d| d.message.starts_with("DELETE without WHERE")));
        // `ON CONFLICT DO UPDATE` and `FOR UPDATE` are not UPDATE statements.
        assert!(run("insert into t values (1) on conflict do update set a = 2", Postgres).is_empty());
        assert!(run("select * from t for update", Postgres).is_empty());
        // Two statements: only the dangerous one is flagged, at its own offset.
        let sql = "select 1;\ndelete from t;";
        let d = run(sql, Postgres);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].range.start, sql.find("delete").unwrap());
    }

    #[test]
    fn update_without_set_and_delete_without_from() {
        assert!(
            run("update t", Postgres).is_empty()
                || run("update t", Postgres).iter().all(|d| d.severity == Severity::Critical)
        );
        assert!(run("update t where a = 1", Postgres).iter().any(|d| d.message.contains("no SET")));
        assert!(run("delete t where a = 1", Postgres).iter().any(|d| d.message.contains("Expected FROM")));
        assert!(run("delete t from t join u on t.a = u.a where u.b = 1", MySql).is_empty());
    }

    #[test]
    fn equals_null_comparison() {
        let sql = "select * from t where a = null";
        let d = run(sql, Postgres);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(snippet(sql, &d[0]), "= null");
        assert!(d[0].message.contains("IS NULL"));
        assert!(run("select * from t where a <> null", Postgres)[0].message.contains("IS NOT NULL"));
        assert!(run("select * from t where a != NULL", Postgres)[0].message.contains("IS NOT NULL"));
        // Assignments and defaults are fine.
        assert!(run("update t set a = null where b = 1", Postgres).is_empty());
        assert!(run("insert into t (a) values (null)", Postgres).is_empty());
        assert!(run("create table t (a int default null)", Postgres).is_empty());
        assert!(run("select * from t where a is null", Postgres).is_empty());
        // Inside a subquery.
        assert_eq!(run("select * from t where x in (select y from u where z = null)", Postgres).len(), 1);
    }

    #[test]
    fn implicit_cross_join() {
        let sql = "select * from customers, orders";
        let d = run(sql, Postgres);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(snippet(sql, &d[0]), "customers, orders");
        assert!(d[0].message.contains("Implicit cross join"));
        assert!(run("select * from customers c, orders o where c.id = o.customer_id", Postgres).is_empty());
        assert!(
            run("select * from customers join orders on true, stores", Postgres)
                .iter()
                .any(|d| d.message.contains("Implicit"))
        );
        assert!(run("select * from t, unnest(t.arr) x", Postgres).is_empty());
        assert!(run("select * from a, (select 1) s where true", Postgres).is_empty());
        let d = run("select * from a, (select 1) s", Postgres);
        assert!(d[0].message.contains("a, s"));
    }

    #[test]
    fn join_without_condition_in_postgres() {
        let sql = "select * from a join b where a.x = 1";
        let d = run(sql, Postgres);
        assert!(d.iter().any(|d| d.message.starts_with("JOIN requires")), "{d:?}");
        assert!(run("select * from a cross join b", Postgres).is_empty());
        assert!(run("select * from a natural join b", Postgres).is_empty());
        assert!(run("select * from a left join b on a.x = b.x join c using (y)", Postgres).is_empty());
        assert!(run("select * from a join b", MySql).is_empty());
    }

    #[test]
    fn in_empty_list() {
        assert!(run("select * from t where a in ()", Postgres).iter().any(|d| d.message.contains("IN ()")));
        assert!(run("select * from t where a in ()", Sqlite).is_empty());
    }

    #[test]
    fn unknown_tables_with_catalog() {
        let sql = "select * from custmers c";
        let d = with_cat(sql);
        assert_eq!(d.len(), 1);
        assert_eq!(snippet(sql, &d[0]), "custmers");
        assert_eq!(d[0].message, "Unknown table 'custmers'");
        assert!(with_cat("select * from analytics.events").is_empty());
        let d = with_cat("select * from analytics.eventz");
        assert_eq!(d[0].message, "Unknown table 'analytics.eventz'");
        // System catalogs, unknown schemas, CTEs and tables created in the script are not flagged.
        assert!(with_cat("select * from pg_catalog.pg_class").is_empty());
        assert!(with_cat("select * from information_schema.tables").is_empty());
        assert!(with_cat("select * from other_schema.t").is_empty());
        assert!(with_cat("with x as (select 1 as a) select a from x").is_empty());
        assert!(with_cat("create temp table scratch (a int); select * from scratch").is_empty());
        assert!(run("select * from custmers", Postgres).is_empty(), "no catalog, no complaints");
    }

    #[test]
    fn unknown_columns_qualified() {
        let sql = "select c.emial from customers c";
        let d = with_cat(sql);
        assert_eq!(d.len(), 1);
        assert_eq!(snippet(sql, &d[0]), "emial");
        assert_eq!(d[0].message, "Column 'emial' not found in table 'customers'");
        assert!(with_cat("select c.* , c.id from customers c").is_empty());
        // Correlated reference to an outer alias.
        assert!(
            with_cat(
                "select * from customers c where exists (select 1 from orders o where o.customer_id = c.id)"
            )
            .is_empty()
        );
        let d = with_cat(
            "select * from customers c where exists (select 1 from orders o where o.customer_id = c.idd)",
        );
        assert_eq!(d.len(), 1);
        // Unknown qualifier: stay quiet.
        assert!(with_cat("select zz.a from customers c").is_empty());
    }

    #[test]
    fn unknown_columns_unqualified() {
        let sql = "select * from customers where emial = 'x'";
        let d = with_cat(sql);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "Unknown column 'emial'");
        assert!(with_cat("select email, upper(name) n from customers where id > 1 order by n").is_empty());
        assert!(with_cat("select email as e from customers order by e").is_empty());
        assert!(
            with_cat("select cast(id as text), id::bigint, current_date, now() from customers").is_empty()
        );
        assert!(with_cat("select extract(year from created_at) from customers").is_empty());
        // Ambiguous scope (two tables) is not checked unqualified unless all columns are known... it is, but both known.
        assert!(
            with_cat("select email, total from customers c join orders o on o.customer_id = c.id").is_empty()
        );
        let d = with_cat("select email, bogus from customers c join orders o on o.customer_id = c.id");
        assert_eq!(d.len(), 1);
        // Unknown source in scope disables the check.
        assert!(with_cat("select zzz from customers, generate_series(1,2) g").is_empty());
    }

    #[test]
    fn update_set_unknown_column_and_insert_columns() {
        let d = with_cat("update customers set emial = 'x' where id = 1");
        assert!(d.iter().any(|d| d.message == "Unknown column 'emial'"), "{d:?}");
        let d = with_cat("insert into customers (id, emial) values (1, 'x')");
        assert!(d.iter().any(|d| d.message.contains("'emial' not found in table 'customers'")));
        assert!(with_cat("insert into customers (id, email) values (1, 'x')").is_empty());
    }

    #[test]
    fn routine_bodies_are_not_linted_for_structure() {
        let sql = "create procedure p() begin update t set a = 1; end";
        assert!(run(sql, MySql).is_empty());
        let d = run(
            "create function f() returns int as $$ begin delete from t; end $$ language plpgsql",
            Postgres,
        );
        assert!(d.is_empty());
    }

    #[test]
    fn diagnostics_are_sorted_and_deduplicated() {
        let d = run("select a,, b from t where x = null; update t set a = 1", Postgres);
        assert!(d.windows(2).all(|w| w[0].range.start <= w[1].range.start));
        assert!(d.len() >= 3);
    }

    #[test]
    fn lint_never_panics_on_prefixes() {
        let sql = "select c.id, (select max(o.total) from orders o where o.customer_id = c.id) from customers c join orders o on o.customer_id = c.id where c.tier = 'x' group by c.id order by 1;";
        let cat = fixtures::shop();
        for end in 0..=sql.len() {
            let _ = lint(&sql[..end], Postgres, Some(&cat));
        }
    }
}
