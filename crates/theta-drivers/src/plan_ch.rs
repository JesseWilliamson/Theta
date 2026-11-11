//! ClickHouse `EXPLAIN json = 1` into a [`Plan`].
//!
//! ClickHouse's EXPLAIN describes the query *plan* (a pipeline of steps: `Expression`, `Filter`,
//! `ReadFromMergeTree`, `Aggregating`...), not a cost-based tree: there are no row estimates or costs, and
//! no ANALYZE. So the mapping is honest about what exists: node type, description (the table a read step
//! scans), and for MergeTree reads the index usage (`Parts` and `Granules` as `after/before`), kept as
//! text in `extra` where the plan insights can show it.

use theta_core::{Error, Plan, PlanNode, Result};

fn node(v: &serde_json::Value) -> PlanNode {
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let node_type = text("Node Type").unwrap_or_default();
    let description = text("Description");
    let mut n = PlanNode { node_type: node_type.clone(), ..Default::default() };
    // `ReadFromMergeTree (db.table)`: the description of a read step is the table it reads.
    if node_type.starts_with("ReadFrom") {
        n.relation = description.clone().filter(|d| !d.is_empty());
    } else if let Some(d) = &description {
        n.detail = Some(d.clone()).filter(|d| !d.is_empty());
    }
    if let Some(id) = text("Node Id") {
        n.extra.insert("node id".into(), id);
    }
    if let Some(items) = v.get("Indexes").and_then(|i| i.as_array()) {
        for (i, ix) in items.iter().enumerate() {
            let field =
                |k: &str| ix.get(k).map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string()));
            let ty = field("Type").unwrap_or_default();
            let name = field("Name").map(|n| format!(" {n}")).unwrap_or_default();
            let mut parts = Vec::new();
            for k in ["Condition", "Parts", "Granules"] {
                if let Some(x) = field(k) {
                    parts.push(format!("{}: {x}", k.to_ascii_lowercase()));
                }
            }
            n.extra.insert(format!("index {}: {ty}{name}", i + 1), parts.join(", "));
            // The primary key's condition is what a reader wants as "the filter": `true` means no pruning.
            if ty == "PrimaryKey"
                && let Some(c) = field("Condition")
            {
                n.filter = Some(c).filter(|c| c != "true");
            }
        }
    }
    for key in ["Header", "Expression", "Actions"] {
        if let Some(x) = v.get(key) {
            n.extra.insert(key.to_ascii_lowercase(), x.to_string());
        }
    }
    if let Some(children) = v.get("Plans").and_then(|p| p.as_array()) {
        n.children = children.iter().map(node).collect();
    }
    n
}

/// `[{"Plan": {...}}]` (or the bare object) to a plan; `raw` is kept for the Raw tab.
pub fn parse_json(raw: &str) -> Result<Plan> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| Error::other(format!("not an EXPLAIN json document: {e}")))?;
    let first = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
    let plan = first.get("Plan").ok_or_else(|| Error::other("the EXPLAIN output has no \"Plan\""))?;
    Ok(Plan {
        root: node(plan),
        analyzed: false,
        planning_time_ms: None,
        execution_time_ms: None,
        raw: raw.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Written from the key names in the ClickHouse documentation of `EXPLAIN` (json = 1, description = 1,
    // indexes = 1): `Node Type`, `Node Id`, `Description`, `Plans`, and on a ReadFromMergeTree node an
    // `Indexes` array whose entries have `Type`, `Condition`, `Parts` and `Granules` as `after/before`.
    // The page could not be copied verbatim, so this is the documented shape with invented values, weaker
    // evidence than a capture from a server.
    const PLAN: &str = r#"[{"Plan": {"Node Type": "Expression", "Node Id": "Expression_5", "Description": "(Project names)",
      "Plans": [{"Node Type": "Filter", "Node Id": "Filter_4", "Description": "(WHERE)",
        "Plans": [{"Node Type": "ReadFromMergeTree", "Node Id": "ReadFromMergeTree_0", "Description": "default.events",
          "Indexes": [
            {"Type": "MinMax", "Condition": "true", "Parts": "4/4", "Granules": "12/12"},
            {"Type": "PrimaryKey", "Keys": ["id"], "Condition": "(id in [42, 42])", "Parts": "1/4", "Granules": "1/12"}]}]}]}}]"#;

    #[test]
    fn plan_tree_and_index_usage() {
        let p = parse_json(PLAN).unwrap();
        assert_eq!(p.root.node_type, "Expression");
        assert_eq!(p.root.detail.as_deref(), Some("(Project names)"));
        let filter = &p.root.children[0];
        assert_eq!(filter.node_type, "Filter");
        let read = &filter.children[0];
        assert_eq!(read.relation.as_deref(), Some("default.events"));
        assert_eq!(read.filter.as_deref(), Some("(id in [42, 42])"));
        assert_eq!(
            read.extra.get("index 2: PrimaryKey").map(String::as_str),
            Some("condition: (id in [42, 42]), parts: 1/4, granules: 1/12")
        );
        assert!(!p.analyzed && p.execution_time_ms.is_none());
    }

    #[test]
    fn bare_object_and_errors() {
        let p =
            parse_json(r#"{"Plan": {"Node Type": "ReadFromStorage", "Description": "SystemOne"}}"#).unwrap();
        assert_eq!(p.root.relation.as_deref(), Some("SystemOne"));
        assert!(parse_json("not json").is_err());
        assert!(parse_json("[]").is_err());
    }
}
