//! DuckDB `EXPLAIN (FORMAT JSON)` and `EXPLAIN (ANALYZE, FORMAT JSON)` into a [`Plan`].
//!
//! Plain EXPLAIN answers an array whose first element is the root operator (`name`, `children`,
//! `extra_info`). ANALYZE answers the profiler's document: an object with an `operator` array (a
//! `RESULT_COLLECTOR` / `EXECUTE` / `EXPLAIN_ANALYZE` wrapper around the real root), per-operator `type`,
//! `timing` (seconds, the operator's own), `intermediate_rows` and `extra_info`, and `query.total_time`.
//! One walk reads both shapes, because key names have moved between DuckDB releases (older ones used
//! `operator_type` / `operator_timing` / `operator_cardinality`); whatever is not mapped stays in `extra`.

use serde_json::Value as J;
use theta_core::{Error, Plan, PlanNode, Result};

fn text(v: &J) -> String {
    match v {
        J::String(s) => s.trim().to_string(),
        J::Array(a) => a.iter().map(text).collect::<Vec<_>>().join(", "),
        J::Null => String::new(),
        other => other.to_string(),
    }
}

/// `~1000`, `1,000` or `1000` to a number.
fn number(v: &J) -> Option<f64> {
    match v {
        J::Number(n) => n.as_f64(),
        J::String(s) => s.trim().trim_start_matches('~').replace(',', "").parse().ok(),
        _ => None,
    }
}

fn node(v: &J) -> PlanNode {
    let name = ["type", "operator_type", "operator_name", "name"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|n| n.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();
    let mut n = PlanNode { node_type: name, ..Default::default() };
    n.actual_rows = v.get("intermediate_rows").or_else(|| v.get("operator_cardinality")).and_then(number);
    if let Some(r) = v.get("rows_scanned").and_then(number) {
        n.extra.insert("rows scanned".into(), format!("{r}"));
    }
    // The profiler reports seconds; `actual_total_ms` is milliseconds and, per the contract, inclusive of
    // children while the profiler's timing is the operator's own, so children are added in below.
    let own_ms = v.get("timing").or_else(|| v.get("operator_timing")).and_then(number).map(|s| s * 1000.0);
    if let Some(info) = v.get("extra_info").and_then(|e| e.as_object()) {
        for (k, val) in info {
            let t = text(val);
            match k.as_str() {
                "Estimated Cardinality" | "Estimated Cardinality:" => n.plan_rows = number(val),
                "Table" => n.relation = Some(t).filter(|t| !t.is_empty()),
                "Filters" | "Filter" | "Condition" => n.filter = Some(t).filter(|t| !t.is_empty()),
                _ if !t.is_empty() => {
                    n.extra.insert(k.trim_end_matches(':').to_string(), t);
                }
                _ => {}
            }
        }
    }
    if let Some(children) = v.get("children").and_then(|c| c.as_array()) {
        n.children = children.iter().map(node).collect();
    }
    if let Some(own) = own_ms {
        let below: f64 = n.children.iter().filter_map(|c| c.actual_total_ms).sum();
        n.actual_total_ms = Some(own + below);
        n.loops = Some(1.0);
    }
    n
}

pub fn parse(raw: &str, analyzed: bool) -> Result<Plan> {
    let v: J = serde_json::from_str(raw.trim())
        .map_err(|e| Error::other(format!("not an EXPLAIN json document: {e}")))?;
    let top = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
    let secs =
        |path: &[&str]| path.iter().try_fold(top, |j, k| j.get(*k)).and_then(number).map(|s| s * 1000.0);
    // Where the profiler keeps the operators: `operator` (current), `children` (older).
    let ops = top.get("operator").or_else(|| top.get("children")).and_then(|c| c.as_array());
    let mut root = match (ops, top.get("query").is_some() || top.get("latency").is_some()) {
        (Some(ops), true) => match ops.first() {
            Some(first) => node(first),
            None => return Err(Error::other("the EXPLAIN document has no operators")),
        },
        _ => node(top),
    };
    // The profiler wraps the plan in pseudo-operators; the real root is below them.
    while matches!(root.node_type.as_str(), "RESULT_COLLECTOR" | "EXECUTE" | "EXPLAIN_ANALYZE")
        && root.children.len() == 1
    {
        root = root.children.remove(0);
    }
    if root.node_type.is_empty() && root.children.is_empty() {
        return Err(Error::other("the EXPLAIN document has no operators"));
    }
    let execution_time_ms =
        if analyzed { secs(&["query", "total_time"]).or_else(|| secs(&["latency"])) } else { None };
    let planning_time_ms = analyzed
        .then(|| {
            let parts: Vec<f64> =
                [["planner", "total_time"], ["optimizer", "total_time"], ["physical_planner", "total_time"]]
                    .iter()
                    .filter_map(|p| secs(p))
                    .collect();
            (!parts.is_empty()).then(|| parts.iter().sum())
        })
        .flatten();
    Ok(Plan { root, analyzed, planning_time_ms, execution_time_ms, raw: raw.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from DuckDB 2.0.0 (the version bundled by duckdb 1.20000.1) with `EXPLAIN (FORMAT JSON)` on
    // `SELECT e.name FROM e JOIN d ON d.id = e.dept_id WHERE e.name = 'a'`.
    const PLAIN: &str = r#"[{"name": "PROJECTION", "children": [{"name": "HASH_JOIN", "children": [
      {"name": "SEQ_SCAN", "children": [], "extra_info": {"Table": "memory.main.d", "Type": "Sequential Scan", "Projections": "id", "Estimated Cardinality": "2"}},
      {"name": "SEQ_SCAN", "children": [], "extra_info": {"Table": "memory.main.e", "Type": "Sequential Scan", "Projections": ["dept_id", "name"], "Filters": "(name = 'a')", "Estimated Cardinality": "1"}}],
      "extra_info": {"Join Type": "INNER", "Conditions": "id = dept_id", "Estimated Cardinality": "1"}}],
      "extra_info": {"Projections": "name", "Estimated Cardinality": "1"}}]"#;

    // Captured from DuckDB 2.0.0 with `EXPLAIN (ANALYZE, FORMAT JSON)` on the same query; the optimizer and
    // system sections of the document are cut, they are not read.
    const ANALYZED: &str = r#"{"operator": [{"extra_info": {}, "intermediate_rows": 0, "timing": 0.0, "type": "RESULT_COLLECTOR", "children": [
      {"extra_info": {}, "intermediate_rows": 0, "timing": 0.0, "type": "EXECUTE", "children": [
       {"extra_info": {}, "intermediate_rows": 0, "timing": 7.6e-8, "type": "EXPLAIN_ANALYZE", "children": [
        {"extra_info": {"Projections": "name", "Estimated Cardinality": "1"}, "intermediate_rows": 1, "timing": 2.24e-7, "type": "PROJECTION", "children": [
         {"extra_info": {"Join Type": "INNER", "Conditions": "id = dept_id", "Estimated Cardinality": "1"}, "intermediate_rows": 1, "timing": 0.000046975, "type": "HASH_JOIN", "children": [
          {"extra_info": {"Table": "memory.main.d", "Type": "Index Scan", "Projections": "id", "Dynamic Filters": "(id = 1)", "Estimated Cardinality": "2"}, "intermediate_rows": 1, "rows_scanned": 1, "timing": 0.0000055739999999999995, "type": "TABLE_SCAN"},
          {"extra_info": {"Table": "memory.main.e", "Type": "Index Scan", "Projections": ["dept_id", "name"], "Filters": "(name = 'a')", "Estimated Cardinality": "1"}, "intermediate_rows": 1, "rows_scanned": 1, "timing": 0.000011183, "type": "TABLE_SCAN"}]}]}]}]}]}],
      "planner": {"binding_time": 0.00021846, "total_time": 0.000180459},
      "optimizer": {"total_time": 0.000309637}, "physical_planner": {"total_time": 0.000030326},
      "query": {"cpu_time": 0.000064032, "total_time": 0.001097525}}"#;

    #[test]
    fn plain_plan() {
        let p = parse(PLAIN, false).unwrap();
        assert_eq!(p.root.node_type, "PROJECTION");
        let join = &p.root.children[0];
        assert_eq!(join.node_type, "HASH_JOIN");
        assert_eq!(join.extra.get("Conditions").map(String::as_str), Some("id = dept_id"));
        assert_eq!(join.plan_rows, Some(1.0));
        let scan = &join.children[1];
        assert_eq!(scan.relation.as_deref(), Some("memory.main.e"));
        assert_eq!(scan.filter.as_deref(), Some("(name = 'a')"));
        assert_eq!(scan.extra.get("Projections").map(String::as_str), Some("dept_id, name"));
        assert!(!p.analyzed && p.execution_time_ms.is_none());
    }

    #[test]
    fn analyzed_plan_unwraps_the_profiler_and_sums_time() {
        let p = parse(ANALYZED, true).unwrap();
        assert!(p.analyzed);
        assert_eq!(p.root.node_type, "PROJECTION");
        assert_eq!(p.root.actual_rows, Some(1.0));
        let join = &p.root.children[0];
        assert_eq!(join.node_type, "HASH_JOIN");
        // inclusive time = own + children
        let own = 0.000046975 * 1000.0;
        let kids = (0.0000055739999999999995 + 0.000011183) * 1000.0;
        assert!((join.actual_total_ms.unwrap() - (own + kids)).abs() < 1e-9);
        assert!((join.exclusive_ms().unwrap() - own).abs() < 1e-9);
        assert_eq!(join.children[0].extra.get("rows scanned").map(String::as_str), Some("1"));
        assert!((p.execution_time_ms.unwrap() - 1.097525).abs() < 1e-9);
        assert!(p.planning_time_ms.unwrap() > 0.0);
    }

    #[test]
    fn errors() {
        assert!(parse("nope", false).is_err());
        assert!(parse("[]", false).is_err());
    }
}
