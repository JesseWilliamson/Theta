//! A database-neutral query plan, produced by `Connection::explain` and
//! analysed by `theta-sql`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub root: PlanNode,
    /// Whether the statement was actually executed (`EXPLAIN ANALYZE`), i.e.
    /// whether `actual_*` fields are populated.
    pub analyzed: bool,
    pub planning_time_ms: Option<f64>,
    pub execution_time_ms: Option<f64>,
    /// The server's raw output (JSON or text), for the "Raw" tab.
    pub raw: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanNode {
    /// Operator name, e.g. `Seq Scan`, `Hash Join`, `SEARCH`, `table scan`.
    pub node_type: String,
    pub relation: Option<String>,
    pub alias: Option<String>,
    pub index: Option<String>,
    /// Join type, scan direction, strategy etc. flattened to one line.
    pub detail: Option<String>,

    // Planner estimates (unit: the database's abstract cost unit).
    pub startup_cost: Option<f64>,
    pub total_cost: Option<f64>,
    pub plan_rows: Option<f64>,

    // Measured values, only when `Plan::analyzed`.
    /// Rows produced **per loop**.
    pub actual_rows: Option<f64>,
    /// Inclusive time for this node and its children, **summed over loops**, in ms.
    pub actual_total_ms: Option<f64>,
    pub loops: Option<f64>,

    pub filter: Option<String>,
    pub rows_removed_by_filter: Option<f64>,
    pub shared_hit_blocks: Option<u64>,
    pub shared_read_blocks: Option<u64>,
    /// Any other properties the server reported, as display strings.
    pub extra: BTreeMap<String, String>,
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    /// Time spent in this node alone (inclusive minus children), in ms.
    pub fn exclusive_ms(&self) -> Option<f64> {
        let total = self.actual_total_ms?;
        let children: f64 = self.children.iter().filter_map(|c| c.actual_total_ms).sum();
        Some((total - children).max(0.0))
    }

    /// Depth-first walk, parents before children.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a PlanNode, usize)) {
        fn go<'a>(n: &'a PlanNode, depth: usize, f: &mut dyn FnMut(&'a PlanNode, usize)) {
            f(n, depth);
            for c in &n.children {
                go(c, depth + 1, f);
            }
        }
        go(self, 0, f)
    }
}
