//! Foreign-key driven features: tables in scope, JOIN suggestions (declared and
//! inferred relationships) and "virtual" foreign-key inference.

use crate::keywords::{is_reserved, qualified_name, quote_if_needed};
use crate::scope::{Parsed, SourceKind, scan_scope};
use quarry_core::{Catalog, DatabaseKind, Schema, Table, TableKind, TypeKind};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub table: String,
    pub alias: Option<String>,
}

impl TableRef {
    /// How the rest of the statement refers to this table.
    pub fn reference_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.table)
    }
}

/// Tables (and CTEs) the statement's top-level query reads from or writes to.
pub fn tables_in_scope(statement: &str, kind: DatabaseKind) -> Vec<TableRef> {
    let p = Parsed::from_sql(statement, kind);
    let scan = scan_scope(&p, 0, p.len(), None, &[]);
    scan.sources
        .into_iter()
        .filter(|s| matches!(s.kind, SourceKind::Table | SourceKind::Cte))
        .map(|s| TableRef { schema: s.schema, table: s.name, alias: s.alias })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct JoinSuggestion {
    /// Table to join, with a generated unique alias.
    pub table: TableRef,
    /// e.g. `o.customer_id = c.id`
    pub condition: String,
    /// e.g. `JOIN orders o ON o.customer_id = c.id`
    pub clause: String,
    /// FK constraint name, or `inferred: orders.customer_id → customers.id`.
    pub via: String,
    pub inferred: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VirtualForeignKey {
    pub schema: String,
    pub table: String,
    pub column: String,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_column: String,
    pub confidence: f32,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// inference

fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.extend(c.to_lowercase());
    }
    out.replace("__", "_")
}

fn plural_variants(stem: &str) -> Vec<String> {
    let mut v = vec![stem.to_string(), format!("{stem}s"), format!("{stem}es")];
    if let Some(base) = stem.strip_suffix('y') {
        v.push(format!("{base}ies"));
    }
    if let Some(base) = stem.strip_suffix("ies") {
        v.push(format!("{base}y"));
    }
    if let Some(base) = stem.strip_suffix("es") {
        v.push(base.to_string());
    }
    if let Some(base) = stem.strip_suffix('s') {
        v.push(base.to_string());
    }
    v
}

const KEY_SUFFIXES: &[&str] = &["id", "uuid", "guid", "key", "code"];
const SELF_REF_STEMS: &[&str] = &["parent", "manager", "supervisor", "boss", "ancestor"];

fn type_family(k: TypeKind) -> &'static str {
    match k {
        TypeKind::Integer => "integer",
        TypeKind::Uuid => "uuid",
        TypeKind::Text => "text",
        TypeKind::Decimal => "numeric",
        _ => "other",
    }
}

fn unique_single(t: &Table, col: &str) -> Option<&'static str> {
    if t.primary_key.len() == 1 && t.primary_key[0].eq_ignore_ascii_case(col) {
        return Some("primary key");
    }
    if t.indexes.iter().any(|i| (i.unique || i.primary) && i.columns.len() == 1 && i.columns[0].eq_ignore_ascii_case(col)) {
        return Some("unique");
    }
    None
}

struct TableIndex<'a> {
    by_name: HashMap<String, Vec<(&'a Schema, &'a Table)>>,
}

impl<'a> TableIndex<'a> {
    fn new(catalog: &'a Catalog) -> Self {
        let mut by_name: HashMap<String, Vec<(&Schema, &Table)>> = HashMap::new();
        for s in &catalog.schemas {
            for t in s.tables.iter().filter(|t| t.kind == TableKind::Table) {
                by_name.entry(t.name.to_lowercase()).or_default().push((s, t));
            }
        }
        TableIndex { by_name }
    }
}

fn in_declared_fk(t: &Table, col: &str) -> bool {
    t.foreign_keys.iter().any(|f| f.columns.iter().any(|c| c.eq_ignore_ascii_case(col)))
}

/// Relationships implied by naming and types but not declared as constraints.
///
/// A column `customer_id` is matched to a table named `customer`/`customers` whose
/// `id` column is a primary key or unique and has the same type family. Columns
/// that already participate in a real FK are skipped, so nothing returned here
/// duplicates a declared constraint.
pub fn infer_foreign_keys(catalog: &Catalog) -> Vec<VirtualForeignKey> {
    let index = TableIndex::new(catalog);
    let mut out = Vec::new();
    for schema in &catalog.schemas {
        for table in schema.tables.iter().filter(|t| t.kind == TableKind::Table) {
            for col in &table.columns {
                if in_declared_fk(table, &col.name) {
                    continue;
                }
                if let Some(v) = infer_for_column(catalog, &index, schema, table, &col.name, col.kind) {
                    out.push(v);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (&a.schema, &a.table, &a.column).cmp(&(&b.schema, &b.table, &b.column)))
    });
    out
}

fn infer_for_column(
    catalog: &Catalog,
    index: &TableIndex,
    schema: &Schema,
    table: &Table,
    col_name: &str,
    col_kind: TypeKind,
) -> Option<VirtualForeignKey> {
    let snake = snake_case(col_name);
    let words: Vec<&str> = snake.split('_').filter(|w| !w.is_empty()).collect();
    if words.len() < 2 {
        return None;
    }
    let suffix = *words.last()?;
    if !KEY_SUFFIXES.contains(&suffix) {
        return None;
    }
    let stem_words = &words[..words.len() - 1];
    let mut best: Option<(f32, &Schema, &Table, String, String)> = None;

    // Self reference (`parent_id` in `categories`).
    if stem_words.len() == 1 && SELF_REF_STEMS.contains(&stem_words[0]) && suffix == "id" && table.primary_key.len() == 1 {
        let pk = &table.primary_key[0];
        let pk_kind = table.columns.iter().find(|c| c.name.eq_ignore_ascii_case(pk)).map(|c| c.kind);
        if pk_kind == Some(col_kind) {
            let reason = format!(
                "`{col_name}` looks like a self-reference: `{}` is the primary key of the same table ({})",
                pk,
                type_family(col_kind)
            );
            return Some(VirtualForeignKey {
                schema: schema.name.clone(),
                table: table.name.clone(),
                column: col_name.to_string(),
                referenced_schema: schema.name.clone(),
                referenced_table: table.name.clone(),
                referenced_column: pk.clone(),
                confidence: 0.7,
                reason,
            });
        }
    }

    for drop in 0..stem_words.len() {
        let stem = stem_words[drop..].join("_");
        for (vi, variant) in plural_variants(&stem).into_iter().enumerate() {
            let Some(cands) = index.by_name.get(&variant) else { continue };
            for (ts, tt) in cands {
                if std::ptr::eq(*tt, table) {
                    continue;
                }
                // Target column: same name as the suffix, else the single-column key.
                let named = tt.columns.iter().find(|c| c.name.eq_ignore_ascii_case(suffix));
                let (target, exact_name) = match named {
                    Some(c) => (c, true),
                    None => match tt.primary_key.as_slice() {
                        [pk] => match tt.columns.iter().find(|c| c.name.eq_ignore_ascii_case(pk)) {
                            Some(c) => (c, false),
                            None => continue,
                        },
                        _ => continue,
                    },
                };
                if target.kind != col_kind || type_family(col_kind) == "other" {
                    continue;
                }
                let Some(uniq) = unique_single(tt, &target.name) else { continue };
                let mut conf = 0.5f32;
                if vi <= 3 && drop == 0 {
                    conf += 0.2;
                }
                if exact_name {
                    conf += 0.1;
                }
                if std::ptr::eq(*ts, schema) {
                    conf += 0.1;
                } else if catalog.default_schema.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(&ts.name)) {
                    conf += 0.05;
                }
                if uniq == "primary key" {
                    conf += 0.05;
                }
                if drop > 0 {
                    conf -= 0.15;
                }
                if !matches!(suffix, "id" | "uuid" | "guid") {
                    conf -= 0.1;
                }
                let conf = conf.clamp(0.3, 0.98);
                if best.as_ref().is_none_or(|b| conf > b.0) {
                    let reason = format!(
                        "`{col_name}` follows the `{stem}_{suffix}` naming pattern and matches {}.{}; both are {}, and the target is a {uniq}",
                        tt.name,
                        target.name,
                        type_family(col_kind)
                    );
                    best = Some((conf, ts, tt, target.name.clone(), reason));
                }
            }
        }
        if best.is_some() {
            break; // a longer stem match beats shorter role-prefix guesses
        }
    }
    best.map(|(confidence, ts, tt, rc, reason)| VirtualForeignKey {
        schema: schema.name.clone(),
        table: table.name.clone(),
        column: col_name.to_string(),
        referenced_schema: ts.name.clone(),
        referenced_table: tt.name.clone(),
        referenced_column: rc,
        confidence,
        reason,
    })
}

// ---------------------------------------------------------------------------
// join suggestions

fn initials(name: &str) -> String {
    let s: String = name.split(['_', ' ', '-']).filter_map(|w| w.chars().next()).collect::<String>().to_lowercase();
    if s.is_empty() { "t".into() } else { s }
}

/// A short alias for `table` that collides with nothing in `used` (lower-cased).
fn unique_alias(table: &str, used: &HashSet<String>, kind: DatabaseKind) -> String {
    let base = initials(table);
    let ok = |a: &str| !used.contains(a) && !is_reserved(a, kind);
    if ok(&base) {
        return base;
    }
    // Longer prefix of the first word, then numeric suffixes.
    let first: String = table.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    for n in 2..=first.len().min(4) {
        let a: String = first.chars().take(n).collect();
        if ok(&a) {
            return a;
        }
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|a| ok(a))
        .unwrap_or(base)
}

struct Edge<'a> {
    /// Referencing (child) side.
    child: (&'a Schema, &'a Table),
    child_cols: Vec<String>,
    parent: (&'a Schema, &'a Table),
    parent_cols: Vec<String>,
    via: String,
    inferred: bool,
}

fn resolve_ref<'a>(catalog: &'a Catalog, from: &'a Schema, fk_schema: Option<&str>, name: &str) -> Option<(&'a Schema, &'a Table)> {
    match fk_schema {
        Some(s) => catalog.find_table(Some(s), name),
        None => catalog.find_table(Some(&from.name), name).or_else(|| catalog.find_table(None, name)),
    }
}

/// Suggest JOINs for the tables already in scope, following declared foreign keys
/// in both directions and then inferred ones (marked `inferred`).
pub fn suggest_joins(catalog: &Catalog, in_scope: &[TableRef], kind: DatabaseKind) -> Vec<JoinSuggestion> {
    let resolved: Vec<(&TableRef, &Schema, &Table)> = in_scope
        .iter()
        .filter_map(|r| catalog.find_table(r.schema.as_deref(), &r.table).map(|(s, t)| (r, s, t)))
        .collect();
    if resolved.is_empty() {
        return Vec::new();
    }
    let same = |a: (&Schema, &Table), b: (&Schema, &Table)| std::ptr::eq(a.1, b.1);
    let in_scope_tables: Vec<(&Schema, &Table)> = resolved.iter().map(|(_, s, t)| (*s, *t)).collect();

    // Collect candidate edges touching a scope table.
    let mut edges: Vec<(usize, bool, Edge)> = Vec::new(); // (scope index, scope-is-child, edge)
    for (si, (_, s_schema, s_table)) in resolved.iter().enumerate() {
        for fk in &s_table.foreign_keys {
            if let Some(parent) = resolve_ref(catalog, s_schema, fk.referenced_schema.as_deref(), &fk.referenced_table) {
                edges.push((
                    si,
                    true,
                    Edge {
                        child: (s_schema, s_table),
                        child_cols: fk.columns.clone(),
                        parent,
                        parent_cols: fk.referenced_columns.clone(),
                        via: fk.name.clone(),
                        inferred: false,
                    },
                ));
            }
        }
        for other_schema in &catalog.schemas {
            for other in &other_schema.tables {
                for fk in &other.foreign_keys {
                    if let Some(parent) = resolve_ref(catalog, other_schema, fk.referenced_schema.as_deref(), &fk.referenced_table)
                        && std::ptr::eq(parent.1, *s_table)
                    {
                        edges.push((
                            si,
                            false,
                            Edge {
                                child: (other_schema, other),
                                child_cols: fk.columns.clone(),
                                parent,
                                parent_cols: fk.referenced_columns.clone(),
                                via: fk.name.clone(),
                                inferred: false,
                            },
                        ));
                    }
                }
            }
        }
    }
    for v in infer_foreign_keys(catalog) {
        let child = catalog.find_table(Some(&v.schema), &v.table);
        let parent = catalog.find_table(Some(&v.referenced_schema), &v.referenced_table);
        let (Some(child), Some(parent)) = (child, parent) else { continue };
        for (si, (_, s_schema, s_table)) in resolved.iter().enumerate() {
            let touches_child = std::ptr::eq(child.1, *s_table) && std::ptr::eq(child.0, *s_schema);
            let touches_parent = std::ptr::eq(parent.1, *s_table) && std::ptr::eq(parent.0, *s_schema);
            if touches_child || touches_parent {
                edges.push((
                    si,
                    touches_child,
                    Edge {
                        child,
                        child_cols: vec![v.column.clone()],
                        parent,
                        parent_cols: vec![v.referenced_column.clone()],
                        via: format!("inferred: {}.{} → {}.{}", v.table, v.column, v.referenced_table, v.referenced_column),
                        inferred: true,
                    },
                ));
            }
        }
    }

    let mut used: HashSet<String> = in_scope.iter().map(|r| r.reference_name().to_lowercase()).collect();
    let default = catalog.default_schema.as_deref();
    let mut out: Vec<JoinSuggestion> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Real constraints first, then inferred ones, each in scope order.
    edges.sort_by_key(|(si, _, e)| (e.inferred, *si));
    for (si, scope_is_child, e) in edges {
        let (scope_ref, _, _) = resolved[si];
        let new_side = if scope_is_child { e.parent } else { e.child };
        // Already joined: skip unless it is a self-join on the scope table.
        if in_scope_tables.iter().any(|t| same(*t, new_side)) && !same(new_side, (resolved[si].1, resolved[si].2)) {
            continue;
        }
        let key = format!("{}|{}|{}|{:?}|{:?}", si, new_side.0.name, new_side.1.name, e.child_cols, e.parent_cols);
        if !seen.insert(key) {
            continue;
        }
        let alias = unique_alias(&new_side.1.name, &used, kind);
        let scope_name = quote_if_needed(scope_ref.reference_name(), kind);
        let new_alias_q = quote_if_needed(&alias, kind);
        let (child_alias, parent_alias) = if scope_is_child { (&scope_name, &new_alias_q) } else { (&new_alias_q, &scope_name) };
        let condition = e
            .child_cols
            .iter()
            .zip(&e.parent_cols)
            .map(|(c, p)| {
                format!("{child_alias}.{} = {parent_alias}.{}", quote_if_needed(c, kind), quote_if_needed(p, kind))
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let schema = if default.is_some_and(|d| d.eq_ignore_ascii_case(&new_side.0.name)) { None } else { Some(new_side.0.name.clone()) };
        let clause = format!(
            "JOIN {} {} ON {}",
            qualified_name(schema.as_deref(), &new_side.1.name, kind),
            new_alias_q,
            condition
        );
        used.insert(alias.to_lowercase());
        out.push(JoinSuggestion {
            table: TableRef { schema, table: new_side.1.name.clone(), alias: Some(alias) },
            condition,
            clause,
            via: e.via,
            inferred: e.inferred,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, col, int, table, text};
    use DatabaseKind::Postgres;

    fn tr(t: &str, a: Option<&str>) -> TableRef {
        TableRef { schema: None, table: t.into(), alias: a.map(|s| s.into()) }
    }

    #[test]
    fn tables_in_scope_basic() {
        let refs = tables_in_scope("select * from public.customers c join orders on true, stores s", Postgres);
        assert_eq!(
            refs,
            vec![
                TableRef { schema: Some("public".into()), table: "customers".into(), alias: Some("c".into()) },
                tr("orders", None),
                tr("stores", Some("s")),
            ]
        );
        assert!(tables_in_scope("select 1", Postgres).is_empty());
        let refs = tables_in_scope("with x as (select 1) select * from x, (select 2) d", Postgres);
        assert_eq!(refs, vec![tr("x", None)]);
    }

    #[test]
    fn joins_from_customers_follow_fks_in_reverse() {
        let cat = fixtures::shop();
        let joins = suggest_joins(&cat, &[tr("customers", Some("c"))], Postgres);
        let orders = joins.iter().find(|j| j.table.table == "orders" && !j.inferred).expect("orders join");
        assert_eq!(orders.condition, "o.customer_id = c.id");
        assert_eq!(orders.clause, "JOIN orders o ON o.customer_id = c.id");
        assert_eq!(orders.via, "orders_customer_id_fkey");
        assert_eq!(orders.table.alias.as_deref(), Some("o"));
    }

    #[test]
    fn joins_from_orders_go_both_ways() {
        let cat = fixtures::shop();
        let joins = suggest_joins(&cat, &[tr("orders", Some("o"))], Postgres);
        let names: Vec<&str> = joins.iter().map(|j| j.table.table.as_str()).collect();
        assert!(names.contains(&"customers"));
        assert!(names.contains(&"stores"));
        assert!(names.contains(&"order_items"));
        let c = joins.iter().find(|j| j.table.table == "customers").unwrap();
        assert_eq!(c.clause, "JOIN customers c ON o.customer_id = c.id");
        let oi = joins.iter().find(|j| j.table.table == "order_items").unwrap();
        assert_eq!(oi.condition, "oi.order_id = o.id");
    }

    #[test]
    fn already_joined_tables_are_not_suggested_again() {
        let cat = fixtures::shop();
        let joins = suggest_joins(&cat, &[tr("customers", Some("c")), tr("orders", Some("o"))], Postgres);
        assert!(!joins.iter().any(|j| j.table.table == "customers" || j.table.table == "orders"));
        assert!(joins.iter().any(|j| j.table.table == "order_items"));
    }

    #[test]
    fn alias_collisions_are_resolved() {
        let cat = fixtures::shop();
        let joins = suggest_joins(&cat, &[tr("customers", Some("o"))], Postgres);
        let orders = joins.iter().find(|j| j.table.table == "orders").unwrap();
        assert_ne!(orders.table.alias.as_deref(), Some("o"));
        assert!(orders.clause.starts_with("JOIN orders or"), "{}", orders.clause);
    }

    #[test]
    fn composite_fk_conditions_are_anded() {
        let mut cat = fixtures::shop();
        let parent = table("p", vec![int("a"), int("b")], &["a", "b"], vec![], None);
        let child = table("ch", vec![int("pa"), int("pb")], &[], vec![fixtures::fk("ch_fk", &["pa", "pb"], None, "p", &["a", "b"])], None);
        cat.schemas[0].tables.push(parent);
        cat.schemas[0].tables.push(child);
        let joins = suggest_joins(&cat, &[tr("p", Some("p"))], Postgres);
        let j = joins.iter().find(|j| j.table.table == "ch").unwrap();
        assert_eq!(j.condition, "c.pa = p.a AND c.pb = p.b");
    }

    #[test]
    fn other_schema_tables_are_qualified() {
        let mut cat = fixtures::shop();
        // A declared FK from analytics.events to public.customers.
        cat.schemas[1].tables[0].foreign_keys.push(fixtures::fk("ev_c", &["customer_id"], Some("public"), "customers", &["id"]));
        let joins = suggest_joins(&cat, &[tr("customers", Some("c"))], Postgres);
        let e = joins.iter().find(|j| j.table.table == "events").unwrap();
        assert_eq!(e.table.schema.as_deref(), Some("analytics"));
        assert_eq!(e.clause, "JOIN analytics.events e ON e.customer_id = c.id");
    }

    #[test]
    fn self_referencing_fk_gets_a_fresh_alias() {
        let mut cat = fixtures::shop();
        let emp = table(
            "employees",
            vec![int("id"), int("manager_id")],
            &["id"],
            vec![fixtures::fk("emp_mgr", &["manager_id"], None, "employees", &["id"])],
            None,
        );
        cat.schemas[0].tables.push(emp);
        let joins = suggest_joins(&cat, &[tr("employees", Some("e"))], Postgres);
        let j = joins.iter().find(|j| j.table.table == "employees").unwrap();
        assert_ne!(j.table.alias.as_deref(), Some("e"));
        assert!(j.condition.starts_with("e.manager_id = ") || j.condition.ends_with(" = e.id") || j.condition.contains("manager_id"));
    }

    #[test]
    fn inferred_joins_are_marked() {
        let cat = fixtures::shop();
        let joins = suggest_joins(&cat, &[tr("customers", Some("c"))], Postgres);
        let r = joins.iter().find(|j| j.table.table == "reviews").expect("inferred reviews join");
        assert!(r.inferred);
        assert_eq!(r.via, "inferred: reviews.customer_id → customers.id");
        // Real constraints sort before inferred ones.
        let first_inferred = joins.iter().position(|j| j.inferred).unwrap();
        assert!(joins[..first_inferred].iter().all(|j| !j.inferred));
    }

    #[test]
    fn unknown_tables_yield_nothing() {
        let cat = fixtures::shop();
        assert!(suggest_joins(&cat, &[tr("nope", None)], Postgres).is_empty());
        assert!(suggest_joins(&cat, &[], Postgres).is_empty());
    }

    #[test]
    fn infers_the_obvious_pairs() {
        let cat = fixtures::shop();
        let v = infer_foreign_keys(&cat);
        let find = |t: &str, c: &str| v.iter().find(|x| x.table == t && x.column == c);
        let r = find("reviews", "customer_id").expect("customer_id");
        assert_eq!((r.referenced_table.as_str(), r.referenced_column.as_str()), ("customers", "id"));
        assert!(r.confidence > 0.8, "{}", r.confidence);
        assert!(r.reason.contains("customer_id"));
        let r = find("reviews", "product_id").expect("product_id");
        assert_eq!(r.referenced_table, "products");
        let r = find("events", "customer_id").expect("cross-schema customer_id");
        assert_eq!((r.referenced_schema.as_str(), r.referenced_table.as_str()), ("public", "customers"));
        let r = find("categories", "parent_id").expect("self reference");
        assert_eq!(r.referenced_table, "categories");
        assert_eq!(find("products", "category_id").map(|x| x.referenced_table.as_str()), Some("categories"));
    }

    #[test]
    fn never_repeats_declared_foreign_keys() {
        let cat = fixtures::shop();
        let v = infer_foreign_keys(&cat);
        assert!(!v.iter().any(|x| x.table == "orders" && x.column == "customer_id"));
        assert!(!v.iter().any(|x| x.table == "order_items" && x.column == "order_id"));
        assert!(!v.iter().any(|x| x.column == "store_id" && x.table == "orders"));
    }

    #[test]
    fn uuid_and_type_family_rules() {
        let users = table("users", vec![col("uuid", "uuid", quarry_core::TypeKind::Uuid), text("name")], &["uuid"], vec![], None);
        let posts = table(
            "posts",
            vec![int("id"), col("user_uuid", "uuid", quarry_core::TypeKind::Uuid), text("user_id")],
            &["id"],
            vec![],
            None,
        );
        let cat = Catalog { schemas: vec![Schema { name: "public".into(), tables: vec![users, posts], routines: vec![], sequences: vec![] }], default_schema: Some("public".into()) };
        let v = infer_foreign_keys(&cat);
        assert!(v.iter().any(|x| x.table == "posts" && x.column == "user_uuid" && x.referenced_column == "uuid"));
        // text `user_id` cannot reference a uuid-typed key.
        assert!(!v.iter().any(|x| x.column == "user_id"));
    }

    #[test]
    fn target_must_be_a_key() {
        let t = table("accounts", vec![int("id"), text("name")], &[], vec![], None);
        let u = table("tx", vec![int("id"), int("account_id")], &["id"], vec![], None);
        let cat = Catalog { schemas: vec![Schema { name: "public".into(), tables: vec![t, u], routines: vec![], sequences: vec![] }], default_schema: None };
        assert!(infer_foreign_keys(&cat).is_empty());
    }

    #[test]
    fn camel_case_and_role_prefix() {
        let c = table("customers", vec![int("id")], &["id"], vec![], None);
        let o = table("orders", vec![int("id"), int("customerId"), int("billing_customer_id")], &["id"], vec![], None);
        let cat = Catalog { schemas: vec![Schema { name: "public".into(), tables: vec![c, o], routines: vec![], sequences: vec![] }], default_schema: None };
        let v = infer_foreign_keys(&cat);
        let camel = v.iter().find(|x| x.column == "customerId").expect("camel");
        let role = v.iter().find(|x| x.column == "billing_customer_id").expect("role prefix");
        assert!(camel.confidence > role.confidence);
    }

    #[test]
    fn inference_is_fast_on_large_catalogs() {
        let cat = fixtures::big(500);
        let t = std::time::Instant::now();
        let _ = infer_foreign_keys(&cat);
        assert!(t.elapsed().as_millis() < 100, "{:?}", t.elapsed());
    }
}
