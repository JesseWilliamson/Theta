//! The resolver: one walk over each statement with a scope stack.

use super::names::{self, Ident, find_all, strict_eq};
use super::*;
use crate::lexer::{TokenKind, unquote_ident};
use crate::lint::Severity;
use crate::syntax::{Kind, Node, parse};
use std::collections::HashMap;
use theta_core::{Catalog, DatabaseKind, Family, Table};

/// Parse and resolve a script. `catalog` is the connection's schema; `None` (or an empty catalog) means
/// "unknown": scoping inside the script still works, but no name is ever reported as unknown.
pub fn analyze(sql: &str, kind: DatabaseKind, catalog: Option<&Catalog>) -> Analysis {
    let parsed = parse(sql, kind);
    let catalog = catalog.filter(|c| c.schemas.iter().any(|s| !s.tables.is_empty()));
    let result = {
        let mut r = Resolver::new(sql, kind, catalog);
        r.collect_script_tables(&parsed.root);
        for stmt in parsed.root.children_of(Kind::Statement) {
            r.statement(stmt);
        }
        r.finish()
    };
    Analysis {
        parse: parsed,
        symbols: result.symbols,
        refs: result.refs,
        scopes: result.scopes,
        stars: result.stars,
        diagnostics: result.diagnostics,
    }
}

struct Output {
    symbols: Vec<Symbol>,
    refs: Vec<Ref>,
    scopes: Vec<ScopeInfo>,
    stars: Vec<StarExpansion>,
    diagnostics: Vec<Diag>,
}

/// A table the script itself creates.
struct ScriptTable {
    schema: Option<String>,
    name: String,
    cols: Option<Vec<String>>,
}

/// What a query exposes.
#[derive(Clone, Default)]
struct QOut {
    cols: Vec<Col>,
    /// Every column has a usable name.
    complete: bool,
    /// The number of columns is exact (no `*` over an unknown source).
    counted: bool,
    /// The scope of the select that produced it (for ORDER BY).
    scope: Option<ScopeId>,
}

impl QOut {
    fn unknown() -> QOut {
        QOut { cols: Vec::new(), complete: false, counted: false, scope: None }
    }
}

enum TableLookup<'a> {
    Catalog {
        schema: &'a str,
        table: &'a Table,
    },
    Script(usize),
    /// Certainly not there.
    Missing,
    /// Cannot tell.
    Unknown,
}

enum ColFound {
    One(Resolution),
    Many(Vec<String>),
    None,
    Unknown,
}

struct Resolver<'a> {
    sql: &'a str,
    fam: Family,
    cat: Option<&'a Catalog>,
    /// lower-cased table name -> (schema index, table index)
    index: HashMap<String, Vec<(usize, usize)>>,
    script_tables: Vec<ScriptTable>,
    symbols: Vec<Symbol>,
    refs: Vec<Ref>,
    scopes: Vec<ScopeInfo>,
    stars: Vec<StarExpansion>,
    diagnostics: Vec<Diag>,
    /// Columns of each CTE (and whether they are all named).
    cte_cols: HashMap<SymbolId, (Vec<Col>, bool, bool)>,
    /// While positive, "not found" is "unknown": the text around is not understood well enough to say.
    lenient: u32,
    /// span start of a reference -> index in `refs` (before sorting)
    ref_index: HashMap<usize, usize>,
}

fn contains_kind(n: &Node, kind: Kind) -> bool {
    n.kind == kind || n.children.iter().any(|c| !c.kind.is_token() && contains_kind(c, kind))
}

impl<'a> Resolver<'a> {
    fn new(sql: &'a str, kind: DatabaseKind, cat: Option<&'a Catalog>) -> Self {
        let mut index: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        if let Some(c) = cat {
            for (si, s) in c.schemas.iter().enumerate() {
                for (ti, t) in s.tables.iter().enumerate() {
                    index.entry(t.name.to_lowercase()).or_default().push((si, ti));
                }
            }
        }
        Resolver {
            sql,
            fam: kind.family(),
            cat,
            index,
            script_tables: Vec::new(),
            symbols: Vec::new(),
            refs: Vec::new(),
            scopes: Vec::new(),
            stars: Vec::new(),
            diagnostics: Vec::new(),
            cte_cols: HashMap::new(),
            lenient: 0,
            ref_index: HashMap::new(),
        }
    }

    fn finish(mut self) -> Output {
        self.refs.sort_by_key(|r| (r.span.start, r.span.end));
        self.diagnostics.sort_by_key(|d| (d.range.start, d.range.end));
        self.diagnostics.dedup();
        Output {
            symbols: self.symbols,
            refs: self.refs,
            scopes: self.scopes,
            stars: self.stars,
            diagnostics: self.diagnostics,
        }
    }

    // ---- identifiers

    fn ident_of(&self, n: &Node) -> Ident {
        let text = n.text(self.sql);
        let quoted = matches!(n.kind, Kind::Token(TokenKind::QuotedIdent));
        Ident { text: unquote_ident(text), span: n.span.clone(), quoted }
    }

    /// The identifier parts of a `Name` / `ColumnRef` / `Star` node, in order. A T-SQL `#tmp` is one part.
    fn parts(&self, n: &Node) -> Vec<Ident> {
        let mut out = Vec::new();
        let toks: Vec<&Node> = n.tokens().collect();
        let mut i = 0;
        while i < toks.len() {
            let t = toks[i];
            match t.kind {
                Kind::Token(TokenKind::Word | TokenKind::QuotedIdent) => out.push(self.ident_of(t)),
                Kind::Token(TokenKind::Operator) if matches!(t.text(self.sql), "#" | "##") => {
                    if let Some(nx) = toks.get(i + 1)
                        && nx.span.start == t.span.end
                    {
                        let mut id = self.ident_of(nx);
                        id.text = format!("{}{}", t.text(self.sql), id.text);
                        id.span = t.span.start..nx.span.end;
                        out.push(id);
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        out
    }

    /// The name stored for an identifier introduced by the script: PostgreSQL folds unquoted names.
    fn canon(&self, id: &Ident) -> String {
        if self.fam == Family::Postgres && !id.quoted { id.text.to_lowercase() } else { id.text.clone() }
    }

    // ---- model helpers

    fn new_scope(&mut self, kind: ScopeKind, span: Range<usize>, parent: Option<ScopeId>) -> ScopeId {
        self.scopes.push(ScopeInfo {
            kind,
            span,
            parent,
            sources: Vec::new(),
            ctes: Vec::new(),
            outputs: Vec::new(),
        });
        self.scopes.len() - 1
    }

    fn new_symbol(
        &mut self,
        kind: SymbolKind,
        id: &Ident,
        span: Range<usize>,
        renamable: bool,
        scope: ScopeId,
    ) -> SymbolId {
        self.symbols.push(Symbol { kind, name: id.text.clone(), span, quoted: id.quoted, renamable, scope });
        self.symbols.len() - 1
    }

    fn add_ref(&mut self, id: &Ident, role: RefRole, resolution: Resolution, scope: ScopeId) {
        self.ref_index.insert(id.span.start, self.refs.len());
        self.refs.push(Ref {
            span: id.span.clone(),
            role,
            name: id.text.clone(),
            quoted: id.quoted,
            resolution,
            scope,
        });
    }

    fn diag(&mut self, range: Range<usize>, message: String, kind: DiagKind) {
        if self.lenient > 0 {
            return;
        }
        self.diagnostics.push(Diag { range, severity: Severity::Warning, message, kind });
    }

    // ---- the catalog

    fn default_schema(&self) -> Option<&'a str> {
        let c = self.cat?;
        c.default_schema.as_deref()
    }

    fn script_table_cols(&self, i: usize) -> Option<Vec<Col>> {
        self.script_tables[i]
            .cols
            .as_ref()
            .map(|v| v.iter().map(|n| Col { name: n.clone(), symbol: None, quoted: false }).collect())
    }

    fn find_table(&self, parts: &[Ident]) -> TableLookup<'a> {
        let Some(name) = parts.last() else { return TableLookup::Unknown };
        let schema = if parts.len() == 2 { parts.first() } else { None };
        if parts.len() > 2 {
            return TableLookup::Unknown;
        }
        // Tables the script creates shadow nothing and are never "unknown".
        let scripted: Vec<usize> = self
            .script_tables
            .iter()
            .enumerate()
            .filter(|(_, t)| strict_eq(self.fam, name, &t.name) || names::loose_eq(&name.text, &t.name))
            .filter(|(_, t)| match (schema, &t.schema) {
                (Some(s), Some(ts)) => names::loose_eq(&s.text, ts),
                _ => true,
            })
            .map(|(i, _)| i)
            .collect();
        if let Some(&i) = scripted.first() {
            return TableLookup::Script(i);
        }
        let Some(cat) = self.cat else { return TableLookup::Unknown };
        if names::is_system_table(self.fam, &name.text) {
            return TableLookup::Unknown;
        }
        match schema {
            Some(s) => {
                let sidx = find_all(self.fam, s, &cat.schemas, |x| x.name.as_str());
                // An unknown schema is more likely a partially loaded catalog than a typo: no verdict.
                let Some(&si) = sidx.first() else { return TableLookup::Unknown };
                let sch = &cat.schemas[si];
                let ti = find_all(self.fam, name, &sch.tables, |t| t.name.as_str());
                match ti.first() {
                    Some(&ti) => TableLookup::Catalog { schema: &sch.name, table: &sch.tables[ti] },
                    None => TableLookup::Missing,
                }
            }
            None => {
                // The default schema first, then any schema (a search path with several entries).
                let default = self.default_schema();
                let mut best: Option<(usize, usize)> = None;
                if let Some(cands) = self.index.get(&name.text.to_lowercase()) {
                    let strict: Vec<&(usize, usize)> = cands
                        .iter()
                        .filter(|(si, ti)| strict_eq(self.fam, name, &cat.schemas[*si].tables[*ti].name))
                        .collect();
                    let pool: Vec<&(usize, usize)> =
                        if strict.is_empty() { cands.iter().collect() } else { strict };
                    best = pool
                        .iter()
                        .find(|(si, _)| Some(cat.schemas[*si].name.as_str()) == default)
                        .or(pool.first())
                        .map(|p| **p);
                }
                match best {
                    Some((si, ti)) => TableLookup::Catalog {
                        schema: &cat.schemas[si].name,
                        table: &cat.schemas[si].tables[ti],
                    },
                    None => TableLookup::Missing,
                }
            }
        }
    }

    fn table_id(&self, schema: &str, table: &Table) -> TableId {
        TableId { schema: Some(schema.to_string()), name: table.name.clone(), script: false }
    }

    fn catalog_cols(table: &Table) -> Vec<Col> {
        table.columns.iter().map(|c| Col { name: c.name.clone(), symbol: None, quoted: false }).collect()
    }

    /// Names to offer in "did you mean" for a table.
    fn table_candidates(&self, sc: ScopeId) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut cur = Some(sc);
        while let Some(s) = cur {
            for &c in &self.scopes[s].ctes {
                out.push(self.symbols[c].name.clone());
            }
            cur = self.scopes[s].parent;
        }
        for t in &self.script_tables {
            out.push(t.name.clone());
        }
        if let Some(cat) = self.cat {
            let default = self.default_schema();
            for s in &cat.schemas {
                if Some(s.name.as_str()) == default || cat.schemas.len() == 1 {
                    out.extend(s.tables.iter().map(|t| t.name.clone()));
                }
            }
        }
        out.truncate(1000);
        out
    }

    // ---- script-created tables

    fn collect_script_tables(&mut self, root: &Node) {
        for stmt in root.children_of(Kind::Statement) {
            for n in stmt.nodes() {
                self.collect_in(n);
            }
        }
    }

    fn collect_in(&mut self, n: &Node) {
        match n.kind {
            Kind::Ddl => {
                let create = n.tokens().next().is_some_and(|t| t.is_kw(self.sql, "CREATE"));
                let object = n.tokens().find(|t| t.is_kw(self.sql, "TABLE") || t.is_kw(self.sql, "VIEW"));
                if create
                    && let Some(obj) = object
                    && let Some(name) = n.child(Kind::Name)
                {
                    let _ = obj;
                    let parts = self.parts(name);
                    if let Some(last) = parts.last() {
                        let schema =
                            if parts.len() >= 2 { Some(parts[parts.len() - 2].text.clone()) } else { None };
                        let cols = self.defined_columns(n);
                        self.script_tables.push(ScriptTable { schema, name: last.text.clone(), cols });
                    }
                }
            }
            Kind::Select => {
                // T-SQL / PostgreSQL `SELECT ... INTO new_table`
                if let Some(into) = n.child(Kind::IntoClause)
                    && let Some(name) = into.child(Kind::Name)
                {
                    let parts = self.parts(name);
                    if let Some(last) = parts.last() {
                        self.script_tables.push(ScriptTable {
                            schema: None,
                            name: last.text.clone(),
                            cols: None,
                        });
                    }
                }
                for c in n.nodes() {
                    self.collect_in(c);
                }
            }
            _ => {
                for c in n.nodes() {
                    self.collect_in(c);
                }
            }
        }
    }

    /// The column names a CREATE TABLE lists, or `None` when they cannot be told (AS query, LIKE, ...).
    fn defined_columns(&self, ddl: &Node) -> Option<Vec<String>> {
        if ddl.child(Kind::Query).is_some() {
            return None;
        }
        let list = ddl.child(Kind::ColumnDefList)?;
        let mut cols = Vec::new();
        for def in list.children_of(Kind::ColumnDef) {
            let first = def.tokens().next()?;
            if first.kind == Kind::Token(TokenKind::Word) {
                let w = first.text(self.sql).to_ascii_uppercase();
                if matches!(w.as_str(), "LIKE" | "INHERITS") {
                    return None;
                }
                if matches!(
                    w.as_str(),
                    "PRIMARY"
                        | "UNIQUE"
                        | "FOREIGN"
                        | "CHECK"
                        | "CONSTRAINT"
                        | "KEY"
                        | "INDEX"
                        | "EXCLUDE"
                        | "PERIOD"
                        | "FULLTEXT"
                        | "SPATIAL"
                ) && def.tokens().nth(1).is_some_and(|t| {
                    t.kind != Kind::Token(TokenKind::Word)
                        || w == "PRIMARY"
                        || w == "FOREIGN"
                        || w == "CONSTRAINT"
                        || w == "UNIQUE"
                        || w == "CHECK"
                }) {
                    continue;
                }
            }
            cols.push(self.ident_of(first).text);
        }
        Some(cols)
    }

    // ---- statements

    fn statement(&mut self, stmt: &Node) {
        let base_lenient = self.lenient;
        if contains_kind(stmt, Kind::Unknown) {
            self.lenient += 1;
        }
        for n in stmt.nodes() {
            self.top(n);
        }
        self.lenient = base_lenient;
    }

    fn top(&mut self, n: &Node) {
        match n.kind {
            Kind::Query => {
                self.query(n, None);
            }
            Kind::Insert => self.insert(n),
            Kind::Update => self.update(n),
            Kind::Delete => self.delete(n),
            Kind::Merge => self.merge(n),
            Kind::Explain => {
                for c in n.nodes() {
                    self.top(c);
                }
            }
            Kind::Ddl => {
                self.ddl_references(n);
                self.embedded(n);
            }
            Kind::Other => self.embedded(n),
            _ => {}
        }
    }

    /// The tables an ALTER, DROP, TRUNCATE or CREATE INDEX names must exist (unless `IF EXISTS`, or the script
    /// creates them). Everything else in DDL is left alone.
    fn ddl_references(&mut self, n: &Node) {
        let words: Vec<String> = n
            .tokens()
            .take_while(|t| t.kind != Kind::Name)
            .filter(|t| t.kind == Kind::Token(TokenKind::Word))
            .map(|t| t.text(self.sql).to_ascii_uppercase())
            .take(8)
            .collect();
        let first = words.first().map(String::as_str).unwrap_or("");
        let has = |w: &str| words.iter().any(|x| x == w);
        if has("EXISTS") {
            return;
        }
        let is_table = has("TABLE") || has("VIEW") || (first == "TRUNCATE");
        let check_all = (matches!(first, "ALTER" | "DROP")
            && (has("TABLE") || (has("VIEW") && !has("MATERIALIZED"))))
            || first == "TRUNCATE";
        let sid = self.new_scope(ScopeKind::Empty, n.span.clone(), None);
        if check_all && is_table {
            for name in n.children_of(Kind::Name) {
                self.check_table_name(name, sid);
            }
        } else if first == "CREATE" && has("INDEX") && !has("EXISTS") {
            // CREATE INDEX name ON table (...): the table is the second name
            let names: Vec<&Node> = n.children_of(Kind::Name).collect();
            if let Some(t) = names.get(1).or(names.first().filter(|_| names.len() == 1 && false)) {
                self.check_table_name(t, sid);
            }
        }
    }

    fn check_table_name(&mut self, name: &Node, sid: ScopeId) {
        let parts = self.parts(name);
        let Some(last) = parts.last().cloned() else { return };
        let resolution = match self.find_table(&parts) {
            TableLookup::Catalog { schema, table } => Resolution::Table(self.table_id(schema, table)),
            TableLookup::Script(i) => {
                let t = &self.script_tables[i];
                Resolution::Table(TableId { schema: t.schema.clone(), name: t.name.clone(), script: true })
            }
            TableLookup::Missing => {
                let display = parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(".");
                let cands = self.table_candidates(sid);
                self.diag(
                    last.span.clone(),
                    format!("Unknown table '{display}'"),
                    DiagKind::UnknownTable { name: display.clone(), candidates: cands },
                );
                Resolution::Unresolved(Problem::UnknownTable)
            }
            TableLookup::Unknown => Resolution::Unknown,
        };
        self.add_ref(&last, RefRole::Table, resolution, sid);
    }

    /// Queries inside statements this resolver does not model (a view's query, `COPY (query)`, ...) are
    /// resolved on their own.
    fn embedded(&mut self, n: &Node) {
        // SQLite does not look at a view's query when the view is created.
        if self.fam == Family::Sqlite && n.kind == Kind::Ddl {
            self.lenient += 1;
            self.embedded_inner(n);
            self.lenient -= 1;
        } else {
            self.embedded_inner(n);
        }
    }

    fn embedded_inner(&mut self, n: &Node) {
        for c in n.nodes() {
            match c.kind {
                Kind::Query => {
                    self.query(c, None);
                }
                Kind::Subquery => {
                    for q in c.children_of(Kind::Query) {
                        self.query(q, None);
                    }
                }
                _ => self.embedded_inner(c),
            }
        }
    }

    // ---- queries

    fn query(&mut self, q: &Node, outer: Option<ScopeId>) -> QOut {
        let mut base = outer;
        if let Some(w) = q.child(Kind::WithClause) {
            base = Some(self.with_clause(w, outer));
        }
        let body = q.nodes().find(|n| {
            matches!(n.kind, Kind::Select | Kind::SetOp | Kind::ParenQuery | Kind::Values | Kind::TableStmt)
        });
        let out = match body {
            Some(b) => self.body(b, base),
            None => QOut::unknown(),
        };
        if let Some(ob) = q.child(Kind::OrderBy) {
            match out.scope {
                Some(s) => self.order_by(ob, s),
                None => {
                    // ORDER BY over a set operation names output columns.
                    let s = self.new_scope(ScopeKind::Select, ob.span.clone(), base);
                    self.scopes[s].sources.push(Source {
                        name: String::new(),
                        quoted: false,
                        kind: SourceKind::Derived,
                        table: None,
                        symbol: None,
                        cols: Some(out.cols.clone()),
                        complete: out.complete,
                        span: ob.span.clone(),
                        alias_span: None,
                        merged: Vec::new(),
                    });
                    self.lenient += 1;
                    self.order_by(ob, s);
                    self.lenient -= 1;
                }
            }
        }
        if let Some(l) = q.child(Kind::Limit) {
            let s = self.new_scope(ScopeKind::Empty, l.span.clone(), base);
            self.expr(l, s);
        }
        out
    }

    fn order_by(&mut self, ob: &Node, scope: ScopeId) {
        // An ORDER BY name prefers the select-list alias (`order by total` after `sum(x) as total`).
        for item in ob.nodes() {
            for e in item.nodes() {
                self.expr_pref(e, scope, true);
            }
        }
    }

    fn body(&mut self, b: &Node, outer: Option<ScopeId>) -> QOut {
        match b.kind {
            Kind::Select => self.select(b, outer),
            Kind::SetOp => {
                let mut first: Option<QOut> = None;
                for arm in b.nodes() {
                    if !matches!(
                        arm.kind,
                        Kind::Select | Kind::SetOp | Kind::ParenQuery | Kind::Values | Kind::TableStmt
                    ) {
                        continue;
                    }
                    let o = self.body(arm, outer);
                    match &first {
                        None => first = Some(o),
                        Some(f) => {
                            if f.counted && o.counted && f.cols.len() != o.cols.len() {
                                let (expected, found) = (f.cols.len(), o.cols.len());
                                self.diag(
                                    arm.span.clone(),
                                    format!("Each set operation branch must have the same number of columns: {expected} before, {found} here"),
                                    DiagKind::ColumnCount { expected, found },
                                );
                            }
                        }
                    }
                }
                let mut f = first.unwrap_or_else(QOut::unknown);
                f.scope = None;
                f
            }
            Kind::ParenQuery => match b.child(Kind::Query) {
                Some(q) => {
                    let mut o = self.query(q, outer);
                    o.scope = None;
                    o
                }
                None => QOut::unknown(),
            },
            Kind::Values => {
                let s = self.new_scope(ScopeKind::Empty, b.span.clone(), outer);
                let mut width = None;
                let mut known = true;
                for row in b.children_of(Kind::RowCtor) {
                    self.expr(row, s);
                    if contains_kind(row, Kind::Unknown) {
                        known = false;
                    }
                    let w = row.nodes().count();
                    match width {
                        None => width = Some(w),
                        Some(prev) if prev != w => known = false,
                        _ => {}
                    }
                }
                let n = width.unwrap_or(0);
                QOut {
                    cols: (0..n).map(|_| Col { name: String::new(), symbol: None, quoted: false }).collect(),
                    complete: false,
                    counted: known && width.is_some(),
                    scope: None,
                }
            }
            Kind::TableStmt => {
                let mut o = QOut::unknown();
                if let Some(name) = b.child(Kind::Name) {
                    let parts = self.parts(name);
                    if let TableLookup::Catalog { table, .. } = self.find_table(&parts) {
                        o = QOut {
                            cols: Self::catalog_cols(table),
                            complete: true,
                            counted: true,
                            scope: None,
                        };
                    }
                }
                o
            }
            _ => QOut::unknown(),
        }
    }

    fn with_clause(&mut self, w: &Node, outer: Option<ScopeId>) -> ScopeId {
        let wid = self.new_scope(ScopeKind::With, w.span.clone(), outer);
        let recursive = w.has_kw(self.sql, "RECURSIVE") || self.fam == Family::SqlServer;
        for cte in w.children_of(Kind::Cte) {
            let Some(name_tok) = cte
                .tokens()
                .find(|t| matches!(t.kind, Kind::Token(TokenKind::Word | TokenKind::QuotedIdent)))
            else {
                continue;
            };
            let id = self.ident_of(name_tok);
            let sym = self.new_symbol(SymbolKind::Cte, &id, id.span.clone(), true, wid);
            self.add_ref(&id, RefRole::Declaration, Resolution::Symbol(sym), wid);
            if recursive {
                self.scopes[wid].ctes.push(sym);
                self.cte_cols.insert(sym, (Vec::new(), false, false));
            }
            let mut qo = QOut::unknown();
            for body in cte.nodes() {
                match body.kind {
                    Kind::Query => qo = self.query(body, Some(wid)),
                    Kind::Insert => self.insert(body),
                    Kind::Update => self.update(body),
                    Kind::Delete => self.delete(body),
                    Kind::Merge => self.merge(body),
                    _ => {}
                }
            }
            // `WITH x (a, b) AS (...)` names the columns.
            let mut cols = qo.cols.clone();
            let mut complete = qo.complete;
            let mut counted = qo.counted;
            if let Some(list) = cte.child(Kind::ColumnList) {
                let names = self.list_idents(list);
                for (i, nm) in names.iter().enumerate() {
                    let cs = self.new_symbol(SymbolKind::ColumnAlias, nm, nm.span.clone(), true, wid);
                    self.add_ref(nm, RefRole::Declaration, Resolution::Symbol(cs), wid);
                    let col = Col { name: self.canon(nm), symbol: Some(cs), quoted: nm.quoted };
                    if i < cols.len() {
                        cols[i] = col;
                    } else {
                        cols.push(col);
                        counted = false;
                    }
                }
                if names.len() >= cols.len() {
                    complete = cols.iter().all(|c| !c.name.is_empty());
                }
            }
            self.cte_cols.insert(sym, (cols, complete && counted, counted));
            if !recursive {
                self.scopes[wid].ctes.push(sym);
            }
        }
        wid
    }

    /// The identifiers of a `ColumnList`, in order.
    fn list_idents(&self, list: &Node) -> Vec<Ident> {
        list.tokens()
            .filter(|t| matches!(t.kind, Kind::Token(TokenKind::Word | TokenKind::QuotedIdent)))
            .map(|t| self.ident_of(t))
            .collect()
    }

    fn select(&mut self, sel: &Node, outer: Option<ScopeId>) -> QOut {
        let sid = self.new_scope(ScopeKind::Select, sel.span.clone(), outer);
        if let Some(f) = sel.child(Kind::FromClause) {
            for c in f.nodes() {
                self.item(c, sid);
            }
        }
        let mut out = QOut { cols: Vec::new(), complete: true, counted: true, scope: Some(sid) };
        if let Some(list) = sel.child(Kind::SelectList) {
            for item in list.children_of(Kind::SelectItem) {
                self.select_item(item, sid, &mut out);
            }
        }
        for c in sel.nodes() {
            match c.kind {
                Kind::WhereClause | Kind::Having | Kind::Qualify | Kind::WindowClause => self.expr(c, sid),
                Kind::GroupBy => self.expr_pref(c, sid, true),
                _ => {}
            }
        }
        self.check_grouping(sel, sid);
        out
    }

    fn select_item(&mut self, item: &Node, sid: ScopeId, out: &mut QOut) {
        let alias = item.child(Kind::Alias);
        let alias_id = alias.and_then(|a| self.alias_ident(a));
        let expr = item.nodes().find(|n| n.kind != Kind::Alias && n.kind != Kind::Unknown);
        let Some(expr) = expr else {
            out.complete = false;
            out.counted = false;
            return;
        };
        if expr.kind == Kind::Star {
            self.star(expr, item, sid, out);
            return;
        }
        self.expr(expr, sid);
        // The column's name: its alias, else the column it passes through.
        let (name_id, renamable) = match (&alias_id, expr.kind) {
            (Some(a), _) => (Some(a.clone()), true),
            (None, Kind::ColumnRef) => (self.parts(expr).last().cloned(), false),
            _ => (None, false),
        };
        // A column that merely passes a script-local column through is that same column: renaming one renames
        // the other, and an ORDER BY on it reaches the original.
        if !renamable
            && expr.kind == Kind::ColumnRef
            && let Some(last) = self.parts(expr).last().cloned()
            && let Some(Resolution::Symbol(src_sym)) =
                self.ref_index.get(&last.span.start).map(|&i| self.refs[i].resolution.clone())
            && matches!(self.symbols[src_sym].kind, SymbolKind::OutputColumn | SymbolKind::ColumnAlias)
        {
            if !self.scopes[sid].outputs.contains(&src_sym) {
                self.scopes[sid].outputs.push(src_sym);
            }
            out.cols.push(Col { name: self.canon(&last), symbol: Some(src_sym), quoted: last.quoted });
            return;
        }
        match name_id {
            Some(nm) => {
                let span = if renamable { nm.span.clone() } else { item.span.clone() };
                let sym = self.new_symbol(SymbolKind::OutputColumn, &nm, span, renamable, sid);
                if renamable {
                    self.add_ref(&nm, RefRole::Declaration, Resolution::Symbol(sym), sid);
                }
                self.scopes[sid].outputs.push(sym);
                out.cols.push(Col { name: self.canon(&nm), symbol: Some(sym), quoted: nm.quoted });
            }
            None => {
                out.cols.push(Col { name: String::new(), symbol: None, quoted: false });
                out.complete = false;
            }
        }
    }

    fn alias_ident(&self, alias: &Node) -> Option<Ident> {
        let mut toks = alias.tokens();
        let mut first = toks.next()?;
        if first.is_kw(self.sql, "AS") {
            first = toks.next()?;
        }
        matches!(first.kind, Kind::Token(TokenKind::Word | TokenKind::QuotedIdent | TokenKind::String))
            .then(|| self.ident_of(first))
    }

    fn star(&mut self, star: &Node, item: &Node, sid: ScopeId, out: &mut QOut) {
        let quals: Vec<Ident> = self.parts(star);
        let mut columns: Vec<(String, String)> = Vec::new();
        let mut complete = true;
        let targets: Vec<usize>;
        if quals.is_empty() {
            targets = (0..self.scopes[sid].sources.len()).collect();
        } else {
            match self.lookup_source(sid, quals.last().expect("qualifier")) {
                SrcFound::One(scope, idx) => {
                    let res = self.source_resolution(scope, idx);
                    self.add_ref(quals.last().expect("qualifier"), RefRole::Qualifier, res, sid);
                    if scope == sid {
                        targets = vec![idx];
                    } else {
                        // a correlated qualifier in a select list expands nothing of ours
                        targets = Vec::new();
                        complete = false;
                    }
                }
                SrcFound::Unknown => {
                    self.add_ref(
                        quals.last().expect("qualifier"),
                        RefRole::Qualifier,
                        Resolution::Unknown,
                        sid,
                    );
                    targets = Vec::new();
                    complete = false;
                }
                SrcFound::None => {
                    let q = quals.last().expect("qualifier").clone();
                    self.unknown_qualifier(&q, sid);
                    targets = Vec::new();
                    complete = false;
                }
            }
        }
        for &i in &targets {
            let src = self.scopes[sid].sources[i].clone();
            match &src.cols {
                Some(cols) if src.complete => {
                    for c in cols {
                        if src.merged.iter().any(|m| m.eq_ignore_ascii_case(&c.name)) {
                            continue;
                        }
                        columns.push((src.name.clone(), c.name.clone()));
                        out.cols.push(c.clone());
                    }
                }
                Some(cols) => {
                    complete = false;
                    out.cols.extend(cols.iter().cloned());
                    out.complete = false;
                    out.counted = false;
                }
                None => {
                    complete = false;
                }
            }
        }
        if !complete {
            out.complete = false;
            out.counted = false;
        }
        let qualified = !quals.is_empty();
        self.stars.push(StarExpansion {
            span: item.span.clone(),
            scope: sid,
            columns: if complete { columns } else { Vec::new() },
            complete,
            qualified,
        });
    }

    // ---- FROM

    fn item(&mut self, n: &Node, sid: ScopeId) {
        match n.kind {
            Kind::TableName => self.add_table(n, sid),
            Kind::DerivedTable => self.add_derived(n, sid),
            Kind::TableFunc => self.add_function(n, sid),
            Kind::ParenFrom => {
                for c in n.nodes() {
                    self.item(c, sid);
                }
            }
            Kind::Join => self.join(n, sid),
            _ => {}
        }
    }

    fn join(&mut self, n: &Node, sid: ScopeId) {
        let mut natural = false;
        let mut apply = false;
        for c in &n.children {
            match c.kind {
                Kind::Token(TokenKind::Word) => {
                    if c.is_kw(self.sql, "NATURAL") {
                        natural = true;
                    } else if c.is_kw(self.sql, "APPLY") {
                        apply = true;
                    }
                }
                Kind::TableName | Kind::DerivedTable | Kind::TableFunc | Kind::ParenFrom | Kind::Join => {
                    let before = self.scopes[sid].sources.len();
                    if apply && matches!(c.kind, Kind::DerivedTable | Kind::TableFunc) {
                        // CROSS / OUTER APPLY: the right side sees the left (lateral).
                        self.item_lateral(c, sid);
                    } else {
                        self.item(c, sid);
                    }
                    if natural && before > 0 && self.scopes[sid].sources.len() > before {
                        self.merge_natural(sid, before);
                    }
                    natural = false;
                    apply = false;
                }
                Kind::JoinConstraint => self.constraint(c, sid),
                _ => {}
            }
        }
    }

    fn item_lateral(&mut self, n: &Node, sid: ScopeId) {
        match n.kind {
            Kind::DerivedTable => self.add_derived_with(n, sid, true),
            _ => self.add_function(n, sid),
        }
    }

    fn merge_natural(&mut self, sid: ScopeId, right_start: usize) {
        let left: Vec<String> = self.scopes[sid].sources[..right_start]
            .iter()
            .filter_map(|s| s.cols.as_ref())
            .flat_map(|c| c.iter().map(|c| c.name.to_lowercase()))
            .collect();
        let last = self.scopes[sid].sources.len() - 1;
        let merged: Vec<String> = match &self.scopes[sid].sources[last].cols {
            Some(cols) => cols
                .iter()
                .filter(|c| left.contains(&c.name.to_lowercase()))
                .map(|c| c.name.clone())
                .collect(),
            None => Vec::new(),
        };
        self.scopes[sid].sources[last].merged.extend(merged);
    }

    fn constraint(&mut self, c: &Node, sid: ScopeId) {
        if c.has_kw(self.sql, "USING") {
            let Some(list) = c.child(Kind::ColumnList) else { return };
            let names = self.list_idents(list);
            let n_src = self.scopes[sid].sources.len();
            for nm in names {
                // The column must exist on the right (last) source and on some earlier one.
                let mut res = Resolution::Unknown;
                if n_src >= 2 {
                    let known = |s: &Source| s.cols.is_some() && s.complete;
                    let has = |s: &Source| {
                        s.cols
                            .as_ref()
                            .is_some_and(|c| !find_all(self.fam, &nm, c, |c| c.name.as_str()).is_empty())
                    };
                    let srcs = &self.scopes[sid].sources;
                    let left_all_known = srcs[..n_src - 1].iter().all(known);
                    let in_left = srcs[..n_src - 1].iter().any(has);
                    let right = &srcs[n_src - 1];
                    let missing_left = left_all_known && !in_left;
                    let missing_right = known(right) && !has(right);
                    if missing_left || missing_right {
                        let side =
                            if missing_left { "the tables before the join" } else { "the joined table" };
                        self.diag(
                            nm.span.clone(),
                            format!("Unknown column '{}' in USING: it is not a column of {side}", nm.text),
                            DiagKind::UnknownColumn {
                                name: nm.text.clone(),
                                qualifier: None,
                                candidates: Vec::new(),
                            },
                        );
                    }
                    for si in 0..n_src - 1 {
                        let src = &self.scopes[sid].sources[si];
                        if let Some(cols) = &src.cols {
                            let hits = find_all(self.fam, &nm, cols, |c| c.name.as_str());
                            if let Some(&h) = hits.first() {
                                res = self.col_resolution(sid, si, h);
                                break;
                            }
                        }
                    }
                    let last = n_src - 1;
                    let canon = self.canon(&nm);
                    self.scopes[sid].sources[last].merged.push(canon);
                }
                self.add_ref(&nm, RefRole::ListedColumn, res, sid);
            }
        } else {
            for e in c.nodes() {
                self.expr(e, sid);
            }
        }
    }

    fn add_table(&mut self, n: &Node, sid: ScopeId) {
        let Some(name_node) = n.child(Kind::Name) else { return };
        let parts = self.parts(name_node);
        let Some(last) = parts.last().cloned() else { return };
        let alias = n.child(Kind::Alias);
        let mut src = Source {
            name: self.canon(&last),
            quoted: last.quoted,
            kind: SourceKind::Unknown,
            table: None,
            symbol: None,
            cols: None,
            complete: false,
            span: n.span.clone(),
            alias_span: None,
            merged: Vec::new(),
        };
        let mut resolution = Resolution::Unknown;
        // CTEs first (unqualified names only)
        let cte = if parts.len() == 1 { self.find_cte(sid, &last) } else { None };
        if let Some(sym) = cte {
            let (cols, complete, _) = self.cte_cols.get(&sym).cloned().unwrap_or((Vec::new(), false, false));
            src.kind = SourceKind::Cte;
            src.symbol = Some(sym);
            let known = !cols.is_empty() || complete;
            src.cols = if known { Some(cols) } else { None };
            src.complete = complete;
            resolution = Resolution::Symbol(sym);
        } else {
            match self.find_table(&parts) {
                TableLookup::Catalog { schema, table } => {
                    src.kind = SourceKind::Table;
                    src.table = Some(self.table_id(schema, table));
                    src.cols = Some(Self::catalog_cols(table));
                    src.complete = true;
                    src.name = table.name.clone();
                    resolution = Resolution::Table(self.table_id(schema, table));
                }
                TableLookup::Script(i) => {
                    let t = &self.script_tables[i];
                    let id = TableId { schema: t.schema.clone(), name: t.name.clone(), script: true };
                    src.kind = SourceKind::Table;
                    src.table = Some(id.clone());
                    src.cols = self.script_table_cols(i);
                    src.complete = src.cols.is_some();
                    src.name = t.name.clone();
                    resolution = Resolution::Table(id);
                }
                TableLookup::Missing => {
                    resolution = Resolution::Unresolved(Problem::UnknownTable);
                    let display = parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(".");
                    let cands = self.table_candidates(sid);
                    self.diag(
                        last.span.clone(),
                        format!("Unknown table '{display}'"),
                        DiagKind::UnknownTable { name: display.clone(), candidates: cands },
                    );
                }
                TableLookup::Unknown => {}
            }
        }
        self.add_ref(&last, RefRole::Table, resolution, sid);
        if let Some(a) = alias {
            self.apply_alias(&mut src, a, sid);
        }
        self.push_source(sid, src, &last);
    }

    /// Give a source its alias: the new visible name, an alias symbol and renamed columns.
    fn apply_alias(&mut self, src: &mut Source, alias: &Node, sid: ScopeId) {
        let Some(aid) = self.alias_ident(alias) else { return };
        let sym = self.new_symbol(SymbolKind::TableAlias, &aid, aid.span.clone(), true, sid);
        self.add_ref(&aid, RefRole::Declaration, Resolution::Symbol(sym), sid);
        src.name = self.canon(&aid);
        src.quoted = aid.quoted;
        src.symbol = Some(sym);
        src.alias_span = Some(aid.span.clone());
        if let Some(list) = alias.child(Kind::ColumnList) {
            let names = self.list_idents(list);
            let mut cols = src.cols.clone().unwrap_or_default();
            let had_cols = src.cols.is_some();
            for (i, nm) in names.iter().enumerate() {
                let cs = self.new_symbol(SymbolKind::ColumnAlias, nm, nm.span.clone(), true, sid);
                self.add_ref(nm, RefRole::Declaration, Resolution::Symbol(cs), sid);
                let col = Col { name: self.canon(nm), symbol: Some(cs), quoted: nm.quoted };
                if i < cols.len() {
                    cols[i] = col;
                } else {
                    cols.push(col);
                }
            }
            let all_named = names.len() >= cols.len() && had_cols || !had_cols;
            src.cols = Some(cols);
            src.complete = all_named && (src.complete || !had_cols);
            if !had_cols {
                // `f() AS t(a, b)`: the names are all we know
                src.complete = false;
            }
        }
    }

    fn push_source(&mut self, sid: ScopeId, src: Source, shown: &Ident) {
        if !src.name.is_empty()
            && matches!(self.fam, Family::Postgres | Family::MySql | Family::SqlServer | Family::DuckDb)
            && self.scopes[sid].sources.iter().any(|s| {
                !s.name.is_empty()
                    && s.kind != SourceKind::Pseudo
                    && (s.name == src.name
                        || (self.fam != Family::Postgres && s.name.eq_ignore_ascii_case(&src.name)))
            })
        {
            let span = src.alias_span.clone().unwrap_or_else(|| shown.span.clone());
            let name = src.name.clone();
            self.diag(
                span,
                format!("Table name '{name}' is used more than once in this FROM clause; give one of them an alias"),
                DiagKind::DuplicateAlias { name },
            );
        }
        self.scopes[sid].sources.push(src);
    }

    fn find_cte(&self, sid: ScopeId, id: &Ident) -> Option<SymbolId> {
        let mut cur = Some(sid);
        while let Some(s) = cur {
            let scope = &self.scopes[s];
            let hits = find_all(self.fam, id, &scope.ctes, |&c| self.symbols[c].name.as_str());
            if let Some(&h) = hits.last() {
                return Some(scope.ctes[h]);
            }
            cur = scope.parent;
        }
        None
    }

    fn add_derived(&mut self, n: &Node, sid: ScopeId) {
        let lateral = n.has_kw(self.sql, "LATERAL");
        self.add_derived_with(n, sid, lateral);
    }

    fn add_derived_with(&mut self, n: &Node, sid: ScopeId, lateral: bool) {
        // A plain derived table sees the enclosing query's scope (a correlated subquery), not its siblings.
        let outer = if lateral { Some(sid) } else { self.scopes[sid].parent };
        let qo = match n.child(Kind::Query) {
            Some(q) => self.query(q, outer),
            None => QOut::unknown(),
        };
        let mut src = Source {
            name: String::new(),
            quoted: false,
            kind: SourceKind::Derived,
            table: None,
            symbol: None,
            cols: Some(qo.cols.clone()),
            complete: qo.complete && qo.counted,
            span: n.span.clone(),
            alias_span: None,
            merged: Vec::new(),
        };
        if !qo.counted && qo.cols.is_empty() {
            src.cols = None;
        }
        let shown = Ident { text: String::new(), span: n.span.start..n.span.start, quoted: false };
        if let Some(a) = n.child(Kind::Alias) {
            self.apply_alias(&mut src, a, sid);
        }
        self.push_source(sid, src, &shown);
    }

    fn add_function(&mut self, n: &Node, sid: ScopeId) {
        // Arguments may refer to the sources to the left (an implicit LATERAL).
        let args = self.new_scope(ScopeKind::Empty, n.span.clone(), Some(sid));
        for c in n.nodes() {
            if matches!(c.kind, Kind::FuncCall | Kind::Literal | Kind::Param) {
                self.expr(c, args);
            }
        }
        let fname = n
            .child(Kind::FuncCall)
            .and_then(|f| f.child(Kind::Name))
            .map(|nm| self.parts(nm))
            .and_then(|p| p.last().cloned());
        let mut src = Source {
            name: fname.as_ref().map(|f| self.canon(f)).unwrap_or_default(),
            quoted: false,
            kind: SourceKind::Function,
            table: None,
            symbol: None,
            cols: None,
            complete: false,
            span: n.span.clone(),
            alias_span: None,
            merged: Vec::new(),
        };
        let shown =
            fname.unwrap_or(Ident { text: String::new(), span: n.span.start..n.span.start, quoted: false });
        if let Some(a) = n.child(Kind::Alias) {
            self.apply_alias(&mut src, a, sid);
        }
        self.push_source(sid, src, &shown);
    }

    // ---- expressions

    fn expr(&mut self, n: &Node, sc: ScopeId) {
        self.expr_pref(n, sc, false)
    }

    /// Walk an expression tree resolving the columns in it. `prefer_alias`: a bare name that is a select-list
    /// alias of the scope resolves to it before any column (ORDER BY, GROUP BY).
    fn expr_pref(&mut self, n: &Node, sc: ScopeId, prefer_alias: bool) {
        match n.kind {
            Kind::ColumnRef => self.column_ref(n, sc, prefer_alias),
            Kind::Star => self.star_in_expr(n, sc),
            Kind::Query => {
                self.query(n, Some(sc));
            }
            Kind::Subquery | Kind::Exists => {
                for c in n.nodes() {
                    self.expr_pref(c, sc, false);
                }
            }
            Kind::FuncCall => self.func_call(n, sc, prefer_alias),
            Kind::TypeName
            | Kind::Alias
            | Kind::Name
            | Kind::Literal
            | Kind::Param
            | Kind::Unknown
            | Kind::ColumnList => {}
            Kind::Binary => {
                // DuckDB / ClickHouse lambda `x -> x + 1`: the parameter names are not columns.
                let lambda = matches!(self.fam, Family::DuckDb | Family::ClickHouse)
                    && n.tokens().any(|t| t.text(self.sql) == "->");
                if lambda {
                    self.lenient += 1;
                }
                for c in n.nodes() {
                    self.expr_pref(c, sc, prefer_alias);
                }
                if lambda {
                    self.lenient -= 1;
                }
            }
            Kind::ArrayCtor => {
                // DuckDB list comprehension `[x + 1 FOR x IN l]` binds its own names.
                let comprehension = n.children.iter().any(|c| {
                    c.is_kw(self.sql, "FOR") || (c.kind == Kind::Unknown && c.has_kw(self.sql, "FOR"))
                });
                if comprehension {
                    self.lenient += 1;
                }
                for c in n.nodes() {
                    self.expr_pref(c, sc, prefer_alias);
                }
                if comprehension {
                    self.lenient -= 1;
                }
            }
            _ => {
                for c in n.nodes() {
                    self.expr_pref(c, sc, prefer_alias);
                }
            }
        }
    }

    fn func_call(&mut self, n: &Node, sc: ScopeId, prefer_alias: bool) {
        if let Some(name) = n.child(Kind::Name) {
            let parts = self.parts(name);
            if let Some(last) = parts.last().cloned() {
                self.add_ref(&last, RefRole::Function, Resolution::Function(last.text.clone()), sc);
            }
        }
        for c in n.nodes() {
            if c.kind != Kind::Name {
                self.expr_pref(c, sc, prefer_alias);
            }
        }
    }

    fn star_in_expr(&mut self, n: &Node, sc: ScopeId) {
        let quals = self.parts(n);
        if let Some(q) = quals.last() {
            self.qualifier_ref(q, sc);
        }
    }

    /// Resolve `q` as a table or alias in scope and record the reference. Returns whether found.
    fn qualifier_ref(&mut self, q: &Ident, sc: ScopeId) -> bool {
        match self.lookup_source(sc, q) {
            SrcFound::One(scope, idx) => {
                let res = self.source_resolution(scope, idx);
                self.add_ref(q, RefRole::Qualifier, res, sc);
                true
            }
            SrcFound::Unknown => {
                self.add_ref(q, RefRole::Qualifier, Resolution::Unknown, sc);
                false
            }
            SrcFound::None => {
                self.unknown_qualifier(q, sc);
                false
            }
        }
    }

    fn unknown_qualifier(&mut self, q: &Ident, sc: ScopeId) {
        if names::is_niladic(&q.text)
            || matches!(
                q.text.to_ascii_lowercase().as_str(),
                "excluded" | "inserted" | "deleted" | "new" | "old"
            )
        {
            self.add_ref(q, RefRole::Qualifier, Resolution::Pseudo, sc);
            return;
        }
        self.add_ref(q, RefRole::Qualifier, Resolution::Unresolved(Problem::UnknownQualifier), sc);
        let cands = self.visible_source_names(sc);
        self.diag(
            q.span.clone(),
            format!("'{}' is not a table or alias in this query", q.text),
            DiagKind::UnknownQualifier { name: q.text.clone(), candidates: cands },
        );
    }

    fn visible_source_names(&self, sc: ScopeId) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = Some(sc);
        while let Some(s) = cur {
            for src in &self.scopes[s].sources {
                if !src.name.is_empty() && src.kind != SourceKind::Pseudo && !out.contains(&src.name) {
                    out.push(src.name.clone());
                }
            }
            cur = self.scopes[s].parent;
        }
        out
    }

    fn source_resolution(&self, scope: ScopeId, idx: usize) -> Resolution {
        let src = &self.scopes[scope].sources[idx];
        match (&src.symbol, &src.table) {
            (Some(sym), _) => Resolution::Symbol(*sym),
            (None, Some(t)) => Resolution::Table(t.clone()),
            _ => Resolution::Unknown,
        }
    }

    fn col_resolution(&self, scope: ScopeId, src: usize, col: usize) -> Resolution {
        let s = &self.scopes[scope].sources[src];
        let c = &s.cols.as_ref().expect("cols")[col];
        match (&s.table, c.symbol) {
            (Some(t), _) => Resolution::Column { table: t.clone(), column: c.name.clone() },
            (None, Some(sym)) => Resolution::Symbol(sym),
            _ => Resolution::Unknown,
        }
    }

    fn column_ref(&mut self, n: &Node, sc: ScopeId, prefer_alias: bool) {
        let parts = self.parts(n);
        match parts.len() {
            0 => {}
            1 => self.unqualified(&parts[0], sc, prefer_alias),
            _ => self.qualified(&parts, sc),
        }
    }

    fn unqualified(&mut self, id: &Ident, sc: ScopeId, prefer_alias: bool) {
        if !id.quoted && id.text.starts_with('$') {
            // SQLite's `$name` parameter, which the lexer reads as a word outside the PostgreSQL family
            self.add_ref(id, RefRole::Column, Resolution::Pseudo, sc);
            return;
        }
        if !id.quoted && (names::is_niladic(&id.text) || names::is_system_column(&id.text)) {
            // `current_date`, `ctid`: values the grammar or the engine defines
            let res =
                if names::is_system_column(&id.text) { Resolution::Unknown } else { Resolution::Pseudo };
            self.add_ref(id, RefRole::Column, res, sc);
            return;
        }
        let mut searched = Vec::new();
        match self.lookup_column(sc, id, prefer_alias, &mut searched) {
            ColFound::One(res) => self.add_ref(id, RefRole::Column, res, sc),
            ColFound::Unknown => self.add_ref(id, RefRole::Column, Resolution::Unknown, sc),
            ColFound::Many(quals) => {
                self.add_ref(
                    id,
                    RefRole::Column,
                    Resolution::Unresolved(Problem::Ambiguous(quals.clone())),
                    sc,
                );
                self.diag(
                    id.span.clone(),
                    format!("Column '{}' is ambiguous (found in {})", id.text, quals.join(", ")),
                    DiagKind::AmbiguousColumn { name: id.text.clone(), qualifiers: quals },
                );
            }
            ColFound::None => {
                // `row_to_json(c)`, `(c).field`: a table alias used as a value
                if let SrcFound::One(scope, idx) = self.lookup_source(sc, id) {
                    let res = self.source_resolution(scope, idx);
                    self.add_ref(id, RefRole::Qualifier, res, sc);
                    return;
                }
                // SQLite reads a double-quoted name that matches no column as a string literal.
                if self.lenient > 0 || (self.fam == Family::Sqlite && id.quoted) {
                    self.add_ref(id, RefRole::Column, Resolution::Unknown, sc);
                    return;
                }
                self.add_ref(id, RefRole::Column, Resolution::Unresolved(Problem::UnknownColumn), sc);
                let cands = self.visible_columns(sc);
                let looked = if searched.is_empty() {
                    String::new()
                } else {
                    format!(" (looked in {})", searched.join(", "))
                };
                self.diag(
                    id.span.clone(),
                    format!("Unknown column '{}'{looked}", id.text),
                    DiagKind::UnknownColumn { name: id.text.clone(), qualifier: None, candidates: cands },
                );
            }
        }
    }

    fn visible_columns(&self, sc: ScopeId) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut cur = Some(sc);
        while let Some(s) = cur {
            for src in &self.scopes[s].sources {
                if let Some(cols) = &src.cols {
                    for c in cols {
                        if !c.name.is_empty() && !out.contains(&c.name) {
                            out.push(c.name.clone());
                        }
                    }
                }
            }
            for &o in &self.scopes[s].outputs {
                let nm = self.symbols[o].name.clone();
                if !out.contains(&nm) {
                    out.push(nm);
                }
            }
            cur = self.scopes[s].parent;
        }
        out
    }

    fn find_output(&self, sid: ScopeId, id: &Ident) -> Option<SymbolId> {
        let scope = &self.scopes[sid];
        let hits = find_all(self.fam, id, &scope.outputs, |&o| self.symbols[o].name.as_str());
        hits.first().map(|&h| scope.outputs[h])
    }

    fn lookup_column(
        &self,
        sc: ScopeId,
        id: &Ident,
        prefer_alias: bool,
        searched: &mut Vec<String>,
    ) -> ColFound {
        let mut cur = Some(sc);
        while let Some(s) = cur {
            let scope = &self.scopes[s];
            if prefer_alias && let Some(sym) = self.find_output(s, id) {
                return ColFound::One(Resolution::Symbol(sym));
            }
            let mut matches: Vec<(usize, usize)> = Vec::new();
            let mut unknown = false;
            for (i, src) in scope.sources.iter().enumerate() {
                if !src.name.is_empty() && !searched.contains(&src.name) && src.kind != SourceKind::Pseudo {
                    searched.push(src.name.clone());
                }
                match &src.cols {
                    None => unknown = true,
                    Some(cols) => {
                        let hits = find_all(self.fam, id, cols, |c| c.name.as_str());
                        match hits.first() {
                            Some(&h) => {
                                let merged = src.merged.iter().any(|m| m.eq_ignore_ascii_case(&id.text));
                                if !merged {
                                    matches.push((i, h));
                                }
                            }
                            None if !src.complete => unknown = true,
                            None => {}
                        }
                    }
                }
            }
            // `excluded` and friends never make a name ambiguous: the real table wins.
            if matches.len() > 1 {
                let real: Vec<(usize, usize)> = matches
                    .iter()
                    .copied()
                    .filter(|(i, _)| scope.sources[*i].kind != SourceKind::Pseudo)
                    .collect();
                if real.len() == 1 {
                    matches = real;
                } else if real.is_empty() {
                    matches.truncate(1);
                }
            }
            match matches.len() {
                1 => return ColFound::One(self.col_resolution(s, matches[0].0, matches[0].1)),
                n if n > 1 => {
                    let quals = matches.iter().map(|(i, _)| scope.sources[*i].name.clone()).collect();
                    return ColFound::Many(quals);
                }
                _ => {}
            }
            if !prefer_alias && let Some(sym) = self.find_output(s, id) {
                return ColFound::One(Resolution::Symbol(sym));
            }
            if unknown {
                return ColFound::Unknown;
            }
            cur = scope.parent;
        }
        ColFound::None
    }

    fn lookup_source(&self, sc: ScopeId, q: &Ident) -> SrcFound {
        let mut cur = Some(sc);
        let mut unknown = false;
        while let Some(s) = cur {
            let scope = &self.scopes[s];
            let hits = find_all(self.fam, q, &scope.sources, |src| src.name.as_str());
            let hits: Vec<usize> = hits.into_iter().filter(|&i| !scope.sources[i].name.is_empty()).collect();
            if let Some(&h) = hits.first() {
                return SrcFound::One(s, h);
            }
            // a CTE referenced as `cte.col` without being in FROM is not valid; ignore
            if scope.sources.iter().any(|src| src.cols.is_none() || !src.complete) {
                unknown = unknown
                    || scope
                        .sources
                        .iter()
                        .any(|src| src.kind == SourceKind::Unknown || src.kind == SourceKind::Function);
            }
            cur = scope.parent;
        }
        if unknown { SrcFound::Unknown } else { SrcFound::None }
    }

    fn qualified(&mut self, parts: &[Ident], sc: ScopeId) {
        // `q.col`, `schema.table.col`, or `col.field...` (a struct field).
        let n = parts.len();
        let qual = if n >= 3 { &parts[n - 2] } else { &parts[0] };
        let col = if n >= 3 { &parts[n - 1] } else { &parts[1] };
        let found = if n >= 3 {
            // schema.table: match the table id's schema as well
            self.lookup_source_schema(sc, &parts[n - 3], qual)
        } else {
            self.lookup_source(sc, qual)
        };
        match found {
            SrcFound::One(scope, idx) => {
                let res = self.source_resolution(scope, idx);
                self.add_ref(qual, RefRole::Qualifier, res, sc);
                let src = self.scopes[scope].sources[idx].clone();
                match &src.cols {
                    Some(cols) => {
                        let hits = find_all(self.fam, col, cols, |c| c.name.as_str());
                        match hits.first() {
                            Some(&h) => {
                                let r = self.col_resolution(scope, idx, h);
                                self.add_ref(col, RefRole::Column, r, sc);
                            }
                            None if src.complete && !names::is_system_column(&col.text) && n == 2 => {
                                if self.lenient > 0 {
                                    self.add_ref(col, RefRole::Column, Resolution::Unknown, sc);
                                    return;
                                }
                                self.add_ref(
                                    col,
                                    RefRole::Column,
                                    Resolution::Unresolved(Problem::UnknownColumn),
                                    sc,
                                );
                                let cands: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
                                let shown = match &src.table {
                                    Some(t) => t.name.clone(),
                                    None => src.name.clone(),
                                };
                                self.diag(
                                    col.span.clone(),
                                    format!("Unknown column '{}' in '{shown}'", col.text),
                                    DiagKind::UnknownColumn {
                                        name: col.text.clone(),
                                        qualifier: Some(shown),
                                        candidates: cands,
                                    },
                                );
                            }
                            None => self.add_ref(col, RefRole::Column, Resolution::Unknown, sc),
                        }
                    }
                    None => self.add_ref(col, RefRole::Column, Resolution::Unknown, sc),
                }
            }
            SrcFound::Unknown => {
                self.add_ref(qual, RefRole::Qualifier, Resolution::Unknown, sc);
                self.add_ref(col, RefRole::Column, Resolution::Unknown, sc);
            }
            SrcFound::None => {
                // Maybe `column.field`: a struct or composite column (DuckDB, ClickHouse tuples).
                let mut searched = Vec::new();
                match self.lookup_column(sc, &parts[0], false, &mut searched) {
                    ColFound::One(res) => self.add_ref(&parts[0], RefRole::Column, res, sc),
                    ColFound::Unknown | ColFound::Many(_) => {
                        self.add_ref(&parts[0], RefRole::Column, Resolution::Unknown, sc)
                    }
                    ColFound::None => {
                        // `schema.table.col` with an unknown table, or `x.col` with unknown `x`
                        let qualifier = if n >= 3 { qual.clone() } else { parts[0].clone() };
                        self.unknown_qualifier(&qualifier, sc);
                    }
                }
            }
        }
    }

    fn lookup_source_schema(&self, sc: ScopeId, schema: &Ident, table: &Ident) -> SrcFound {
        let mut cur = Some(sc);
        while let Some(s) = cur {
            let scope = &self.scopes[s];
            for (i, src) in scope.sources.iter().enumerate() {
                if let Some(t) = &src.table
                    && t.schema.as_deref().is_some_and(|ts| names::loose_eq(&schema.text, ts))
                    && names::loose_eq(&table.text, &t.name)
                    && src.symbol.is_none()
                {
                    return SrcFound::One(s, i);
                }
            }
            cur = scope.parent;
        }
        // the schema-qualified table is not in this query: cannot say which of several things is meant
        SrcFound::Unknown
    }

    // ---- DML

    fn dml_scope(&mut self, n: &Node, outer: Option<ScopeId>) -> (ScopeId, Option<ScopeId>) {
        let mut base = outer;
        if let Some(w) = n.child(Kind::WithClause) {
            base = Some(self.with_clause(w, outer));
        }
        let sid = self.new_scope(ScopeKind::Select, n.span.clone(), base);
        (sid, base)
    }

    fn insert(&mut self, n: &Node) {
        let (sid, base) = self.dml_scope(n, None);
        let mut target: Option<usize> = None;
        let mut ins_cols: Vec<Ident> = Vec::new();
        if let Some(name) = n.child(Kind::Name) {
            let parts = self.parts(name);
            if let Some(last) = parts.last().cloned() {
                let alias = n.child(Kind::Alias);
                self.add_target_source(sid, &parts, &last, name.span.clone(), alias);
                target = Some(self.scopes[sid].sources.len() - 1);
            }
        }
        let mut shown_cols: Option<usize> = None;
        for c in n.nodes() {
            match c.kind {
                Kind::ColumnList if target.is_some() => {
                    let names = self.list_idents(c);
                    for nm in &names {
                        self.listed_column(nm, sid, target.expect("target"));
                    }
                    shown_cols = Some(names.len());
                    ins_cols = names;
                }
                Kind::Values => {
                    let e = self.new_scope(ScopeKind::Empty, c.span.clone(), base);
                    self.body_values_check(c, &ins_cols, shown_cols, sid, target, e);
                }
                Kind::Query => {
                    let qo = self.query(c, base);
                    self.count_check_query(c, &qo, shown_cols, sid, target);
                }
                Kind::SetClause => self.assignments(c, sid, target),
                Kind::OnConflict => self.on_conflict(c, sid, base, target),
                Kind::OnDuplicate => self.assignments(c, sid, target),
                Kind::Returning => self.returning(c, sid, target),
                _ => {}
            }
        }
    }

    /// `INSERT INTO t (a, b) VALUES (1, 2)`: each row must supply what the list (or the table) names.
    fn body_values_check(
        &mut self,
        values: &Node,
        _cols: &[Ident],
        listed: Option<usize>,
        sid: ScopeId,
        target: Option<usize>,
        e: ScopeId,
    ) {
        let table_cols = target.and_then(|t| {
            let s = &self.scopes[sid].sources[t];
            if s.complete { s.cols.as_ref().map(|c| c.len()) } else { None }
        });
        for row in values.children_of(Kind::RowCtor) {
            self.expr(row, e);
            if contains_kind(row, Kind::Unknown) {
                continue;
            }
            let found = row.nodes().count();
            let expected = match (listed, table_cols) {
                (Some(l), _) => Some(l),
                (None, Some(t)) if found > t => Some(t),
                _ => None,
            };
            if let Some(expected) = expected
                && expected != found
            {
                self.diag(
                    row.span.clone(),
                    format!(
                        "INSERT has {expected} target column{} but this row has {found} value{}",
                        plural(expected),
                        plural(found)
                    ),
                    DiagKind::ColumnCount { expected, found },
                );
            }
        }
    }

    fn count_check_query(
        &mut self,
        q: &Node,
        qo: &QOut,
        listed: Option<usize>,
        sid: ScopeId,
        target: Option<usize>,
    ) {
        if !qo.counted {
            return;
        }
        let table_cols = target.and_then(|t| {
            let s = &self.scopes[sid].sources[t];
            if s.complete { s.cols.as_ref().map(|c| c.len()) } else { None }
        });
        let found = qo.cols.len();
        let expected = match (listed, table_cols) {
            (Some(l), _) => Some(l),
            (None, Some(t)) if found > t => Some(t),
            _ => None,
        };
        if let Some(expected) = expected
            && expected != found
        {
            self.diag(
                q.span.clone(),
                format!(
                    "INSERT has {expected} target column{} but the query returns {found}",
                    plural(expected)
                ),
                DiagKind::ColumnCount { expected, found },
            );
        }
    }

    fn add_target_source(
        &mut self,
        sid: ScopeId,
        parts: &[Ident],
        last: &Ident,
        span: Range<usize>,
        alias: Option<&Node>,
    ) {
        let mut src = Source {
            name: self.canon(last),
            quoted: last.quoted,
            kind: SourceKind::Unknown,
            table: None,
            symbol: None,
            cols: None,
            complete: false,
            span,
            alias_span: None,
            merged: Vec::new(),
        };
        let mut resolution = Resolution::Unknown;
        match self.find_table(parts) {
            TableLookup::Catalog { schema, table } => {
                src.kind = SourceKind::Table;
                src.table = Some(self.table_id(schema, table));
                src.cols = Some(Self::catalog_cols(table));
                src.complete = true;
                src.name = table.name.clone();
                resolution = Resolution::Table(self.table_id(schema, table));
            }
            TableLookup::Script(i) => {
                let t = &self.script_tables[i];
                let id = TableId { schema: t.schema.clone(), name: t.name.clone(), script: true };
                src.kind = SourceKind::Table;
                src.table = Some(id.clone());
                src.cols = self.script_table_cols(i);
                src.complete = src.cols.is_some();
                src.name = t.name.clone();
                resolution = Resolution::Table(id);
            }
            TableLookup::Missing => {
                resolution = Resolution::Unresolved(Problem::UnknownTable);
                let display = parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(".");
                let cands = self.table_candidates(sid);
                self.diag(
                    last.span.clone(),
                    format!("Unknown table '{display}'"),
                    DiagKind::UnknownTable { name: display.clone(), candidates: cands },
                );
            }
            TableLookup::Unknown => {}
        }
        self.add_ref(last, RefRole::Table, resolution, sid);
        if let Some(a) = alias {
            self.apply_alias(&mut src, a, sid);
        }
        self.scopes[sid].sources.push(src);
    }

    /// A column named in a list (`INSERT INTO t (a, b)`, `ON CONFLICT (a)`) of the target source.
    fn listed_column(&mut self, id: &Ident, sid: ScopeId, src_idx: usize) {
        let src = &self.scopes[sid].sources[src_idx];
        let Some(cols) = &src.cols else {
            self.add_ref(id, RefRole::ListedColumn, Resolution::Unknown, sid);
            return;
        };
        let hits = find_all(self.fam, id, cols, |c| c.name.as_str());
        if let Some(&h) = hits.first() {
            let r = self.col_resolution(sid, src_idx, h);
            self.add_ref(id, RefRole::ListedColumn, r, sid);
        } else if src.complete && self.lenient == 0 {
            let table = src.table.as_ref().map(|t| t.name.clone()).unwrap_or_else(|| src.name.clone());
            let cands: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
            self.add_ref(id, RefRole::ListedColumn, Resolution::Unresolved(Problem::UnknownColumn), sid);
            self.diag(
                id.span.clone(),
                format!("Unknown column '{}' in '{table}'", id.text),
                DiagKind::UnknownColumn { name: id.text.clone(), qualifier: Some(table), candidates: cands },
            );
        } else {
            self.add_ref(id, RefRole::ListedColumn, Resolution::Unknown, sid);
        }
    }

    fn pseudo_source(&mut self, to: ScopeId, name: &str, from: ScopeId, like: usize, span: Range<usize>) {
        let src = self.scopes[from].sources[like].clone();
        self.scopes[to].sources.push(Source {
            name: name.to_string(),
            quoted: false,
            kind: SourceKind::Pseudo,
            table: src.table,
            symbol: None,
            cols: src.cols,
            complete: src.complete,
            span,
            alias_span: None,
            merged: Vec::new(),
        });
    }

    fn on_conflict(&mut self, c: &Node, sid: ScopeId, base: Option<ScopeId>, target: Option<usize>) {
        let cs = self.new_scope(ScopeKind::Select, c.span.clone(), base);
        if let Some(t) = target {
            let src = self.scopes[sid].sources[t].clone();
            self.scopes[cs].sources.push(src);
            self.pseudo_source(cs, "excluded", cs, 0, c.span.clone());
        }
        for ch in c.nodes() {
            match ch.kind {
                Kind::ColumnList => {
                    if target.is_some() {
                        for nm in self.list_idents(ch) {
                            self.listed_column(&nm, cs, 0);
                        }
                    }
                }
                Kind::SetClause => self.assignments(ch, cs, target.map(|_| 0)),
                Kind::WhereClause => self.expr(ch, cs),
                _ => {}
            }
        }
    }

    fn returning(&mut self, r: &Node, sid: ScopeId, target: Option<usize>) {
        // T-SQL OUTPUT sees the `inserted` / `deleted` pseudo tables as well as the target.
        let rs = self.new_scope(ScopeKind::Select, r.span.clone(), Some(sid));
        if let Some(t) = target
            && self.fam == Family::SqlServer
        {
            self.pseudo_source(rs, "inserted", sid, t, r.span.clone());
            self.pseudo_source(rs, "deleted", sid, t, r.span.clone());
        }
        if let Some(list) = r.child(Kind::SelectList) {
            let mut out = QOut { cols: Vec::new(), complete: true, counted: true, scope: Some(rs) };
            for item in list.children_of(Kind::SelectItem) {
                self.select_item(item, rs, &mut out);
            }
        }
        if let Some(name) = r.child(Kind::Name) {
            let _ = name;
        }
    }

    /// `SET a = expr, ...` (UPDATE, INSERT ... SET, ON CONFLICT DO UPDATE, ON DUPLICATE KEY UPDATE).
    fn assignments(&mut self, set: &Node, sid: ScopeId, target: Option<usize>) {
        for a in set.children_of(Kind::Assignment) {
            let mut seen_target = false;
            for c in a.nodes() {
                if !seen_target {
                    seen_target = true;
                    self.assignment_target(c, sid, target);
                } else {
                    self.expr(c, sid);
                }
            }
        }
    }

    fn assignment_target(&mut self, t: &Node, sid: ScopeId, target: Option<usize>) {
        match t.kind {
            Kind::ColumnRef => {
                let parts = self.parts(t);
                if parts.len() == 1
                    && let Some(ti) = target
                {
                    // An unqualified target is a column of the target table, never of a FROM source.
                    self.listed_target(&parts[0], sid, ti);
                } else {
                    self.expr(t, sid);
                }
            }
            Kind::RowCtor => {
                for c in t.nodes() {
                    self.assignment_target(c, sid, target);
                }
            }
            Kind::Index => {
                for c in t.nodes() {
                    self.assignment_target(c, sid, target);
                }
            }
            _ => self.expr(t, sid),
        }
    }

    fn listed_target(&mut self, id: &Ident, sid: ScopeId, ti: usize) {
        let src = &self.scopes[sid].sources[ti];
        match &src.cols {
            Some(cols) => {
                let hits = find_all(self.fam, id, cols, |c| c.name.as_str());
                if let Some(&h) = hits.first() {
                    let r = self.col_resolution(sid, ti, h);
                    self.add_ref(id, RefRole::Column, r, sid);
                } else if src.complete && self.lenient == 0 && !names::is_system_column(&id.text) {
                    let table =
                        src.table.as_ref().map(|t| t.name.clone()).unwrap_or_else(|| src.name.clone());
                    let cands: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
                    self.add_ref(id, RefRole::Column, Resolution::Unresolved(Problem::UnknownColumn), sid);
                    self.diag(
                        id.span.clone(),
                        format!("Unknown column '{}' in '{table}'", id.text),
                        DiagKind::UnknownColumn {
                            name: id.text.clone(),
                            qualifier: Some(table),
                            candidates: cands,
                        },
                    );
                } else {
                    self.add_ref(id, RefRole::Column, Resolution::Unknown, sid);
                }
            }
            None => self.add_ref(id, RefRole::Column, Resolution::Unknown, sid),
        }
    }

    /// The target tables of UPDATE / DELETE: the nodes before the first SET / FROM / WHERE clause. A
    /// T-SQL / MySQL target may name an alias of the FROM clause instead of a table.
    fn dml_targets(&mut self, n: &Node, sid: ScopeId, from_first: bool) -> Option<usize> {
        let mut first_target: Option<usize> = None;
        if from_first {
            for pre in n.nodes().filter(|c| matches!(c.kind, Kind::FromClause | Kind::UsingClause)) {
                for c in pre.nodes() {
                    self.item(c, sid);
                }
            }
        }
        for c in n.nodes() {
            match c.kind {
                Kind::TableName => {
                    let name = c.child(Kind::Name);
                    let parts = name.map(|nm| self.parts(nm)).unwrap_or_default();
                    // an alias of the FROM clause?
                    if from_first && parts.len() == 1 && c.child(Kind::Alias).is_none() {
                        let hits =
                            find_all(self.fam, &parts[0], &self.scopes[sid].sources, |s| s.name.as_str());
                        if let Some(&h) = hits.first() {
                            let res = self.source_resolution(sid, h);
                            self.add_ref(&parts[0], RefRole::Qualifier, res, sid);
                            first_target.get_or_insert(h);
                            continue;
                        }
                    }
                    self.add_table(c, sid);
                    first_target.get_or_insert(self.scopes[sid].sources.len() - 1);
                }
                Kind::Join | Kind::DerivedTable | Kind::ParenFrom | Kind::TableFunc => {
                    let before = self.scopes[sid].sources.len();
                    self.item(c, sid);
                    if self.scopes[sid].sources.len() > before {
                        first_target.get_or_insert(before);
                    }
                }
                _ => {}
            }
        }
        first_target
    }

    fn update(&mut self, n: &Node) {
        let (sid, _base) = self.dml_scope(n, None);
        let target = self.dml_targets(n, sid, true);
        if let Some(f) = n.child(Kind::FromClause) {
            // when the targets did not already pull the FROM clause in (no alias match), the sources are in
            // scope from `dml_targets`; nothing more to do
            let _ = f;
        }
        for c in n.nodes() {
            match c.kind {
                Kind::SetClause => self.assignments(c, sid, target),
                Kind::WhereClause => self.expr(c, sid),
                Kind::Returning => self.returning(c, sid, target),
                Kind::OrderBy => self.order_by(c, sid),
                Kind::Limit => self.expr(c, sid),
                _ => {}
            }
        }
    }

    fn delete(&mut self, n: &Node) {
        let (sid, _base) = self.dml_scope(n, None);
        let target = self.dml_targets(n, sid, true);
        for c in n.nodes() {
            match c.kind {
                Kind::WhereClause => self.expr(c, sid),
                Kind::Returning => self.returning(c, sid, target),
                Kind::OrderBy => self.order_by(c, sid),
                Kind::Limit => self.expr(c, sid),
                _ => {}
            }
        }
    }

    fn merge(&mut self, n: &Node) {
        let (sid, _base) = self.dml_scope(n, None);
        // The target is the first factor, the source sits in the USING clause.
        let mut target: Option<usize> = None;
        for c in n.nodes() {
            if matches!(c.kind, Kind::TableName | Kind::DerivedTable | Kind::TableFunc) && target.is_none() {
                self.item(c, sid);
                target = Some(self.scopes[sid].sources.len() - 1);
            }
        }
        if let Some(u) = n.child(Kind::UsingClause) {
            for c in u.nodes() {
                self.item(c, sid);
            }
        }
        for c in n.nodes() {
            match c.kind {
                Kind::JoinConstraint => self.expr(c, sid),
                Kind::MergeClause => {
                    for m in c.nodes() {
                        match m.kind {
                            Kind::SetClause => self.assignments(m, sid, target),
                            Kind::ColumnList => {
                                if let Some(t) = target {
                                    for nm in self.list_idents(m) {
                                        self.listed_column(&nm, sid, t);
                                    }
                                }
                            }
                            Kind::Values => {
                                for row in m.children_of(Kind::RowCtor) {
                                    self.expr(row, sid);
                                }
                            }
                            _ => self.expr(m, sid),
                        }
                    }
                }
                Kind::Returning => self.returning(c, sid, target),
                _ => {}
            }
        }
    }

    // ---- GROUP BY

    /// A column in the select list that is neither grouped nor aggregated. Reported only where it is certain:
    /// PostgreSQL, DuckDB and T-SQL reject it (MySQL and MariaDB depend on `sql_mode`, SQLite allows it), the
    /// grouping is by plain columns, and the column is not determined by a grouped primary key.
    fn check_grouping(&mut self, sel: &Node, sid: ScopeId) {
        if !matches!(self.fam, Family::Postgres | Family::DuckDb | Family::SqlServer) || self.lenient > 0 {
            return;
        }
        let Some(list) = sel.child(Kind::SelectList) else { return };
        let group = sel.child(Kind::GroupBy);
        let has_aggregate = contains_aggregate(self.sql, list);
        if group.is_none() && !has_aggregate {
            return;
        }
        // Grouped expressions: only plain column references (or positions / aliases) are understood.
        let mut grouped_cols: Vec<Ident> = Vec::new();
        if let Some(g) = group {
            for e in g.nodes() {
                match e.kind {
                    Kind::ColumnRef => {
                        let p = self.parts(e);
                        if let Some(last) = p.last() {
                            grouped_cols.push(last.clone());
                        }
                    }
                    Kind::Literal => return, // GROUP BY 1: positions, not analysed
                    _ => return,             // an expression, ROLLUP, GROUPING SETS: not analysed
                }
            }
            if g.has_kw(self.sql, "ALL") {
                return;
            }
        }
        if sel.child(Kind::Having).is_some() && group.is_none() {
            return;
        }
        // Primary keys of the tables whose every key column is grouped: everything of that table is determined.
        let mut determined: Vec<TableId> = Vec::new();
        let cat = self.cat;
        if let Some(cat) = cat {
            for src in &self.scopes[sid].sources {
                if let Some(t) = &src.table
                    && !t.script
                    && let Some(table) = cat
                        .schemas
                        .iter()
                        .find(|s| Some(&s.name) == t.schema.as_ref())
                        .and_then(|s| s.tables.iter().find(|x| x.name == t.name))
                    && !table.primary_key.is_empty()
                    && table
                        .primary_key
                        .iter()
                        .all(|k| grouped_cols.iter().any(|g| names::loose_eq(&g.text, k)))
                {
                    determined.push(t.clone());
                }
            }
        }
        let mut reports: Vec<(Range<usize>, String)> = Vec::new();
        for item in list.children_of(Kind::SelectItem) {
            for e in item.nodes() {
                if e.kind == Kind::Alias {
                    continue;
                }
                self.ungrouped_in(e, sid, &grouped_cols, &determined, &mut reports);
            }
        }
        for (range, col) in reports {
            self.diag(
                range,
                format!(
                    "Column '{col}' must appear in the GROUP BY clause or be used in an aggregate function"
                ),
                DiagKind::NotGrouped { column: col },
            );
        }
    }

    fn ungrouped_in(
        &self,
        n: &Node,
        sid: ScopeId,
        grouped: &[Ident],
        determined: &[TableId],
        out: &mut Vec<(Range<usize>, String)>,
    ) {
        match n.kind {
            // Arguments of any call are left alone: an aggregate hides its columns, and a function this list does
            // not know may be an aggregate too.
            Kind::FuncCall => {}
            Kind::Subquery | Kind::Exists | Kind::Query | Kind::Select => {}
            Kind::ColumnRef => {
                let Some(last) = self.parts(n).last().cloned() else { return };
                let Some(r) = self.ref_index.get(&last.span.start).map(|&i| &self.refs[i]) else { return };
                if r.role != RefRole::Column {
                    return;
                }
                let Resolution::Column { table, column } = &r.resolution else { return };
                // only a column of a source of this very select can be (un)grouped here
                let in_scope = self.scopes[sid].sources.iter().any(|s| s.table.as_ref() == Some(table));
                if !in_scope || determined.contains(table) {
                    return;
                }
                if grouped.iter().any(|g| names::loose_eq(&g.text, column)) {
                    return;
                }
                out.push((r.span.clone(), r.name.clone()));
            }
            _ => {
                for c in n.nodes() {
                    self.ungrouped_in(c, sid, grouped, determined, out);
                }
            }
        }
    }
}

enum SrcFound {
    One(ScopeId, usize),
    Unknown,
    None,
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn is_aggregate_call(sql: &str, call: &Node) -> bool {
    const AGGS: &[&str] = &[
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
        "ARRAY_AGG",
        "STRING_AGG",
        "GROUP_CONCAT",
        "BOOL_AND",
        "BOOL_OR",
        "EVERY",
        "JSON_AGG",
        "JSONB_AGG",
        "JSON_OBJECT_AGG",
        "JSONB_OBJECT_AGG",
        "XMLAGG",
        "STDDEV",
        "STDDEV_POP",
        "STDDEV_SAMP",
        "VARIANCE",
        "VAR_POP",
        "VAR_SAMP",
        "BIT_AND",
        "BIT_OR",
        "BIT_XOR",
        "LISTAGG",
        "PERCENTILE_CONT",
        "PERCENTILE_DISC",
        "MODE",
        "COUNT_BIG",
        "CORR",
        "COVAR_POP",
        "COVAR_SAMP",
        "REGR_SLOPE",
        "ANY_VALUE",
        "FIRST",
        "LAST",
        "LIST",
        "ARG_MAX",
        "ARG_MIN",
        "MEDIAN",
        "HISTOGRAM",
        "JSON_ARRAYAGG",
        "JSON_OBJECTAGG",
        "RANK",
        "DENSE_RANK",
        "CUME_DIST",
        "PERCENT_RANK",
        "GROUPING",
    ];
    let name = call
        .child(Kind::Name)
        .and_then(|nm| nm.ident_tokens().last().map(|t| t.text(sql).to_ascii_uppercase()));
    name.is_some_and(|n| AGGS.contains(&n.as_str()))
}

fn contains_aggregate(sql: &str, n: &Node) -> bool {
    match n.kind {
        // a window function call is evaluated after grouping and does not make the query an aggregate query
        Kind::FuncCall if n.child(Kind::Over).is_some() => false,
        Kind::FuncCall if is_aggregate_call(sql, n) => true,
        Kind::Subquery | Kind::Exists | Kind::Query | Kind::Select => false,
        _ => n.nodes().any(|c| contains_aggregate(sql, c)),
    }
}
