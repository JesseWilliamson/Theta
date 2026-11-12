//! Name resolution over the syntax tree: scopes, declarations, references and the diagnostics that follow
//! from them.
//!
//! `analyze(sql, kind, catalog)` parses a script ([`crate::syntax`]) and walks every statement once with a
//! scope stack, producing
//!
//! * [`Symbol`]s: the names the script itself introduces (a table alias, a CTE, the columns a CTE or a
//!   subquery exposes, a select-list alias);
//! * [`Ref`]s: every identifier use with what it resolved to ([`Resolution`]);
//! * [`ScopeInfo`]s: for each query, the sources visible in it and their columns (what completion and the
//!   quick fixes want to know);
//! * [`Diag`]s: unknown / ambiguous names and a few certain structural problems.
//!
//! # The contract: "unknown", never a wrong guess
//!
//! A reference resolves to [`Resolution::Unresolved`] only when every source that could have supplied it is
//! fully known (catalog tables with their columns, CTEs and subqueries with named columns) and none did.
//! Anything else (an opaque construct, a table function, `SELECT *` over an unknown table, a schema that is
//! not in the catalog, a name the parser could not place) resolves to [`Resolution::Unknown`], which never
//! produces a diagnostic. `crates/theta-drivers/tests/sql_differential.rs` checks this against real servers:
//! no unresolved-name diagnostic may appear on a statement the server accepted against the demo schema.
//!
//! Case folding and quoting follow the dialect ([`names`]); where a name matches only after ignoring case
//! the lookup still succeeds, because a false "unknown column" is worse than a missed typo.

mod names;
mod resolve;

pub use resolve::analyze;

use crate::lint::Severity;
use crate::syntax::Parse;
use std::ops::Range;

pub type SymbolId = usize;
pub type ScopeId = usize;

/// What a script-local name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// `FROM orders AS o`, `FROM (select ...) o`, `FROM f() o`.
    TableAlias,
    /// `WITH x AS (...)`.
    Cte,
    /// A name in an alias list: `AS t (a, b)` or `WITH x (a, b) AS (...)`.
    ColumnAlias,
    /// A column a query exposes: a select-list alias (`select a + 1 as n`) or a plain column that passes through.
    OutputColumn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymbolKind,
    /// The name as written, without quotes.
    pub name: String,
    /// Span of the identifier token (quotes included) where the name is introduced. For a passthrough
    /// `OutputColumn` it is the span of the select item.
    pub span: Range<usize>,
    pub quoted: bool,
    /// Whether the name can be renamed by editing `span` (not for passthrough columns, whose name comes
    /// from a table).
    pub renamable: bool,
    /// The scope the name is introduced in.
    pub scope: ScopeId,
}

/// A table or view known to the catalog or created by the script.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableId {
    pub schema: Option<String>,
    pub name: String,
    /// Created by a statement of this script, not (necessarily) in the catalog.
    pub script: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A name this script introduced.
    Symbol(SymbolId),
    Table(TableId),
    Column {
        table: TableId,
        column: String,
    },
    Function(String),
    /// `excluded`, `inserted`, `deleted`, a lambda parameter: a name the grammar defines.
    Pseudo,
    /// Certainly not found.
    Unresolved(Problem),
    /// Cannot tell (opaque construct, incomplete catalog).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    UnknownTable,
    UnknownColumn,
    /// `x.col` where `x` is no table or alias in scope.
    UnknownQualifier,
    /// Found in several sources of the same scope; the qualifiers that would disambiguate.
    Ambiguous(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefRole {
    /// The identifier that introduces a symbol.
    Declaration,
    /// A table or CTE name in a FROM clause, `INSERT INTO`, `UPDATE`, ...
    Table,
    /// The `x` of `x.col`.
    Qualifier,
    Column,
    /// A column named in an INSERT column list, `ON CONFLICT (..)` or a USING list.
    ListedColumn,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// Span of the identifier token (quotes included).
    pub span: Range<usize>,
    pub role: RefRole,
    /// The identifier without quotes.
    pub name: String,
    pub quoted: bool,
    pub resolution: Resolution,
    pub scope: ScopeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// A `SELECT`, or the sources of a DML statement.
    Select,
    /// The CTEs of a WITH clause (the scope the query body hangs below).
    With,
    /// An empty scope under which VALUES rows and similar are resolved.
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Col {
    pub name: String,
    /// The symbol of a column a query exposes, when it has one.
    pub symbol: Option<SymbolId>,
    pub quoted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Table,
    Cte,
    Derived,
    Function,
    Pseudo,
    Unknown,
}

/// One entry of a FROM clause (or the target of a DML statement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The name the rest of the query uses: the alias, else the table's own name.
    pub name: String,
    pub quoted: bool,
    pub kind: SourceKind,
    pub table: Option<TableId>,
    /// The alias or CTE symbol.
    pub symbol: Option<SymbolId>,
    /// The columns, in order; `None` when unknown (table function, unknown table, `*` over such).
    pub cols: Option<Vec<Col>>,
    /// False when some column of `cols` has no usable name (`select count(*)` without alias): a name that
    /// is not listed may still exist.
    pub complete: bool,
    /// Span of the whole FROM item.
    pub span: Range<usize>,
    /// Span of the alias identifier, if the item has one.
    pub alias_span: Option<Range<usize>>,
    /// Folded names merged into an earlier source by USING / NATURAL.
    pub merged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeInfo {
    pub kind: ScopeKind,
    /// Span of the node the scope belongs to (a Select, or a DML statement).
    pub span: Range<usize>,
    pub parent: Option<ScopeId>,
    pub sources: Vec<Source>,
    /// CTEs introduced here (for `With` scopes), as symbols.
    pub ctes: Vec<SymbolId>,
    /// Select-list aliases (and named output columns), visible to ORDER BY and friends.
    pub outputs: Vec<SymbolId>,
}

/// A `*` or `t.*` in a select list and the columns it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarExpansion {
    /// Span of the whole select item (`*` or `t.*`).
    pub span: Range<usize>,
    pub scope: ScopeId,
    /// `(qualifier as written or the source's visible name, column)`, in order. Empty when `complete` is false.
    pub columns: Vec<(String, String)>,
    /// All sources were known; the list is exactly what the server would return.
    pub complete: bool,
    /// `t.*` rather than `*`.
    pub qualified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagKind {
    UnknownTable { name: String, candidates: Vec<String> },
    UnknownColumn { name: String, qualifier: Option<String>, candidates: Vec<String> },
    UnknownQualifier { name: String, candidates: Vec<String> },
    AmbiguousColumn { name: String, qualifiers: Vec<String> },
    DuplicateAlias { name: String },
    ColumnCount { expected: usize, found: usize },
    NotGrouped { column: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
    pub kind: DiagKind,
}

/// The result of [`analyze`]: the tree, plus everything resolution found out.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub parse: Parse,
    pub symbols: Vec<Symbol>,
    /// Sorted by span start.
    pub refs: Vec<Ref>,
    pub scopes: Vec<ScopeInfo>,
    pub stars: Vec<StarExpansion>,
    pub diagnostics: Vec<Diag>,
}

impl Analysis {
    /// The reference whose identifier contains `offset` (a cursor at either end counts).
    pub fn ref_at(&self, offset: usize) -> Option<&Ref> {
        let i = self.refs.partition_point(|r| r.span.end < offset);
        self.refs[i..]
            .iter()
            .take_while(|r| r.span.start <= offset)
            .find(|r| r.span.start <= offset && offset <= r.span.end)
    }

    /// All references that resolve to the symbol, the declaration included.
    pub fn refs_to_symbol(&self, id: SymbolId) -> impl Iterator<Item = &Ref> {
        self.refs.iter().filter(move |r| r.resolution == Resolution::Symbol(id))
    }
}

/// Unknown table / column / qualifier and ambiguity diagnostics only, as `(range, message)`. Used by the
/// differential test, which asserts that none appears on a statement the server accepted.
pub fn name_diagnostics(
    sql: &str,
    kind: theta_core::DatabaseKind,
    catalog: &theta_core::Catalog,
) -> Vec<(Range<usize>, String)> {
    analyze(sql, kind, Some(catalog)).diagnostics.into_iter().map(|d| (d.range, d.message)).collect()
}

#[cfg(test)]
mod tests;
