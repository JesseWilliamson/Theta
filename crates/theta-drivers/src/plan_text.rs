//! Parsers for the two PostgreSQL-wire products whose EXPLAIN is a text tree rather than JSON:
//! CockroachDB (`EXPLAIN` / `EXPLAIN ANALYZE`) and Amazon Redshift (`EXPLAIN`).
//!
//! Both are pure functions over the text the server returned (one line per result row), so they are
//! tested with fixtures. The fixtures come from each vendor's documentation, not from a server: see the
//! comment on each one.

use std::collections::BTreeMap;
use theta_core::{Error, Plan, PlanNode, Result};

fn num(s: &str) -> Option<f64> {
    s.trim().replace(',', "").parse().ok()
}

/// `4ms`, `443µs`, `1.5s`, `2m3s` is not produced by CockroachDB; its durations are one number and a unit.
fn duration_ms(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (n, unit) = s.split_at(split);
    let n: f64 = n.parse().ok()?;
    Some(match unit.trim() {
        "ns" => n / 1e6,
        "µs" | "us" => n / 1e3,
        "ms" => n,
        "s" => n * 1e3,
        "m" => n * 60e3,
        _ => return None,
    })
}

// ------------------------------------------------------------------------------------ CockroachDB

/// The plan tree CockroachDB prints. Header lines (`distribution: full`, `planning time: 4ms`) come
/// before the first `•`; each node is `• name` followed by its `key: value` attributes; a node's depth
/// is the column of its bullet (4 columns per level: `└── • `).
pub fn parse_cockroach(raw: &str, analyzed: bool) -> Result<Plan> {
    let mut header: BTreeMap<String, String> = BTreeMap::new();
    // (depth, node) in document order; the tree is assembled afterwards.
    let mut flat: Vec<(usize, PlanNode)> = Vec::new();
    for line in raw.lines() {
        if let Some(bullet) = line.chars().position(|c| c == '•') {
            let name: String = line.chars().skip(bullet + 1).collect::<String>().trim().to_string();
            if name.is_empty() {
                continue;
            }
            // The first bullet sits at column 2; every level adds 4 (`└── ` puts the next bullet at 6).
            let depth = bullet.saturating_sub(2) / 4;
            flat.push((depth, PlanNode { node_type: name, ..Default::default() }));
            continue;
        }
        let text = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '│' | '├' | '└' | '─'));
        let Some((k, v)) = text.split_once(": ") else { continue };
        match flat.last_mut() {
            Some((_, node)) => apply_cockroach_attr(node, k.trim(), v.trim()),
            None => {
                header.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    if flat.is_empty() {
        return Err(Error::other("EXPLAIN returned no plan nodes"));
    }
    let mut root = assemble(flat);
    // Header lines are plan-wide facts; the root keeps them in `extra` so the Raw/inspector views show them.
    for (k, v) in &header {
        if !matches!(k.as_str(), "planning time" | "execution time") {
            root.extra.insert(k.clone(), v.clone());
        }
    }
    let ms = |k: &str| header.get(k).and_then(|v| duration_ms(v));
    Ok(Plan {
        root,
        analyzed,
        planning_time_ms: ms("planning time"),
        execution_time_ms: ms("execution time"),
        raw: raw.to_string(),
    })
}

fn apply_cockroach_attr(node: &mut PlanNode, key: &str, value: &str) {
    match key {
        // `125,000 (100% of the table; stats collected 19 minutes ago)`: the number is the estimate.
        "estimated row count" => {
            node.plan_rows = num(value.split_whitespace().next().unwrap_or(""));
            if value.contains('(') {
                node.extra.insert(key.into(), value.into());
            }
        }
        "actual row count" => node.actual_rows = num(value),
        // `rides@rides_pkey`: table and index in one token.
        "table" => match value.split_once('@') {
            Some((t, i)) => {
                node.relation = Some(t.to_string());
                node.index = Some(i.to_string());
            }
            None => node.relation = Some(value.to_string()),
        },
        "filter" => node.filter = Some(value.to_string()),
        "spans" => node.detail = Some(format!("spans: {value}")),
        _ => {
            node.extra.insert(key.to_string(), value.to_string());
        }
    }
}

/// Turn (depth, node) in pre-order into a tree. The text has one root; anything after it that comes back
/// to depth 0 is folded under the first root rather than dropped.
fn assemble(flat: Vec<(usize, PlanNode)>) -> PlanNode {
    // Open ancestors, outermost first. A node closes every open node at its depth or deeper.
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    for (depth, node) in flat {
        while stack.len() > 1 && stack.last().is_some_and(|(d, _)| *d >= depth.max(1)) {
            let (_, done) = stack.pop().unwrap();
            stack.last_mut().unwrap().1.children.push(done);
        }
        stack.push((depth, node));
    }
    while stack.len() > 1 {
        let (_, done) = stack.pop().unwrap();
        stack.last_mut().unwrap().1.children.push(done);
    }
    stack.pop().unwrap().1
}

// ---------------------------------------------------------------------------------------- Redshift

/// Redshift's `EXPLAIN`: `XN Seq Scan on event  (cost=0.00..87.98 rows=8798 width=23)` lines, children
/// introduced by `->` and indented under their parent, `Key: value` lines for conditions.
pub fn parse_redshift(raw: &str) -> Result<Plan> {
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    let mut root: Option<PlanNode> = None;
    for line in raw.lines() {
        if line.trim().is_empty()
            || line.trim_start().starts_with("QUERY PLAN")
            || line.trim().starts_with("---")
        {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let body = line.trim_start();
        let (arrow, text) = match body.strip_prefix("->") {
            Some(rest) => (true, rest.trim_start()),
            None => (false, body),
        };
        if let Some(node) = redshift_node(text) {
            // Pop to the parent: the nearest open node indented less than this arrow.
            while let Some((d, _)) = stack.last() {
                if *d >= indent && !stack.is_empty() {
                    let (_, done) = stack.pop().unwrap();
                    match stack.last_mut() {
                        Some((_, p)) => p.children.push(done),
                        None => root = Some(done),
                    }
                } else {
                    break;
                }
            }
            let _ = arrow;
            stack.push((indent, node));
        } else if let Some((k, v)) = text.split_once(": ") {
            // A condition line belongs to the node above it.
            if let Some((_, n)) = stack.last_mut() {
                if k.trim() == "Filter" {
                    n.filter = Some(v.trim().to_string());
                } else {
                    n.extra.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    while let Some((_, done)) = stack.pop() {
        match stack.last_mut() {
            Some((_, p)) => p.children.push(done),
            None => root = Some(done),
        }
    }
    let root = root.ok_or_else(|| Error::other("EXPLAIN returned no plan nodes"))?;
    Ok(Plan { root, analyzed: false, planning_time_ms: None, execution_time_ms: None, raw: raw.to_string() })
}

/// `XN Hash Join DS_DIST_OUTER  (cost=2.52..58653620.93 rows=8712 width=43)` to a node, or `None` when the
/// line has no cost clause.
fn redshift_node(text: &str) -> Option<PlanNode> {
    let at = text.rfind("(cost=")?;
    let (head, costs) = text.split_at(at);
    let costs = costs.trim_start_matches("(cost=").trim_end_matches(')');
    let (range, rest) = costs.split_once(' ')?;
    let (startup, total) = range.split_once("..")?;
    let rows = rest.split_whitespace().find_map(|w| w.strip_prefix("rows="));
    let width = rest.split_whitespace().find_map(|w| w.strip_prefix("width="));
    let head = head.trim().trim_start_matches("XN ").trim();
    // `Seq Scan on event e` / `Hash Join DS_DIST_OUTER`
    let (op, relation, alias) = match head.split_once(" on ") {
        Some((op, rel)) => {
            let mut p = rel.split_whitespace();
            (op.trim(), p.next().map(str::to_string), p.next().map(str::to_string))
        }
        None => (head, None, None),
    };
    let mut node = PlanNode {
        node_type: op.to_string(),
        relation,
        alias,
        startup_cost: num(startup),
        total_cost: num(total),
        plan_rows: rows.and_then(num),
        ..Default::default()
    };
    // A distribution strategy (`DS_DIST_OUTER`, `DS_BCAST_INNER`) trails a join's name and is the most
    // important fact about it on Redshift; keep it as the detail.
    if let Some(pos) = node.node_type.find(" DS_") {
        node.detail = Some(node.node_type[pos + 1..].to_string());
        node.node_type.truncate(pos);
    }
    if let Some(w) = width {
        node.extra.insert("width".into(), w.to_string());
    }
    Some(node)
}

// -------------------------------------------------------------------------------------------- TiDB

/// TiDB's `EXPLAIN [ANALYZE]` table. Columns: `id` (operator name, a numeric suffix and a tree prefix of
/// `└─`, `├─` and `│ ` pairs), `estRows`, `task` (`root`, `cop[tikv]`, `cop[tiflash]`), `access object`,
/// `operator info`, and with ANALYZE also `actRows`, `execution info`, `memory`, `disk`.
pub fn parse_tidb(columns: &[String], rows: &[Vec<String>], analyzed: bool) -> Result<Plan> {
    let col = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let (Some(id), est) = (col("id"), col("estRows")) else {
        return Err(Error::other("unexpected EXPLAIN columns from TiDB"));
    };
    fn get(row: &[String], at: Option<usize>) -> Option<&str> {
        at.and_then(|i| row.get(i)).map(|v| v.trim()).filter(|v| !v.is_empty())
    }
    let mut flat: Vec<(usize, PlanNode)> = Vec::new();
    for row in rows {
        let Some(raw_id) = row.get(id) else { continue };
        // Two characters per level; the operator name starts at the first letter.
        let name_at = raw_id.find(|c: char| c.is_alphanumeric()).unwrap_or(0);
        let depth = raw_id[..name_at].chars().count() / 2;
        // `TableReader_12(Probe)`: the parenthesis names the join side, not the operator.
        let operator = raw_id[name_at..].split('(').next().unwrap_or("");
        // `TableReader_5`: the suffix is the operator's id in the plan, not part of its name.
        let node_type = match operator.rsplit_once('_') {
            Some((n, d)) if d.chars().all(|c| c.is_ascii_digit()) && !d.is_empty() => n,
            _ => operator,
        };
        let mut node = PlanNode {
            node_type: node_type.to_string(),
            plan_rows: get(row, est).and_then(num),
            actual_rows: get(row, col("actRows")).and_then(num),
            detail: get(row, col("task")).map(|t| format!("task: {t}")),
            ..Default::default()
        };
        if let Some(obj) = get(row, col("access object")) {
            // `table:t1, index:idx(a)`
            for part in obj.split(", ") {
                if let Some(t) = part.strip_prefix("table:") {
                    node.relation = Some(t.to_string());
                } else if let Some(i) = part.strip_prefix("index:") {
                    node.index = Some(i.split('(').next().unwrap_or(i).to_string());
                } else {
                    node.extra.entry("access object".into()).or_default().push_str(part);
                }
            }
        }
        if let Some(info) = get(row, col("operator info")) {
            node.extra.insert("operator info".into(), info.to_string());
        }
        if let Some(exec) = get(row, col("execution info")) {
            // `time:1.2ms, loops:2, ...`: the first two are the per-operator timing and iteration count.
            for part in exec.split(", ") {
                if let Some(t) = part.strip_prefix("time:") {
                    node.actual_total_ms = duration_ms(t);
                } else if let Some(l) = part.strip_prefix("loops:") {
                    node.loops = num(l);
                }
            }
            node.extra.insert("execution info".into(), exec.to_string());
        }
        for key in ["memory", "disk"] {
            if let Some(v) = get(row, col(key)).filter(|v| *v != "N/A") {
                node.extra.insert(key.into(), v.to_string());
            }
        }
        flat.push((depth, node));
    }
    if flat.is_empty() {
        return Err(Error::other("EXPLAIN returned no plan nodes"));
    }
    let root = assemble(flat);
    let raw = std::iter::once(columns.join("\t"))
        .chain(rows.iter().map(|r| r.join("\t")))
        .collect::<Vec<_>>()
        .join("\n");
    let execution_time_ms = analyzed.then_some(root.actual_total_ms).flatten();
    Ok(Plan { root, analyzed, planning_time_ms: None, execution_time_ms, raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Copied from the CockroachDB documentation, "EXPLAIN" page (default example, `rides` table). Taken
    // from the docs, not captured from a server, so the real output of a given version may differ.
    const CRDB_EXPLAIN: &str = "  distribution: full
  vectorized: true

  • sort
  │ estimated row count: 12,385
  │ order: +revenue
  │
  └── • filter
      │ estimated row count: 12,385
      │ filter: revenue > 90
      │
      └── • scan
            estimated row count: 125,000 (100% of the table; stats collected 19 minutes ago)
            table: rides@rides_pkey
            spans: FULL SCAN";

    // Assembled from the values the CockroachDB documentation lists for its `EXPLAIN ANALYZE` example
    // (`SELECT city, AVG(revenue) FROM rides GROUP BY city`, MovR): planning time 4ms, execution time 5ms,
    // `group (streaming)` over a `scan` with the stated row counts. The docs page could not be copied
    // verbatim, so line layout follows the `EXPLAIN` fixture above; weaker evidence than a capture.
    const CRDB_ANALYZE: &str = "  planning time: 4ms
  execution time: 5ms
  distribution: full
  vectorized: true
  rows decoded from KV: 500 (87 KiB, 1 gRPC call)
  cumulative time spent in KV: 4ms
  maximum memory usage: 240 KiB
  network usage: 0 B (0 messages)

  • group (streaming)
  │ sql nodes: n1
  │ regions: us-east1
  │ actual row count: 9
  │ estimated row count: 9
  │ group by: city
  │ ordered: +city
  │
  └── • scan
        sql nodes: n1
        kv nodes: n1
        actual row count: 500
        KV time: 4ms
        KV rows decoded: 500
        estimated row count: 500 (100% of the table; stats collected 3 minutes ago)
        table: rides@rides_pkey
        spans: FULL SCAN";

    #[test]
    fn cockroach_tree_and_attributes() {
        let p = parse_cockroach(CRDB_EXPLAIN, false).unwrap();
        assert_eq!(p.root.node_type, "sort");
        assert_eq!(p.root.plan_rows, Some(12385.0));
        let filter = &p.root.children[0];
        assert_eq!(filter.node_type, "filter");
        assert_eq!(filter.filter.as_deref(), Some("revenue > 90"));
        let scan = &filter.children[0];
        assert_eq!(scan.node_type, "scan");
        assert_eq!(scan.relation.as_deref(), Some("rides"));
        assert_eq!(scan.index.as_deref(), Some("rides_pkey"));
        assert_eq!(scan.plan_rows, Some(125000.0));
        assert_eq!(scan.detail.as_deref(), Some("spans: FULL SCAN"));
        assert_eq!(p.root.extra.get("distribution").map(String::as_str), Some("full"));
        assert!(!p.analyzed && p.execution_time_ms.is_none());
    }

    #[test]
    fn cockroach_analyze_times_and_actual_rows() {
        let p = parse_cockroach(CRDB_ANALYZE, true).unwrap();
        assert_eq!(p.planning_time_ms, Some(4.0));
        assert_eq!(p.execution_time_ms, Some(5.0));
        assert_eq!(p.root.node_type, "group (streaming)");
        assert_eq!(p.root.actual_rows, Some(9.0));
        let scan = &p.root.children[0];
        assert_eq!(scan.actual_rows, Some(500.0));
        assert_eq!(scan.extra.get("KV time").map(String::as_str), Some("4ms"));
    }

    #[test]
    fn cockroach_siblings_and_empty_input() {
        let two = "  distribution: local\n\n  • hash join\n  │ equality: (a) = (b)\n  │\n  ├── • scan\n  │     table: t@t_pkey\n  │\n  └── • scan\n        table: u@u_pkey";
        let p = parse_cockroach(two, false).unwrap();
        assert_eq!(p.root.children.len(), 2);
        assert_eq!(p.root.children[1].relation.as_deref(), Some("u"));
        assert!(parse_cockroach("  distribution: local", false).is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(duration_ms("443µs"), Some(0.443));
        assert_eq!(duration_ms("1.5s"), Some(1500.0));
        assert_eq!(duration_ms("nonsense"), None);
    }

    // Copied from the Amazon Redshift documentation, EXPLAIN page, "Using EXPLAIN for RLS" (the example
    // that keeps its indentation; the markdown emphasis around one line is removed). Docs, not a capture.
    const REDSHIFT_EXPLAIN: &str = " XN Hash Join DS_DIST_ALL_NONE  (cost=0.08..0.25 rows=1 width=4)
   Hash Cond: (\"outer\".k_dim = \"inner\".\"k\")
   ->  XN RLS SecureScan f  (cost=0.00..0.14 rows=2 width=4)
         Filter: ((k_dim / 10) > 0)
   ->  XN Hash  (cost=0.07..0.07 rows=2 width=8)
         ->  XN Seq Scan on dim_tbl d  (cost=0.00..0.07 rows=2 width=8)
               Filter: ((\"k\" / 10) > 0)";

    #[test]
    fn redshift_tree_distribution_and_filters() {
        let p = parse_redshift(REDSHIFT_EXPLAIN).unwrap();
        assert_eq!(p.root.node_type, "Hash Join");
        assert_eq!(p.root.detail.as_deref(), Some("DS_DIST_ALL_NONE"));
        assert_eq!(p.root.total_cost, Some(0.25));
        assert_eq!(
            p.root.extra.get("Hash Cond").map(String::as_str),
            Some("(\"outer\".k_dim = \"inner\".\"k\")")
        );
        assert_eq!(p.root.children.len(), 2);
        assert_eq!(p.root.children[0].filter.as_deref(), Some("((k_dim / 10) > 0)"));
        let hash = &p.root.children[1];
        assert_eq!(hash.node_type, "Hash");
        let scan = &hash.children[0];
        assert_eq!(
            (scan.node_type.as_str(), scan.relation.as_deref(), scan.alias.as_deref()),
            ("Seq Scan", Some("dim_tbl"), Some("d"))
        );
        assert_eq!(scan.plan_rows, Some(2.0));
        assert!(!p.analyzed);
    }

    #[test]
    fn redshift_single_node_and_garbage() {
        let p = parse_redshift(
            "XN Seq Scan on venue  (cost=0.00..2.02 rows=187 width=45)\n  Filter: (venueseats IS NOT NULL)",
        )
        .unwrap();
        assert_eq!(p.root.filter.as_deref(), Some("(venueseats IS NOT NULL)"));
        assert!(parse_redshift("nothing useful").is_err());
    }

    // The row layout (`└─` prefix on the child, `cop[tikv]` task, `table:t1` access object, numeric id
    // suffixes) and the two-operator shape follow the TiDB documentation's EXPLAIN ANALYZE example for
    // `SELECT * FROM t1` (TableReader_5 over TableFullScan_4, estRows 10000.00, actRows 3). The docs page
    // could not be copied verbatim, so the timing text is an assumption of the documented format.
    #[test]
    fn tidb_explain_analyze_table() {
        let cols: Vec<String> = [
            "id",
            "estRows",
            "actRows",
            "task",
            "access object",
            "execution info",
            "operator info",
            "memory",
            "disk",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let rows = [
            [
                "TableReader_5",
                "10000.00",
                "3",
                "root",
                "",
                "time:1.2ms, loops:2, cop_task: {num: 1}",
                "data:TableFullScan_4",
                "N/A",
                "N/A",
            ],
            [
                "└─TableFullScan_4",
                "10000.00",
                "3",
                "cop[tikv]",
                "table:t1",
                "tikv_task:{time:0s, loops:1}",
                "keep order:false, stats:pseudo",
                "N/A",
                "N/A",
            ],
        ]
        .iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect::<Vec<Vec<String>>>();
        let p = parse_tidb(&cols, &rows, true).unwrap();
        assert_eq!(p.root.node_type, "TableReader");
        assert_eq!(p.root.actual_total_ms, Some(1.2));
        assert_eq!(p.root.loops, Some(2.0));
        assert_eq!(p.execution_time_ms, Some(1.2));
        let scan = &p.root.children[0];
        assert_eq!(scan.node_type, "TableFullScan");
        assert_eq!(scan.relation.as_deref(), Some("t1"));
        assert_eq!(scan.plan_rows, Some(10000.0));
        assert_eq!(scan.actual_rows, Some(3.0));
        assert_eq!(scan.detail.as_deref(), Some("task: cop[tikv]"));
        assert!(p.analyzed);
    }

    #[test]
    fn tidb_plain_explain_nesting() {
        let cols: Vec<String> = ["id", "estRows", "task", "access object", "operator info"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows: Vec<Vec<String>> = [
            ["HashJoin_7", "12487.50", "root", "", "inner join, equal:[eq(test.a.id, test.b.id)]"],
            ["├─TableReader_9(Build)", "9990.00", "root", "", "data:Selection_8"],
            ["│ └─Selection_8", "9990.00", "cop[tikv]", "", "not(isnull(test.b.id))"],
            ["│   └─TableFullScan_17(Build)", "10000.00", "cop[tikv]", "table:b", "keep order:false"],
            ["└─TableReader_12(Probe)", "10000.00", "root", "", "data:TableFullScan_11"],
        ]
        .iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect();
        let p = parse_tidb(&cols, &rows, false).unwrap();
        assert_eq!(p.root.children.len(), 2);
        let build = &p.root.children[0];
        assert_eq!(build.children[0].node_type, "Selection");
        assert_eq!(build.children[0].children[0].relation.as_deref(), Some("b"));
        assert_eq!(p.root.children[1].node_type, "TableReader");
        assert!(parse_tidb(&["x".to_string()], &[], false).is_err());
    }
}
