//! Pure model behind the explorer: turns connections + catalogs + expansion
//! state + a filter into a flat list of rows.
//!
//! Flattening only descends into *expanded* nodes, so the cost is proportional
//! to what is visible (plus a name scan when filtering), not to the size of the
//! catalog. The view virtualises the resulting list.

use quarry_core::{Catalog, DatabaseKind, Routine, Schema, Table, TableKind};
use quarry_sql::VirtualForeignKey;
use quarry_ui::fuzzy::fuzzy_match;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Tables,
    Views,
    Routines,
    Sequences,
}

impl GroupKind {
    pub const ALL: [GroupKind; 4] =
        [GroupKind::Tables, GroupKind::Views, GroupKind::Routines, GroupKind::Sequences];
    pub fn label(self) -> &'static str {
        match self {
            GroupKind::Tables => "Tables",
            GroupKind::Views => "Views",
            GroupKind::Routines => "Routines",
            GroupKind::Sequences => "Sequences",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Keys,
    ForeignKeys,
    /// Inferred foreign keys, nested under `ForeignKeys`.
    Suggested,
    Indexes,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Keys => "Keys",
            Section::ForeignKeys => "Foreign Keys",
            Section::Suggested => "Suggested",
            Section::Indexes => "Indexes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Connection(String),
    /// Status line under a connection (connecting, failed, loading, empty).
    Status(String),
    /// One wrapped line of a long status message (a connection error), under its `Status` row.
    Detail(String, usize),
    Schema(String, String),
    Group(String, String, GroupKind),
    Table(String, String, String),
    Column(String, String, String, String),
    Section(String, String, String, Section),
    Leaf(String, String, String, Section, String),
    Routine(String, String, String),
    Sequence(String, String, String),
}

impl NodeKey {
    pub fn connection_id(&self) -> &str {
        match self {
            NodeKey::Connection(c)
            | NodeKey::Status(c)
            | NodeKey::Detail(c, _)
            | NodeKey::Schema(c, _)
            | NodeKey::Group(c, _, _)
            | NodeKey::Table(c, _, _)
            | NodeKey::Column(c, _, _, _)
            | NodeKey::Section(c, _, _, _)
            | NodeKey::Leaf(c, _, _, _, _)
            | NodeKey::Routine(c, _, _)
            | NodeKey::Sequence(c, _, _) => c,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowIcon {
    Connection(DatabaseKind),
    Schema,
    Group(GroupKind),
    Table,
    View,
    MaterializedView,
    ForeignTable,
    Column,
    PrimaryKeyColumn,
    ForeignKeyColumn,
    Key,
    ForeignKey,
    /// An inferred (undeclared) foreign key: dashed, muted.
    SuggestedForeignKey,
    Index,
    Routine,
    Sequence,
    Spinner,
    Error,
    Info,
    /// No icon: continuation lines of a message.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnDot {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: NodeKey,
    pub depth: u8,
    pub icon: RowIcon,
    pub label: String,
    /// Muted right-aligned text: row estimate, or a count for groups.
    pub detail: Option<String>,
    /// Muted text right after the label: a column's data type.
    pub type_text: Option<String>,
    pub expandable: bool,
    pub expanded: bool,
    /// Byte offsets of matched filter characters in `label`.
    pub highlights: Vec<usize>,
    pub nullable: Option<bool>,
    pub dot: Option<ConnDot>,
    /// Hex colour tag of a connection.
    pub color: Option<String>,
    /// Error-styled text (failed connection message).
    pub is_error: bool,
    /// A clickable "Retry" affordance on this row.
    pub retry: bool,
    /// Hover text.
    pub tooltip: Option<String>,
    /// Drawn in the muted colour (suggestions).
    pub muted: bool,
}

impl Row {
    fn new(key: NodeKey, depth: u8, icon: RowIcon, label: impl Into<String>) -> Self {
        Row {
            key,
            depth,
            icon,
            label: label.into(),
            detail: None,
            type_text: None,
            expandable: false,
            expanded: false,
            highlights: Vec::new(),
            nullable: None,
            dot: None,
            color: None,
            is_error: false,
            retry: false,
            tooltip: None,
            muted: false,
        }
    }
}

pub enum Phase<'a> {
    Disconnected,
    Connecting,
    Failed(String),
    Connected { catalog: Option<&'a Catalog>, loading: bool, catalog_error: Option<String> },
}

pub struct ConnView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub kind: DatabaseKind,
    pub color: Option<&'a str>,
    pub phase: Phase<'a>,
    /// Foreign keys implied by naming (see `quarry_sql::infer_foreign_keys`).
    pub inferred: &'a [VirtualForeignKey],
}

/// `1234 -> "1.2k"`, `5_600_000 -> "5.6M"`.
/// Stable leaf name of a suggested foreign key: `column|schema.table.column`.
pub fn suggested_leaf_name(v: &VirtualForeignKey) -> String {
    format!("{}|{}.{}.{}", v.column, v.referenced_schema, v.referenced_table, v.referenced_column)
}

pub fn format_estimate(n: u64) -> String {
    fn scaled(n: u64, unit: f64, suffix: &str) -> String {
        let v = n as f64 / unit;
        if v < 10.0 {
            let s = format!("{v:.1}");
            format!("{}{suffix}", s.trim_end_matches(".0"))
        } else {
            format!("{}{suffix}", v.round() as u64)
        }
    }
    match n {
        0..=999 => n.to_string(),
        1_000..=999_499 => scaled(n, 1e3, "k"),
        999_500..=999_499_999 => scaled(n, 1e6, "M"),
        _ => scaled(n, 1e9, "B"),
    }
}

/// Nodes expanded when a connection first connects: the connection, its
/// default schema and that schema's tables.
pub fn default_expansion(conn_id: &str, catalog: &Catalog) -> Vec<NodeKey> {
    let mut keys = vec![NodeKey::Connection(conn_id.to_string())];
    let default = catalog
        .default_schema
        .clone()
        .or_else(|| (catalog.schemas.len() == 1).then(|| catalog.schemas[0].name.clone()));
    if let Some(s) = default {
        keys.push(NodeKey::Schema(conn_id.to_string(), s.clone()));
        keys.push(NodeKey::Group(conn_id.to_string(), s, GroupKind::Tables));
    }
    keys
}

fn table_icon(kind: TableKind) -> RowIcon {
    match kind {
        TableKind::Table => RowIcon::Table,
        TableKind::View => RowIcon::View,
        TableKind::MaterializedView => RowIcon::MaterializedView,
        TableKind::ForeignTable => RowIcon::ForeignTable,
    }
}

fn group_of(kind: TableKind) -> GroupKind {
    match kind {
        TableKind::Table | TableKind::ForeignTable => GroupKind::Tables,
        TableKind::View | TableKind::MaterializedView => GroupKind::Views,
    }
}

struct Filter<'a> {
    query: String,
    /// Inferred (undeclared) foreign keys of the connection being flattened.
    inferred: &'a [VirtualForeignKey],
}

impl Filter<'_> {
    fn active(&self) -> bool {
        !self.query.trim().is_empty()
    }
    fn matches(&self, text: &str) -> Option<Vec<usize>> {
        fuzzy_match(&self.query, text).map(|m| m.positions)
    }
}

/// Flatten everything visible. With a non-empty `filter`, only branches that
/// contain matches are kept and they are expanded automatically.
pub fn flatten(conns: &[ConnView], expanded: &HashSet<NodeKey>, filter: &str) -> Vec<Row> {
    flatten_wrapped(conns, expanded, filter, DEFAULT_WRAP_COLS)
}

/// Characters per detail line when the explorer width is not known.
pub const DEFAULT_WRAP_COLS: usize = 36;
/// Error detail blocks stop here; the rest is behind the tooltip / "Copy Error".
const MAX_DETAIL_LINES: usize = 8;

/// [`flatten`] with long status messages wrapped to `wrap_cols` characters per line.
pub fn flatten_wrapped(
    conns: &[ConnView],
    expanded: &HashSet<NodeKey>,
    filter: &str,
    wrap_cols: usize,
) -> Vec<Row> {
    let filter = Filter { query: filter.to_string(), inferred: &[] };
    let mut rows = Vec::new();
    for c in conns {
        flatten_connection(c, expanded, &filter, wrap_cols, &mut rows);
    }
    rows
}

/// Greedy word wrap to `cols` characters; words longer than a line are split. Blank lines in
/// the input are dropped, the output never has more than `max_lines` lines (the last one ends
/// in an ellipsis when text was cut).
pub fn wrap_text(text: &str, cols: usize, max_lines: usize) -> Vec<String> {
    let cols = cols.max(8);
    let mut lines: Vec<String> = Vec::new();
    for para in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let mut word: Vec<char> = word.chars().collect();
            while word.len() > cols {
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                }
                lines.push(word.drain(..cols).collect());
            }
            let w: String = word.into_iter().collect();
            if w.is_empty() {
                continue;
            }
            if cur.is_empty() {
                cur = w;
            } else if cur.chars().count() + 1 + w.chars().count() <= cols {
                cur.push(' ');
                cur.push_str(&w);
            } else {
                lines.push(std::mem::replace(&mut cur, w));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            let keep = cols.saturating_sub(1);
            *last = format!("{}…", last.chars().take(keep).collect::<String>().trim_end());
        }
    }
    lines
}

/// An error under a connection: the first line on the status row, the whole text wrapped in
/// detail rows below it when it does not fit on that one line.
fn push_error(conn: &str, message: &str, wrap_cols: usize, children: &mut Vec<Row>) {
    let first = message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(message).to_string();
    let mut r = Row::new(NodeKey::Status(conn.to_string()), 1, RowIcon::Error, first.clone());
    r.is_error = true;
    r.retry = true;
    r.tooltip = Some(message.to_string());
    children.push(r);
    let multi_line = message.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    if multi_line || first.chars().count() > wrap_cols {
        for (i, line) in wrap_text(message, wrap_cols, MAX_DETAIL_LINES).into_iter().enumerate() {
            let mut d = Row::new(NodeKey::Detail(conn.to_string(), i), 1, RowIcon::None, line);
            d.is_error = true;
            children.push(d);
        }
    }
}

fn flatten_connection(
    c: &ConnView,
    expanded: &HashSet<NodeKey>,
    filter: &Filter,
    wrap_cols: usize,
    rows: &mut Vec<Row>,
) {
    let local = Filter { query: filter.query.clone(), inferred: c.inferred };
    let filter = &local;
    let key = NodeKey::Connection(c.id.to_string());
    let name_match = filter.active().then(|| filter.matches(c.name)).flatten();
    let mut children: Vec<Row> = Vec::new();
    let mut child_matches = false;

    match &c.phase {
        Phase::Disconnected => {}
        Phase::Connecting => {
            let mut r = Row::new(NodeKey::Status(c.id.to_string()), 1, RowIcon::Spinner, "Connecting…");
            r.type_text = None;
            children.push(r);
        }
        Phase::Failed(message) => push_error(c.id, message, wrap_cols, &mut children),
        Phase::Connected { catalog: None, loading, catalog_error } => {
            if let Some(e) = catalog_error {
                push_error(c.id, e, wrap_cols, &mut children);
            } else if *loading {
                children.push(Row::new(
                    NodeKey::Status(c.id.to_string()),
                    1,
                    RowIcon::Spinner,
                    "Loading schema…",
                ));
            }
        }
        Phase::Connected { catalog: Some(catalog), .. } => {
            let force = filter.active() && name_match.is_none();
            let single = catalog.schemas.len() == 1;
            for schema in &catalog.schemas {
                let before = children.len();
                flatten_schema(c.id, schema, expanded, filter, name_match.is_some(), single, &mut children);
                if children.len() > before {
                    child_matches = true;
                }
            }
            if children.is_empty() && !filter.active() {
                children.push(Row::new(NodeKey::Status(c.id.to_string()), 1, RowIcon::Info, "No schemas"));
            }
            let _ = force;
        }
    }

    if filter.active() && name_match.is_none() && !child_matches {
        return;
    }

    let is_expanded = filter.active() && child_matches || expanded.contains(&key);
    let mut row = Row::new(key, 0, RowIcon::Connection(c.kind), c.name);
    row.expandable = true;
    row.expanded = is_expanded;
    row.highlights = name_match.unwrap_or_default();
    row.color = c.color.map(str::to_string);
    row.dot = Some(match c.phase {
        Phase::Disconnected => ConnDot::Disconnected,
        Phase::Connecting => ConnDot::Connecting,
        Phase::Failed(_) => ConnDot::Failed,
        Phase::Connected { .. } => ConnDot::Connected,
    });
    rows.push(row);
    if is_expanded {
        rows.extend(children);
    }
}

fn flatten_schema(
    conn: &str,
    schema: &Schema,
    expanded: &HashSet<NodeKey>,
    filter: &Filter,
    parent_matched: bool,
    single_schema: bool,
    out: &mut Vec<Row>,
) {
    let key = NodeKey::Schema(conn.to_string(), schema.name.clone());
    let name_match = filter.active().then(|| filter.matches(&schema.name)).flatten();
    // A matching connection or schema name shows its whole subtree.
    let show_all = !filter.active() || parent_matched || name_match.is_some();

    let mut groups: Vec<Row> = Vec::new();
    let mut any = false;
    for kind in GroupKind::ALL {
        let mut items: Vec<Row> = Vec::new();
        let gkey = NodeKey::Group(conn.to_string(), schema.name.clone(), kind);
        let group_open = filter.active() || expanded.contains(&gkey);
        let mut total = 0usize;
        match kind {
            GroupKind::Tables | GroupKind::Views => {
                for t in schema.tables.iter().filter(|t| group_of(t.kind) == kind) {
                    let m = if show_all { Some(Vec::new()) } else { filter.matches(&t.name) };
                    let Some(m) = m else { continue };
                    total += 1;
                    if group_open {
                        flatten_table(conn, &schema.name, t, m, expanded, filter.inferred, &mut items);
                    }
                }
            }
            GroupKind::Routines => {
                for r in &schema.routines {
                    let m = if show_all { Some(Vec::new()) } else { filter.matches(&r.name) };
                    let Some(m) = m else { continue };
                    total += 1;
                    if group_open {
                        items.push(routine_row(conn, &schema.name, r, m));
                    }
                }
            }
            GroupKind::Sequences => {
                for s in &schema.sequences {
                    let m = if show_all { Some(Vec::new()) } else { filter.matches(s) };
                    let Some(m) = m else { continue };
                    total += 1;
                    if group_open {
                        let mut row = Row::new(
                            NodeKey::Sequence(conn.to_string(), schema.name.clone(), s.clone()),
                            3,
                            RowIcon::Sequence,
                            s.clone(),
                        );
                        row.highlights = m;
                        items.push(row);
                    }
                }
            }
        }
        if total == 0 {
            continue;
        }
        any = true;
        let mut g = Row::new(gkey, 2, RowIcon::Group(kind), kind.label());
        g.expandable = true;
        g.expanded = group_open;
        g.detail = Some(total.to_string());
        groups.push(g);
        groups.extend(items);
    }

    if filter.active() && !any && name_match.is_none() {
        return;
    }
    // Schemas of a single-schema database (SQLite `main`) still get a row so the
    // hierarchy is the same for every engine.
    let _ = single_schema;
    let open = (filter.active() && any) || expanded.contains(&key);
    let mut row = Row::new(key, 1, RowIcon::Schema, schema.name.clone());
    row.expandable = true;
    row.expanded = open;
    row.highlights = name_match.unwrap_or_default();
    out.push(row);
    if open {
        out.extend(groups);
    }
}

fn routine_row(conn: &str, schema: &str, r: &Routine, highlights: Vec<usize>) -> Row {
    let mut row = Row::new(
        NodeKey::Routine(conn.to_string(), schema.to_string(), format!("{}{}", r.name, r.signature)),
        3,
        RowIcon::Routine,
        r.name.clone(),
    );
    row.type_text = Some(r.signature.clone());
    row.detail = r.return_type.clone();
    row.highlights = highlights;
    row
}

fn flatten_table(
    conn: &str,
    schema: &str,
    t: &Table,
    highlights: Vec<usize>,
    expanded: &HashSet<NodeKey>,
    inferred: &[VirtualForeignKey],
    out: &mut Vec<Row>,
) {
    let key = NodeKey::Table(conn.to_string(), schema.to_string(), t.name.clone());
    let open = expanded.contains(&key);
    let mut row = Row::new(key, 3, table_icon(t.kind), t.name.clone());
    row.expandable = true;
    row.expanded = open;
    row.highlights = highlights;
    row.detail = t.row_estimate.map(format_estimate);
    out.push(row);
    if !open {
        return;
    }

    let fk_cols: HashSet<&str> =
        t.foreign_keys.iter().flat_map(|f| f.columns.iter().map(String::as_str)).collect();
    for c in &t.columns {
        let is_pk = t.primary_key.iter().any(|p| p == &c.name);
        let icon = if is_pk {
            RowIcon::PrimaryKeyColumn
        } else if fk_cols.contains(c.name.as_str()) {
            RowIcon::ForeignKeyColumn
        } else {
            RowIcon::Column
        };
        let mut r = Row::new(
            NodeKey::Column(conn.to_string(), schema.to_string(), t.name.clone(), c.name.clone()),
            4,
            icon,
            c.name.clone(),
        );
        r.type_text = Some(c.data_type.clone());
        r.nullable = Some(c.nullable);
        out.push(r);
    }

    let section_key = |s: Section| NodeKey::Section(conn.to_string(), schema.to_string(), t.name.clone(), s);
    let leaf_key = |s: Section, n: &str| {
        NodeKey::Leaf(conn.to_string(), schema.to_string(), t.name.clone(), s, n.to_string())
    };

    // Keys: the primary key as one row.
    if !t.primary_key.is_empty() {
        let k = section_key(Section::Keys);
        let open = expanded.contains(&k);
        let mut s = Row::new(k, 4, RowIcon::Key, Section::Keys.label());
        s.expandable = true;
        s.expanded = open;
        s.detail = Some("1".into());
        out.push(s);
        if open {
            let mut r = Row::new(leaf_key(Section::Keys, "PRIMARY"), 5, RowIcon::Key, "PRIMARY KEY");
            r.type_text = Some(format!("({})", t.primary_key.join(", ")));
            out.push(r);
        }
    }
    let suggested: Vec<&VirtualForeignKey> =
        inferred.iter().filter(|v| v.schema == schema && v.table == t.name).collect();
    if !t.foreign_keys.is_empty() || !suggested.is_empty() {
        let k = section_key(Section::ForeignKeys);
        let open = expanded.contains(&k);
        let mut s = Row::new(k, 4, RowIcon::ForeignKey, Section::ForeignKeys.label());
        s.expandable = true;
        s.expanded = open;
        s.detail = Some((t.foreign_keys.len() + suggested.len()).to_string());
        out.push(s);
        if open {
            for fk in &t.foreign_keys {
                let target = match &fk.referenced_schema {
                    Some(sch) if sch != schema => format!("{sch}.{}", fk.referenced_table),
                    _ => fk.referenced_table.clone(),
                };
                let mut r = Row::new(
                    leaf_key(Section::ForeignKeys, &fk.name),
                    5,
                    RowIcon::ForeignKey,
                    fk.columns.join(", "),
                );
                r.type_text = Some(format!("→ {target}.{}", fk.referenced_columns.join(", ")));
                out.push(r);
            }
            if !suggested.is_empty() {
                let sk = section_key(Section::Suggested);
                let sopen = expanded.contains(&sk);
                let mut sr = Row::new(sk, 5, RowIcon::SuggestedForeignKey, Section::Suggested.label());
                sr.expandable = true;
                sr.expanded = sopen;
                sr.detail = Some(suggested.len().to_string());
                sr.tooltip = Some(
                    "Relationships implied by column names and types, not declared as constraints".into(),
                );
                out.push(sr);
                if sopen {
                    for v in &suggested {
                        let mut r = Row::new(
                            leaf_key(Section::Suggested, &suggested_leaf_name(v)),
                            6,
                            RowIcon::SuggestedForeignKey,
                            v.column.clone(),
                        );
                        let target = if v.referenced_schema == schema {
                            v.referenced_table.clone()
                        } else {
                            format!("{}.{}", v.referenced_schema, v.referenced_table)
                        };
                        // Two parts so a narrow explorer cuts the target (middle), never the confidence.
                        r.type_text = Some(format!("→ {target}.{}", v.referenced_column));
                        r.detail = Some(format!("{:.0}%", v.confidence * 100.0));
                        r.tooltip = Some(v.reason.clone());
                        r.muted = true;
                        out.push(r);
                    }
                }
            }
        }
    }
    if !t.indexes.is_empty() {
        let k = section_key(Section::Indexes);
        let open = expanded.contains(&k);
        let mut s = Row::new(k, 4, RowIcon::Index, Section::Indexes.label());
        s.expandable = true;
        s.expanded = open;
        s.detail = Some(t.indexes.len().to_string());
        out.push(s);
        if open {
            for i in &t.indexes {
                let mut r = Row::new(leaf_key(Section::Indexes, &i.name), 5, RowIcon::Index, i.name.clone());
                let mut info = format!("({})", i.columns.join(", "));
                if i.unique && !i.primary {
                    info.push_str(" unique");
                }
                r.type_text = Some(info);
                out.push(r);
            }
        }
    }
}

// ---- navigation helpers on the flat list ----

/// Index of the nearest row above `ix` with a smaller depth.
pub fn parent_index(rows: &[Row], ix: usize) -> Option<usize> {
    let depth = rows.get(ix)?.depth;
    (0..ix).rev().find(|&i| rows[i].depth < depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::{ColumnInfo, ForeignKey, Index, TypeKind};

    pub(crate) fn column(name: &str, ty: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            kind: TypeKind::Other,
            nullable,
            default: None,
            comment: None,
            geometry: None,
        }
    }

    pub(crate) fn table(name: &str, kind: TableKind, rows: Option<u64>) -> Table {
        Table {
            name: name.into(),
            kind,
            columns: vec![
                column("id", "integer", false),
                column("customer_id", "integer", true),
                column("name", "text", true),
            ],
            primary_key: vec!["id".into()],
            foreign_keys: vec![ForeignKey {
                name: "fk_customer".into(),
                columns: vec!["customer_id".into()],
                referenced_schema: Some("public".into()),
                referenced_table: "customers".into(),
                referenced_columns: vec!["id".into()],
                on_delete: None,
                on_update: None,
            }],
            indexes: vec![Index {
                name: format!("{name}_pkey"),
                columns: vec!["id".into()],
                unique: true,
                primary: true,
                method: Some("btree".into()),
                definition: None,
            }],
            row_estimate: rows,
            comment: None,
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            schemas: vec![
                Schema {
                    name: "public".into(),
                    tables: vec![
                        table("customers", TableKind::Table, Some(1234)),
                        table("orders", TableKind::Table, Some(5_600_000)),
                        table("order_items", TableKind::Table, Some(42)),
                        table("active_orders", TableKind::View, None),
                    ],
                    routines: vec![Routine {
                        name: "total".into(),
                        kind: quarry_core::RoutineKind::Function,
                        signature: "(int)".into(),
                        return_type: Some("int".into()),
                    }],
                    sequences: vec!["orders_id_seq".into()],
                },
                Schema {
                    name: "audit".into(),
                    tables: vec![table("log", TableKind::Table, None)],
                    routines: vec![],
                    sequences: vec![],
                },
            ],
            default_schema: Some("public".into()),
        }
    }

    fn view<'a>(c: &'a Catalog) -> Vec<ConnView<'a>> {
        vec![ConnView {
            id: "c1",
            name: "Demo",
            kind: DatabaseKind::Postgres,
            color: None,
            phase: Phase::Connected { catalog: Some(c), loading: false, catalog_error: None },
            inferred: &[],
        }]
    }

    #[test]
    fn suggested_foreign_keys_nest_under_foreign_keys() {
        let c = catalog();
        let inferred = vec![VirtualForeignKey {
            schema: "public".into(),
            table: "orders".into(),
            column: "region_id".into(),
            referenced_schema: "public".into(),
            referenced_table: "customers".into(),
            referenced_column: "id".into(),
            confidence: 0.8,
            reason: "region_id looks like a reference to customers.id".into(),
        }];
        let mut views = view(&c);
        views[0].inferred = &inferred;
        let mut exp = set(default_expansion("c1", &c));
        exp.insert(NodeKey::Table("c1".into(), "public".into(), "orders".into()));
        let sec = |s| NodeKey::Section("c1".into(), "public".into(), "orders".into(), s);
        exp.insert(sec(Section::ForeignKeys));
        exp.insert(sec(Section::Suggested));
        let rows = flatten(&views, &exp, "");
        let group = rows.iter().find(|r| r.label == "Suggested").expect("suggested group");
        assert_eq!(group.detail.as_deref(), Some("1"));
        assert_eq!(group.icon, RowIcon::SuggestedForeignKey);
        let leaf = rows.iter().find(|r| r.label == "region_id").expect("suggested leaf");
        assert!(leaf.muted && leaf.detail.as_deref() == Some("80%"));
        assert_eq!(leaf.type_text.as_deref(), Some("→ customers.id"));
        assert!(leaf.tooltip.as_deref().unwrap().contains("customers.id"));
        let fk = rows.iter().find(|r| r.label == "Foreign Keys").unwrap();
        assert_eq!(fk.detail.as_deref(), Some("2"), "one declared plus one suggested");
    }

    #[test]
    fn wrapping_splits_on_words_and_hard_splits_long_tokens() {
        assert_eq!(wrap_text("connection refused (os error 111)", 16, 8), vec!["connection", "refused (os", "error 111)"]);
        assert_eq!(wrap_text("aaaaaaaaaaaaaaaaaaaa", 8, 8), vec!["aaaaaaaa", "aaaaaaaa", "aaaa"]);
        assert_eq!(wrap_text("a\n\n b", 8, 8), vec!["a", "b"]);
        let cut = wrap_text(&"word ".repeat(40), 10, 3);
        assert_eq!(cut.len(), 3);
        assert!(cut[2].ends_with('…'));
    }

    #[test]
    fn long_connection_errors_get_an_inline_detail_block() {
        let c = catalog();
        let long = "could not connect: error connecting to server: Connection refused (os error 111) at 127.0.0.1:5999";
        let r = flatten_wrapped(&[ConnView { phase: Phase::Failed(long.into()), ..view(&c).remove(0) }], &set(vec![NodeKey::Connection("c1".into())]), "", 30);
        assert!(r[1].is_error && r[1].retry && r[1].icon == RowIcon::Error);
        let details: Vec<_> = r.iter().filter(|x| matches!(x.key, NodeKey::Detail(..))).collect();
        assert!(details.len() >= 3);
        assert!(details.iter().all(|d| d.label.chars().count() <= 30 && d.icon == RowIcon::None));
        let joined = details.iter().map(|d| d.label.as_str()).collect::<Vec<_>>().join(" ");
        assert_eq!(joined, long);
        // A short one-liner needs no block.
        let r = flatten_wrapped(&[ConnView { phase: Phase::Failed("refused".into()), ..view(&c).remove(0) }], &set(vec![NodeKey::Connection("c1".into())]), "", 30);
        assert_eq!(r.len(), 2);
    }

    fn set(keys: Vec<NodeKey>) -> HashSet<NodeKey> {
        keys.into_iter().collect()
    }

    #[test]
    fn estimates_are_compact() {
        assert_eq!(format_estimate(0), "0");
        assert_eq!(format_estimate(999), "999");
        assert_eq!(format_estimate(1000), "1k");
        assert_eq!(format_estimate(1234), "1.2k");
        assert_eq!(format_estimate(12_345), "12k");
        assert_eq!(format_estimate(999_499), "999k");
        assert_eq!(format_estimate(999_500), "1M");
        assert_eq!(format_estimate(5_600_000), "5.6M");
        assert_eq!(format_estimate(2_000_000_000), "2B");
    }

    #[test]
    fn collapsed_connection_shows_one_row() {
        let c = catalog();
        let rows = flatten(&view(&c), &HashSet::new(), "");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].expandable && !rows[0].expanded);
        assert_eq!(rows[0].dot, Some(ConnDot::Connected));
    }

    #[test]
    fn default_expansion_opens_default_schema_tables() {
        let c = catalog();
        let exp = set(default_expansion("c1", &c));
        let rows = flatten(&view(&c), &exp, "");
        let labels: Vec<_> = rows.iter().map(|r| (r.depth, r.label.as_str())).collect();
        assert_eq!(
            labels,
            vec![
                (0, "Demo"),
                (1, "public"),
                (2, "Tables"),
                (3, "customers"),
                (3, "orders"),
                (3, "order_items"),
                (2, "Views"),
                (2, "Routines"),
                (2, "Sequences"),
                (1, "audit"),
            ]
        );
        assert_eq!(rows[2].detail.as_deref(), Some("3"));
        assert_eq!(rows[3].detail.as_deref(), Some("1.2k"));
    }

    #[test]
    fn table_expands_to_columns_and_sections() {
        let c = catalog();
        let mut exp = set(default_expansion("c1", &c));
        let t = NodeKey::Table("c1".into(), "public".into(), "orders".into());
        exp.insert(t);
        exp.insert(NodeKey::Section("c1".into(), "public".into(), "orders".into(), Section::ForeignKeys));
        let rows = flatten(&view(&c), &exp, "");
        let start = rows.iter().position(|r| r.label == "orders").unwrap();
        let slice: Vec<_> = rows[start + 1..start + 8].iter().map(|r| (r.icon, r.label.as_str())).collect();
        assert_eq!(slice[0], (RowIcon::PrimaryKeyColumn, "id"));
        assert_eq!(slice[1], (RowIcon::ForeignKeyColumn, "customer_id"));
        assert_eq!(slice[2], (RowIcon::Column, "name"));
        assert_eq!(slice[3], (RowIcon::Key, "Keys"));
        assert_eq!(slice[4], (RowIcon::ForeignKey, "Foreign Keys"));
        let fk = &rows[start + 6];
        assert_eq!(fk.label, "customer_id");
        assert_eq!(fk.type_text.as_deref(), Some("→ customers.id"));
        assert_eq!(rows[start + 1].type_text.as_deref(), Some("integer"));
        assert_eq!(rows[start + 1].nullable, Some(false));
    }

    #[test]
    fn filter_keeps_matches_and_auto_expands() {
        let c = catalog();
        let rows = flatten(&view(&c), &HashSet::new(), "ordit");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["Demo", "public", "Tables", "order_items"]);
        assert!(!rows[3].highlights.is_empty());
        assert_eq!(rows[2].detail.as_deref(), Some("1"));
    }

    #[test]
    fn filter_with_no_matches_yields_nothing() {
        let c = catalog();
        assert!(flatten(&view(&c), &HashSet::new(), "zzzz").is_empty());
    }

    #[test]
    fn matching_connection_name_shows_its_whole_subtree() {
        let c = catalog();
        let rows = flatten(&view(&c), &HashSet::new(), "Demo");
        assert_eq!(rows[0].label, "Demo");
        assert!(rows[0].expanded && !rows[0].highlights.is_empty());
        assert!(rows.iter().any(|r| r.label == "orders"), "children stay visible: {}", rows.len());
    }

    #[test]
    fn views_and_routines_are_grouped() {
        let c = catalog();
        let rows = flatten(&view(&c), &HashSet::new(), "total");
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["Demo", "public", "Routines", "total"]
        );
    }

    #[test]
    fn status_rows_for_connection_phases() {
        let key = |p: Phase<'static>| {
            let v = vec![ConnView {
                id: "c",
                name: "X",
                kind: DatabaseKind::Sqlite,
                color: None,
                phase: p,
                inferred: &[],
            }];
            flatten(&v, &set(vec![NodeKey::Connection("c".into())]), "")
        };
        assert_eq!(key(Phase::Disconnected).len(), 1);
        let r = key(Phase::Connecting);
        assert_eq!((r[1].icon, r[1].label.as_str()), (RowIcon::Spinner, "Connecting…"));
        let r = key(Phase::Failed("password authentication failed".into()));
        assert!(r[1].is_error && r[1].retry);
        let r = key(Phase::Connected { catalog: None, loading: true, catalog_error: None });
        assert_eq!(r[1].label, "Loading schema…");
    }

    #[test]
    fn parent_lookup() {
        let c = catalog();
        let exp = set(default_expansion("c1", &c));
        let rows = flatten(&view(&c), &exp, "");
        let i = rows.iter().position(|r| r.label == "orders").unwrap();
        assert_eq!(rows[parent_index(&rows, i).unwrap()].label, "Tables");
        assert_eq!(parent_index(&rows, 0), None);
    }

    #[test]
    fn five_thousand_tables_flatten_quickly_and_filter() {
        let tables: Vec<Table> = (0..5000)
            .map(|i| table(&format!("table_{i:05}"), TableKind::Table, Some(i as u64 * 7)))
            .collect();
        let catalog = Catalog {
            schemas: vec![Schema { name: "public".into(), tables, routines: vec![], sequences: vec![] }],
            default_schema: Some("public".into()),
        };
        let exp = set(default_expansion("c1", &catalog));
        let started = std::time::Instant::now();
        let rows = flatten(&view(&catalog), &exp, "");
        assert_eq!(rows.len(), 3 + 5000);
        let filtered = flatten(&view(&catalog), &exp, "tbl4999");
        assert!(filtered.iter().any(|r| r.label == "table_04999"), "{:?}", filtered.len());
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }
}
