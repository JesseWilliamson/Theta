//! Search Across Tables: the queries, planned without touching a database.
//!
//! For every table in the scope this module writes **one** statement that answers, in a single pass over
//! at most `row_limit` rows, "how many rows did we look at, how many matched, and how many matches did
//! each column have". The text being looked for always enters as a string literal built by
//! [`sql_literal`], and every identifier is quoted by [`DatabaseKind::quote_ident`], so neither a
//! needle such as `O'Brien; DROP TABLE x` nor a table called `weird "name"` can change what the
//! statement means.
//!
//! The dialects differ in four places and the match predicate is where each shows:
//!
//! | | text of a column | case-insensitive substring | case-sensitive substring |
//! | --- | --- | --- | --- |
//! | PostgreSQL, DuckDB | `CAST(c AS text / VARCHAR)` | `strpos(lower(x), lower(n)) > 0` | `strpos(x, n) > 0` |
//! | MySQL, MariaDB | `CAST(c AS CHAR)` | `LOCATE(lower(n), lower(x)) > 0` | `LOCATE(CAST(n AS BINARY), x) > 0` |
//! | SQLite | `CAST(c AS TEXT)` | `instr(lower(x), lower(n)) > 0` | `instr(x, n) > 0` |
//!
//! `strpos`/`LOCATE`/`instr` are used instead of `LIKE` because a `LIKE` pattern needs its own escaping
//! (`%`, `_` and the escape character), which is a second place to get wrong. MySQL's default collations
//! are case-insensitive, so "case-sensitive" there has to ask for a binary comparison explicitly, and
//! "case-insensitive" calls `lower()` explicitly rather than trusting the column's collation. SQLite's
//! `lower()` only folds ASCII; that is documented in the user guide, not worked around.

use crate::generate::sql_literal;
use std::collections::BTreeSet;
use theta_core::{Catalog, DatabaseKind, Family, TableKind, TypeKind, Value};

/// Which kinds of column are searched. Binary and geometry columns never are: their text form is a hex
/// string or a WKB blob that a human is not looking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColumnClass {
    Text,
    Number,
    Date,
    Json,
    /// UUIDs, booleans, arrays, enums, network addresses: everything else that has a text form.
    Other,
}

impl ColumnClass {
    pub const ALL: [ColumnClass; 5] =
        [ColumnClass::Text, ColumnClass::Number, ColumnClass::Date, ColumnClass::Json, ColumnClass::Other];

    pub fn of(kind: TypeKind) -> Option<ColumnClass> {
        Some(match kind {
            TypeKind::Text => ColumnClass::Text,
            TypeKind::Integer | TypeKind::Float | TypeKind::Decimal => ColumnClass::Number,
            TypeKind::Date | TypeKind::Time | TypeKind::Timestamp | TypeKind::Interval => ColumnClass::Date,
            TypeKind::Json => ColumnClass::Json,
            TypeKind::Bool | TypeKind::Uuid | TypeKind::Array | TypeKind::Other => ColumnClass::Other,
            TypeKind::Bytes | TypeKind::Geometry => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ColumnClass::Text => "Text",
            ColumnClass::Number => "Numbers",
            ColumnClass::Date => "Dates and times",
            ColumnClass::Json => "JSON",
            ColumnClass::Other => "UUID, boolean, other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOptions {
    pub text: String,
    pub case_sensitive: bool,
    /// The whole cell must equal the text, instead of containing it.
    pub whole_value: bool,
    pub classes: BTreeSet<ColumnClass>,
    /// Rows looked at per table; the statement reads `LIMIT row_limit` rows and says how many it saw.
    pub row_limit: usize,
    /// Columns searched per table. A table with more is searched on its first columns and says so.
    pub max_columns: usize,
}

impl SearchOptions {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: false,
            whole_value: false,
            classes: ColumnClass::ALL.into_iter().collect(),
            row_limit: 100_000,
            max_columns: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every table of every user schema.
    Connection,
    Schema(String),
    /// `(schema, table)` pairs.
    Tables(Vec<(String, String)>),
}

/// One table to ask about.
#[derive(Debug, Clone, PartialEq)]
pub struct TableJob {
    pub schema: String,
    pub table: String,
    /// The columns the statement looks in, in table order.
    pub columns: Vec<String>,
    /// How many columns of the table were left out because of `max_columns`.
    pub columns_cut: usize,
    pub sql: String,
    pub estimate: Option<u64>,
}

/// A table that is not searched, and why. Shown to the user, never dropped silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub schema: String,
    pub table: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub jobs: Vec<TableJob>,
    pub skipped: Vec<Skipped>,
}

/// What one table's statement returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counts {
    /// Rows the statement looked at (at most the row limit).
    pub scanned: u64,
    /// Rows in which at least one searched column matched.
    pub matched_rows: u64,
    /// `(column, matching rows)` for the columns that matched.
    pub columns: Vec<(String, u64)>,
}

/// Schemas that hold the database's own objects. Skipped when the whole connection is searched, not
/// when the user names one.
fn is_system_schema(kind: DatabaseKind, name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    match kind.family() {
        Family::Postgres => n == "information_schema" || n.starts_with("pg_") || n == "crdb_internal",
        Family::MySql => matches!(n.as_str(), "information_schema" | "mysql" | "performance_schema" | "sys"),
        Family::DuckDb => matches!(n.as_str(), "information_schema" | "pg_catalog"),
        _ => false,
    }
}

fn text_of(kind: DatabaseKind, column: &str) -> String {
    let c = kind.quote_ident(column);
    match kind.family() {
        Family::MySql => format!("CAST({c} AS CHAR)"),
        Family::Sqlite => format!("CAST({c} AS TEXT)"),
        Family::DuckDb => format!("CAST({c} AS VARCHAR)"),
        _ => format!("CAST({c} AS text)"),
    }
}

/// The condition "this column matches", for the dialect and the options. See the module docs for why
/// each form is used. Callers have already refused a text with a NUL byte (`plan`).
pub fn predicate(kind: DatabaseKind, column: &str, opts: &SearchOptions) -> String {
    let x = text_of(kind, column);
    let needle = sql_literal(&Value::Text(opts.text.clone()), kind);
    let family = kind.family();
    match (opts.whole_value, opts.case_sensitive) {
        (true, true) if family == Family::MySql => format!("{x} = CAST({needle} AS BINARY)"),
        (true, true) => format!("{x} = {needle}"),
        (true, false) => format!("lower({x}) = lower({needle})"),
        (false, true) => match family {
            Family::MySql => format!("LOCATE(CAST({needle} AS BINARY), {x}) > 0"),
            Family::Sqlite => format!("instr({x}, {needle}) > 0"),
            _ => format!("strpos({x}, {needle}) > 0"),
        },
        (false, false) => match family {
            Family::MySql => format!("LOCATE(lower({needle}), lower({x})) > 0"),
            Family::Sqlite => format!("instr(lower({x}), lower({needle})) > 0"),
            _ => format!("strpos(lower({x}), lower({needle})) > 0"),
        },
    }
}

fn target(kind: DatabaseKind, schema: &str, table: &str) -> String {
    if schema.is_empty() {
        kind.quote_ident(table)
    } else {
        format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(table))
    }
}

/// The statement for one table. Column `i` is counted as `c{i}`: an alias made of the position cannot
/// collide with, or be broken by, a column name.
fn count_statement(
    kind: DatabaseKind,
    schema: &str,
    table: &str,
    columns: &[String],
    opts: &SearchOptions,
) -> String {
    let preds: Vec<String> = columns.iter().map(|c| predicate(kind, c, opts)).collect();
    let any = preds.iter().map(|p| format!("({p})")).collect::<Vec<_>>().join(" OR ");
    let select_list = columns.iter().map(|c| kind.quote_ident(c)).collect::<Vec<_>>().join(", ");
    // COUNT(CASE ...) rather than SUM(CASE ...): COUNT is a BIGINT on every engine and is never NULL,
    // whereas SUM is NUMERIC on MySQL and NULL for an empty table.
    let mut items =
        vec!["COUNT(*) AS scanned".to_string(), format!("COUNT(CASE WHEN {any} THEN 1 END) AS matched")];
    for (i, p) in preds.iter().enumerate() {
        items.push(format!("COUNT(CASE WHEN {p} THEN 1 END) AS c{i}"));
    }
    format!(
        "SELECT {} FROM (SELECT {select_list} FROM {} LIMIT {}) AS theta_search",
        items.join(", "),
        target(kind, schema, table),
        opts.row_limit
    )
}

/// Why `text` cannot be searched for, or `None`.
pub fn check_text(text: &str) -> Option<&'static str> {
    if text.is_empty() {
        Some("Enter the text to look for.")
    } else if text.contains('\0') {
        // A NUL byte cannot be written into a SQL literal on most servers.
        Some("The text contains a NUL character, which cannot be searched for.")
    } else {
        None
    }
}

/// Plan the search: which tables are asked, with what, and which are left out and why.
pub fn plan(
    catalog: &Catalog,
    kind: DatabaseKind,
    scope: &Scope,
    opts: &SearchOptions,
) -> Result<Plan, String> {
    if let Some(reason) = kind.unsupported(theta_core::Feature::TableSearch) {
        return Err(format!("{}: {reason}", kind.label()));
    }
    if let Some(why) = check_text(&opts.text) {
        return Err(why.to_string());
    }
    if opts.classes.is_empty() {
        return Err("Choose at least one kind of column to search.".to_string());
    }
    if opts.row_limit == 0 {
        return Err("The row limit must be at least 1.".to_string());
    }
    let mut out = Plan::default();
    for schema in &catalog.schemas {
        let in_scope = match scope {
            Scope::Connection => !is_system_schema(kind, &schema.name),
            Scope::Schema(s) => &schema.name == s,
            Scope::Tables(list) => list.iter().any(|(s, _)| s == &schema.name),
        };
        if !in_scope {
            continue;
        }
        for table in &schema.tables {
            if let Scope::Tables(list) = scope
                && !list.iter().any(|(s, t)| s == &schema.name && t == &table.name)
            {
                continue;
            }
            let skip = |reason: &str| Skipped {
                schema: schema.name.clone(),
                table: table.name.clone(),
                reason: reason.to_string(),
            };
            // Views and foreign tables can be arbitrarily expensive to scan and may not be readable at
            // all, so only ordinary tables are searched. A table named explicitly is the exception: the
            // user asked for it.
            let named = matches!(scope, Scope::Tables(_));
            if table.kind != TableKind::Table && !named {
                continue;
            }
            let all: Vec<&str> = table
                .columns
                .iter()
                .filter(|c| ColumnClass::of(c.kind).is_some_and(|k| opts.classes.contains(&k)))
                .map(|c| c.name.as_str())
                .collect();
            if all.is_empty() {
                out.skipped.push(skip("no column of the chosen types"));
                continue;
            }
            let columns: Vec<String> = all.iter().take(opts.max_columns).map(|c| c.to_string()).collect();
            let columns_cut = all.len() - columns.len();
            out.jobs.push(TableJob {
                schema: schema.name.clone(),
                table: table.name.clone(),
                sql: count_statement(kind, &schema.name, &table.name, &columns, opts),
                columns,
                columns_cut,
                estimate: table.row_estimate,
            });
        }
    }
    Ok(out)
}

fn count_of(v: &Value) -> Option<u64> {
    match v {
        Value::Int(i) => u64::try_from(*i).ok(),
        Value::UInt(u) => Some(*u),
        Value::Float(f) if *f >= 0.0 && f.fract() == 0.0 => Some(*f as u64),
        Value::Null => Some(0),
        // MySQL's COUNT is a BIGINT, but DuckDB and others may hand a count back as a decimal or text.
        other => other.display().trim().parse::<u64>().ok(),
    }
}

/// Read the one row a [`TableJob::sql`] returns.
pub fn parse_counts(job: &TableJob, row: &[Value]) -> Result<Counts, String> {
    let want = 2 + job.columns.len();
    if row.len() != want {
        return Err(format!("expected {want} columns in the answer, the server returned {}", row.len()));
    }
    let n = |i: usize| count_of(&row[i]).ok_or_else(|| format!("column {i} of the answer is not a count"));
    let columns = job
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| Ok((c.clone(), n(2 + i)?)))
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .filter(|(_, hits)| *hits > 0)
        .collect();
    Ok(Counts { scanned: n(0)?, matched_rows: n(1)?, columns })
}

/// The statement that opens a table on the rows a hit counted: `columns` are the matching columns (all
/// of them for the table's own entry, one for a column's entry).
pub fn filter_statement(
    kind: DatabaseKind,
    schema: &str,
    table: &str,
    columns: &[String],
    opts: &SearchOptions,
) -> String {
    let any =
        columns.iter().map(|c| format!("({})", predicate(kind, c, opts))).collect::<Vec<_>>().join(" OR ");
    format!("SELECT * FROM {} WHERE {any}", target(kind, schema, table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{ColumnInfo, Schema, Table};

    fn col(name: &str, kind: TypeKind) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: format!("{kind:?}"),
            kind,
            nullable: true,
            default: None,
            comment: None,
            geometry: None,
            identity: None,
            generated: None,
        }
    }

    fn table(name: &str, columns: Vec<ColumnInfo>) -> Table {
        Table { name: name.into(), columns, ..Default::default() }
    }

    fn catalog() -> Catalog {
        Catalog {
            schemas: vec![
                Schema {
                    name: "public".into(),
                    tables: vec![
                        table(
                            "customers",
                            vec![
                                col("id", TypeKind::Integer),
                                col("name", TypeKind::Text),
                                col("photo", TypeKind::Bytes),
                            ],
                        ),
                        table("blobs", vec![col("data", TypeKind::Bytes)]),
                        Table { kind: TableKind::View, ..table("v", vec![col("a", TypeKind::Text)]) },
                    ],
                    ..Default::default()
                },
                Schema {
                    name: "pg_catalog".into(),
                    tables: vec![table("pg_class", vec![col("relname", TypeKind::Text)])],
                    ..Default::default()
                },
                Schema {
                    name: "other".into(),
                    tables: vec![table("x", vec![col("t", TypeKind::Text)])],
                    ..Default::default()
                },
            ],
            default_schema: Some("public".into()),
        }
    }

    fn opts(text: &str) -> SearchOptions {
        SearchOptions::new(text)
    }

    #[test]
    fn predicates_per_dialect() {
        let o = opts("Ann");
        let cs = SearchOptions { case_sensitive: true, ..opts("Ann") };
        let whole = SearchOptions { whole_value: true, ..opts("Ann") };
        let whole_cs = SearchOptions { whole_value: true, case_sensitive: true, ..opts("Ann") };
        let pg = DatabaseKind::Postgres;
        assert_eq!(predicate(pg, "name", &o), "strpos(lower(CAST(\"name\" AS text)), lower('Ann')) > 0");
        assert_eq!(predicate(pg, "name", &cs), "strpos(CAST(\"name\" AS text), 'Ann') > 0");
        assert_eq!(predicate(pg, "name", &whole), "lower(CAST(\"name\" AS text)) = lower('Ann')");
        assert_eq!(predicate(pg, "name", &whole_cs), "CAST(\"name\" AS text) = 'Ann'");
        let my = DatabaseKind::MySql;
        assert_eq!(predicate(my, "name", &o), "LOCATE(lower('Ann'), lower(CAST(`name` AS CHAR))) > 0");
        assert_eq!(predicate(my, "name", &cs), "LOCATE(CAST('Ann' AS BINARY), CAST(`name` AS CHAR)) > 0");
        assert_eq!(predicate(my, "name", &whole_cs), "CAST(`name` AS CHAR) = CAST('Ann' AS BINARY)");
        let lite = DatabaseKind::Sqlite;
        assert_eq!(predicate(lite, "name", &o), "instr(lower(CAST(\"name\" AS TEXT)), lower('Ann')) > 0");
        assert_eq!(predicate(lite, "name", &cs), "instr(CAST(\"name\" AS TEXT), 'Ann') > 0");
        let duck = DatabaseKind::DuckDb;
        assert_eq!(predicate(duck, "name", &o), "strpos(lower(CAST(\"name\" AS VARCHAR)), lower('Ann')) > 0");
    }

    #[test]
    fn the_needle_and_the_identifiers_are_escaped() {
        let o = opts("O'Brien'; DROP TABLE x; --");
        let sql = predicate(DatabaseKind::Postgres, "we\"ird", &o);
        assert!(sql.contains("'O''Brien''; DROP TABLE x; --'"), "{sql}");
        assert!(sql.contains("\"we\"\"ird\""), "{sql}");
        // MySQL treats a backslash as an escape inside a literal; the literal builder doubles it.
        let sql = predicate(DatabaseKind::MySql, "c", &opts("a\\'b"));
        assert!(!sql.contains("'a\\'b'"), "an unescaped backslash would end the literal early: {sql}");
    }

    #[test]
    fn a_table_becomes_one_counting_statement() {
        let p =
            plan(&catalog(), DatabaseKind::Postgres, &Scope::Schema("public".into()), &opts("x")).unwrap();
        // `blobs` has only a binary column, the view is not a table.
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(
            p.skipped,
            vec![Skipped {
                schema: "public".into(),
                table: "blobs".into(),
                reason: "no column of the chosen types".into()
            }]
        );
        let j = &p.jobs[0];
        assert_eq!(j.columns, ["id", "name"], "the binary column is never searched");
        assert!(j.sql.starts_with("SELECT COUNT(*) AS scanned, COUNT(CASE WHEN ("), "{}", j.sql);
        assert!(j.sql.contains(" AS c0,") && j.sql.contains(" AS c1 FROM (SELECT \"id\", \"name\" FROM \"public\".\"customers\" LIMIT 100000) AS theta_search"), "{}", j.sql);
    }

    #[test]
    fn scope_decides_the_tables() {
        let all = plan(&catalog(), DatabaseKind::Postgres, &Scope::Connection, &opts("x")).unwrap();
        let names: Vec<_> = all.jobs.iter().map(|j| format!("{}.{}", j.schema, j.table)).collect();
        assert_eq!(names, ["public.customers", "other.x"], "pg_catalog is a system schema");
        let sys = plan(&catalog(), DatabaseKind::Postgres, &Scope::Schema("pg_catalog".into()), &opts("x"))
            .unwrap();
        assert_eq!(sys.jobs.len(), 1, "naming a system schema searches it");
        let one = plan(
            &catalog(),
            DatabaseKind::Postgres,
            &Scope::Tables(vec![("public".into(), "v".into()), ("other".into(), "x".into())]),
            &opts("x"),
        )
        .unwrap();
        assert_eq!(one.jobs.len(), 2, "an explicitly named view is searched");
    }

    #[test]
    fn column_classes_filter_columns() {
        let o = SearchOptions { classes: [ColumnClass::Text].into_iter().collect(), ..opts("x") };
        let p = plan(&catalog(), DatabaseKind::Sqlite, &Scope::Schema("public".into()), &o).unwrap();
        assert_eq!(p.jobs[0].columns, ["name"]);
        assert!(p.jobs[0].sql.contains("FROM \"public\".\"customers\""));
    }

    #[test]
    fn too_many_columns_are_cut_and_counted() {
        let many = table("wide", (0..5).map(|i| col(&format!("c{i}"), TypeKind::Text)).collect());
        let cat = Catalog {
            schemas: vec![Schema { name: "s".into(), tables: vec![many], ..Default::default() }],
            default_schema: None,
        };
        let o = SearchOptions { max_columns: 3, ..opts("x") };
        let p = plan(&cat, DatabaseKind::Postgres, &Scope::Connection, &o).unwrap();
        assert_eq!((p.jobs[0].columns.len(), p.jobs[0].columns_cut), (3, 2));
    }

    #[test]
    fn refusals() {
        let c = catalog();
        let pg = DatabaseKind::Postgres;
        assert!(plan(&c, pg, &Scope::Connection, &opts("")).is_err());
        assert!(plan(&c, pg, &Scope::Connection, &opts("a\0b")).is_err());
        let none = SearchOptions { classes: BTreeSet::new(), ..opts("x") };
        assert!(plan(&c, pg, &Scope::Connection, &none).is_err());
        let zero = SearchOptions { row_limit: 0, ..opts("x") };
        assert!(plan(&c, pg, &Scope::Connection, &zero).is_err());
        let e = plan(&c, DatabaseKind::SqlServer, &Scope::Connection, &opts("x")).unwrap_err();
        assert!(e.starts_with("SQL Server: "), "{e}");
        assert!(plan(&c, DatabaseKind::ClickHouse, &Scope::Connection, &opts("x")).is_err());
    }

    #[test]
    fn counts_are_read_from_the_row() {
        let job = TableJob {
            schema: "s".into(),
            table: "t".into(),
            columns: vec!["a".into(), "b".into(), "c".into()],
            columns_cut: 0,
            sql: String::new(),
            estimate: None,
        };
        let row = [Value::Int(1000), Value::Int(7), Value::Int(5), Value::Int(0), Value::Decimal("3".into())];
        let c = parse_counts(&job, &row).unwrap();
        assert_eq!(
            c,
            Counts { scanned: 1000, matched_rows: 7, columns: vec![("a".into(), 5), ("c".into(), 3)] }
        );
        assert!(parse_counts(&job, &row[..3]).is_err());
        assert!(
            parse_counts(
                &job,
                &[Value::Text("x".into()), Value::Int(0), Value::Int(0), Value::Int(0), Value::Int(0)]
            )
            .is_err()
        );
    }

    #[test]
    fn the_filter_uses_the_same_predicates() {
        let o = opts("ann");
        let sql = filter_statement(
            DatabaseKind::Postgres,
            "public",
            "customers",
            &["name".into(), "email".into()],
            &o,
        );
        assert_eq!(
            sql,
            "SELECT * FROM \"public\".\"customers\" WHERE (strpos(lower(CAST(\"name\" AS text)), lower('ann')) > 0) OR (strpos(lower(CAST(\"email\" AS text)), lower('ann')) > 0)"
        );
        let lite = filter_statement(DatabaseKind::Sqlite, "", "t", &["c".into()], &o);
        assert!(lite.starts_with("SELECT * FROM \"t\" WHERE "), "{lite}");
    }

    #[test]
    fn classes_cover_every_type_kind() {
        for kind in [
            TypeKind::Bool,
            TypeKind::Integer,
            TypeKind::Float,
            TypeKind::Decimal,
            TypeKind::Text,
            TypeKind::Json,
            TypeKind::Uuid,
            TypeKind::Date,
            TypeKind::Time,
            TypeKind::Timestamp,
            TypeKind::Interval,
            TypeKind::Array,
            TypeKind::Other,
        ] {
            assert!(ColumnClass::of(kind).is_some(), "{kind:?}");
        }
        assert!(ColumnClass::of(TypeKind::Bytes).is_none() && ColumnClass::of(TypeKind::Geometry).is_none());
    }
}
