//! The DDL engine: a table as the user is editing it (a [`TableDraft`]), `CREATE TABLE` for it, and the
//! minimal, ordered `ALTER` script that turns one draft into another, per dialect.
//!
//! # Identity, not names
//!
//! Every column and constraint in a draft carries an `id` that survives edits. [`diff`] pairs the original
//! and the edited draft by id, so a rename is a rename (`RENAME COLUMN`), never a drop and an add that
//! would destroy the data. Names are never compared to guess intent.
//!
//! # What each statement says about itself
//!
//! A [`Statement`] carries [`Flag`]s (destructive, table rewrite, implicit commit, lock, runs outside a
//! transaction) and a one-line human explanation. The UI shows them next to the SQL and counts them in the
//! summary; nothing here executes anything.
//!
//! # Where the dialects differ
//!
//! * PostgreSQL has the richest `ALTER TABLE` and transactional DDL: the whole script can run in one
//!   transaction.
//! * MySQL / MariaDB commit implicitly after every DDL statement (each statement is flagged
//!   [`Flag::ImplicitCommit`]); a column change is one `MODIFY COLUMN` restating the whole definition.
//! * SQLite cannot change a column's type, nullability or default, nor add or drop keys and checks, with
//!   `ALTER TABLE`. Those changes use the documented table rebuild (create the new table, copy the rows,
//!   drop the old one, rename, recreate indexes), see <https://www.sqlite.org/lang_altertable.html>.

use crate::keywords::{qualified_name, quote_if_needed};
use crate::sql_literal;
use theta_core::Family;
use theta_core::{Catalog, DatabaseKind, GeneratedColumn, Identity, Table, TableKind, TypeKind, Value};

// ------------------------------------------------------------------- model

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDraft {
    /// Stable across edits; unique within the table.
    pub id: u32,
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub identity: Option<Identity>,
    pub generated: Option<GeneratedColumn>,
    pub comment: Option<String>,
}

/// A primary key or unique constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyDraft {
    pub id: u32,
    /// `None` lets the database name it.
    pub name: Option<String>,
    /// Column ids, in key order.
    pub columns: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyDraft {
    pub id: u32,
    pub name: Option<String>,
    pub columns: Vec<u32>,
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
    pub deferrable: bool,
    pub initially_deferred: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexDraft {
    pub id: u32,
    pub name: String,
    pub columns: Vec<u32>,
    pub unique: bool,
    pub method: Option<String>,
    pub predicate: Option<String>,
    /// The full `CREATE INDEX` of an index the draft cannot model (key parts that are expressions). It is
    /// carried through untouched; editing it is not supported.
    pub raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckDraft {
    pub id: u32,
    pub name: Option<String>,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableDraft {
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnDraft>,
    pub primary_key: Option<KeyDraft>,
    pub uniques: Vec<KeyDraft>,
    pub foreign_keys: Vec<ForeignKeyDraft>,
    pub indexes: Vec<IndexDraft>,
    pub checks: Vec<CheckDraft>,
    pub comment: Option<String>,
    /// Statements to run after a SQLite table rebuild, which drops the table's triggers with it (the
    /// caller fills this from `sqlite_master`; the catalog does not carry triggers).
    pub recreate: Vec<String>,
}

impl TableDraft {
    /// A draft of an existing table. Ids are assigned in catalog order starting at 1.
    pub fn from_table(schema: &str, t: &Table) -> TableDraft {
        let mut next = 0u32;
        let mut id = || {
            next += 1;
            next
        };
        let columns: Vec<ColumnDraft> = t
            .columns
            .iter()
            .map(|c| ColumnDraft {
                id: id(),
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: c.nullable,
                // The catalog spells identity in the default text too; the structured field wins.
                default: if c.identity.is_some() || c.generated.is_some() { None } else { c.default.clone() },
                identity: c.identity,
                generated: c.generated.clone(),
                comment: c.comment.clone(),
            })
            .collect();
        let ids = |names: &[String]| -> Vec<u32> {
            names.iter().filter_map(|n| columns.iter().find(|c| c.name == *n).map(|c| c.id)).collect()
        };
        let pk_name = t.indexes.iter().find(|i| i.primary).map(|i| i.name.clone());
        let primary_key = (!t.primary_key.is_empty()).then(|| KeyDraft {
            id: id(),
            // MySQL's key is always called PRIMARY; naming it would be an error.
            name: pk_name.filter(|n| n != "PRIMARY"),
            columns: ids(&t.primary_key),
        });
        let uniques = t
            .unique_constraints
            .iter()
            .map(|u| KeyDraft { id: id(), name: Some(u.name.clone()), columns: ids(&u.columns) })
            .collect();
        let foreign_keys = t
            .foreign_keys
            .iter()
            .map(|f| ForeignKeyDraft {
                id: id(),
                name: Some(f.name.clone()),
                columns: ids(&f.columns),
                ref_schema: f.referenced_schema.clone(),
                ref_table: f.referenced_table.clone(),
                ref_columns: f.referenced_columns.clone(),
                on_delete: f.on_delete.clone(),
                on_update: f.on_update.clone(),
                deferrable: f.deferrable,
                initially_deferred: f.initially_deferred,
            })
            .collect();
        let indexes = t
            .indexes
            .iter()
            // The primary key's and the unique constraints' indexes are part of those constraints.
            .filter(|i| !i.primary && !t.unique_constraints.iter().any(|u| u.name == i.name))
            .map(|i| IndexDraft {
                id: id(),
                name: i.name.clone(),
                columns: ids(&i.columns),
                unique: i.unique,
                method: i.method.clone(),
                predicate: i.predicate.clone(),
                raw: i.has_expressions.then(|| i.definition.clone()).flatten(),
            })
            .collect();
        let checks = t
            .checks
            .iter()
            .map(|c| CheckDraft { id: id(), name: Some(c.name.clone()), expression: c.expression.clone() })
            .collect();
        TableDraft {
            schema: Some(schema.to_string()).filter(|s| !s.is_empty()),
            name: t.name.clone(),
            columns,
            primary_key,
            uniques,
            foreign_keys,
            indexes,
            checks,
            comment: t.comment.clone(),
            recreate: Vec::new(),
        }
    }

    /// An id not used by anything in the draft, for a new column or constraint.
    pub fn next_id(&self) -> u32 {
        let mut max = 0;
        let mut see = |i: u32| max = max.max(i);
        self.columns.iter().for_each(|c| see(c.id));
        self.primary_key.iter().for_each(|k| see(k.id));
        self.uniques.iter().for_each(|k| see(k.id));
        self.foreign_keys.iter().for_each(|k| see(k.id));
        self.indexes.iter().for_each(|k| see(k.id));
        self.checks.iter().for_each(|k| see(k.id));
        max + 1
    }

    fn col(&self, id: u32) -> Option<&ColumnDraft> {
        self.columns.iter().find(|c| c.id == id)
    }

    fn names(&self, ids: &[u32]) -> Vec<String> {
        ids.iter().filter_map(|i| self.col(*i)).map(|c| c.name.clone()).collect()
    }
}

// --------------------------------------------------------------- statements

/// What a statement does beyond its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flag {
    /// Data or structure is lost: a dropped column, table or constraint, or a narrowing type change.
    Destructive,
    /// The server rewrites the whole table (slow on big tables, takes a lock for the duration).
    TableRewrite,
    /// MySQL / MariaDB commit before and after this statement: it cannot be rolled back.
    ImplicitCommit,
    /// A lock or scan worth knowing about, in words.
    Lock(String),
    /// Must run outside a transaction (SQLite `PRAGMA foreign_keys`).
    OutsideTransaction,
    /// Part of the SQLite table-rebuild procedure.
    Rebuild,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub sql: String,
    pub flags: Vec<Flag>,
    /// One line, for people.
    pub explanation: String,
}

impl Statement {
    pub(crate) fn new(sql: impl Into<String>, explanation: impl Into<String>) -> Self {
        Statement { sql: sql.into(), flags: Vec::new(), explanation: explanation.into() }
    }
    pub(crate) fn flag(mut self, f: Flag) -> Self {
        if !self.flags.contains(&f) {
            self.flags.push(f);
        }
        self
    }
    pub fn is_destructive(&self) -> bool {
        self.flags.contains(&Flag::Destructive)
    }
}

/// `3 statements · 1 destructive · 1 table rewrite`.
pub fn summarize(stmts: &[Statement]) -> String {
    if stmts.is_empty() {
        return "No changes".to_string();
    }
    let count = |f: fn(&Flag) -> bool| stmts.iter().filter(|s| s.flags.iter().any(f)).count();
    let mut parts = vec![theta_core::text::plural_s(stmts.len() as u64, "statement")];
    for (n, label) in [
        (count(|f| *f == Flag::Destructive), "destructive"),
        (count(|f| *f == Flag::TableRewrite), "table rewrite"),
        (count(|f| *f == Flag::ImplicitCommit), "implicit commit"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    parts.join(" · ")
}

/// Whether the whole script can run in one transaction on `kind`: PostgreSQL and SQLite have transactional
/// DDL; MySQL and MariaDB do not.
pub fn transactional_ddl(kind: DatabaseKind) -> bool {
    ddl_family(kind) != Family::MySql
}

// ------------------------------------------------------------- rendering

/// The dialect the generators write. DuckDB, SQL Server and ClickHouse have the table editor and the
/// DDL entry points disabled (`DatabaseKind::unsupported`), so these generators never run for them;
/// mapping them to the PostgreSQL branch only keeps every `match` total, and quoting still follows the
/// real kind through `DatabaseKind::quote_ident`.
fn ddl_family(kind: DatabaseKind) -> Family {
    match kind.family() {
        Family::DuckDb | Family::SqlServer | Family::ClickHouse => Family::Postgres,
        f => f,
    }
}

fn q(name: &str, kind: DatabaseKind) -> String {
    quote_if_needed(name, kind)
}

fn lit(s: &str, kind: DatabaseKind) -> String {
    sql_literal(&Value::Text(s.to_string()), kind)
}

fn qtable(d: &TableDraft, kind: DatabaseKind) -> String {
    qualified_name(d.schema.as_deref(), &d.name, kind)
}

fn cols_list(d: &TableDraft, ids: &[u32], kind: DatabaseKind) -> String {
    d.names(ids).iter().map(|n| q(n, kind)).collect::<Vec<_>>().join(", ")
}

pub(crate) fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

/// The column definition after its name: type, generation, nullability, default, identity, comment.
fn column_tail(c: &ColumnDraft, kind: DatabaseKind, inline_pk_autoincrement: bool) -> String {
    let mut s = c.data_type.trim().to_string();
    match ddl_family(kind) {
        Family::Postgres | Family::DuckDb | Family::SqlServer | Family::ClickHouse => {
            match (&c.identity, &c.generated) {
                (Some(Identity::Always), _) => s.push_str(" GENERATED ALWAYS AS IDENTITY"),
                (Some(Identity::ByDefault), _) => s.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                (None, Some(g)) => s.push_str(&format!(" GENERATED ALWAYS AS ({}) STORED", g.expression)),
                (None, None) => {}
            }
            if c.identity.is_none()
                && c.generated.is_none()
                && let Some(d) = &c.default
            {
                s.push_str(&format!(" DEFAULT {d}"));
            }
            if !c.nullable {
                s.push_str(" NOT NULL");
            }
        }
        Family::MySql => {
            if let Some(g) = &c.generated {
                s.push_str(&format!(
                    " GENERATED ALWAYS AS ({}) {}",
                    g.expression,
                    if g.stored { "STORED" } else { "VIRTUAL" }
                ));
            }
            s.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
            if c.generated.is_none() {
                if let Some(d) = &c.default {
                    s.push_str(&format!(" DEFAULT {d}"));
                }
                if c.identity.is_some() {
                    s.push_str(" AUTO_INCREMENT");
                }
            }
            if let Some(cm) = &c.comment {
                s.push_str(&format!(" COMMENT {}", lit(cm, kind)));
            }
        }
        Family::Sqlite => {
            if inline_pk_autoincrement {
                // SQLite spells the rowid alias this way and nowhere else.
                s.push_str(" PRIMARY KEY AUTOINCREMENT");
            }
            if let Some(g) = &c.generated {
                s.push_str(&format!(
                    " GENERATED ALWAYS AS ({}) {}",
                    g.expression,
                    if g.stored { "STORED" } else { "VIRTUAL" }
                ));
            } else if let Some(d) = &c.default {
                s.push_str(&format!(" DEFAULT {d}"));
            }
            if !c.nullable && !inline_pk_autoincrement {
                s.push_str(" NOT NULL");
            }
        }
    }
    s
}

fn fk_clause(d: &TableDraft, f: &ForeignKeyDraft, kind: DatabaseKind) -> String {
    let mut s = String::new();
    if let Some(n) = &f.name {
        s.push_str(&format!("CONSTRAINT {} ", q(n, kind)));
    }
    // SQLite takes the parent as a bare table name; the catalog reports its database as the schema.
    let parent_schema = if ddl_family(kind) == Family::Sqlite { None } else { f.ref_schema.as_deref() };
    s.push_str(&format!(
        "FOREIGN KEY ({}) REFERENCES {} ({})",
        cols_list(d, &f.columns, kind),
        qualified_name(parent_schema, &f.ref_table, kind),
        f.ref_columns.iter().map(|c| q(c, kind)).collect::<Vec<_>>().join(", ")
    ));
    for (what, v) in [("DELETE", &f.on_delete), ("UPDATE", &f.on_update)] {
        if let Some(a) = v.as_deref().filter(|a| !a.eq_ignore_ascii_case("NO ACTION")) {
            s.push_str(&format!(" ON {what} {}", a.to_ascii_uppercase()));
        }
    }
    if f.deferrable && ddl_family(kind) != Family::MySql {
        s.push_str(" DEFERRABLE");
        if f.initially_deferred {
            s.push_str(" INITIALLY DEFERRED");
        }
    }
    s
}

fn key_clause(d: &TableDraft, k: &KeyDraft, what: &str, kind: DatabaseKind) -> String {
    let name = match (&k.name, kind) {
        (Some(n), _) if !n.is_empty() => format!("CONSTRAINT {} ", q(n, kind)),
        _ => String::new(),
    };
    format!("{name}{what} ({})", cols_list(d, &k.columns, kind))
}

fn check_clause(c: &CheckDraft, kind: DatabaseKind) -> String {
    let name = c.name.as_ref().filter(|n| !n.is_empty()).map(|n| format!("CONSTRAINT {} ", q(n, kind)));
    format!("{}CHECK ({})", name.unwrap_or_default(), c.expression)
}

fn create_index_sql(d: &TableDraft, i: &IndexDraft, kind: DatabaseKind) -> String {
    if let Some(raw) = &i.raw {
        return raw.trim_end_matches(';').to_string();
    }
    let using = match (&i.method, ddl_family(kind)) {
        (Some(m), Family::Postgres) if !m.eq_ignore_ascii_case("btree") => format!(" USING {m}"),
        _ => String::new(),
    };
    // In MySQL, FULLTEXT and SPATIAL are kinds of index (`CREATE SPATIAL INDEX`), not `USING` methods;
    // `INFORMATION_SCHEMA.STATISTICS.INDEX_TYPE` reports them in the same column as BTREE and HASH.
    let mysql_kind = match (&i.method, ddl_family(kind)) {
        (Some(m), Family::MySql) if m.eq_ignore_ascii_case("fulltext") => "FULLTEXT ",
        (Some(m), Family::MySql) if m.eq_ignore_ascii_case("spatial") => "SPATIAL ",
        _ => "",
    };
    let mysql_using = match (&i.method, ddl_family(kind)) {
        (Some(m), Family::MySql) if mysql_kind.is_empty() && !m.eq_ignore_ascii_case("btree") => {
            format!(" USING {}", m.to_ascii_uppercase())
        }
        _ => String::new(),
    };
    let predicate = match (&i.predicate, ddl_family(kind)) {
        (Some(p), Family::Postgres | Family::Sqlite) => format!(" WHERE {p}"),
        _ => String::new(),
    };
    // An index lives in its table's schema. PostgreSQL does not accept a qualified name here (it takes the
    // table's schema), SQLite accepts one, and MySQL's index names belong to the table.
    let name = match ddl_family(kind) {
        Family::Sqlite => qualified_name(d.schema.as_deref(), &i.name, kind),
        _ => q(&i.name, kind),
    };
    let on = match ddl_family(kind) {
        Family::Sqlite => q(&d.name, kind),
        _ => qtable(d, kind),
    };
    format!(
        "CREATE {}{mysql_kind}INDEX {name} ON {on}{using} ({}){mysql_using}{predicate}",
        if i.unique { "UNIQUE " } else { "" },
        cols_list(d, &i.columns, kind)
    )
}

fn drop_index_sql(d: &TableDraft, i: &IndexDraft, kind: DatabaseKind) -> String {
    match ddl_family(kind) {
        Family::MySql => format!("DROP INDEX {} ON {}", q(&i.name, kind), qtable(d, kind)),
        _ => format!("DROP INDEX {}", qualified_name(d.schema.as_deref(), &i.name, kind)),
    }
}

/// `CREATE TABLE` for a draft, followed by the statements that are separate in some dialects (indexes,
/// Postgres comments).
pub fn create_table(d: &TableDraft, kind: DatabaseKind) -> Vec<Statement> {
    let table = qtable(d, kind);
    let sqlite_rowid_alias = sqlite_alias_column(d, kind);
    let mut lines: Vec<String> = Vec::new();
    for c in &d.columns {
        let inline = sqlite_rowid_alias == Some(c.id);
        lines.push(format!("{} {}", q(&c.name, kind), column_tail(c, kind, inline)));
    }
    if let Some(pk) = &d.primary_key
        && sqlite_rowid_alias.is_none()
    {
        lines.push(key_clause(d, pk, "PRIMARY KEY", kind));
    }
    for u in &d.uniques {
        lines.push(key_clause(d, u, "UNIQUE", kind));
    }
    for c in &d.checks {
        lines.push(check_clause(c, kind));
    }
    for f in &d.foreign_keys {
        lines.push(fk_clause(d, f, kind));
    }
    let mut sql = format!("CREATE TABLE {table} (\n");
    sql.push_str(&lines.iter().map(|l| format!("    {l}")).collect::<Vec<_>>().join(",\n"));
    sql.push_str("\n)");
    if ddl_family(kind) == Family::MySql
        && let Some(cm) = &d.comment
    {
        sql.push_str(&format!(" COMMENT = {}", lit(cm, kind)));
    }
    let mut out = vec![Statement::new(sql, format!("Create table {}", d.name))];
    if ddl_family(kind) == Family::MySql {
        out[0].flags.push(Flag::ImplicitCommit);
    }
    for i in &d.indexes {
        let mut s = Statement::new(create_index_sql(d, i, kind), format!("Create index {}", i.name));
        if ddl_family(kind) == Family::MySql {
            s.flags.push(Flag::ImplicitCommit);
        }
        out.push(s);
    }
    if ddl_family(kind) == Family::Postgres {
        if let Some(cm) = &d.comment {
            out.push(Statement::new(
                format!("COMMENT ON TABLE {table} IS {}", lit(cm, kind)),
                "Set the table comment",
            ));
        }
        for c in d.columns.iter().filter(|c| c.comment.is_some()) {
            out.push(Statement::new(
                format!(
                    "COMMENT ON COLUMN {table}.{} IS {}",
                    q(&c.name, kind),
                    lit(c.comment.as_deref().unwrap_or(""), kind)
                ),
                format!("Set the comment of {}", c.name),
            ));
        }
    }
    out
}

/// The column that SQLite must declare as `INTEGER PRIMARY KEY AUTOINCREMENT`: the identity column that is
/// the whole primary key.
fn sqlite_alias_column(d: &TableDraft, kind: DatabaseKind) -> Option<u32> {
    if ddl_family(kind) != Family::Sqlite {
        return None;
    }
    let pk = d.primary_key.as_ref()?;
    let [only] = pk.columns[..] else { return None };
    d.col(only)
        .filter(|c| c.identity.is_some() && c.data_type.trim().eq_ignore_ascii_case("integer"))
        .map(|c| c.id)
}

// -------------------------------------------------------------------- diff

pub(crate) fn same_col(a: &ColumnDraft, b: &ColumnDraft) -> bool {
    norm(&a.data_type) == norm(&b.data_type)
        && a.nullable == b.nullable
        && a.default.as_deref().map(str::trim) == b.default.as_deref().map(str::trim)
        && a.identity == b.identity
        && a.generated == b.generated
        && a.comment == b.comment
}

/// Constraint equality ignoring ids and column *names* (compare through ids, which survive renames).
pub(crate) fn same_key(a: &KeyDraft, b: &KeyDraft) -> bool {
    a.columns == b.columns && a.name == b.name
}

pub(crate) fn same_fk(a: &ForeignKeyDraft, b: &ForeignKeyDraft) -> bool {
    let norm_action =
        |x: &Option<String>| x.as_deref().map(str::to_ascii_uppercase).filter(|s| s != "NO ACTION");
    a.name == b.name
        && a.columns == b.columns
        && a.ref_schema == b.ref_schema
        && a.ref_table == b.ref_table
        && a.ref_columns == b.ref_columns
        && norm_action(&a.on_delete) == norm_action(&b.on_delete)
        && norm_action(&a.on_update) == norm_action(&b.on_update)
        && a.deferrable == b.deferrable
        && a.initially_deferred == b.initially_deferred
}

pub(crate) fn same_index(a: &IndexDraft, b: &IndexDraft) -> bool {
    a.name == b.name
        && a.columns == b.columns
        && a.unique == b.unique
        && a.method.as_deref().map(str::to_ascii_lowercase)
            == b.method.as_deref().map(str::to_ascii_lowercase)
        && a.predicate == b.predicate
        && a.raw == b.raw
}

fn mentions(text: &str, word: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let w = word.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find(&w) {
        let at = from + p;
        let ok = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        if ok(lower[..at].chars().next_back()) && ok(lower[at + w.len()..].chars().next()) {
            return true;
        }
        from = at + w.len();
    }
    false
}

/// Base type name without length or precision: `varchar(20)` is `varchar`.
fn base_type(t: &str) -> String {
    norm(t).split('(').next().unwrap_or("").trim().to_string()
}

/// The first number in parentheses, for `varchar(20)` / `numeric(10,2)`.
fn type_size(t: &str) -> Option<u32> {
    let a = t.find('(')?;
    t[a + 1..].split([',', ')']).next()?.trim().parse().ok()
}

/// Whether `from` -> `to` can lose data: a different base type, or a smaller length.
pub(crate) fn narrows(from: &str, to: &str) -> bool {
    if base_type(from) != base_type(to) {
        return true;
    }
    match (type_size(from), type_size(to)) {
        (Some(a), Some(b)) => b < a,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// The ordered `ALTER` script that turns `original` into `edited`. Empty when nothing changed.
pub fn diff(original: &TableDraft, edited: &TableDraft, kind: DatabaseKind) -> Vec<Statement> {
    if ddl_family(kind) == Family::Sqlite {
        return diff_sqlite(original, edited);
    }
    let mut out = Vec::new();
    let table = qtable(original, kind);
    let mysql = ddl_family(kind) == Family::MySql;
    let alter = |sql: String, why: String| {
        let s = Statement::new(sql, why);
        if mysql { s.flag(Flag::ImplicitCommit) } else { s }
    };

    // 1. Constraints and indexes that disappear or change come off first: they may depend on columns that
    //    are about to change or go.
    for f in &original.foreign_keys {
        if !edited.foreign_keys.iter().any(|e| e.id == f.id && same_fk(f, e)) {
            out.push(drop_foreign_key(original, f, kind));
        }
    }
    for c in &original.checks {
        if !edited
            .checks
            .iter()
            .any(|e| e.id == c.id && e.name == c.name && norm(&e.expression) == norm(&c.expression))
        {
            let name = c.name.clone().unwrap_or_default();
            out.push(
                alter(
                    format!("ALTER TABLE {table} DROP CONSTRAINT {}", q(&name, kind)),
                    format!("Drop check {name}"),
                )
                .flag(Flag::Destructive),
            );
        }
    }
    for u in &original.uniques {
        if !edited.uniques.iter().any(|e| e.id == u.id && same_key(u, e)) {
            let name = u.name.clone().unwrap_or_default();
            let sql = if mysql {
                format!("ALTER TABLE {table} DROP INDEX {}", q(&name, kind))
            } else {
                format!("ALTER TABLE {table} DROP CONSTRAINT {}", q(&name, kind))
            };
            out.push(alter(sql, format!("Drop unique constraint {name}")).flag(Flag::Destructive));
        }
    }
    if let Some(pk) = &original.primary_key
        && !edited.primary_key.as_ref().is_some_and(|e| e.id == pk.id && same_key(pk, e))
    {
        let sql = if mysql {
            format!("ALTER TABLE {table} DROP PRIMARY KEY")
        } else {
            // Postgres names the key `<table>_pkey` unless the catalog said otherwise.
            let name = pk.name.clone().unwrap_or_else(|| format!("{}_pkey", original.name));
            format!("ALTER TABLE {table} DROP CONSTRAINT {}", q(&name, kind))
        };
        out.push(alter(sql, "Drop the primary key".into()).flag(Flag::Destructive));
    }
    for i in &original.indexes {
        if !edited.indexes.iter().any(|e| e.id == i.id && same_index(i, e)) {
            out.push(
                alter(drop_index_sql(original, i, kind), format!("Drop index {}", i.name))
                    .flag(Flag::Destructive),
            );
        }
    }

    // 2. Columns: drops, renames, adds, changes.
    for c in &original.columns {
        if !edited.columns.iter().any(|e| e.id == c.id) {
            out.push(
                alter(
                    format!("ALTER TABLE {table} DROP COLUMN {}", q(&c.name, kind)),
                    format!("Drop column {} and all of its data", c.name),
                )
                .flag(Flag::Destructive),
            );
        }
    }
    for e in &edited.columns {
        if let Some(o) = original.col(e.id)
            && o.name != e.name
        {
            out.push(alter(
                format!("ALTER TABLE {table} RENAME COLUMN {} TO {}", q(&o.name, kind), q(&e.name, kind)),
                format!("Rename column {} to {}", o.name, e.name),
            ));
        }
    }
    for e in &edited.columns {
        if original.col(e.id).is_none() {
            out.extend(add_column(&table, e, kind));
        }
    }
    for e in &edited.columns {
        let Some(o) = original.col(e.id) else { continue };
        if same_col(o, e) {
            continue;
        }
        out.extend(change_column(&table, o, e, kind));
    }

    // 3. New and changed constraints and indexes.
    if let Some(pk) = &edited.primary_key
        && !original.primary_key.as_ref().is_some_and(|o| o.id == pk.id && same_key(o, pk))
    {
        out.push(
            alter(
                format!("ALTER TABLE {table} ADD {}", key_clause(edited, pk, "PRIMARY KEY", kind)),
                format!("Add the primary key ({})", edited.names(&pk.columns).join(", ")),
            )
            .flag(Flag::Lock("builds a unique index and fails if the data has duplicates or NULLs".into())),
        );
    }
    for u in &edited.uniques {
        if !original.uniques.iter().any(|o| o.id == u.id && same_key(o, u)) {
            out.push(alter(
                format!("ALTER TABLE {table} ADD {}", key_clause(edited, u, "UNIQUE", kind)),
                format!("Add a unique constraint on ({})", edited.names(&u.columns).join(", ")),
            ));
        }
    }
    for c in &edited.checks {
        if !original
            .checks
            .iter()
            .any(|o| o.id == c.id && o.name == c.name && norm(&o.expression) == norm(&c.expression))
        {
            out.push(
                alter(
                    format!("ALTER TABLE {table} ADD {}", check_clause(c, kind)),
                    format!("Add a check: {}", c.expression),
                )
                .flag(Flag::Lock("validates every existing row".into())),
            );
        }
    }
    let staged =
        TableDraft { name: original.name.clone(), schema: original.schema.clone(), ..edited.clone() };
    for f in &edited.foreign_keys {
        if !original.foreign_keys.iter().any(|o| o.id == f.id && same_fk(o, f)) {
            // The table is renamed last, so the statement says the old name.
            out.push(add_foreign_key(&staged, f, kind));
        }
    }
    // The table is renamed last, so everything before that still says the old name.
    for i in &edited.indexes {
        if !original.indexes.iter().any(|o| o.id == i.id && same_index(o, i)) {
            let mut s = alter(create_index_sql(&staged, i, kind), format!("Create index {}", i.name));
            if !mysql {
                s = s.flag(Flag::Lock(
                    "blocks writes while it builds (CREATE INDEX CONCURRENTLY avoids that)".into(),
                ));
            }
            out.push(s);
        }
    }

    // 4. The table itself.
    if original.comment != edited.comment {
        let text = edited.comment.clone();
        out.push(match (ddl_family(kind), text) {
            (Family::Postgres, t) => Statement::new(
                format!(
                    "COMMENT ON TABLE {table} IS {}",
                    t.as_deref().map_or("NULL".to_string(), |t| lit(t, kind))
                ),
                "Change the table comment",
            ),
            (_, t) => alter(
                format!("ALTER TABLE {table} COMMENT = {}", lit(t.as_deref().unwrap_or(""), kind)),
                "Change the table comment".into(),
            ),
        });
    }
    if original.name != edited.name {
        out.push(alter(
            format!("ALTER TABLE {table} RENAME TO {}", q(&edited.name, kind)),
            format!("Rename the table to {}", edited.name),
        ));
    }
    out
}

/// `ALTER TABLE ... DROP FOREIGN KEY` (MySQL) or `DROP CONSTRAINT`. Public because a schema comparison orders
/// foreign keys separately from the rest of a table's changes: they come off before any table is dropped and go
/// back on after every table exists.
pub fn drop_foreign_key(d: &TableDraft, f: &ForeignKeyDraft, kind: DatabaseKind) -> Statement {
    let table = qtable(d, kind);
    let name = f.name.clone().unwrap_or_default();
    let mysql = ddl_family(kind) == Family::MySql;
    let sql = if mysql {
        format!("ALTER TABLE {table} DROP FOREIGN KEY {}", q(&name, kind))
    } else {
        format!("ALTER TABLE {table} DROP CONSTRAINT {}", q(&name, kind))
    };
    let s = Statement::new(sql, format!("Drop foreign key {name}")).flag(Flag::Destructive);
    if mysql { s.flag(Flag::ImplicitCommit) } else { s }
}

/// `ALTER TABLE ... ADD FOREIGN KEY`, see [`drop_foreign_key`].
pub fn add_foreign_key(d: &TableDraft, f: &ForeignKeyDraft, kind: DatabaseKind) -> Statement {
    let table = qtable(d, kind);
    let s = Statement::new(
        format!("ALTER TABLE {table} ADD {}", fk_clause(d, f, kind)),
        format!("Add a foreign key from ({}) to {}", d.names(&f.columns).join(", "), f.ref_table),
    )
    .flag(Flag::Lock("validates every existing row against the referenced table".into()));
    if ddl_family(kind) == Family::MySql { s.flag(Flag::ImplicitCommit) } else { s }
}

fn add_column(table: &str, e: &ColumnDraft, kind: DatabaseKind) -> Vec<Statement> {
    let mysql = ddl_family(kind) == Family::MySql;
    let mut s = Statement::new(
        format!("ALTER TABLE {table} ADD COLUMN {} {}", q(&e.name, kind), column_tail(e, kind, false)),
        format!("Add column {}", e.name),
    );
    if mysql {
        s = s.flag(Flag::ImplicitCommit);
    }
    let volatile_default = e.default.as_deref().is_some_and(|d| {
        let d = d.to_ascii_lowercase();
        ["random(", "nextval(", "uuid", "gen_random"].iter().any(|v| d.contains(v))
    });
    if e.generated.as_ref().is_some_and(|g| g.stored) || e.identity.is_some() || volatile_default {
        s = s.flag(Flag::TableRewrite);
    }
    if !e.nullable && e.default.is_none() && e.identity.is_none() && e.generated.is_none() {
        s = s.flag(Flag::Lock("fails if the table already has rows (NOT NULL without a default)".into()));
    }
    let mut out = vec![s];
    if ddl_family(kind) == Family::Postgres
        && let Some(c) = &e.comment
    {
        out.push(Statement::new(
            format!("COMMENT ON COLUMN {table}.{} IS {}", q(&e.name, kind), lit(c, kind)),
            format!("Set the comment of {}", e.name),
        ));
    }
    out
}

fn change_column(table: &str, o: &ColumnDraft, e: &ColumnDraft, kind: DatabaseKind) -> Vec<Statement> {
    let name = q(&e.name, kind);
    if ddl_family(kind) == Family::MySql {
        // One MODIFY restates the whole definition, so every attribute change travels together.
        let mut s = Statement::new(
            format!("ALTER TABLE {table} MODIFY COLUMN {name} {}", column_tail(e, kind, false)),
            format!("Change the definition of {}", e.name),
        )
        .flag(Flag::ImplicitCommit);
        if norm(&o.data_type) != norm(&e.data_type) || o.nullable != e.nullable || o.generated != e.generated
        {
            s = s.flag(Flag::TableRewrite);
        }
        if norm(&o.data_type) != norm(&e.data_type) && narrows(&o.data_type, &e.data_type) {
            s = s.flag(Flag::Destructive);
        }
        if o.nullable && !e.nullable {
            s = s.flag(Flag::Lock("fails if any row holds NULL".into()));
        }
        return vec![s];
    }
    let mut out = Vec::new();
    let alter = |sql: String, why: String| {
        Statement::new(format!("ALTER TABLE {table} ALTER COLUMN {name} {sql}"), why)
    };
    if o.generated != e.generated {
        // PostgreSQL can only drop a generation expression (or change it from 17 on); recreating the
        // column is the version-independent way, and loses its stored values.
        out.push(
            Statement::new(
                format!("ALTER TABLE {table} DROP COLUMN {name}"),
                format!("Recreate generated column {} (drop)", e.name),
            )
            .flag(Flag::Destructive),
        );
        out.extend(add_column(table, e, kind));
        return out;
    }
    if norm(&o.data_type) != norm(&e.data_type) {
        let using = if base_type(&o.data_type) != base_type(&e.data_type) {
            format!(" USING {name}::{}", e.data_type.trim())
        } else {
            String::new()
        };
        let mut s = alter(
            format!("TYPE {}{using}", e.data_type.trim()),
            format!("Change the type of {} to {}", e.name, e.data_type.trim()),
        );
        let widening_varchar =
            base_type(&o.data_type) == base_type(&e.data_type) && !narrows(&o.data_type, &e.data_type);
        if !widening_varchar {
            s = s.flag(Flag::TableRewrite);
        }
        if narrows(&o.data_type, &e.data_type) {
            s = s.flag(Flag::Destructive);
        }
        out.push(s);
    }
    if o.identity != e.identity {
        out.push(match (&o.identity, &e.identity) {
            (None, Some(Identity::Always)) => alter(
                "ADD GENERATED ALWAYS AS IDENTITY".into(),
                format!("Make {} an identity column", e.name),
            ),
            (None, Some(Identity::ByDefault)) => alter(
                "ADD GENERATED BY DEFAULT AS IDENTITY".into(),
                format!("Make {} an identity column", e.name),
            ),
            (Some(_), Some(Identity::Always)) => {
                alter("SET GENERATED ALWAYS".into(), format!("Identity of {} is now ALWAYS", e.name))
            }
            (Some(_), Some(Identity::ByDefault)) => {
                alter("SET GENERATED BY DEFAULT".into(), format!("Identity of {} is now BY DEFAULT", e.name))
            }
            (_, None) => alter("DROP IDENTITY".into(), format!("{} is no longer an identity column", e.name)),
        });
    }
    if o.default.as_deref().map(str::trim) != e.default.as_deref().map(str::trim) && e.identity.is_none() {
        out.push(match &e.default {
            Some(d) => alter(format!("SET DEFAULT {d}"), format!("Set the default of {}", e.name)),
            None => alter("DROP DEFAULT".into(), format!("Remove the default of {}", e.name)),
        });
    }
    if o.nullable != e.nullable {
        out.push(if e.nullable {
            alter("DROP NOT NULL".into(), format!("Allow NULL in {}", e.name))
        } else {
            alter("SET NOT NULL".into(), format!("Forbid NULL in {}", e.name))
                .flag(Flag::Lock("scans the table and fails if any row holds NULL".into()))
        });
    }
    if o.comment != e.comment {
        out.push(Statement::new(
            format!(
                "COMMENT ON COLUMN {table}.{name} IS {}",
                e.comment.as_deref().map_or("NULL".to_string(), |c| lit(c, kind))
            ),
            format!("Change the comment of {}", e.name),
        ));
    }
    out
}

// ------------------------------------------------------------------ SQLite

/// Whether `ALTER TABLE ... DROP COLUMN` can drop `c` in SQLite: it refuses columns that are keys, indexed,
/// in foreign keys, or mentioned by a check or a generated column.
fn sqlite_can_drop(d: &TableDraft, c: &ColumnDraft) -> bool {
    let in_ids = |ids: &[u32]| ids.contains(&c.id);
    !(d.primary_key.as_ref().is_some_and(|k| in_ids(&k.columns))
        || d.uniques.iter().any(|k| in_ids(&k.columns))
        || d.indexes.iter().any(|i| in_ids(&i.columns) || i.raw.is_some())
        || d.foreign_keys.iter().any(|f| in_ids(&f.columns))
        || d.checks.iter().any(|k| mentions(&k.expression, &c.name))
        || d.columns.iter().any(|o| o.generated.as_ref().is_some_and(|g| mentions(&g.expression, &c.name))))
}

fn diff_sqlite(original: &TableDraft, edited: &TableDraft) -> Vec<Statement> {
    let kind = DatabaseKind::Sqlite;
    let table = q(&original.name, kind);
    let mut out = Vec::new();
    let mut rebuild = false;
    let mut reasons: Vec<String> = Vec::new();

    let removed: Vec<&ColumnDraft> =
        original.columns.iter().filter(|c| !edited.columns.iter().any(|e| e.id == c.id)).collect();
    let added: Vec<&ColumnDraft> = edited.columns.iter().filter(|e| original.col(e.id).is_none()).collect();
    let keys_changed = original.primary_key.as_ref().map(|k| (k.id, &k.columns, &k.name))
        != edited.primary_key.as_ref().map(|k| (k.id, &k.columns, &k.name));
    let uniques_changed = original.uniques.len() != edited.uniques.len()
        || original.uniques.iter().any(|u| !edited.uniques.iter().any(|e| e.id == u.id && same_key(u, e)));
    let fks_changed = original.foreign_keys.len() != edited.foreign_keys.len()
        || original
            .foreign_keys
            .iter()
            .any(|f| !edited.foreign_keys.iter().any(|e| e.id == f.id && same_fk(f, e)));
    let checks_changed = original.checks.len() != edited.checks.len()
        || original.checks.iter().any(|c| {
            !edited
                .checks
                .iter()
                .any(|e| e.id == c.id && e.name == c.name && norm(&e.expression) == norm(&c.expression))
        });
    if keys_changed {
        reasons.push("the primary key changes".into());
    }
    if uniques_changed {
        reasons.push("unique constraints change".into());
    }
    if fks_changed {
        reasons.push("foreign keys change".into());
    }
    if checks_changed {
        reasons.push("check constraints change".into());
    }
    for e in &edited.columns {
        if let Some(o) = original.col(e.id) {
            let ddl_changed = norm(&o.data_type) != norm(&e.data_type)
                || o.nullable != e.nullable
                || o.default.as_deref().map(str::trim) != e.default.as_deref().map(str::trim)
                || o.identity != e.identity
                || o.generated != e.generated;
            if ddl_changed {
                reasons.push(format!("the definition of {} changes", e.name));
            }
        }
    }
    for c in &removed {
        if !sqlite_can_drop(original, c) {
            reasons
                .push(format!("{} is part of a key, index, foreign key, check or generated column", c.name));
        }
    }
    for c in &added {
        let stored_generated = c.generated.as_ref().is_some_and(|g| g.stored);
        let not_null_no_default = !c.nullable && c.default.is_none();
        let in_key = edited.primary_key.as_ref().is_some_and(|k| k.columns.contains(&c.id));
        if stored_generated || not_null_no_default || in_key || c.identity.is_some() {
            reasons.push(format!("column {} cannot be added in place", c.name));
        }
    }
    rebuild = rebuild || !reasons.is_empty();

    if rebuild {
        out.extend(sqlite_rebuild(original, edited, &reasons));
        return out;
    }

    // In-place changes: indexes, drops, renames, adds, table rename.
    for i in &original.indexes {
        if !edited.indexes.iter().any(|e| e.id == i.id && same_index(i, e)) {
            out.push(
                Statement::new(drop_index_sql(original, i, kind), format!("Drop index {}", i.name))
                    .flag(Flag::Destructive),
            );
        }
    }
    for c in &removed {
        out.push(
            Statement::new(
                format!("ALTER TABLE {table} DROP COLUMN {}", q(&c.name, kind)),
                format!("Drop column {} and all of its data", c.name),
            )
            .flag(Flag::Destructive),
        );
    }
    for e in &edited.columns {
        if let Some(o) = original.col(e.id)
            && o.name != e.name
        {
            out.push(Statement::new(
                format!("ALTER TABLE {table} RENAME COLUMN {} TO {}", q(&o.name, kind), q(&e.name, kind)),
                format!("Rename column {} to {}", o.name, e.name),
            ));
        }
    }
    for c in &added {
        out.push(Statement::new(
            format!("ALTER TABLE {table} ADD COLUMN {} {}", q(&c.name, kind), column_tail(c, kind, false)),
            format!("Add column {}", c.name),
        ));
    }
    for i in &edited.indexes {
        if !original.indexes.iter().any(|o| o.id == i.id && same_index(o, i)) {
            // Index definitions name columns, and columns may have been renamed above: render from the
            // edited draft, which already has the new names.
            let staged = TableDraft { name: original.name.clone(), ..edited.clone() };
            out.push(Statement::new(create_index_sql(&staged, i, kind), format!("Create index {}", i.name)));
        }
    }
    if original.name != edited.name {
        out.push(Statement::new(
            format!("ALTER TABLE {table} RENAME TO {}", q(&edited.name, kind)),
            format!("Rename the table to {}", edited.name),
        ));
    }
    out
}

/// The documented 12-step rebuild (the steps SQLite lists as 1, 2, 4..10, 11 and 12; step 3 and 5 are the
/// `CREATE`/`INSERT`; the transaction in step 2 is the caller's).
fn sqlite_rebuild(original: &TableDraft, edited: &TableDraft, reasons: &[String]) -> Vec<Statement> {
    let kind = DatabaseKind::Sqlite;
    let why =
        format!("SQLite cannot do this with ALTER TABLE ({}), so the table is rebuilt", reasons.join("; "));
    let temp_name = format!("{}__new", edited.name);
    let mut staged = edited.clone();
    staged.name = temp_name.clone();
    // Indexes are recreated after the rename, under the final table name.
    staged.indexes.clear();
    let mut out = vec![
        Statement::new(
            "PRAGMA foreign_keys = OFF",
            "Stop foreign keys from cascading while the table is replaced",
        )
        .flag(Flag::OutsideTransaction)
        .flag(Flag::Rebuild),
    ];
    let mut create = create_table(&staged, kind);
    let first = create.remove(0);
    out.push(Statement { explanation: format!("Create the new table. {why}"), ..first }.flag(Flag::Rebuild));
    // Copy the columns that exist on both sides, matched by id; generated columns compute themselves.
    let pairs: Vec<(&ColumnDraft, &ColumnDraft)> = edited
        .columns
        .iter()
        .filter(|e| e.generated.is_none())
        .filter_map(|e| original.col(e.id).filter(|o| o.generated.is_none()).map(|o| (o, e)))
        .collect();
    if !pairs.is_empty() {
        out.push(
            Statement::new(
                format!(
                    "INSERT INTO {} ({}) SELECT {} FROM {}",
                    q(&temp_name, kind),
                    pairs.iter().map(|(_, e)| q(&e.name, kind)).collect::<Vec<_>>().join(", "),
                    pairs.iter().map(|(o, _)| q(&o.name, kind)).collect::<Vec<_>>().join(", "),
                    q(&original.name, kind)
                ),
                "Copy every row into the new table",
            )
            .flag(Flag::Rebuild),
        );
    }
    out.push(
        Statement::new(
            format!("DROP TABLE {}", q(&original.name, kind)),
            format!("Drop the old table {}", original.name),
        )
        .flag(Flag::Destructive)
        .flag(Flag::Rebuild),
    );
    out.push(
        Statement::new(
            format!("ALTER TABLE {} RENAME TO {}", q(&temp_name, kind), q(&edited.name, kind)),
            "Give the new table the old table's name",
        )
        .flag(Flag::Rebuild),
    );
    for i in &edited.indexes {
        out.push(
            Statement::new(create_index_sql(edited, i, kind), format!("Recreate index {}", i.name))
                .flag(Flag::Rebuild),
        );
    }
    for sql in &edited.recreate {
        out.push(Statement::new(sql.clone(), "Recreate a trigger the rebuild dropped").flag(Flag::Rebuild));
    }
    out.push(
        Statement::new("PRAGMA foreign_key_check", "Report any foreign key the rebuilt table now violates")
            .flag(Flag::Rebuild),
    );
    out.push(
        Statement::new("PRAGMA foreign_keys = ON", "Turn foreign key enforcement back on")
            .flag(Flag::OutsideTransaction)
            .flag(Flag::Rebuild),
    );
    out
}

/// Classify a free-text SQL type. Unknown names are `Other`, which converts to an untyped literal the
/// server coerces (that is what lets a `geometry` or `inet` column accept text).
pub fn kind_of_type_name(sql_type: &str) -> TypeKind {
    let t = sql_type.trim().to_ascii_lowercase();
    let base = t.split(['(', ' ']).next().unwrap_or("");
    let has = |w: &str| t.contains(w);
    if base == "tinyint" && t.starts_with("tinyint(1)") {
        return TypeKind::Bool;
    }
    match base {
        "bool" | "boolean" => TypeKind::Bool,
        "smallint" | "int" | "integer" | "bigint" | "tinyint" | "mediumint" | "int2" | "int4" | "int8"
        | "serial" | "bigserial" | "smallserial" | "year" => TypeKind::Integer,
        "numeric" | "decimal" | "money" | "number" | "dec" => TypeKind::Decimal,
        "real" | "float" | "float4" | "float8" | "double" => TypeKind::Float,
        "date" => TypeKind::Date,
        "time" | "timetz" => TypeKind::Time,
        "timestamp" | "timestamptz" | "datetime" => TypeKind::Timestamp,
        "json" | "jsonb" => TypeKind::Json,
        "uuid" => TypeKind::Uuid,
        "bytea" | "blob" | "binary" | "varbinary" | "longblob" | "mediumblob" | "tinyblob" => TypeKind::Bytes,
        "interval" => TypeKind::Interval,
        "text" | "varchar" | "char" | "character" | "nvarchar" | "nchar" | "longtext" | "mediumtext"
        | "tinytext" | "citext" | "string" | "clob" | "enum" => TypeKind::Text,
        _ if has("char") || has("text") => TypeKind::Text,
        _ => TypeKind::Other,
    }
}

// ------------------------------------------------------- editor vocabulary

/// Column types the editor offers for `kind`, parameters included. The list is only a convenience:
/// any text is accepted, because a server knows far more types (extensions, domains, enums) than a list can.
pub fn type_suggestions(kind: DatabaseKind) -> &'static [&'static str] {
    // Products of one family with a different type vocabulary come first.
    if kind == DatabaseKind::Redshift {
        // Redshift's documented data types (it has no json/jsonb/uuid/array columns; SUPER holds JSON).
        return &[
            "integer",
            "bigint",
            "smallint",
            "varchar(255)",
            "char(1)",
            "boolean",
            "decimal(12,2)",
            "real",
            "double precision",
            "date",
            "time",
            "timetz",
            "timestamp",
            "timestamptz",
            "super",
            "varbyte(255)",
            "geometry",
            "geography",
            "hllsketch",
        ];
    }
    match kind.family() {
        Family::DuckDb => {
            return &[
                "INTEGER",
                "BIGINT",
                "SMALLINT",
                "TINYINT",
                "HUGEINT",
                "UBIGINT",
                "VARCHAR",
                "BOOLEAN",
                "DECIMAL(18,3)",
                "REAL",
                "DOUBLE",
                "UUID",
                "DATE",
                "TIME",
                "TIMESTAMP",
                "TIMESTAMPTZ",
                "INTERVAL",
                "JSON",
                "BLOB",
                "BIT",
                "INTEGER[]",
                "STRUCT(a INTEGER, b VARCHAR)",
                "MAP(VARCHAR, INTEGER)",
                "ENUM('a', 'b')",
            ];
        }
        Family::SqlServer => {
            return &[
                "int",
                "bigint",
                "smallint",
                "tinyint",
                "bit",
                "nvarchar(255)",
                "nvarchar(max)",
                "varchar(255)",
                "nchar(10)",
                "char(10)",
                "decimal(12,2)",
                "money",
                "float",
                "real",
                "date",
                "time",
                "datetime2",
                "datetimeoffset",
                "smalldatetime",
                "uniqueidentifier",
                "varbinary(max)",
                "xml",
                "geometry",
                "geography",
                "sql_variant",
                "rowversion",
            ];
        }
        Family::ClickHouse => {
            return &[
                "UInt8",
                "UInt32",
                "UInt64",
                "Int32",
                "Int64",
                "Float32",
                "Float64",
                "Decimal(18, 4)",
                "String",
                "FixedString(16)",
                "Bool",
                "Date",
                "DateTime",
                "DateTime64(3)",
                "UUID",
                "IPv4",
                "IPv6",
                "LowCardinality(String)",
                "Nullable(String)",
                "Array(String)",
                "Map(String, String)",
                "Tuple(String, UInt32)",
                "JSON",
                "Enum8('a' = 1, 'b' = 2)",
            ];
        }
        _ => {}
    }
    match ddl_family(kind) {
        Family::Postgres | Family::DuckDb | Family::SqlServer | Family::ClickHouse => &[
            "integer",
            "bigint",
            "smallint",
            "text",
            "varchar(255)",
            "char(1)",
            "boolean",
            "numeric(12,2)",
            "real",
            "double precision",
            "uuid",
            "date",
            "time",
            "timestamp",
            "timestamptz",
            "interval",
            "json",
            "jsonb",
            "bytea",
            "inet",
            "cidr",
            "text[]",
            "integer[]",
        ],
        Family::MySql => &[
            "int",
            "bigint",
            "smallint",
            "tinyint",
            "tinyint(1)",
            "varchar(255)",
            "char(1)",
            "text",
            "mediumtext",
            "longtext",
            "decimal(12,2)",
            "float",
            "double",
            "date",
            "time",
            "datetime",
            "timestamp",
            "year",
            "json",
            "blob",
            "longblob",
            "binary(16)",
            "bit(1)",
        ],
        Family::Sqlite => &[
            "INTEGER",
            "TEXT",
            "REAL",
            "NUMERIC",
            "BLOB",
            "BOOLEAN",
            "DATE",
            "DATETIME",
            "TIMESTAMP",
            "VARCHAR(255)",
            "DECIMAL(12,2)",
            "JSON",
        ],
    }
}

/// The suggestions that match what has been typed so far: by prefix of the whole text first, then of the
/// base name, in list order. An exact match alone is not offered (there is nothing left to complete).
pub fn matching_types(kind: DatabaseKind, typed: &str) -> Vec<&'static str> {
    let t = typed.trim().to_ascii_lowercase();
    let all = type_suggestions(kind);
    if t.is_empty() {
        return all.to_vec();
    }
    let mut out: Vec<&'static str> =
        all.iter().copied().filter(|s| s.to_ascii_lowercase().starts_with(&t)).collect();
    let rest: Vec<&'static str> =
        all.iter().copied().filter(|s| !out.contains(s) && s.to_ascii_lowercase().contains(&t)).collect();
    out.extend(rest);
    out.retain(|s| !s.eq_ignore_ascii_case(&t));
    out
}

/// Index access methods the dialect lets `CREATE INDEX` choose. SQLite has none.
pub fn index_methods(kind: DatabaseKind) -> &'static [&'static str] {
    match ddl_family(kind) {
        Family::Postgres | Family::DuckDb | Family::SqlServer | Family::ClickHouse => {
            &["btree", "hash", "gin", "gist", "brin", "spgist"]
        }
        Family::MySql => &["BTREE", "HASH"],
        Family::Sqlite => &[],
    }
}

/// `ON DELETE` / `ON UPDATE` actions. InnoDB parses `SET DEFAULT` and then refuses it, so MySQL does not
/// get to pick it.
pub fn fk_actions(kind: DatabaseKind) -> &'static [&'static str] {
    match ddl_family(kind) {
        Family::MySql => &["NO ACTION", "RESTRICT", "CASCADE", "SET NULL"],
        _ => &["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"],
    }
}

/// The identity choices of a column, in the order the editor cycles through them. The label is the
/// dialect's own word for it.
pub fn identity_choices(kind: DatabaseKind) -> &'static [(Option<Identity>, &'static str)] {
    match ddl_family(kind) {
        Family::Postgres | Family::DuckDb | Family::SqlServer | Family::ClickHouse => {
            &[(None, ""), (Some(Identity::ByDefault), "by default"), (Some(Identity::Always), "always")]
        }
        Family::MySql => &[(None, ""), (Some(Identity::ByDefault), "AUTO_INCREMENT")],
        Family::Sqlite => &[(None, ""), (Some(Identity::ByDefault), "AUTOINCREMENT")],
    }
}

/// Whether the dialect stores a comment on a column (SQLite has no such thing).
pub fn supports_column_comments(kind: DatabaseKind) -> bool {
    ddl_family(kind) != Family::Sqlite
}

/// Whether `CREATE INDEX ... WHERE` exists.
pub fn supports_partial_indexes(kind: DatabaseKind) -> bool {
    ddl_family(kind) != Family::MySql
}

/// Whether a foreign key can be `DEFERRABLE`.
pub fn supports_deferrable(kind: DatabaseKind) -> bool {
    ddl_family(kind) != Family::MySql
}

/// Whether the table comment can be edited. SQLite has no table comments either.
pub fn supports_table_comment(kind: DatabaseKind) -> bool {
    ddl_family(kind) != Family::Sqlite
}

// ---------------------------------------------------------------- running

/// How a script relates to the session's own transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// Auto-commit: the script brings its own transaction where the dialect has transactional DDL.
    Own,
    /// Manual-commit (or a transaction is already open): the statements join it and nothing commits.
    /// `begin` opens the transaction first when none is open yet.
    Joined { begin: Vec<String> },
}

/// One thing to send to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub sql: String,
    /// The statement of the script this step runs; `None` for `BEGIN` / `COMMIT`.
    pub statement: Option<usize>,
    /// The step reports problems as rows (`PRAGMA foreign_key_check`): any row is a failure.
    pub fails_on_rows: bool,
    /// Runs even after an earlier step failed (turning `PRAGMA foreign_keys` back on).
    pub always: bool,
}

/// What to send, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub steps: Vec<Step>,
    /// The script opened its own transaction: after a failure it must be rolled back.
    pub own_transaction: bool,
}

/// The steps that run `stmts` on `kind`.
///
/// PostgreSQL and SQLite run the script in one transaction, so a failure leaves nothing behind. SQLite's
/// table rebuild is the exception to the exception: `PRAGMA foreign_keys` is ignored inside a transaction,
/// so the statements flagged [`Flag::OutsideTransaction`] at the start and end of the script run around
/// the `BEGIN` / `COMMIT` instead of inside it. MySQL / MariaDB commit after every DDL statement; the
/// statements simply run one by one. In a joined session nothing is wrapped, and a statement that
/// cannot run inside a transaction is refused.
pub fn plan_script(stmts: &[Statement], kind: DatabaseKind, session: &Session) -> Result<Script, String> {
    let step = |i: usize, s: &Statement, always: bool| Step {
        sql: s.sql.clone(),
        statement: Some(i),
        fails_on_rows: s.sql.trim_start().to_ascii_lowercase().starts_with("pragma foreign_key_check"),
        always,
    };
    let control = |sql: &str| Step { sql: sql.into(), statement: None, fails_on_rows: false, always: false };
    let outside = |s: &Statement| s.flags.contains(&Flag::OutsideTransaction);
    if let Session::Joined { begin } = session {
        if stmts.iter().any(outside) {
            return Err("This change must run outside a transaction; commit or roll back first".into());
        }
        let mut steps: Vec<Step> = begin.iter().map(|b| control(b)).collect();
        steps.extend(stmts.iter().enumerate().map(|(i, s)| step(i, s, false)));
        return Ok(Script { steps, own_transaction: false });
    }
    let lead = stmts.iter().take_while(|s| outside(s)).count();
    let trail = stmts[lead..].iter().rev().take_while(|s| outside(s)).count();
    let middle = lead..stmts.len() - trail;
    let wrap = transactional_ddl(kind) && (middle.len() > 1 || lead + trail > 0);
    let mut steps = Vec::new();
    for (i, s) in stmts.iter().enumerate().take(lead) {
        steps.push(step(i, s, false));
    }
    if wrap {
        steps.push(control("BEGIN"));
    }
    for i in middle.clone() {
        steps.push(step(i, &stmts[i], false));
    }
    if wrap {
        steps.push(control("COMMIT"));
    }
    for (i, s) in stmts.iter().enumerate().skip(stmts.len() - trail) {
        steps.push(step(i, s, true));
    }
    Ok(Script { steps, own_transaction: wrap })
}

// -------------------------------------------------------------- validation

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Make the column NOT NULL (a primary key column cannot hold NULL).
    MakeNotNull(u32),
}

/// Which part of the table an [`Issue`] is about, so an editor can mark the right section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Table,
    Columns,
    Keys,
    ForeignKeys,
    Indexes,
    Checks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub scope: Scope,
    pub message: String,
    /// The column concerned, if any.
    pub column: Option<u32>,
    pub fix: Option<Fix>,
}

/// Problems that can be seen from the draft alone. Errors block Execute; warnings do not.
pub fn validate(d: &TableDraft, kind: DatabaseKind) -> Vec<Issue> {
    let mut out = Vec::new();
    let err = |m: String, c: Option<u32>| Issue {
        severity: Severity::Error,
        scope: if c.is_some() { Scope::Columns } else { Scope::Table },
        message: m,
        column: c,
        fix: None,
    };
    let err_in = |scope: Scope, m: String| Issue {
        severity: Severity::Error,
        scope,
        message: m,
        column: None,
        fix: None,
    };
    if d.name.trim().is_empty() {
        out.push(err("The table needs a name.".into(), None));
    }
    if d.columns.is_empty() {
        out.push(err("A table needs at least one column.".into(), None));
    }
    for (i, c) in d.columns.iter().enumerate() {
        if c.name.trim().is_empty() {
            out.push(err("A column has no name.".into(), Some(c.id)));
        } else if d.columns[..i].iter().any(|o| o.name.eq_ignore_ascii_case(&c.name)) {
            out.push(err(format!("Column name {} is used twice.", c.name), Some(c.id)));
        }
        if c.data_type.trim().is_empty() {
            out.push(err(format!("Column {} has no type.", c.name), Some(c.id)));
        }
        if c.identity.is_some() && c.generated.is_some() {
            out.push(err(format!("Column {} cannot be both identity and generated.", c.name), Some(c.id)));
        }
        if c.generated.as_ref().is_some_and(|g| g.expression.trim().is_empty()) {
            out.push(err(format!("Column {} is generated but has no expression.", c.name), Some(c.id)));
        }
        if c.generated.as_ref().is_some_and(|g| !g.stored) && ddl_family(kind) == Family::Postgres {
            out.push(err(
                format!("PostgreSQL only supports STORED generated columns ({}).", c.name),
                Some(c.id),
            ));
        }
    }
    if let Some(pk) = &d.primary_key {
        if pk.columns.is_empty() {
            out.push(err_in(Scope::Keys, "The primary key has no columns.".into()));
        }
        for id in &pk.columns {
            if let Some(c) = d.col(*id)
                && c.nullable
            {
                out.push(Issue {
                    severity: Severity::Warning,
                    scope: Scope::Keys,
                    message: format!("Primary key column {} allows NULL; it will be made NOT NULL.", c.name),
                    column: Some(c.id),
                    fix: Some(Fix::MakeNotNull(c.id)),
                });
            }
        }
    }
    let known = |ids: &[u32]| ids.iter().all(|i| d.col(*i).is_some());
    for u in &d.uniques {
        if u.columns.is_empty() || !known(&u.columns) {
            out.push(err_in(Scope::Keys, "A unique constraint refers to a missing column.".into()));
        }
    }
    for f in &d.foreign_keys {
        if f.columns.is_empty()
            || !known(&f.columns)
            || f.ref_table.trim().is_empty()
            || f.ref_columns.is_empty()
        {
            out.push(err_in(Scope::ForeignKeys, "A foreign key is incomplete.".into()));
        } else if f.columns.len() != f.ref_columns.len() {
            out.push(err_in(
                Scope::ForeignKeys,
                format!(
                    "Foreign key to {} pairs {} columns with {}.",
                    f.ref_table,
                    f.columns.len(),
                    f.ref_columns.len()
                ),
            ));
        }
    }
    for i in &d.indexes {
        if i.raw.is_none() && (i.columns.is_empty() || !known(&i.columns)) {
            out.push(err_in(Scope::Indexes, format!("Index {} has no valid columns.", i.name)));
        }
        if i.name.trim().is_empty() {
            out.push(err_in(Scope::Indexes, "An index has no name.".into()));
        }
    }
    for c in &d.checks {
        if c.expression.trim().is_empty() {
            out.push(err_in(Scope::Checks, "A check constraint is empty.".into()));
        }
    }
    out
}

/// Apply [`Fix`]es: the draft with every auto-fixable warning resolved.
pub fn apply_fix(d: &mut TableDraft, fix: &Fix) {
    match fix {
        Fix::MakeNotNull(id) => {
            if let Some(c) = d.columns.iter_mut().find(|c| c.id == *id) {
                c.nullable = false;
            }
        }
    }
}

/// Warnings about foreign keys that the catalog can judge: a referenced table or column that does not
/// exist, and columns whose type families differ (an `integer` pointing at `text`).
pub fn check_foreign_keys(d: &TableDraft, catalog: &Catalog) -> Vec<Issue> {
    let mut out = Vec::new();
    for f in &d.foreign_keys {
        // A table that points at itself: for a table being created the catalog cannot know it yet, and
        // the draft's own columns are what the key refers to.
        if f.ref_table.eq_ignore_ascii_case(&d.name) {
            continue;
        }
        let Some((_, rt)) = catalog
            .find_table(f.ref_schema.as_deref().or(d.schema.as_deref()), &f.ref_table)
            .or_else(|| catalog.find_table(None, &f.ref_table))
        else {
            if !f.ref_table.is_empty() {
                out.push(Issue {
                    severity: Severity::Error,
                    scope: Scope::ForeignKeys,
                    message: format!("Referenced table {} does not exist.", f.ref_table),
                    column: None,
                    fix: None,
                });
            }
            continue;
        };
        if rt.kind != TableKind::Table {
            continue;
        }
        for (mine, theirs) in f.columns.iter().zip(&f.ref_columns) {
            let (Some(a), Some(b)) =
                (d.col(*mine), rt.columns.iter().find(|c| c.name.eq_ignore_ascii_case(theirs)))
            else {
                if d.col(*mine).is_some() {
                    out.push(Issue {
                        severity: Severity::Error,
                        scope: Scope::ForeignKeys,
                        message: format!("{} has no column {theirs}.", rt.name),
                        column: Some(*mine),
                        fix: None,
                    });
                }
                continue;
            };
            let family = |t: &str| kind_of_type_name(t);
            if family(&a.data_type) != b.kind {
                out.push(Issue {
                    severity: Severity::Warning,
                    scope: Scope::ForeignKeys,
                    message: format!(
                        "{} ({}) references {}.{} ({}): the types differ and the server may refuse or cast.",
                        a.name, a.data_type, rt.name, b.name, b.data_type
                    ),
                    column: Some(a.id),
                    fix: None,
                });
            }
        }
    }
    out
}

/// Changes the server will refuse that only the original and the edit together reveal. MySQL / MariaDB do
/// not change the type of a column while a foreign key uses it, on either side of the key (error 1832 /
/// 1833); the way through is to drop the key, change the column and add the key again, which is the
/// user's decision (it re-validates every row), so it is reported rather than scripted.
pub fn check_changes(
    original: &TableDraft,
    edited: &TableDraft,
    catalog: Option<&Catalog>,
    kind: DatabaseKind,
) -> Vec<Issue> {
    let mut out = Vec::new();
    if ddl_family(kind) != Family::MySql {
        return out;
    }
    for e in &edited.columns {
        let Some(o) = original.col(e.id) else { continue };
        if norm(&o.data_type) == norm(&e.data_type) {
            continue;
        }
        // A key of this table that still stands (unchanged) and uses the column.
        let local = original.foreign_keys.iter().find(|f| {
            f.columns.contains(&e.id) && edited.foreign_keys.iter().any(|x| x.id == f.id && same_fk(f, x))
        });
        // A key of another table that points at this one.
        let remote = catalog.and_then(|c| {
            c.schemas.iter().flat_map(|s| s.tables.iter().map(move |t| (s, t))).find_map(|(s, t)| {
                let own = t.name.eq_ignore_ascii_case(&original.name)
                    && original.schema.as_deref().is_none_or(|os| os.eq_ignore_ascii_case(&s.name));
                (!own)
                    .then(|| {
                        t.foreign_keys
                            .iter()
                            .find(|f| {
                                f.referenced_table.eq_ignore_ascii_case(&original.name)
                                    && f.referenced_columns.iter().any(|c| c.eq_ignore_ascii_case(&o.name))
                            })
                            .map(|f| format!("{}.{}", t.name, f.name))
                    })
                    .flatten()
            })
        });
        let because = match (local, remote) {
            (Some(f), _) => format!("the foreign key {} uses it", f.name.clone().unwrap_or_default()),
            (None, Some(r)) => format!("the foreign key {r} points at it"),
            (None, None) => continue,
        };
        out.push(Issue {
            severity: Severity::Error,
            scope: Scope::Columns,
            message: format!(
                "MySQL cannot change the type of {} while {because}. Remove the foreign key, change the type, then add the key again.",
                o.name
            ),
            column: Some(e.id),
            fix: None,
        });
    }
    out
}

/// Name clashes the draft cannot see by itself: the table name against the tables of its schema (unless it
/// is the table's own name before the edit), and constraint or index names used twice in the draft.
/// PostgreSQL and SQLite keep index names schema-wide, so an index named like another table's index fails
/// at the server; this reports the clashes the catalog proves.
pub fn check_names(
    d: &TableDraft,
    original_name: Option<&str>,
    catalog: &Catalog,
    kind: DatabaseKind,
) -> Vec<Issue> {
    let mut out = Vec::new();
    let issue = |scope: Scope, message: String| Issue {
        severity: Severity::Error,
        scope,
        message,
        column: None,
        fix: None,
    };
    let name = d.name.trim();
    let own = original_name.is_some_and(|o| o.eq_ignore_ascii_case(name));
    if !name.is_empty() && !own {
        let schema = d.schema.as_deref().or(catalog.default_schema.as_deref());
        let clash = schema
            .and_then(|s| catalog.schema(s))
            .is_some_and(|s| s.tables.iter().any(|t| t.name.eq_ignore_ascii_case(name)));
        if clash {
            out.push(issue(Scope::Table, format!("A table named {name} already exists.")));
        }
    }
    let mut seen: Vec<(String, &'static str)> = Vec::new();
    let mut note = |scope: Scope, n: &str, what: &'static str, out: &mut Vec<Issue>| {
        if n.is_empty() {
            return;
        }
        let key = n.to_ascii_lowercase();
        if let Some((_, other)) = seen.iter().find(|(k, _)| *k == key) {
            out.push(issue(scope, format!("The name {n} is used by a {what} and by a {other}.")));
        } else {
            seen.push((key, what));
        }
    };
    for u in &d.uniques {
        note(Scope::Keys, u.name.as_deref().unwrap_or(""), "unique constraint", &mut out);
    }
    for f in &d.foreign_keys {
        note(Scope::ForeignKeys, f.name.as_deref().unwrap_or(""), "foreign key", &mut out);
    }
    for c in &d.checks {
        note(Scope::Checks, c.name.as_deref().unwrap_or(""), "check", &mut out);
    }
    // Index names are per table in MySQL; elsewhere they share the schema with every other index.
    for i in &d.indexes {
        note(Scope::Indexes, &i.name, "index", &mut out);
        if ddl_family(kind) != Family::MySql {
            let schema = d.schema.as_deref().or(catalog.default_schema.as_deref());
            let elsewhere = schema.and_then(|s| catalog.schema(s)).is_some_and(|s| {
                s.tables.iter().any(|t| {
                    !t.name.eq_ignore_ascii_case(original_name.unwrap_or(name))
                        && t.indexes.iter().any(|x| x.name.eq_ignore_ascii_case(&i.name))
                })
            });
            if elsewhere && !i.name.is_empty() {
                out.push(issue(
                    Scope::Indexes,
                    format!("An index named {} already exists on another table.", i.name),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;
    use theta_core::{ColumnInfo, ForeignKey, Index, UniqueConstraint};

    fn col(id: u32, name: &str, ty: &str, nullable: bool) -> ColumnDraft {
        ColumnDraft {
            id,
            name: name.into(),
            data_type: ty.into(),
            nullable,
            default: None,
            identity: None,
            generated: None,
            comment: None,
        }
    }

    /// `customers`: identity PK, a NOT NULL name, a unique email, a default timestamp, an index on name.
    fn customers(kind: DatabaseKind) -> TableDraft {
        let (int, text, ts, now) = match kind {
            Postgres => ("integer", "text", "timestamp", "now()"),
            MySql => ("int", "varchar(100)", "datetime", "CURRENT_TIMESTAMP"),
            Sqlite => ("INTEGER", "TEXT", "TIMESTAMP", "CURRENT_TIMESTAMP"),
            _ => unreachable!("the DDL tests cover the three base kinds"),
        };
        let mut id = col(1, "id", int, false);
        id.identity = Some(Identity::ByDefault);
        let mut created = col(4, "created", ts, true);
        created.default = Some(now.into());
        TableDraft {
            schema: match kind {
                Postgres => Some("public".into()),
                _ => None,
            },
            name: "customers".into(),
            columns: vec![id, col(2, "name", text, false), col(3, "email", text, true), created],
            primary_key: Some(KeyDraft { id: 5, name: None, columns: vec![1] }),
            uniques: vec![KeyDraft { id: 6, name: Some("customers_email_key".into()), columns: vec![3] }],
            indexes: vec![IndexDraft {
                id: 7,
                name: "customers_name_idx".into(),
                columns: vec![2],
                unique: false,
                method: None,
                predicate: None,
                raw: None,
            }],
            ..Default::default()
        }
    }

    fn sql(stmts: &[Statement]) -> Vec<&str> {
        stmts.iter().map(|s| s.sql.as_str()).collect()
    }

    // ------------------------------------------------------------ create

    #[test]
    fn create_table_postgres() {
        let mut d = customers(Postgres);
        d.comment = Some("People".into());
        d.columns[1].comment = Some("Full name".into());
        d.checks.push(CheckDraft {
            id: 9,
            name: Some("name_nonempty".into()),
            expression: "length(name) > 0".into(),
        });
        d.foreign_keys.push(ForeignKeyDraft {
            id: 8,
            name: Some("customers_region_fk".into()),
            columns: vec![3],
            ref_schema: Some("public".into()),
            ref_table: "regions".into(),
            ref_columns: vec!["code".into()],
            on_delete: Some("CASCADE".into()),
            on_update: Some("NO ACTION".into()),
            deferrable: true,
            initially_deferred: true,
        });
        let out = create_table(&d, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "CREATE TABLE public.customers (\n    id integer GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n    name text NOT NULL,\n    email text,\n    created timestamp DEFAULT now(),\n    PRIMARY KEY (id),\n    CONSTRAINT customers_email_key UNIQUE (email),\n    CONSTRAINT name_nonempty CHECK (length(name) > 0),\n    CONSTRAINT customers_region_fk FOREIGN KEY (email) REFERENCES public.regions (code) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED\n)",
                "CREATE INDEX customers_name_idx ON public.customers (name)",
                "COMMENT ON TABLE public.customers IS 'People'",
                "COMMENT ON COLUMN public.customers.name IS 'Full name'",
            ]
        );
    }

    #[test]
    fn create_table_mysql_has_inline_comments_and_auto_increment() {
        let mut d = customers(MySql);
        d.comment = Some("People".into());
        d.columns[1].comment = Some("it's the name".into());
        let out = create_table(&d, MySql);
        assert_eq!(
            sql(&out),
            vec![
                "CREATE TABLE customers (\n    id int NOT NULL AUTO_INCREMENT,\n    name varchar(100) NOT NULL COMMENT 'it''s the name',\n    email varchar(100) NULL,\n    created datetime NULL DEFAULT CURRENT_TIMESTAMP,\n    PRIMARY KEY (id),\n    CONSTRAINT customers_email_key UNIQUE (email)\n) COMMENT = 'People'",
                "CREATE INDEX customers_name_idx ON customers (name)",
            ]
        );
        assert!(out.iter().all(|s| s.flags.contains(&Flag::ImplicitCommit)));
    }

    #[test]
    fn create_table_sqlite_uses_the_rowid_alias_spelling() {
        let d = customers(Sqlite);
        let out = create_table(&d, Sqlite);
        assert_eq!(
            sql(&out),
            vec![
                "CREATE TABLE customers (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    name TEXT NOT NULL,\n    email TEXT,\n    created TIMESTAMP DEFAULT CURRENT_TIMESTAMP,\n    CONSTRAINT customers_email_key UNIQUE (email)\n)",
                "CREATE INDEX customers_name_idx ON customers (name)",
            ]
        );
    }

    #[test]
    fn identifiers_are_quoted_only_when_needed() {
        let mut d = customers(Postgres);
        d.name = "Order Items".into();
        d.columns[1].name = "select".into();
        let out = create_table(&d, Postgres);
        assert!(out[0].sql.starts_with("CREATE TABLE public.\"Order Items\" ("));
        assert!(out[0].sql.contains("\n    \"select\" text NOT NULL"));
        assert!(out[0].sql.contains("\n    id integer"));
    }

    #[test]
    fn generated_columns_per_dialect() {
        let mut c = col(5, "total", "numeric", true);
        c.generated = Some(GeneratedColumn { expression: "price * qty".into(), stored: true });
        assert_eq!(column_tail(&c, Postgres, false), "numeric GENERATED ALWAYS AS (price * qty) STORED");
        assert_eq!(column_tail(&c, MySql, false), "numeric GENERATED ALWAYS AS (price * qty) STORED NULL");
        c.generated.as_mut().unwrap().stored = false;
        assert_eq!(column_tail(&c, Sqlite, false), "numeric GENERATED ALWAYS AS (price * qty) VIRTUAL");
        assert!(
            validate(&TableDraft { name: "t".into(), columns: vec![c], ..Default::default() }, Postgres)
                .iter()
                .any(|i| i.message.contains("only supports STORED"))
        );
    }

    // -------------------------------------------------------------- diff

    #[test]
    fn no_edit_no_statements() {
        for k in [Postgres, MySql, Sqlite] {
            let d = customers(k);
            assert!(diff(&d, &d.clone(), k).is_empty(), "{k:?}");
        }
        assert_eq!(summarize(&[]), "No changes");
    }

    #[test]
    fn postgres_add_column_with_default_and_rename_by_id() {
        let o = customers(Postgres);
        let mut e = o.clone();
        let mut score = col(10, "score", "integer", false);
        score.default = Some("0".into());
        e.columns.push(score);
        e.columns[2].name = "email_address".into(); // same id 3: a rename
        let out = diff(&o, &e, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE public.customers RENAME COLUMN email TO email_address",
                "ALTER TABLE public.customers ADD COLUMN score integer DEFAULT 0 NOT NULL",
            ]
        );
        // A rename is not destructive and drops nothing.
        assert!(out.iter().all(|s| !s.is_destructive()));
        // The unique constraint refers to the column by id, so it needs no change.
        assert!(!out.iter().any(|s| s.sql.contains("UNIQUE")));
    }

    #[test]
    fn postgres_type_nullability_default_and_drop() {
        let o = customers(Postgres);
        let mut e = o.clone();
        e.columns[1].data_type = "varchar(10)".into(); // text -> varchar(10): different base, narrows
        e.columns[2].nullable = false;
        e.columns[3].default = None;
        let out = diff(&o, &e, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE public.customers ALTER COLUMN name TYPE varchar(10) USING name::varchar(10)",
                "ALTER TABLE public.customers ALTER COLUMN email SET NOT NULL",
                "ALTER TABLE public.customers ALTER COLUMN created DROP DEFAULT",
            ]
        );
        assert!(out[0].is_destructive() && out[0].flags.contains(&Flag::TableRewrite));
        assert!(out[1].flags.iter().any(|f| matches!(f, Flag::Lock(_))));
        assert_eq!(summarize(&out), "3 statements · 1 destructive · 1 table rewrite");
        // Widening a varchar is neither.
        let mut a = customers(Postgres);
        a.columns[1].data_type = "varchar(10)".into();
        let mut b = a.clone();
        b.columns[1].data_type = "varchar(50)".into();
        let w = diff(&a, &b, Postgres);
        assert_eq!(sql(&w), vec!["ALTER TABLE public.customers ALTER COLUMN name TYPE varchar(50)"]);
        assert!(w[0].flags.is_empty());
    }

    #[test]
    fn postgres_drop_column_and_constraints_come_first() {
        let o = customers(Postgres);
        let mut e = o.clone();
        e.columns.remove(2); // email
        e.uniques.clear();
        let out = diff(&o, &e, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE public.customers DROP CONSTRAINT customers_email_key",
                "ALTER TABLE public.customers DROP COLUMN email",
            ]
        );
        assert!(out.iter().all(Statement::is_destructive));
    }

    #[test]
    fn postgres_foreign_key_index_check_and_table_rename() {
        let o = customers(Postgres);
        let mut e = o.clone();
        e.foreign_keys.push(ForeignKeyDraft {
            id: 20,
            name: Some("customers_region_fk".into()),
            columns: vec![3],
            ref_schema: Some("public".into()),
            ref_table: "regions".into(),
            ref_columns: vec!["code".into()],
            on_delete: None,
            on_update: None,
            deferrable: false,
            initially_deferred: false,
        });
        e.indexes.push(IndexDraft {
            id: 21,
            name: "customers_email_lower".into(),
            columns: vec![3],
            unique: true,
            method: Some("btree".into()),
            predicate: Some("email IS NOT NULL".into()),
            raw: None,
        });
        e.checks.push(CheckDraft {
            id: 22,
            name: Some("name_ok".into()),
            expression: "length(name) > 0".into(),
        });
        e.name = "clients".into();
        e.comment = Some("Who buys".into());
        let out = diff(&o, &e, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE public.customers ADD CONSTRAINT name_ok CHECK (length(name) > 0)",
                "ALTER TABLE public.customers ADD CONSTRAINT customers_region_fk FOREIGN KEY (email) REFERENCES public.regions (code)",
                "CREATE UNIQUE INDEX customers_email_lower ON public.customers (email) WHERE email IS NOT NULL",
                "COMMENT ON TABLE public.customers IS 'Who buys'",
                "ALTER TABLE public.customers RENAME TO clients",
            ]
        );
        assert!(transactional_ddl(Postgres));
    }

    #[test]
    fn postgres_primary_key_change_and_identity() {
        let o = customers(Postgres);
        let mut e = o.clone();
        e.primary_key = Some(KeyDraft { id: 5, name: None, columns: vec![1, 2] });
        e.columns[0].identity = Some(Identity::Always);
        let out = diff(&o, &e, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE public.customers DROP CONSTRAINT customers_pkey",
                "ALTER TABLE public.customers ALTER COLUMN id SET GENERATED ALWAYS",
                "ALTER TABLE public.customers ADD PRIMARY KEY (id, name)",
            ]
        );
    }

    #[test]
    fn postgres_changing_a_generated_expression_recreates_the_column() {
        let mut o = customers(Postgres);
        let mut total = col(11, "total", "numeric", true);
        total.generated = Some(GeneratedColumn { expression: "id * 2".into(), stored: true });
        o.columns.push(total);
        let mut e = o.clone();
        e.columns[4].generated = Some(GeneratedColumn { expression: "id * 3".into(), stored: true });
        let out = diff(&o, &e, Postgres);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE public.customers DROP COLUMN total",
                "ALTER TABLE public.customers ADD COLUMN total numeric GENERATED ALWAYS AS (id * 3) STORED",
            ]
        );
        assert!(out[0].is_destructive() && out[1].flags.contains(&Flag::TableRewrite));
    }

    #[test]
    fn mysql_modify_column_restates_the_definition_and_every_statement_commits() {
        let o = customers(MySql);
        let mut e = o.clone();
        e.columns[1].data_type = "varchar(40)".into(); // narrowing
        e.columns[1].comment = Some("name".into());
        e.columns[2].name = "mail".into();
        e.columns.push(col(10, "score", "int", true));
        e.uniques.clear();
        e.foreign_keys.push(ForeignKeyDraft {
            id: 30,
            name: Some("fk_score".into()),
            columns: vec![10],
            ref_schema: None,
            ref_table: "scores".into(),
            ref_columns: vec!["id".into()],
            on_delete: Some("SET NULL".into()),
            on_update: None,
            deferrable: true, // MySQL has none: ignored
            initially_deferred: false,
        });
        let out = diff(&o, &e, MySql);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE customers DROP INDEX customers_email_key",
                "ALTER TABLE customers RENAME COLUMN email TO mail",
                "ALTER TABLE customers ADD COLUMN score int NULL",
                "ALTER TABLE customers MODIFY COLUMN name varchar(40) NOT NULL COMMENT 'name'",
                "ALTER TABLE customers ADD CONSTRAINT fk_score FOREIGN KEY (score) REFERENCES scores (id) ON DELETE SET NULL",
            ]
        );
        assert!(out.iter().all(|s| s.flags.contains(&Flag::ImplicitCommit)));
        assert!(out[3].is_destructive());
        assert!(!transactional_ddl(MySql));
        assert_eq!(summarize(&out), "5 statements · 2 destructive · 1 table rewrite · 5 implicit commit");
    }

    #[test]
    fn mysql_drops_use_their_own_syntax() {
        let mut o = customers(MySql);
        o.foreign_keys.push(ForeignKeyDraft {
            id: 8,
            name: Some("fk_a".into()),
            columns: vec![3],
            ref_schema: None,
            ref_table: "t".into(),
            ref_columns: vec!["c".into()],
            on_delete: None,
            on_update: None,
            deferrable: false,
            initially_deferred: false,
        });
        o.checks.push(CheckDraft { id: 9, name: Some("chk".into()), expression: "id > 0".into() });
        let mut e = o.clone();
        e.foreign_keys.clear();
        e.checks.clear();
        e.indexes.clear();
        e.primary_key = None;
        let out = diff(&o, &e, MySql);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE customers DROP FOREIGN KEY fk_a",
                "ALTER TABLE customers DROP CONSTRAINT chk",
                "ALTER TABLE customers DROP PRIMARY KEY",
                "DROP INDEX customers_name_idx ON customers",
            ]
        );
    }

    #[test]
    fn sqlite_simple_changes_stay_in_place() {
        let o = customers(Sqlite);
        let mut e = o.clone();
        e.columns[2].name = "mail".into();
        let mut notes = col(10, "notes", "TEXT", true);
        notes.default = Some("''".into());
        e.columns.push(notes);
        e.indexes.push(IndexDraft {
            id: 40,
            name: "customers_mail_idx".into(),
            columns: vec![3],
            unique: false,
            method: None,
            predicate: Some("mail IS NOT NULL".into()),
            raw: None,
        });
        e.name = "clients".into();
        let out = diff(&o, &e, Sqlite);
        assert_eq!(
            sql(&out),
            vec![
                "ALTER TABLE customers RENAME COLUMN email TO mail",
                "ALTER TABLE customers ADD COLUMN notes TEXT DEFAULT ''",
                "CREATE INDEX customers_mail_idx ON customers (mail) WHERE mail IS NOT NULL",
                "ALTER TABLE customers RENAME TO clients",
            ]
        );
        // Dropping an unindexed, unconstrained column is also in place.
        let mut e2 = o.clone();
        e2.columns.remove(3);
        assert_eq!(sql(&diff(&o, &e2, Sqlite)), vec!["ALTER TABLE customers DROP COLUMN created"]);
    }

    #[test]
    fn sqlite_type_change_rebuilds_the_table_in_the_documented_order() {
        let o = customers(Sqlite);
        let mut e = o.clone();
        e.columns[1].data_type = "VARCHAR(40)".into();
        e.columns[2].name = "mail".into();
        let out = diff(&o, &e, Sqlite);
        assert_eq!(
            sql(&out),
            vec![
                "PRAGMA foreign_keys = OFF",
                "CREATE TABLE customers__new (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    name VARCHAR(40) NOT NULL,\n    mail TEXT,\n    created TIMESTAMP DEFAULT CURRENT_TIMESTAMP,\n    CONSTRAINT customers_email_key UNIQUE (mail)\n)",
                "INSERT INTO customers__new (id, name, mail, created) SELECT id, name, email, created FROM customers",
                "DROP TABLE customers",
                "ALTER TABLE customers__new RENAME TO customers",
                "CREATE INDEX customers_name_idx ON customers (name)",
                "PRAGMA foreign_key_check",
                "PRAGMA foreign_keys = ON",
            ]
        );
        assert!(
            out[0].flags.contains(&Flag::OutsideTransaction)
                && out[7].flags.contains(&Flag::OutsideTransaction)
        );
        assert!(out[3].is_destructive());
        assert!(out[1].explanation.contains("cannot do this with ALTER TABLE"), "{}", out[1].explanation);
        assert!(transactional_ddl(Sqlite));
    }

    #[test]
    fn sqlite_rebuild_cases() {
        // Dropping an indexed column cannot be done in place.
        let o = customers(Sqlite);
        let mut e = o.clone();
        e.columns.remove(1);
        e.indexes.clear();
        assert!(diff(&o, &e, Sqlite).iter().any(|s| s.flags.contains(&Flag::Rebuild)));
        // A NOT NULL column without a default cannot be added in place.
        let mut e = o.clone();
        e.columns.push(col(10, "must", "TEXT", false));
        assert!(diff(&o, &e, Sqlite).iter().any(|s| s.flags.contains(&Flag::Rebuild)));
        // New foreign keys and checks need a rebuild; a trigger list is replayed afterwards.
        let mut e = o.clone();
        e.checks.push(CheckDraft { id: 9, name: None, expression: "length(name) > 0".into() });
        e.recreate.push("CREATE TRIGGER t1 AFTER INSERT ON customers BEGIN SELECT 1; END".into());
        let out = diff(&o, &e, Sqlite);
        let s = sql(&out);
        assert!(s.contains(&"CREATE TRIGGER t1 AFTER INSERT ON customers BEGIN SELECT 1; END"));
        assert!(out[1].sql.contains("CHECK (length(name) > 0)"));
        // Generated columns are not copied (they compute themselves).
        let mut g = o.clone();
        let mut total = col(11, "total", "INTEGER", true);
        total.generated = Some(GeneratedColumn { expression: "id * 2".into(), stored: false });
        g.columns.push(total);
        let mut g2 = g.clone();
        g2.columns[1].data_type = "TEXT NOT NULL".into();
        let out = diff(&g, &g2, Sqlite);
        let insert = out.iter().find(|s| s.sql.starts_with("INSERT")).unwrap();
        assert!(!insert.sql.contains("total"));
    }

    // ------------------------------------------------------- from_table

    #[test]
    fn a_draft_built_from_the_catalog_round_trips_to_no_changes() {
        let t = Table {
            name: "orders".into(),
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    data_type: "integer".into(),
                    identity: Some(Identity::Always),
                    default: Some("GENERATED ALWAYS AS IDENTITY".into()),
                    ..Default::default()
                },
                ColumnInfo {
                    name: "email".into(),
                    data_type: "text".into(),
                    nullable: true,
                    ..Default::default()
                },
            ],
            primary_key: vec!["id".into()],
            foreign_keys: vec![ForeignKey {
                name: "orders_x_fk".into(),
                columns: vec!["email".into()],
                referenced_table: "x".into(),
                referenced_columns: vec!["e".into()],
                ..Default::default()
            }],
            indexes: vec![
                Index {
                    name: "orders_pkey".into(),
                    columns: vec!["id".into()],
                    unique: true,
                    primary: true,
                    ..Default::default()
                },
                Index {
                    name: "orders_email_key".into(),
                    columns: vec!["email".into()],
                    unique: true,
                    ..Default::default()
                },
                Index {
                    name: "orders_email_idx".into(),
                    columns: vec!["email".into()],
                    ..Default::default()
                },
            ],
            unique_constraints: vec![UniqueConstraint {
                name: "orders_email_key".into(),
                columns: vec!["email".into()],
            }],
            ..Default::default()
        };
        let d = TableDraft::from_table("public", &t);
        assert_eq!(d.primary_key.as_ref().unwrap().name.as_deref(), Some("orders_pkey"));
        // The pk's and the constraint's indexes are not separate indexes.
        assert_eq!(d.indexes.len(), 1);
        assert_eq!(d.columns[0].default, None);
        assert_eq!(d.columns[0].identity, Some(Identity::Always));
        assert!(diff(&d, &d.clone(), Postgres).is_empty());
        assert!(d.next_id() > 6);
        // Renaming the pk column by id keeps the key.
        let mut e = d.clone();
        e.columns[0].name = "order_id".into();
        assert_eq!(
            sql(&diff(&d, &e, Postgres)),
            vec!["ALTER TABLE public.orders RENAME COLUMN id TO order_id"]
        );
    }

    // -------------------------------------------------------- validation

    #[test]
    fn validation_reports_what_the_ui_must_stop_or_offer_to_fix() {
        let mut d = customers(Postgres);
        d.columns[1].name = "EMAIL".into();
        d.columns[2].data_type = " ".into();
        d.columns.push(col(9, "", "text", true));
        d.primary_key = Some(KeyDraft { id: 5, name: None, columns: vec![3] }); // nullable email
        let issues = validate(&d, Postgres);
        let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("is used twice")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("has no type")));
        assert!(msgs.iter().any(|m| m.contains("has no name")));
        let warn = issues.iter().find(|i| i.severity == Severity::Warning).unwrap();
        assert_eq!(warn.fix, Some(Fix::MakeNotNull(3)));
        apply_fix(&mut d, warn.fix.as_ref().unwrap());
        assert!(!d.col(3).unwrap().nullable);
        assert!(validate(&customers(Postgres), Postgres).is_empty());
        assert!(validate(&TableDraft::default(), Postgres).len() >= 2);
    }

    #[test]
    fn foreign_keys_are_checked_against_the_catalog() {
        let regions = Table {
            name: "regions".into(),
            columns: vec![ColumnInfo {
                name: "code".into(),
                data_type: "integer".into(),
                kind: TypeKind::Integer,
                ..Default::default()
            }],
            ..Default::default()
        };
        let catalog = Catalog {
            schemas: vec![theta_core::Schema {
                name: "public".into(),
                tables: vec![regions],
                ..Default::default()
            }],
            default_schema: Some("public".into()),
        };
        let mut d = customers(Postgres);
        let mut fk = ForeignKeyDraft {
            id: 8,
            name: None,
            columns: vec![3], // email text -> integer
            ref_schema: None,
            ref_table: "regions".into(),
            ref_columns: vec!["code".into()],
            on_delete: None,
            on_update: None,
            deferrable: false,
            initially_deferred: false,
        };
        d.foreign_keys.push(fk.clone());
        let issues = check_foreign_keys(&d, &catalog);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("the types differ"));
        fk.ref_table = "nope".into();
        d.foreign_keys = vec![fk];
        assert!(check_foreign_keys(&d, &catalog)[0].message.contains("does not exist"));
    }

    #[test]
    fn type_names_classify() {
        for (t, k) in [
            ("character varying(40)", TypeKind::Text),
            ("INT", TypeKind::Integer),
            ("DECIMAL(10,2)", TypeKind::Decimal),
        ] {
            assert_eq!(kind_of_type_name(t), k);
        }
    }

    // ---------------------------------------------------- editor vocabulary and script plan

    fn plain(sql: &str) -> Statement {
        Statement { sql: sql.into(), flags: vec![], explanation: String::new() }
    }

    fn steps(script: &Script) -> Vec<String> {
        script.steps.iter().map(|s| s.sql.clone()).collect()
    }

    #[test]
    fn type_suggestions_filter_by_what_is_typed() {
        assert!(matching_types(Postgres, "").contains(&"jsonb"));
        assert_eq!(matching_types(Postgres, "var"), vec!["varchar(255)"]);
        // A prefix match comes before a mere substring match.
        assert_eq!(matching_types(Postgres, "int")[0], "integer");
        assert!(matching_types(Postgres, "int").contains(&"interval"));
        // Typing a type completely leaves nothing to suggest.
        assert!(matching_types(Postgres, "jsonb").is_empty());
        assert!(matching_types(MySql, "tiny").contains(&"tinyint(1)"));
        assert!(matching_types(Sqlite, "nothing like it").is_empty());
        for kind in [Postgres, MySql, Sqlite] {
            // Every suggestion is a type the engine can classify or knowingly treats as free text.
            assert!(type_suggestions(kind).iter().all(|t| !t.trim().is_empty()));
        }
    }

    #[test]
    fn dialect_choices_match_what_the_dialects_accept() {
        assert!(index_methods(Sqlite).is_empty());
        assert!(index_methods(Postgres).contains(&"gin"));
        assert!(!fk_actions(MySql).contains(&"SET DEFAULT"));
        assert!(fk_actions(Postgres).contains(&"SET DEFAULT"));
        assert_eq!(identity_choices(Postgres).len(), 3);
        assert_eq!(identity_choices(MySql)[1].1, "AUTO_INCREMENT");
        assert!(!supports_column_comments(Sqlite) && supports_column_comments(MySql));
        assert!(!supports_partial_indexes(MySql) && supports_partial_indexes(Sqlite));
        assert!(!supports_deferrable(MySql));
        assert!(!supports_table_comment(Sqlite));
    }

    #[test]
    fn postgres_runs_several_statements_in_one_transaction() {
        let stmts = [plain("ALTER TABLE a ADD COLUMN b int"), plain("ALTER TABLE a DROP COLUMN c")];
        let script = plan_script(&stmts, Postgres, &Session::Own).unwrap();
        assert_eq!(
            steps(&script),
            vec!["BEGIN", "ALTER TABLE a ADD COLUMN b int", "ALTER TABLE a DROP COLUMN c", "COMMIT"]
        );
        assert!(script.own_transaction);
        assert_eq!(script.steps[1].statement, Some(0));
        assert_eq!(script.steps[0].statement, None);
        // One statement is atomic by itself.
        let one = plan_script(&stmts[..1], Postgres, &Session::Own).unwrap();
        assert_eq!(steps(&one), vec!["ALTER TABLE a ADD COLUMN b int"]);
        assert!(!one.own_transaction);
    }

    #[test]
    fn mysql_never_wraps_because_it_could_not_roll_back() {
        let stmts = [plain("ALTER TABLE a ADD COLUMN b int"), plain("ALTER TABLE a DROP COLUMN c")];
        let script = plan_script(&stmts, MySql, &Session::Own).unwrap();
        assert_eq!(steps(&script), vec!["ALTER TABLE a ADD COLUMN b int", "ALTER TABLE a DROP COLUMN c"]);
        assert!(!script.own_transaction);
    }

    #[test]
    fn a_joined_session_never_commits_and_opens_the_transaction_when_needed() {
        let stmts = [plain("ALTER TABLE a ADD COLUMN b int"), plain("ALTER TABLE a DROP COLUMN c")];
        let begin = Session::Joined { begin: vec!["BEGIN".into()] };
        let script = plan_script(&stmts, Postgres, &begin).unwrap();
        assert_eq!(steps(&script)[0], "BEGIN");
        assert!(!steps(&script).contains(&"COMMIT".to_string()));
        let open = Session::Joined { begin: vec![] };
        assert_eq!(plan_script(&stmts, Postgres, &open).unwrap().steps.len(), 2);
    }

    #[test]
    fn the_sqlite_rebuild_keeps_its_pragmas_outside_the_transaction() {
        let o = customers(Sqlite);
        let mut e = o.clone();
        e.columns[1].data_type = "VARCHAR(40)".into();
        let stmts = diff(&o, &e, Sqlite);
        let script = plan_script(&stmts, Sqlite, &Session::Own).unwrap();
        let sql = steps(&script);
        assert_eq!(sql.first().unwrap(), "PRAGMA foreign_keys = OFF");
        assert_eq!(sql[1], "BEGIN");
        assert_eq!(sql[sql.len() - 2], "COMMIT");
        assert_eq!(sql.last().unwrap(), "PRAGMA foreign_keys = ON");
        let check = script.steps.iter().find(|s| s.sql == "PRAGMA foreign_key_check").unwrap();
        assert!(check.fails_on_rows, "a violation found by the check must roll the rebuild back");
        assert!(script.steps.last().unwrap().always, "the pragma is restored even after a failure");
        assert!(script.own_transaction);
        // Inside a session it cannot run at all.
        let err = plan_script(&stmts, Sqlite, &Session::Joined { begin: vec![] }).unwrap_err();
        assert!(err.contains("outside a transaction"), "{err}");
    }

    #[test]
    fn issues_say_which_section_they_belong_to() {
        let mut d = customers(Postgres);
        d.columns[1].name.clear();
        d.primary_key = Some(KeyDraft { id: 5, name: None, columns: vec![] });
        d.indexes[0].name.clear();
        d.checks.push(CheckDraft { id: 20, name: None, expression: " ".into() });
        d.foreign_keys.push(ForeignKeyDraft {
            id: 21,
            name: None,
            columns: vec![],
            ref_schema: None,
            ref_table: String::new(),
            ref_columns: vec![],
            on_delete: None,
            on_update: None,
            deferrable: false,
            initially_deferred: false,
        });
        let scopes: Vec<Scope> = validate(&d, Postgres).iter().map(|i| i.scope).collect();
        for want in [Scope::Columns, Scope::Keys, Scope::Indexes, Scope::Checks, Scope::ForeignKeys] {
            assert!(scopes.contains(&want), "{want:?} missing from {scopes:?}");
        }
        let empty = validate(&TableDraft::default(), Postgres);
        assert!(empty.iter().all(|i| i.scope == Scope::Table));
    }

    #[test]
    fn name_clashes_are_found_against_the_catalog() {
        let existing = Table { name: "orders".into(), ..Default::default() };
        let other = Table {
            name: "other".into(),
            indexes: vec![Index { name: "shared_idx".into(), ..Default::default() }],
            ..Default::default()
        };
        let catalog = Catalog {
            schemas: vec![theta_core::Schema {
                name: "public".into(),
                tables: vec![existing, other],
                ..Default::default()
            }],
            default_schema: Some("public".into()),
        };
        let mut d = customers(Postgres);
        d.name = "Orders".into();
        let msgs: Vec<String> =
            check_names(&d, None, &catalog, Postgres).into_iter().map(|i| i.message).collect();
        assert_eq!(msgs, vec!["A table named Orders already exists."]);
        // Keeping its own name is no clash.
        assert!(check_names(&d, Some("orders"), &catalog, Postgres).is_empty());
        // Index names are schema-wide in PostgreSQL but per table in MySQL.
        let mut d = customers(Postgres);
        d.indexes[0].name = "shared_idx".into();
        assert_eq!(check_names(&d, Some("customers"), &catalog, Postgres).len(), 1);
        assert!(check_names(&d, Some("customers"), &catalog, MySql).is_empty());
        // The same constraint name twice inside the draft.
        let mut d = customers(Postgres);
        d.checks = vec![
            CheckDraft { id: 30, name: Some("c".into()), expression: "true".into() },
            CheckDraft { id: 31, name: Some("C".into()), expression: "true".into() },
        ];
        assert_eq!(check_names(&d, Some("customers"), &catalog, Postgres).len(), 1);
    }

    #[test]
    fn mysql_refuses_a_type_change_on_a_foreign_key_column_and_says_so_up_front() {
        let mut o = customers(MySql);
        o.foreign_keys.push(ForeignKeyDraft {
            id: 9,
            name: Some("customers_x_fk".into()),
            columns: vec![3],
            ref_schema: None,
            ref_table: "t".into(),
            ref_columns: vec!["e".into()],
            on_delete: None,
            on_update: None,
            deferrable: false,
            initially_deferred: false,
        });
        let mut e = o.clone();
        e.columns[2].data_type = "varchar(200)".into();
        let issues = check_changes(&o, &e, None, MySql);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("customers_x_fk"), "{}", issues[0].message);
        assert_eq!(issues[0].column, Some(3));
        // Dropping the key in the same edit is the way through.
        let mut e2 = e.clone();
        e2.foreign_keys.clear();
        assert!(check_changes(&o, &e2, None, MySql).is_empty());
        // Other databases change the type and re-check the key themselves.
        assert!(check_changes(&o, &e, None, Postgres).is_empty());
        assert!(check_changes(&o, &o, None, MySql).is_empty(), "no change, no problem");
        // The other side of a key: another table's key points at this column.
        let other = Table {
            name: "orders".into(),
            foreign_keys: vec![ForeignKey {
                name: "orders_cust".into(),
                columns: vec!["c".into()],
                referenced_table: "customers".into(),
                referenced_columns: vec!["email".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let catalog = Catalog {
            schemas: vec![theta_core::Schema { name: "d".into(), tables: vec![other], ..Default::default() }],
            default_schema: Some("d".into()),
        };
        let mut plain = customers(MySql);
        plain.foreign_keys.clear();
        let mut changed = plain.clone();
        changed.columns[2].data_type = "varchar(200)".into();
        let issues = check_changes(&plain, &changed, Some(&catalog), MySql);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("orders.orders_cust"), "{}", issues[0].message);
    }

    #[test]
    fn mysql_spatial_and_fulltext_are_index_kinds_not_using_methods() {
        let mut d = TableDraft { name: "stores".into(), ..Default::default() };
        d.columns.push(ColumnDraft {
            id: 1,
            name: "location".into(),
            data_type: "point".into(),
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            comment: None,
        });
        let idx = |m: &str| IndexDraft {
            id: 2,
            name: "ix".into(),
            columns: vec![1],
            unique: false,
            method: Some(m.into()),
            predicate: None,
            raw: None,
        };
        assert_eq!(
            create_index_sql(&d, &idx("spatial"), DatabaseKind::MySql),
            "CREATE SPATIAL INDEX ix ON stores (location)"
        );
        assert_eq!(
            create_index_sql(&d, &idx("fulltext"), DatabaseKind::MySql),
            "CREATE FULLTEXT INDEX ix ON stores (location)"
        );
        assert_eq!(
            create_index_sql(&d, &idx("hash"), DatabaseKind::MySql),
            "CREATE INDEX ix ON stores (location) USING HASH"
        );
        assert_eq!(
            create_index_sql(&d, &idx("btree"), DatabaseKind::MySql),
            "CREATE INDEX ix ON stores (location)"
        );
    }
}
