//! The graph an ER diagram draws, built from a [`Catalog`].
//!
//! The graph is independent of how it is drawn: nodes are tables, edges are foreign keys, and everything the
//! layout and the two exporters need to know about a column (is it a key, does it allow NULL) is copied in
//! here, so they never see the catalog.

use theta_core::{Catalog, ColumnInfo, ForeignKey, Table, TableKind};

/// What to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every table of one schema.
    Schema(String),
    /// One table and every table it references or that references it.
    Table { schema: String, table: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphColumn {
    pub name: String,
    pub data_type: String,
    pub pk: bool,
    /// Part of some foreign key of this table.
    pub fk: bool,
    /// An edge ends at this column (some foreign key references it). Such a column is never hidden when a
    /// long column list is cut short, because the edge needs a row to attach to.
    pub referenced: bool,
    pub nullable: bool,
}

impl GraphColumn {
    /// Columns that must stay visible however long the table is.
    pub fn pinned(&self) -> bool {
        self.pk || self.fk || self.referenced
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    pub schema: String,
    pub name: String,
    pub columns: Vec<GraphColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub name: String,
    /// The table that owns the foreign key (the "many" side).
    pub from: usize,
    /// The referenced table (the "one" side).
    pub to: usize,
    /// Columns of `from`, positionally paired with `to_cols`. A composite key has several.
    pub from_cols: Vec<String>,
    pub to_cols: Vec<String>,
    /// Some referencing column allows NULL, so a row need not have a parent.
    pub optional: bool,
    /// The referencing columns are themselves unique, so each parent has at most one child (one-to-one).
    pub unique: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// Sorted by schema then name (case-insensitively): the picture depends on the tables, not on the order
    /// the database happened to list them in.
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// More than one schema appears, so labels need the schema.
    pub multi_schema: bool,
    /// Things the diagram leaves out, in words, for the status line.
    pub notes: Vec<String>,
}

impl Graph {
    pub fn label(&self, node: usize) -> String {
        let n = &self.nodes[node];
        if self.multi_schema { format!("{}.{}", n.schema, n.name) } else { n.name.clone() }
    }

    pub fn node_index(&self, schema: &str, name: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.schema.eq_ignore_ascii_case(schema) && n.name.eq_ignore_ascii_case(name))
    }

    /// Tables joined to `node` by at least one edge (either direction), without `node` itself.
    pub fn neighbours(&self, node: usize) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .edges
            .iter()
            .filter_map(|e| {
                if e.from == node && e.to != node {
                    Some(e.to)
                } else if e.to == node && e.from != node {
                    Some(e.from)
                } else {
                    None
                }
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn edges_of(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.edges.iter().enumerate().filter(move |(_, e)| e.from == node || e.to == node).map(|(i, _)| i)
    }
}

fn is_diagram_table(t: &Table) -> bool {
    matches!(t.kind, TableKind::Table | TableKind::ForeignTable)
}

/// Resolve a foreign key's target to `(schema index, table index)` in the catalog.
fn resolve(cat: &Catalog, owner_schema: &str, fk: &ForeignKey) -> Option<(usize, usize)> {
    let want = fk.referenced_schema.as_deref().unwrap_or(owner_schema);
    let si = cat.schemas.iter().position(|s| s.name.eq_ignore_ascii_case(want))?;
    let ti = cat.schemas[si].tables.iter().position(|t| t.name.eq_ignore_ascii_case(&fk.referenced_table))?;
    Some((si, ti))
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| y.eq_ignore_ascii_case(x)))
}

fn col_is(cols: &[String], name: &str) -> bool {
    cols.iter().any(|c| c.eq_ignore_ascii_case(name))
}

/// Build the graph for `scope`. A scope that names nothing in the catalog gives an empty graph.
pub fn from_catalog(cat: &Catalog, scope: &Scope) -> Graph {
    // (schema index, table index) of every table that gets a box.
    let mut members: Vec<(usize, usize)> = Vec::new();
    match scope {
        Scope::Schema(name) => {
            if let Some(si) = cat.schemas.iter().position(|s| s.name.eq_ignore_ascii_case(name)) {
                for (ti, t) in cat.schemas[si].tables.iter().enumerate() {
                    if is_diagram_table(t) {
                        members.push((si, ti));
                    }
                }
            }
        }
        Scope::Table { schema, table } => {
            let found = cat.schemas.iter().position(|s| s.name.eq_ignore_ascii_case(schema)).and_then(|si| {
                cat.schemas[si]
                    .tables
                    .iter()
                    .position(|t| t.name.eq_ignore_ascii_case(table))
                    .map(|ti| (si, ti))
            });
            if let Some(centre) = found {
                members.push(centre);
                for (si, s) in cat.schemas.iter().enumerate() {
                    for (ti, t) in s.tables.iter().enumerate() {
                        if (si, ti) == centre || !is_diagram_table(t) {
                            continue;
                        }
                        let points_at_centre =
                            t.foreign_keys.iter().any(|fk| resolve(cat, &s.name, fk) == Some(centre));
                        let centre_points_here = cat.schemas[centre.0].tables[centre.1]
                            .foreign_keys
                            .iter()
                            .any(|fk| resolve(cat, &cat.schemas[centre.0].name, fk) == Some((si, ti)));
                        if points_at_centre || centre_points_here {
                            members.push((si, ti));
                        }
                    }
                }
            }
        }
    }
    members.sort_by_key(|&(si, ti)| {
        (cat.schemas[si].name.to_lowercase(), cat.schemas[si].tables[ti].name.to_lowercase(), si, ti)
    });
    members.dedup();

    let mut g = Graph::default();
    let mut outside = 0usize;
    // The columns each member's edges end at, so the node can be built once with the right `referenced` flags.
    let mut referenced: Vec<Vec<String>> = vec![Vec::new(); members.len()];
    let mut raw_edges: Vec<(usize, usize, &ForeignKey, &Table)> = Vec::new();
    for (from, &(si, ti)) in members.iter().enumerate() {
        let t = &cat.schemas[si].tables[ti];
        for fk in &t.foreign_keys {
            match resolve(cat, &cat.schemas[si].name, fk).and_then(|r| members.iter().position(|m| *m == r)) {
                Some(to) => {
                    referenced[to].extend(fk.referenced_columns.iter().cloned());
                    raw_edges.push((from, to, fk, t));
                }
                None => outside += 1,
            }
        }
    }
    for (i, &(si, ti)) in members.iter().enumerate() {
        let t = &cat.schemas[si].tables[ti];
        g.nodes.push(GraphNode {
            schema: cat.schemas[si].name.clone(),
            name: t.name.clone(),
            columns: t.columns.iter().map(|c| column(t, c, &referenced[i])).collect(),
        });
    }
    g.multi_schema = g.nodes.windows(2).any(|w| !w[0].schema.eq_ignore_ascii_case(&w[1].schema));
    for (from, to, fk, owner) in raw_edges {
        let optional = fk.columns.iter().any(|c| {
            owner.columns.iter().find(|x| x.name.eq_ignore_ascii_case(c)).is_none_or(|x| x.nullable)
        });
        let unique = same_set(&owner.primary_key, &fk.columns)
            || owner.unique_constraints.iter().any(|u| same_set(&u.columns, &fk.columns))
            || owner.indexes.iter().any(|i| {
                i.unique && !i.has_expressions && i.predicate.is_none() && same_set(&i.columns, &fk.columns)
            });
        g.edges.push(GraphEdge {
            name: fk.name.clone(),
            from,
            to,
            from_cols: fk.columns.clone(),
            to_cols: fk.referenced_columns.clone(),
            optional,
            unique,
        });
    }
    if outside > 0 {
        g.notes.push(format!(
            "{outside} foreign key{} to tables outside this diagram {} left out",
            if outside == 1 { "" } else { "s" },
            if outside == 1 { "is" } else { "are" }
        ));
    }
    if matches!(scope, Scope::Schema(_)) {
        g.notes.push("Views are not drawn: the catalogue has no foreign keys for them".to_string());
    }
    g
}

fn column(t: &Table, c: &ColumnInfo, referenced: &[String]) -> GraphColumn {
    GraphColumn {
        name: c.name.clone(),
        data_type: c.data_type.clone(),
        pk: col_is(&t.primary_key, &c.name),
        fk: t.foreign_keys.iter().any(|fk| col_is(&fk.columns, &c.name)),
        referenced: col_is(referenced, &c.name),
        nullable: c.nullable,
    }
}
