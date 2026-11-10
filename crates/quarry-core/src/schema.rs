//! Database structure as shown in the explorer tree and used by autocomplete,
//! foreign-key join suggestions and the editable grid.

use serde::{Deserialize, Serialize};

/// Everything introspected from one connection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Postgres: schemas of the connected database. MySQL: databases
    /// (MySQL calls them schemas too). SQLite: `main` plus attached databases.
    pub schemas: Vec<Schema>,
    /// The schema unqualified names resolve to first (`public`, the current
    /// MySQL database, `main`).
    pub default_schema: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
    pub routines: Vec<Routine>,
    pub sequences: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnInfo>,
    /// Column names of the primary key, in key order. Empty when there is none.
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<Index>,
    /// Planner's row estimate (not an exact count). `None` when unknown.
    pub row_estimate: Option<u64>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    /// Full type as the database prints it, e.g. `numeric(10,2)`.
    pub data_type: String,
    pub kind: crate::TypeKind,
    pub nullable: bool,
    pub default: Option<String>,
    pub comment: Option<String>,
    /// Set for spatial columns.
    pub geometry: Option<GeometryColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeometryColumn {
    /// e.g. `POINT`, `MULTIPOLYGON`, `GEOMETRY` (any).
    pub geometry_type: String,
    /// Spatial reference id; 0 or `None` means unknown.
    pub srid: Option<i32>,
    /// Postgres `geography` (true) vs `geometry` (false).
    pub geography: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    /// Columns in *this* table, positionally paired with `referenced_columns`.
    pub columns: Vec<String>,
    pub referenced_schema: Option<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    /// Access method when known: `btree`, `gist`, `gin`, `hash`, ...
    pub method: Option<String>,
    /// Full definition (`CREATE INDEX ...`) when the database exposes it.
    pub definition: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoutineKind {
    Function,
    Procedure,
    Aggregate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routine {
    pub name: String,
    pub kind: RoutineKind,
    /// Argument list as text, e.g. `(a integer, b text)`.
    pub signature: String,
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Short product + version, e.g. `PostgreSQL 16.4`.
    pub version: String,
    pub current_database: Option<String>,
    pub current_user: Option<String>,
}

impl Catalog {
    pub fn schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Resolve `[schema.]table`, falling back to the default schema and then
    /// to any schema that has a table with that name.
    pub fn find_table(&self, schema: Option<&str>, table: &str) -> Option<(&Schema, &Table)> {
        // A nested fn rather than a closure: closures cannot declare that the
        // returned `&Table` borrows from the `&Schema` argument.
        fn in_schema<'a>(s: &'a Schema, table: &str) -> Option<&'a Table> {
            s.tables.iter().find(|t| t.name.eq_ignore_ascii_case(table))
        }
        if let Some(name) = schema {
            let s = self.schema(name)?;
            return in_schema(s, table).map(|t| (s, t));
        }
        if let Some(default) = self.default_schema.as_deref().and_then(|d| self.schema(d))
            && let Some(t) = in_schema(default, table)
        {
            return Some((default, t));
        }
        self.schemas.iter().find_map(|s| in_schema(s, table).map(|t| (s, t)))
    }
}
