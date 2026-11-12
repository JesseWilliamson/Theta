//! Schema comparison: what differs between two schemas, as data (this module), and the migration script that
//! makes the *target* look like the *source* (`script`).
//!
//! # Direction
//!
//! "Source" is what you want, "target" is what you have. Every difference is stated from the target's point of
//! view in the script (`OnlyInSource` is created, `OnlyInTarget` is dropped) and from both in the tree.
//!
//! # What is compared, and what the catalog does not carry
//!
//! Compared, as far as `theta_core::Catalog` carries them: tables, columns (type, nullability, default, identity,
//! generated expression, comment), primary keys, unique constraints, foreign keys (columns, target, `ON DELETE`
//! and `ON UPDATE`, deferrability), indexes (columns, uniqueness, method, partial predicate, expression
//! definition), check constraints, table comments, MySQL table options, and the definition text of views and
//! materialized views. Routines are compared by name, kind, signature and return type only: **the catalog has
//! no routine body**, so a routine whose body changed is not noticed, and none can be generated.
//!
//! Not carried by the catalog, therefore **not compared**: triggers, sequences' parameters (sequence names are
//! listed but many belong to a column, so they are not compared either), column collations, privileges and
//! ownership, partitioning, tablespaces and storage parameters, custom types (enums, domains), extensions,
//! column order. [`NOT_COMPARED`] is the list the dialog prints.
//!
//! # Equality rules (same for the diff and the script, or the script would not converge)
//!
//! * Names match case-insensitively. A constraint or index is identified by its **name**: one that is renamed
//!   appears as one removed and one added. A primary key is identified by its columns (its name is ignored: it
//!   is `<table>_pkey` by default and `PRIMARY` in MySQL).
//! * Types: the same product compares the type text (whitespace and case folded); two products compare the
//!   canonical type the table copy uses (`copy::canon_of`), so `int4` and `INT` agree.
//! * A reference to another table of the *same schema* is the same reference whichever schema that is; a
//!   `nextval('schema.seq'::regclass)` default is compared without the schema.
//! * `NO ACTION` and no action are the same; so is `RESTRICT` in MySQL.

mod script;
#[cfg(test)]
mod tests;

pub use script::{Script, ScriptStatement, generate};

use crate::copy::canon_of;
use theta_core::{
    ColumnInfo, DatabaseKind, ForeignKey, Identity, Index, Routine, Schema, Table, TableKind,
    UniqueConstraint,
};

/// What the comparison cannot see, in words, for the dialog and the documentation.
pub const NOT_COMPARED: &[&str] = &[
    "triggers",
    "routine bodies (routines are compared by name, signature and return type)",
    "sequences",
    "column collations and character sets",
    "privileges and ownership",
    "partitioning, tablespaces and storage parameters",
    "custom types (enums, domains) and extensions",
    "column order",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Routine,
}

impl ObjectKind {
    pub fn label(self) -> &'static str {
        match self {
            ObjectKind::Table => "table",
            ObjectKind::View => "view",
            ObjectKind::MaterializedView => "materialized view",
            ObjectKind::Routine => "routine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Exists in the source only: the script creates it.
    OnlyInSource,
    /// Exists in the target only: the script drops it.
    OnlyInTarget,
    Different,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::OnlyInSource => "only in source",
            Status::OnlyInTarget => "only in target",
            Status::Different => "different",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Element {
    Column,
    PrimaryKey,
    Unique,
    ForeignKey,
    Index,
    Check,
    TableComment,
    TableOption,
    Definition,
    Signature,
}

impl Element {
    pub fn label(self) -> &'static str {
        match self {
            Element::Column => "column",
            Element::PrimaryKey => "primary key",
            Element::Unique => "unique constraint",
            Element::ForeignKey => "foreign key",
            Element::Index => "index",
            Element::Check => "check",
            Element::TableComment => "table comment",
            Element::TableOption => "table option",
            Element::Definition => "definition",
            Element::Signature => "signature",
        }
    }
}

/// One difference inside an object.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub element: Element,
    pub name: String,
    pub status: Status,
    /// How the element reads in the source / the target (absent where it does not exist).
    pub source: Option<String>,
    pub target: Option<String>,
    /// For a [`Status::Different`] element: which attributes differ (`type`, `nullability`, ...).
    pub attributes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectDiff {
    pub kind: ObjectKind,
    pub name: String,
    pub status: Status,
    /// The differences inside an object that exists on both sides.
    pub changes: Vec<Change>,
    /// A full listing of an object that exists on one side only, and the definition text of a changed view.
    pub source_text: Option<String>,
    pub target_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaDiff {
    pub source_kind: DatabaseKind,
    pub target_kind: DatabaseKind,
    pub objects: Vec<ObjectDiff>,
    /// Notes the result needs (a view definition that could not be read, foreign tables skipped).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub only_in_source: usize,
    pub only_in_target: usize,
    pub different: usize,
}

impl SchemaDiff {
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn same_product(&self) -> bool {
        self.source_kind == self.target_kind
    }

    pub fn counts(&self, kind: ObjectKind) -> Counts {
        let mut c = Counts::default();
        for o in self.objects.iter().filter(|o| o.kind == kind) {
            match o.status {
                Status::OnlyInSource => c.only_in_source += 1,
                Status::OnlyInTarget => c.only_in_target += 1,
                Status::Different => c.different += 1,
            }
        }
        c
    }

    /// Why a script cannot be generated, if so.
    pub fn script_blocker(&self) -> Option<String> {
        if !self.same_product() {
            return Some(format!(
                "The source is {} and the target is {}: SQL generated for one product does not run on the other, so only the differences are shown.",
                self.source_kind.label(),
                self.target_kind.label()
            ));
        }
        if let Some(why) = self.target_kind.unsupported(theta_core::Feature::ModifyTable) {
            return Some(format!("{}: {why}", self.target_kind.label()));
        }
        None
    }

    /// The most useful one-line summary: `3 tables differ · 1 only in source · 2 only in target`.
    pub fn summary(&self) -> String {
        if self.objects.is_empty() {
            return "The schemas are identical in everything that was compared".to_string();
        }
        let mut parts = Vec::new();
        for kind in [ObjectKind::Table, ObjectKind::View, ObjectKind::MaterializedView, ObjectKind::Routine] {
            let c = self.counts(kind);
            let n = c.only_in_source + c.only_in_target + c.different;
            if n == 0 {
                continue;
            }
            let plural = if n == 1 { "" } else { "s" };
            let mut bits = Vec::new();
            if c.different > 0 {
                bits.push(format!("{} different", c.different));
            }
            if c.only_in_source > 0 {
                bits.push(format!("{} only in source", c.only_in_source));
            }
            if c.only_in_target > 0 {
                bits.push(format!("{} only in target", c.only_in_target));
            }
            parts.push(format!("{n} {}{plural} ({})", kind.label(), bits.join(", ")));
        }
        parts.join(" · ")
    }
}

// ------------------------------------------------------------------------------------------ helpers

fn lc(s: &str) -> String {
    s.to_lowercase()
}

/// Case and whitespace folded: `  Integer   NOT NULL` and `integer not null` are one text.
pub(crate) fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// A default, folded; a sequence default loses its schema, because the sequence of `a.t` is `a.t_id_seq` and of
/// `b.t` is `b.t_id_seq`, which is not a difference anyone means.
pub(crate) fn norm_default(d: Option<&str>) -> Option<String> {
    let d = norm(d?);
    if let Some(rest) = d.strip_prefix("nextval(")
        && let (Some(a), Some(b)) = (rest.find('\''), rest.rfind('\''))
        && b > a
    {
        let seq = &rest[a + 1..b];
        let bare = seq.rsplit('.').next().unwrap_or(seq).trim_matches('"');
        return Some(format!("nextval({bare})"));
    }
    Some(d)
}

/// A definition with every `schema.` prefix removed, folded.
pub(crate) fn strip_schema(text: &str, schema: &str) -> String {
    let t = norm(text);
    let s = lc(schema);
    t.replace(&format!("\"{s}\"."), "").replace(&format!("{s}."), "")
}

pub(crate) fn same_type(sk: DatabaseKind, s: &ColumnInfo, tk: DatabaseKind, t: &ColumnInfo) -> bool {
    if sk == tk { norm(&s.data_type) == norm(&t.data_type) } else { canon_of(sk, s) == canon_of(tk, t) }
}

pub(crate) fn actions_equal(kind: DatabaseKind, a: &Option<String>, b: &Option<String>) -> bool {
    let n = |x: &Option<String>| {
        let v = x.as_deref().map(str::to_ascii_uppercase).unwrap_or_default();
        let mysql = kind.family() == theta_core::Family::MySql;
        if v.is_empty() || v == "NO ACTION" || (mysql && v == "RESTRICT") { String::new() } else { v }
    };
    n(a) == n(b)
}

pub(crate) fn same_cols(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn cols_text(c: &[String]) -> String {
    format!("({})", c.join(", "))
}

pub(crate) fn describe_column(c: &ColumnInfo) -> String {
    let mut s = c.data_type.clone();
    s.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
    if c.identity.is_none()
        && c.generated.is_none()
        && let Some(d) = &c.default
    {
        s.push_str(&format!(" DEFAULT {d}"));
    }
    match c.identity {
        Some(Identity::Always) => s.push_str(" IDENTITY ALWAYS"),
        Some(Identity::ByDefault) => s.push_str(" IDENTITY"),
        None => {}
    }
    if let Some(g) = &c.generated {
        s.push_str(&format!(
            " GENERATED AS ({}) {}",
            g.expression,
            if g.stored { "STORED" } else { "VIRTUAL" }
        ));
    }
    if let Some(cm) = c.comment.as_ref().filter(|c| !c.is_empty()) {
        s.push_str(&format!(" COMMENT '{cm}'"));
    }
    s
}

/// Whether two foreign keys are the same key to compare. SQLite's catalog has no constraint names (the driver
/// numbers them by position, which an added key shifts), so there a key is identified by what it connects;
/// elsewhere by its name.
pub(crate) fn fk_matches(kind: DatabaseKind, a: &ForeignKey, b: &ForeignKey) -> bool {
    if kind.family() == theta_core::Family::Sqlite {
        same_cols(&a.columns, &b.columns)
            && a.referenced_table.eq_ignore_ascii_case(&b.referenced_table)
            && same_cols(&a.referenced_columns, &b.referenced_columns)
    } else {
        a.name.eq_ignore_ascii_case(&b.name)
    }
}

fn describe_fk(f: &ForeignKey, own_schema: &str) -> String {
    let mut s = format!("{} -> ", cols_text(&f.columns));
    if let Some(r) = f.referenced_schema.as_deref().filter(|r| !r.eq_ignore_ascii_case(own_schema)) {
        s.push_str(&format!("{r}."));
    }
    s.push_str(&format!("{} {}", f.referenced_table, cols_text(&f.referenced_columns)));
    for (what, v) in [("DELETE", &f.on_delete), ("UPDATE", &f.on_update)] {
        if let Some(a) = v.as_deref().filter(|a| !a.eq_ignore_ascii_case("NO ACTION")) {
            s.push_str(&format!(" ON {what} {}", a.to_ascii_uppercase()));
        }
    }
    if f.deferrable {
        s.push_str(if f.initially_deferred { " DEFERRABLE INITIALLY DEFERRED" } else { " DEFERRABLE" });
    }
    s
}

fn describe_index(i: &Index) -> String {
    let mut s = String::new();
    if i.unique {
        s.push_str("UNIQUE ");
    }
    if let Some(m) = &i.method {
        s.push_str(&format!("{m} "));
    }
    if i.has_expressions {
        s.push_str(i.definition.as_deref().unwrap_or("(expression)"));
    } else {
        s.push_str(&cols_text(&i.columns));
    }
    if let Some(p) = &i.predicate {
        s.push_str(&format!(" WHERE {p}"));
    }
    s
}

/// An index that is the implementation of the primary key or of a unique constraint is compared as that
/// constraint, not twice.
fn is_constraint_index(t: &Table, i: &Index) -> bool {
    i.primary || t.unique_constraints.iter().any(|u| u.name == i.name)
}

/// A table as text, for the detail pane of a table that exists on one side only.
pub fn describe_table(t: &Table, schema: &str) -> String {
    let mut out = format!("{} {}\n", if t.kind == TableKind::Table { "TABLE" } else { "VIEW" }, t.name);
    for c in &t.columns {
        out.push_str(&format!("  {}  {}\n", c.name, describe_column(c)));
    }
    if !t.primary_key.is_empty() {
        out.push_str(&format!("  PRIMARY KEY {}\n", cols_text(&t.primary_key)));
    }
    for u in &t.unique_constraints {
        out.push_str(&format!("  UNIQUE {} {}\n", u.name, cols_text(&u.columns)));
    }
    for f in &t.foreign_keys {
        out.push_str(&format!("  FOREIGN KEY {} {}\n", f.name, describe_fk(f, schema)));
    }
    for c in &t.checks {
        out.push_str(&format!("  CHECK {} ({})\n", c.name, c.expression));
    }
    for i in t.indexes.iter().filter(|i| !is_constraint_index(t, i)) {
        out.push_str(&format!("  INDEX {} {}\n", i.name, describe_index(i)));
    }
    if let Some(c) = t.comment.as_ref().filter(|c| !c.is_empty()) {
        out.push_str(&format!("  COMMENT '{c}'\n"));
    }
    out
}

// ------------------------------------------------------------------------------------------ compare

/// A schema and the product it came from.
#[derive(Debug, Clone, Copy)]
pub struct Side<'a> {
    pub kind: DatabaseKind,
    pub schema: &'a Schema,
}

/// Compare `source` with `target`. Objects are ordered tables first, then views, materialized views and
/// routines, each by name.
pub fn compare(source: Side<'_>, target: Side<'_>) -> SchemaDiff {
    let mut objects = Vec::new();
    let mut notes = Vec::new();
    let pick = |s: &Schema, wanted: &[TableKind]| -> Vec<Table> {
        let mut v: Vec<Table> = s.tables.iter().filter(|t| wanted.contains(&t.kind)).cloned().collect();
        v.sort_by_key(|t| lc(&t.name));
        v
    };
    let skipped = source
        .schema
        .tables
        .iter()
        .chain(&target.schema.tables)
        .filter(|t| t.kind == TableKind::ForeignTable)
        .count();
    if skipped > 0 {
        notes.push(format!(
            "{skipped} foreign table{} not compared",
            if skipped == 1 { " was" } else { "s were" }
        ));
    }
    for (kind, wanted) in [
        (ObjectKind::Table, &[TableKind::Table][..]),
        (ObjectKind::View, &[TableKind::View][..]),
        (ObjectKind::MaterializedView, &[TableKind::MaterializedView][..]),
    ] {
        let (s, t) = (pick(source.schema, wanted), pick(target.schema, wanted));
        let mut names: Vec<String> = s.iter().chain(&t).map(|x| lc(&x.name)).collect();
        names.sort();
        names.dedup();
        for n in names {
            let a = s.iter().find(|x| lc(&x.name) == n);
            let b = t.iter().find(|x| lc(&x.name) == n);
            match (a, b) {
                (Some(a), None) => objects.push(ObjectDiff {
                    kind,
                    name: a.name.clone(),
                    status: Status::OnlyInSource,
                    changes: vec![],
                    source_text: Some(object_text(a, &source.schema.name)),
                    target_text: None,
                }),
                (None, Some(b)) => objects.push(ObjectDiff {
                    kind,
                    name: b.name.clone(),
                    status: Status::OnlyInTarget,
                    changes: vec![],
                    source_text: None,
                    target_text: Some(object_text(b, &target.schema.name)),
                }),
                (Some(a), Some(b)) => {
                    let changes = if kind == ObjectKind::Table {
                        compare_tables(source, a, target, b)
                    } else {
                        compare_views(source, a, target, b, &mut notes)
                    };
                    if !changes.is_empty() {
                        objects.push(ObjectDiff {
                            kind,
                            name: a.name.clone(),
                            status: Status::Different,
                            source_text: (kind != ObjectKind::Table)
                                .then(|| a.view_definition.clone().unwrap_or_default()),
                            target_text: (kind != ObjectKind::Table)
                                .then(|| b.view_definition.clone().unwrap_or_default()),
                            changes,
                        });
                    }
                }
                (None, None) => {}
            }
        }
    }
    compare_routines(source.schema, target.schema, &mut objects);
    SchemaDiff { source_kind: source.kind, target_kind: target.kind, objects, notes }
}

fn object_text(t: &Table, schema: &str) -> String {
    match t.kind {
        TableKind::Table | TableKind::ForeignTable => describe_table(t, schema),
        _ => t.view_definition.clone().unwrap_or_else(|| "(the definition could not be read)".to_string()),
    }
}

fn compare_views(
    source: Side<'_>,
    a: &Table,
    target: Side<'_>,
    b: &Table,
    notes: &mut Vec<String>,
) -> Vec<Change> {
    match (&a.view_definition, &b.view_definition) {
        (Some(x), Some(y)) => {
            if strip_schema(x, &source.schema.name) == strip_schema(y, &target.schema.name) {
                vec![]
            } else {
                vec![Change {
                    element: Element::Definition,
                    name: a.name.clone(),
                    status: Status::Different,
                    source: Some(x.clone()),
                    target: Some(y.clone()),
                    attributes: vec!["definition".into()],
                }]
            }
        }
        (None, None) => vec![],
        _ => {
            notes.push(format!(
                "The definition of view {} could not be read on one side; it was not compared",
                a.name
            ));
            vec![]
        }
    }
}

fn compare_routines(s: &Schema, t: &Schema, out: &mut Vec<ObjectDiff>) {
    let key = |r: &Routine| format!("{}{}", lc(&r.name), norm(&r.signature));
    let text = |r: &Routine| {
        format!(
            "{:?} {}{}{}",
            r.kind,
            r.name,
            r.signature,
            r.return_type.as_deref().map(|x| format!(" RETURNS {x}")).unwrap_or_default()
        )
    };
    let mut keys: Vec<String> = s.routines.iter().chain(&t.routines).map(key).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let a = s.routines.iter().find(|r| key(r) == k);
        let b = t.routines.iter().find(|r| key(r) == k);
        let (name, status, changes) = match (a, b) {
            (Some(a), None) => (a.name.clone(), Status::OnlyInSource, vec![]),
            (None, Some(b)) => (b.name.clone(), Status::OnlyInTarget, vec![]),
            (Some(a), Some(b)) => {
                let mut attrs = Vec::new();
                if a.kind != b.kind {
                    attrs.push("kind".to_string());
                }
                if a.return_type.as_deref().map(norm) != b.return_type.as_deref().map(norm) {
                    attrs.push("return type".to_string());
                }
                if attrs.is_empty() {
                    continue;
                }
                (
                    a.name.clone(),
                    Status::Different,
                    vec![Change {
                        element: Element::Signature,
                        name: a.name.clone(),
                        status: Status::Different,
                        source: Some(text(a)),
                        target: Some(text(b)),
                        attributes: attrs,
                    }],
                )
            }
            (None, None) => continue,
        };
        out.push(ObjectDiff {
            kind: ObjectKind::Routine,
            name: format!("{name}{}", a.or(b).map(|r| r.signature.clone()).unwrap_or_default()),
            status,
            changes,
            source_text: a.map(text),
            target_text: b.map(text),
        });
    }
}

fn column_attributes(sk: DatabaseKind, a: &ColumnInfo, tk: DatabaseKind, b: &ColumnInfo) -> Vec<String> {
    let mut attrs = Vec::new();
    if !same_type(sk, a, tk, b) {
        attrs.push("type".to_string());
    }
    if a.nullable != b.nullable {
        attrs.push("nullability".to_string());
    }
    let (da, db) = (
        if a.identity.is_some() || a.generated.is_some() { None } else { norm_default(a.default.as_deref()) },
        if b.identity.is_some() || b.generated.is_some() { None } else { norm_default(b.default.as_deref()) },
    );
    if da != db {
        attrs.push("default".to_string());
    }
    if a.identity != b.identity {
        attrs.push("identity".to_string());
    }
    if a.generated.as_ref().map(|g| (norm(&g.expression), g.stored))
        != b.generated.as_ref().map(|g| (norm(&g.expression), g.stored))
    {
        attrs.push("generated expression".to_string());
    }
    let cm = |c: &ColumnInfo| c.comment.clone().filter(|c| !c.is_empty());
    if cm(a) != cm(b) {
        attrs.push("comment".to_string());
    }
    attrs
}

fn compare_tables(source: Side<'_>, a: &Table, target: Side<'_>, b: &Table) -> Vec<Change> {
    let (ss, ts) = (source.schema.name.as_str(), target.schema.name.as_str());
    let sqlite = source.kind.family() == theta_core::Family::Sqlite;
    let mut out = Vec::new();
    let mut push =
        |element, name: &str, status, s: Option<String>, t: Option<String>, attributes: Vec<String>| {
            out.push(Change { element, name: name.to_string(), status, source: s, target: t, attributes });
        };

    // Columns.
    for c in &a.columns {
        match b.columns.iter().find(|x| x.name.eq_ignore_ascii_case(&c.name)) {
            None => {
                push(Element::Column, &c.name, Status::OnlyInSource, Some(describe_column(c)), None, vec![])
            }
            Some(d) => {
                let attrs = column_attributes(source.kind, c, target.kind, d);
                if !attrs.is_empty() {
                    push(
                        Element::Column,
                        &c.name,
                        Status::Different,
                        Some(describe_column(c)),
                        Some(describe_column(d)),
                        attrs,
                    );
                }
            }
        }
    }
    for d in &b.columns {
        if !a.columns.iter().any(|x| x.name.eq_ignore_ascii_case(&d.name)) {
            push(Element::Column, &d.name, Status::OnlyInTarget, None, Some(describe_column(d)), vec![]);
        }
    }

    // Primary key: by columns.
    match (a.primary_key.is_empty(), b.primary_key.is_empty()) {
        (false, true) => push(
            Element::PrimaryKey,
            "primary key",
            Status::OnlyInSource,
            Some(cols_text(&a.primary_key)),
            None,
            vec![],
        ),
        (true, false) => push(
            Element::PrimaryKey,
            "primary key",
            Status::OnlyInTarget,
            None,
            Some(cols_text(&b.primary_key)),
            vec![],
        ),
        (false, false) if !same_cols(&a.primary_key, &b.primary_key) => push(
            Element::PrimaryKey,
            "primary key",
            Status::Different,
            Some(cols_text(&a.primary_key)),
            Some(cols_text(&b.primary_key)),
            vec!["columns".into()],
        ),
        _ => {}
    }

    // Unique constraints, foreign keys, checks and indexes: by name.
    named(
        &a.unique_constraints,
        &b.unique_constraints,
        |u: &UniqueConstraint| u.name.clone(),
        &|u, v| u.name.eq_ignore_ascii_case(&v.name),
        |u, v| same_cols(&u.columns, &v.columns).then(Vec::new).ok_or_else(|| vec!["columns".to_string()]),
        |u| cols_text(&u.columns),
        Element::Unique,
        &mut push,
    );
    named(
        &a.foreign_keys,
        &b.foreign_keys,
        |f: &ForeignKey| {
            if sqlite {
                format!("{} -> {}", cols_text(&f.columns), f.referenced_table)
            } else {
                f.name.clone()
            }
        },
        &|f, g| fk_matches(source.kind, f, g),
        |f, g| {
            let mut attrs = Vec::new();
            let own = |r: &Option<String>, me: &str| {
                r.clone().filter(|r| !r.eq_ignore_ascii_case(me)).map(|r| lc(&r))
            };
            if !same_cols(&f.columns, &g.columns) {
                attrs.push("columns".to_string());
            }
            if !f.referenced_table.eq_ignore_ascii_case(&g.referenced_table)
                || own(&f.referenced_schema, ss) != own(&g.referenced_schema, ts)
                || !same_cols(&f.referenced_columns, &g.referenced_columns)
            {
                attrs.push("referenced columns".to_string());
            }
            if !actions_equal(source.kind, &f.on_delete, &g.on_delete) {
                attrs.push("ON DELETE".to_string());
            }
            if !actions_equal(source.kind, &f.on_update, &g.on_update) {
                attrs.push("ON UPDATE".to_string());
            }
            if (f.deferrable, f.initially_deferred) != (g.deferrable, g.initially_deferred) {
                attrs.push("deferrable".to_string());
            }
            if attrs.is_empty() { Ok(Vec::new()) } else { Err(attrs) }
        },
        |f| describe_fk(f, ss),
        Element::ForeignKey,
        &mut push,
    );
    named(
        &a.checks,
        &b.checks,
        |c| c.name.clone(),
        &|c, d| c.name.eq_ignore_ascii_case(&d.name),
        |c, d| {
            (norm(&c.expression) == norm(&d.expression))
                .then(Vec::new)
                .ok_or_else(|| vec!["expression".to_string()])
        },
        |c| c.expression.clone(),
        Element::Check,
        &mut push,
    );
    let ai: Vec<&Index> = a.indexes.iter().filter(|i| !is_constraint_index(a, i)).collect();
    let bi: Vec<&Index> = b.indexes.iter().filter(|i| !is_constraint_index(b, i)).collect();
    named(
        &ai,
        &bi,
        |i| i.name.clone(),
        &|i, j| i.name.eq_ignore_ascii_case(&j.name),
        |i, j| {
            let mut attrs = Vec::new();
            if i.has_expressions || j.has_expressions {
                let d = |x: &Index, s: &str| x.definition.as_deref().map(|d| strip_schema(d, s));
                if i.has_expressions != j.has_expressions || d(i, ss) != d(j, ts) {
                    attrs.push("definition".to_string());
                }
            } else if !same_cols(&i.columns, &j.columns) {
                attrs.push("columns".to_string());
            }
            if i.unique != j.unique {
                attrs.push("uniqueness".to_string());
            }
            let m = |x: &Index| x.method.as_deref().map(lc).filter(|m| m != "btree");
            if m(i) != m(j) {
                attrs.push("method".to_string());
            }
            if i.predicate.as_deref().map(|p| strip_schema(p, ss))
                != j.predicate.as_deref().map(|p| strip_schema(p, ts))
            {
                attrs.push("predicate".to_string());
            }
            if attrs.is_empty() { Ok(Vec::new()) } else { Err(attrs) }
        },
        |i| describe_index(i),
        Element::Index,
        &mut push,
    );

    // Comment and MySQL table options.
    let cm = |t: &Table| t.comment.clone().filter(|c| !c.is_empty());
    if cm(a) != cm(b) {
        let status = match (cm(a), cm(b)) {
            (Some(_), None) => Status::OnlyInSource,
            (None, Some(_)) => Status::OnlyInTarget,
            _ => Status::Different,
        };
        push(Element::TableComment, "comment", status, cm(a), cm(b), vec![]);
    }
    let mut opt_names: Vec<String> = a.options.iter().chain(&b.options).map(|o| lc(&o.name)).collect();
    opt_names.sort();
    opt_names.dedup();
    for n in opt_names {
        let get = |t: &Table| t.options.iter().find(|o| lc(&o.name) == n).map(|o| o.value.clone());
        if get(a).map(|v| lc(&v)) != get(b).map(|v| lc(&v)) {
            push(Element::TableOption, &n.to_uppercase(), Status::Different, get(a), get(b), vec![]);
        }
    }
    out
}

/// Pair two lists by name (case-insensitively) and report what exists on one side only or differs.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn named<T>(
    a: &[T],
    b: &[T],
    name: impl Fn(&T) -> String,
    same: &dyn Fn(&T, &T) -> bool,
    differs: impl Fn(&T, &T) -> Result<Vec<String>, Vec<String>>,
    text: impl Fn(&T) -> String,
    element: Element,
    push: &mut impl FnMut(Element, &str, Status, Option<String>, Option<String>, Vec<String>),
) {
    for x in a {
        match b.iter().find(|y| same(x, y)) {
            None => push(element, &name(x), Status::OnlyInSource, Some(text(x)), None, vec![]),
            Some(y) => {
                if let Err(attrs) = differs(x, y) {
                    push(element, &name(x), Status::Different, Some(text(x)), Some(text(y)), attrs);
                }
            }
        }
    }
    for y in b {
        if !a.iter().any(|x| same(x, y)) {
            push(element, &name(y), Status::OnlyInTarget, None, Some(text(y)), vec![]);
        }
    }
}
