//! Parsers for MySQL / MariaDB query plans.
//!
//! * MariaDB `EXPLAIN FORMAT=JSON` / `ANALYZE FORMAT=JSON`
//! * MySQL 8 `EXPLAIN FORMAT=JSON` (`query_block` shape, with `cost_info`)
//! * MySQL 8 `EXPLAIN ANALYZE` (indented text tree)
//!
//! Both JSON dialects are nested objects whose structure is described by a
//! handful of well-known keys (`nested_loop`, `table`, `ordering_operation`,
//! ...). Rather than model each shape, [`block`] walks the tree generically:
//! structural keys become child nodes, scalar keys become `extra` properties.

use serde_json::{Map, Value as J};
use theta_core::{Error, Plan, PlanNode, Result};

/// Keys whose values are (arrays of) objects that become child plan nodes.
const STRUCTURAL: &[&str] = &[
    "query_block",
    "nested_loop",
    "table",
    "ordering_operation",
    "grouping_operation",
    "duplicates_removal",
    "buffer_result",
    "windowing",
    "window_functions_computation",
    "filesort",
    "temporary_table",
    "materialized",
    "materialized_from_subquery",
    "read_sorted_file",
    "block-nl-join",
    "union_result",
    "query_specifications",
    "subqueries",
    "select_list_subqueries",
    "optimized_away_subqueries",
    "attached_subqueries",
    "order_by_subqueries",
    "group_by_subqueries",
    "having_subqueries",
    "update_value_subqueries",
    "expression_cache",
    "insert",
    "update",
    "delete",
];

fn pretty(key: &str) -> String {
    let words = key.replace(['_', '-'], " ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn is_structural(k: &str, v: &J) -> bool {
    STRUCTURAL.contains(&k) && matches!(v, J::Object(_) | J::Array(_))
}

fn num(v: &J) -> Option<f64> {
    match v {
        J::Number(n) => n.as_f64(),
        // MySQL 8 reports costs and percentages as strings.
        J::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn render(v: &J) -> String {
    match v {
        J::String(s) => s.clone(),
        J::Array(a) if !a.is_empty() && a.iter().all(|x| !x.is_object() && !x.is_array()) => {
            a.iter().map(render).collect::<Vec<_>>().join(", ")
        }
        other => other.to_string(),
    }
}

pub fn parse_json_plan(raw: &str, analyzed: bool) -> Result<Plan> {
    let doc: J =
        serde_json::from_str(raw).map_err(|e| Error::other(format!("unparseable EXPLAIN output: {e}")))?;
    let obj = doc.as_object().ok_or_else(|| Error::other("EXPLAIN output is not a JSON object"))?;
    let qb = obj
        .get("query_block")
        .and_then(J::as_object)
        .ok_or_else(|| Error::other("EXPLAIN output has no query_block"))?;
    let root = block("Query Block", qb);
    let execution_time_ms = qb.get("r_total_time_ms").and_then(num);
    Ok(Plan { root, analyzed, planning_time_ms: None, execution_time_ms, raw: raw.to_string() })
}

/// A node for a structural object (a `query_block`, an operation wrapper, ...).
fn block(label: &str, o: &Map<String, J>) -> PlanNode {
    let mut n = PlanNode { node_type: label.to_string(), ..Default::default() };
    apply_common(&mut n, o);
    collect_children(&mut n, o);
    let child_sum: f64 = n.children.iter().filter_map(|c| c.actual_total_ms).sum();
    let timed = n.children.iter().any(|c| c.actual_total_ms.is_some());
    let own = o.get("r_total_time_ms").and_then(num);
    // `query_block` carries the whole statement's time. Every other wrapper
    // (filesort, ...) reports only the time spent in that step itself, so the
    // contract's inclusive figure is its own time plus its children's.
    n.actual_total_ms = if label == "Query Block" {
        own
    } else if own.is_some() || timed {
        Some(own.unwrap_or(0.0) + child_sum)
    } else {
        None
    };
    n
}

/// Properties shared by blocks and tables: costs, timings and plain scalars.
fn apply_common(n: &mut PlanNode, o: &Map<String, J>) {
    if let Some(c) = o.get("cost").and_then(num) {
        n.total_cost = Some(c);
    }
    if let Some(c) = o.get("cost_info").and_then(J::as_object).and_then(|c| c.get("query_cost")).and_then(num)
    {
        n.total_cost = Some(c);
    }
    if let Some(l) = o.get("r_loops").and_then(num) {
        n.loops = Some(l);
    }
    for (k, v) in o {
        if is_structural(k, v) || v.is_null() {
            continue;
        }
        match k.as_str() {
            // Consumed by the caller or mapped onto first-class fields.
            "cost" | "r_total_time_ms" | "r_loops" => {}
            "cost_info" => {
                if let Some(ci) = v.as_object() {
                    for (ck, cv) in ci {
                        if ck != "query_cost" {
                            n.extra.insert(format!("cost.{ck}"), render(cv));
                        }
                    }
                }
            }
            _ => {
                n.extra.insert(k.clone(), render(v));
            }
        }
    }
}

fn collect_children(n: &mut PlanNode, o: &Map<String, J>) {
    for (k, v) in o {
        if !is_structural(k, v) {
            continue;
        }
        match (k.as_str(), v) {
            ("table", J::Object(t)) => n.children.push(table(t)),
            ("nested_loop", J::Array(items)) => {
                let mut nl = PlanNode { node_type: "Nested loop".into(), ..Default::default() };
                for item in items {
                    if let J::Object(io) = item {
                        collect_children(&mut nl, io);
                    }
                }
                let timed: Vec<f64> = nl.children.iter().filter_map(|c| c.actual_total_ms).collect();
                if !timed.is_empty() {
                    nl.actual_total_ms = Some(timed.iter().sum());
                }
                n.children.push(nl);
            }
            ("query_block", J::Object(q)) => n.children.push(block("Query Block", q)),
            (key, J::Object(inner)) => n.children.push(block(&pretty(key), inner)),
            (key, J::Array(items)) => {
                // Arrays such as `subqueries` / `query_specifications` hold wrappers
                // like `{ "query_block": ... }` or `{ "expression_cache": ... }`.
                for item in items {
                    if let J::Object(io) = item
                        && io.keys().any(|k| STRUCTURAL.contains(&k.as_str()))
                    {
                        let mut wrapper = PlanNode { node_type: pretty(key), ..Default::default() };
                        apply_common(&mut wrapper, io);
                        collect_children(&mut wrapper, io);
                        if wrapper.children.len() == 1 && wrapper.extra.is_empty() {
                            n.children.push(wrapper.children.pop().unwrap());
                        } else {
                            n.children.push(wrapper);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn table(t: &Map<String, J>) -> PlanNode {
    let s = |k: &str| t.get(k).and_then(J::as_str).map(str::to_string);
    let access = s("access_type").unwrap_or_else(|| "table".into());
    let mut n = PlanNode {
        node_type: access.clone(),
        relation: s("table_name"),
        alias: None,
        index: s("key"),
        detail: Some(format!("access type: {access}")),
        filter: s("attached_condition"),
        // MariaDB: `rows`; MySQL 8: `rows_examined_per_scan`.
        plan_rows: t.get("rows").or_else(|| t.get("rows_examined_per_scan")).and_then(num),
        actual_rows: t.get("r_rows").and_then(num),
        ..Default::default()
    };
    if let Some(pc) =
        t.get("cost_info").and_then(J::as_object).and_then(|c| c.get("prefix_cost")).and_then(num)
    {
        n.total_cost = Some(pc);
    }
    // MariaDB reports table time and residual time separately.
    let table_time = t.get("r_table_time_ms").and_then(num);
    let other_time = t.get("r_other_time_ms").and_then(num);
    n.actual_total_ms = match (table_time, other_time) {
        (None, None) => t.get("r_total_time_ms").and_then(num),
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    };
    if let Some(l) = t.get("r_loops").and_then(num) {
        n.loops = Some(l);
        // The contract wants time summed over loops; MariaDB's r_*_time_ms
        // figures are already totals across all loops, so nothing to multiply.
    }
    if let (Some(est), Some(r_filtered)) =
        (t.get("filtered").and_then(num), t.get("r_filtered").and_then(num))
    {
        n.extra.insert("filtered".into(), format!("{est}% estimated, {r_filtered}% actual"));
    }
    for (k, v) in t {
        if matches!(
            k.as_str(),
            "table_name"
                | "access_type"
                | "key"
                | "attached_condition"
                | "rows"
                | "rows_examined_per_scan"
                | "r_rows"
                | "r_table_time_ms"
                | "r_other_time_ms"
                | "r_total_time_ms"
                | "r_loops"
                | "filtered"
                | "r_filtered"
        ) || v.is_null()
        {
            continue;
        }
        if is_structural(k, v) {
            continue;
        }
        if k == "cost_info" {
            if let Some(ci) = v.as_object() {
                for (ck, cv) in ci {
                    n.extra.insert(format!("cost.{ck}"), render(cv));
                }
            }
            continue;
        }
        n.extra.insert(k.clone(), render(v));
    }
    collect_children(&mut n, t);
    n
}

/// Parse MySQL 8's `EXPLAIN ANALYZE` text tree:
///
/// ```text
/// -> Nested loop inner join  (cost=1.2 rows=2) (actual time=0.1..0.2 rows=3 loops=1)
///     -> Table scan on t  (cost=0.35 rows=2) (actual time=0.05..0.06 rows=2 loops=1)
/// ```
pub fn parse_text_plan(raw: &str) -> Result<Plan> {
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    let mut roots: Vec<PlanNode> = Vec::new();
    fn close_to(stack: &mut Vec<(usize, PlanNode)>, roots: &mut Vec<PlanNode>, indent: usize) {
        while let Some((i, _)) = stack.last() {
            if *i < indent {
                break;
            }
            let (_, node) = stack.pop().unwrap();
            match stack.last_mut() {
                Some((_, parent)) => parent.children.push(node),
                None => roots.push(node),
            }
        }
    }
    for line in raw.lines() {
        let Some(arrow) = line.find("-> ") else { continue };
        let indent = arrow;
        close_to(&mut stack, &mut roots, indent);
        stack.push((indent, text_node(&line[arrow + 3..])));
    }
    close_to(&mut stack, &mut roots, 0);
    let root = match roots.len() {
        0 => return Err(Error::other("EXPLAIN ANALYZE returned no plan")),
        1 => roots.pop().unwrap(),
        _ => PlanNode { node_type: "Query".into(), children: roots, ..Default::default() },
    };
    let execution_time_ms = root.actual_total_ms;
    Ok(Plan { root, analyzed: true, planning_time_ms: None, execution_time_ms, raw: raw.to_string() })
}

fn text_node(body: &str) -> PlanNode {
    let (head, stats) = match body.find("  (") {
        Some(i) => (&body[..i], &body[i + 2..]),
        None => (body, ""),
    };
    let mut n = PlanNode { node_type: head.trim().to_string(), ..Default::default() };
    // e.g. "Table scan on t", "Index lookup on o using idx (customer_id=c.id)"
    if let Some(rest) = head.split(" on ").nth(1) {
        let rel = rest.split_whitespace().next().unwrap_or("");
        n.relation = Some(rel.trim_matches('`').to_string());
        if let Some(i) = rest.find(" using ") {
            n.index = rest[i + 7..].split_whitespace().next().map(str::to_string);
        }
    }
    if let Some(f) = head.strip_prefix("Filter: ") {
        n.filter = Some(f.to_string());
    }
    for group in stats.split(") (") {
        let g = group.trim_matches(|c| c == '(' || c == ')');
        let (kind, rest) = if let Some(r) = g.strip_prefix("actual ") { ("actual", r) } else { ("est", g) };
        let kv = |key: &str| {
            rest.split_whitespace().find_map(|w| w.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
        };
        if kind == "est" {
            n.total_cost = kv("cost").and_then(|v| v.parse().ok());
            n.plan_rows = kv("rows").and_then(|v| v.parse().ok());
        } else {
            n.loops = kv("loops").and_then(|v| v.parse().ok());
            n.actual_rows = kv("rows").and_then(|v| v.parse().ok());
            if let Some(t) = kv("time") {
                let end = t.split("..").last().and_then(|v| v.parse::<f64>().ok());
                // `time=a..b` is milliseconds per loop; the contract wants the sum.
                n.actual_total_ms = end.map(|e| e * n.loops.unwrap_or(1.0));
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARIA_PLAIN: &str = include_str!("../tests/fixtures/mariadb_plain.json");
    const MARIA_ANALYZE: &str = include_str!("../tests/fixtures/mariadb_analyze.json");
    const MYSQL8: &str = include_str!("../tests/fixtures/mysql8_plain.json");
    const MYSQL8_ANALYZE: &str = include_str!("../tests/fixtures/mysql8_analyze.txt");

    fn find(root: &PlanNode, f: impl Fn(&PlanNode) -> bool) -> &PlanNode {
        let mut found = None;
        root.walk(&mut |n, _| {
            if found.is_none() && f(n) {
                found = Some(n);
            }
        });
        found.expect("node not found")
    }

    #[test]
    fn mariadb_nested_loop() {
        let p = parse_json_plan(MARIA_PLAIN, false).unwrap();
        assert_eq!(p.root.node_type, "Query Block");
        let sort = &p.root.children[0];
        assert_eq!(sort.node_type, "Filesort");
        assert_eq!(sort.extra["sort_key"], "o.total desc");
        let nl = find(&p.root, |n| n.node_type == "Nested loop");
        assert_eq!(nl.children.len(), 2);
        let c = &nl.children[0];
        assert_eq!((c.relation.as_deref(), c.node_type.as_str()), (Some("c"), "ALL"));
        assert_eq!(c.plan_rows, Some(200.0));
        assert_eq!(c.filter.as_deref(), Some("c.active = 1"));
        let o = &nl.children[1];
        assert_eq!(o.index.as_deref(), Some("idx_orders_customer"));
        assert_eq!(o.extra["possible_keys"], "idx_orders_customer");
        assert_eq!(o.extra["ref"], "fx.c.id");
    }

    #[test]
    fn mariadb_analyze_has_timings() {
        let p = parse_json_plan(MARIA_ANALYZE, true).unwrap();
        assert!(p.analyzed);
        assert!((p.execution_time_ms.unwrap() - 0.775400452).abs() < 1e-9);
        let o = find(&p.root, |n| n.relation.as_deref() == Some("o"));
        assert_eq!(o.actual_rows, Some(3.0));
        assert_eq!(o.loops, Some(100.0));
        // table time + residual time, both already summed over loops by MariaDB
        assert!((o.actual_total_ms.unwrap() - (0.349881532 + 0.087637031)).abs() < 1e-9);
        assert!(o.extra["filtered"].contains("100% actual"));
        // The filesort wrapper's inclusive time is its own plus everything below it.
        let sort = &p.root.children[0];
        assert_eq!(sort.node_type, "Filesort");
        assert!(sort.actual_total_ms.unwrap() > 0.028591909);
        assert!(sort.exclusive_ms().unwrap() > 0.0);
    }

    #[test]
    fn mysql8_query_block() {
        let p = parse_json_plan(MYSQL8, false).unwrap();
        assert_eq!(p.root.total_cost, Some(1.2));
        let ord = &p.root.children[0];
        assert_eq!(ord.node_type, "Ordering operation");
        assert_eq!(ord.extra["using_filesort"], "true");
        let nl = &ord.children[0];
        let o = &nl.children[1];
        assert_eq!(o.relation.as_deref(), Some("orders"));
        assert_eq!(o.index.as_deref(), Some("customer_id"));
        assert_eq!(o.plan_rows, Some(3.0));
        assert_eq!(o.total_cost, Some(2.15));
        assert_eq!(o.extra["used_columns"], "id, customer_id, total");
        assert_eq!(o.extra["cost.read_cost"], "0.75");
    }

    #[test]
    fn mysql8_explain_analyze_tree() {
        let p = parse_text_plan(MYSQL8_ANALYZE).unwrap();
        assert!(p.analyzed);
        assert_eq!(p.root.node_type, "Sort: o.total DESC");
        let join = &p.root.children[0];
        assert!(join.node_type.starts_with("Nested loop inner join"));
        assert_eq!(join.children.len(), 2);
        let scan = &join.children[0];
        assert_eq!(scan.relation.as_deref(), Some("c"));
        assert_eq!(scan.actual_rows, Some(200.0));
        let lookup = &join.children[1];
        assert_eq!(lookup.index.as_deref(), Some("customer_id"));
        // 0.004 ms per loop * 200 loops
        assert!((lookup.actual_total_ms.unwrap() - 0.8).abs() < 1e-9);
        assert_eq!(lookup.loops, Some(200.0));
    }

    #[test]
    fn invalid_input_errors() {
        assert!(parse_json_plan("[]", false).is_err());
        assert!(parse_text_plan("nothing").is_err());
    }
}
