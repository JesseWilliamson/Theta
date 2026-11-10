//! Parser for PostgreSQL's `EXPLAIN (FORMAT JSON)` output.

use quarry_core::{Error, Plan, PlanNode, Result};
use serde_json::{Map, Value as J};

/// Keys mapped onto first-class [`PlanNode`] fields; everything else goes to `extra`.
const HANDLED: &[&str] = &[
    "Node Type",
    "Relation Name",
    "Alias",
    "Index Name",
    "Startup Cost",
    "Total Cost",
    "Plan Rows",
    "Actual Rows",
    "Actual Total Time",
    "Actual Startup Time",
    "Actual Loops",
    "Filter",
    "Rows Removed by Filter",
    "Shared Hit Blocks",
    "Shared Read Blocks",
    "Plans",
];

pub fn parse_plan(raw: &str, analyzed: bool) -> Result<Plan> {
    let doc: J = serde_json::from_str(raw).map_err(|e| Error::other(format!("unparseable EXPLAIN output: {e}")))?;
    let top = match &doc {
        J::Array(a) => a.first(),
        other => Some(other),
    }
    .and_then(J::as_object)
    .ok_or_else(|| Error::other("EXPLAIN output has no top-level object"))?;
    let plan = top.get("Plan").and_then(J::as_object).ok_or_else(|| Error::other("EXPLAIN output has no Plan"))?;
    let mut root = node(plan);
    for key in ["Triggers", "JIT", "Settings"] {
        if let Some(v) = top.get(key) {
            root.extra.insert(key.to_string(), render(v));
        }
    }
    Ok(Plan {
        root,
        analyzed,
        planning_time_ms: top.get("Planning Time").and_then(J::as_f64),
        execution_time_ms: top.get("Execution Time").and_then(J::as_f64),
        raw: raw.to_string(),
    })
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

fn str_of(o: &Map<String, J>, k: &str) -> Option<String> {
    o.get(k).and_then(J::as_str).map(str::to_string)
}

fn node(o: &Map<String, J>) -> PlanNode {
    let f = |k: &str| o.get(k).and_then(J::as_f64);
    let loops = f("Actual Loops");
    let mut n = PlanNode {
        node_type: str_of(o, "Node Type").unwrap_or_else(|| "Unknown".into()),
        relation: str_of(o, "Relation Name"),
        alias: str_of(o, "Alias"),
        index: str_of(o, "Index Name"),
        startup_cost: f("Startup Cost"),
        total_cost: f("Total Cost"),
        plan_rows: f("Plan Rows"),
        actual_rows: f("Actual Rows"),
        // "Actual Total Time" is the average per loop; the contract wants the sum.
        actual_total_ms: f("Actual Total Time").map(|t| t * loops.unwrap_or(1.0)),
        loops,
        filter: str_of(o, "Filter"),
        rows_removed_by_filter: f("Rows Removed by Filter"),
        shared_hit_blocks: o.get("Shared Hit Blocks").and_then(J::as_u64),
        shared_read_blocks: o.get("Shared Read Blocks").and_then(J::as_u64),
        ..Default::default()
    };
    // PostgreSQL reports every write as `ModifyTable`; the operation says which.
    if n.node_type == "ModifyTable" {
        if let Some(op) = str_of(o, "Operation") {
            n.node_type = op;
        }
    }
    let mut detail = Vec::new();
    if let Some(j) = str_of(o, "Join Type") {
        detail.push(format!("{j} join"));
    }
    if let Some(d) = str_of(o, "Scan Direction") {
        if d != "Forward" {
            detail.push(d);
        }
    }
    for k in ["Strategy", "Partial Mode", "Subplan Name", "CTE Name"] {
        if let Some(v) = str_of(o, k) {
            detail.push(v);
        }
    }
    if o.get("Parallel Aware").and_then(J::as_bool) == Some(true) {
        detail.push("Parallel".into());
    }
    if !detail.is_empty() {
        n.detail = Some(detail.join(", "));
    }
    for (k, v) in o {
        if HANDLED.contains(&k.as_str()) || v.is_null() {
            continue;
        }
        n.extra.insert(k.clone(), render(v));
    }
    if let Some(J::Array(kids)) = o.get("Plans") {
        n.children = kids.iter().filter_map(J::as_object).map(node).collect();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANALYZED: &str = include_str!("../tests/fixtures/pg_analyze.json");
    const PLAIN: &str = include_str!("../tests/fixtures/pg_plain.json");

    #[test]
    fn plain_plan_has_costs_but_no_timings() {
        let p = parse_plan(PLAIN, false).unwrap();
        assert!(!p.analyzed);
        assert_eq!(p.root.node_type, "Sort");
        assert!(p.root.total_cost.unwrap() > 0.0);
        assert_eq!(p.root.actual_total_ms, None);
        assert_eq!(p.root.extra["Sort Key"], "o.created_at DESC");
        let join = &p.root.children[0];
        assert_eq!(join.node_type, "Hash Join");
        assert_eq!(join.detail.as_deref(), Some("Inner join"));
        assert_eq!(join.extra["Hash Cond"], "(o.customer_id = c.id)");
    }

    #[test]
    fn analyzed_plan_sums_time_over_loops() {
        let p = parse_plan(ANALYZED, true).unwrap();
        assert_eq!(p.planning_time_ms, Some(0.25));
        assert_eq!(p.execution_time_ms, Some(12.5));
        let nl = &p.root;
        assert_eq!(nl.node_type, "Nested Loop");
        let inner = &nl.children[1];
        // 0.01 ms per loop * 100 loops
        assert_eq!(inner.loops, Some(100.0));
        assert!((inner.actual_total_ms.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(inner.actual_rows, Some(1.0));
        assert_eq!(inner.index.as_deref(), Some("customers_pkey"));
        assert_eq!(inner.shared_hit_blocks, Some(300));
        assert_eq!(inner.extra["Index Cond"], "(id = o.customer_id)");
        assert_eq!(nl.children[0].extra["Workers Launched"], "2");
        assert_eq!(p.root.extra["Triggers"], "[]");
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_plan("not json", false).is_err());
        assert!(parse_plan("[{}]", false).is_err());
    }
}
