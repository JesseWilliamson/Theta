//! The plan as a table: derived per-node metrics, the flattened visible rows
//! under a collapse state, and the path <-> row mappings the views and the
//! insight cards share. No GPUI here, so all of it is unit-tested.

use crate::format::*;
use std::collections::{HashMap, HashSet};
use theta_core::{DatabaseKind, Plan, PlanNode};

/// Operator families drive the icon and the flame graph colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Scan,
    Index,
    Join,
    Sort,
    Aggregate,
    Other,
}

impl Family {
    pub const ALL: [Family; 6] =
        [Family::Scan, Family::Index, Family::Join, Family::Sort, Family::Aggregate, Family::Other];

    pub fn label(self) -> &'static str {
        match self {
            Family::Scan => "Scan",
            Family::Index => "Index",
            Family::Join => "Join",
            Family::Sort => "Sort",
            Family::Aggregate => "Aggregate",
            Family::Other => "Other",
        }
    }
}

/// MySQL/MariaDB access types (`type` column of a classic EXPLAIN).
fn mysql_access_title(t: &str) -> Option<&'static str> {
    Some(match t {
        "ALL" => "Full table scan",
        "index" => "Full index scan",
        "range" => "Index range scan",
        "ref" => "Index lookup",
        "eq_ref" => "Unique index lookup",
        "ref_or_null" => "Index lookup (or NULL)",
        "const" => "Constant row lookup",
        "system" => "System row",
        "index_merge" => "Index merge",
        "fulltext" => "Fulltext search",
        "unique_subquery" | "index_subquery" => "Subquery index lookup",
        _ => return None,
    })
}

pub fn classify(n: &PlanNode) -> Family {
    let raw = n.node_type.trim();
    let t = raw.to_ascii_lowercase();
    if mysql_access_title(raw).is_some() {
        return if raw == "ALL" { Family::Scan } else { Family::Index };
    }
    if t.contains("join") || t.contains("nested loop") || t.contains("cartesian") {
        Family::Join
    } else if t.contains("sort") || t.contains("ordering") || t.contains("order by") {
        Family::Sort
    } else if t.contains("aggregate")
        || t.contains("group")
        || t.contains("unique")
        || t.contains("distinct")
        || t.contains("duplicates")
        || t.contains("windowagg")
        || t.contains("window")
        || t.contains("setop")
    {
        Family::Aggregate
    } else if t.contains("index") || t == "search" {
        Family::Index
    } else if t.contains("scan") {
        Family::Scan
    } else {
        Family::Other
    }
}

/// Operator name and the muted context that follows it (`on orders o`).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeLabel {
    pub title: String,
    pub subtitle: String,
}

impl NodeLabel {
    /// Both parts as one line, for tooltips, flame blocks and plain-text export.
    pub fn full(&self) -> String {
        if self.subtitle.is_empty() {
            self.title.clone()
        } else {
            format!("{}  {}", self.title, self.subtitle)
        }
    }
}

pub fn label(n: &PlanNode, kind: DatabaseKind) -> NodeLabel {
    let title = if kind == DatabaseKind::MySql {
        mysql_access_title(n.node_type.trim()).map(str::to_string).unwrap_or_else(|| n.node_type.clone())
    } else {
        n.node_type.clone()
    };
    let mut sub: Vec<String> = Vec::new();
    if let Some(r) = &n.relation {
        match &n.alias {
            Some(a) if a != r => sub.push(format!("on {r} {a}")),
            _ => sub.push(format!("on {r}")),
        }
    }
    if let Some(i) = &n.index {
        sub.push(format!("using {i}"));
    }
    if let Some(d) = &n.detail {
        let (dl, tl) = (d.to_ascii_lowercase(), n.node_type.to_ascii_lowercase());
        // Parsers for MySQL and SQLite keep their raw one-liner here; it repeats
        // what the title and relation already say.
        if !(dl.starts_with(&tl) || dl.starts_with("access type:")) {
            sub.push(d.clone());
        }
    }
    NodeLabel { title, subtitle: sub.join("  ") }
}

/// One node's derived numbers.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub path: Vec<usize>,
    pub depth: usize,
    pub parent: Option<usize>,
    /// Exclusive end of this node's subtree in pre-order ids.
    pub subtree_end: usize,
    pub child_count: usize,
    pub family: Family,
    pub incl_ms: Option<f64>,
    pub excl_ms: Option<f64>,
    pub excl_cost: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleRow {
    pub id: usize,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

/// Pre-order node table plus collapse state. Node ids are pre-order indexes, so a
/// subtree is the contiguous range `id..subtree_end`.
#[derive(Debug, Clone)]
pub struct PlanTree {
    nodes: Vec<TreeNode>,
    by_path: HashMap<Vec<usize>, usize>,
    collapsed: HashSet<usize>,
    pub total_ms: Option<f64>,
    pub total_cost: Option<f64>,
    pub max_excl_ms: f64,
    pub max_excl_cost: f64,
}

pub fn node_at<'a>(root: &'a PlanNode, path: &[usize]) -> Option<&'a PlanNode> {
    let mut n = root;
    for &i in path {
        n = n.children.get(i)?;
    }
    Some(n)
}

impl PlanTree {
    pub fn new(plan: &Plan) -> Self {
        let mut tree = PlanTree {
            nodes: Vec::new(),
            by_path: HashMap::new(),
            collapsed: HashSet::new(),
            total_ms: plan.root.actual_total_ms.or(plan.execution_time_ms).filter(|_| plan.analyzed),
            total_cost: plan.root.total_cost,
            max_excl_ms: 0.0,
            max_excl_cost: 0.0,
        };
        let mut path = Vec::new();
        tree.build(&plan.root, None, &mut path);
        tree
    }

    fn build(&mut self, n: &PlanNode, parent: Option<usize>, path: &mut Vec<usize>) {
        let id = self.nodes.len();
        let excl_cost =
            n.total_cost.map(|t| (t - n.children.iter().filter_map(|c| c.total_cost).sum::<f64>()).max(0.0));
        let excl_ms = n.exclusive_ms();
        self.max_excl_ms = self.max_excl_ms.max(excl_ms.unwrap_or(0.0));
        self.max_excl_cost = self.max_excl_cost.max(excl_cost.unwrap_or(0.0));
        self.by_path.insert(path.clone(), id);
        self.nodes.push(TreeNode {
            path: path.clone(),
            depth: path.len(),
            parent,
            subtree_end: id + 1,
            child_count: n.children.len(),
            family: classify(n),
            incl_ms: n.actual_total_ms,
            excl_ms,
            excl_cost,
        });
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            self.build(c, Some(id), path);
            path.pop();
        }
        self.nodes[id].subtree_end = self.nodes.len();
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, id: usize) -> &TreeNode {
        &self.nodes[id]
    }

    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub fn id_for_path(&self, path: &[usize]) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    pub fn path_of(&self, id: usize) -> &[usize] {
        &self.nodes[id].path
    }

    pub fn is_expanded(&self, id: usize) -> bool {
        !self.collapsed.contains(&id)
    }

    pub fn toggle(&mut self, id: usize) {
        if self.nodes[id].child_count == 0 {
            return;
        }
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
    }

    pub fn set_expanded(&mut self, id: usize, expanded: bool) {
        if self.nodes[id].child_count == 0 {
            return;
        }
        if expanded {
            self.collapsed.remove(&id);
        } else {
            self.collapsed.insert(id);
        }
    }

    pub fn expand_all(&mut self) {
        self.collapsed.clear();
    }

    /// Collapse every inner node except the root, so the first level stays visible.
    pub fn collapse_all(&mut self) {
        self.collapsed = (1..self.nodes.len()).filter(|&i| self.nodes[i].child_count > 0).collect();
    }

    /// Whether any inner node is collapsed (decides which toolbar toggle to offer).
    pub fn any_collapsed(&self) -> bool {
        !self.collapsed.is_empty()
    }

    /// Expand every ancestor of `id` so its row is visible.
    pub fn reveal(&mut self, id: usize) {
        let mut cur = self.nodes[id].parent;
        while let Some(p) = cur {
            self.collapsed.remove(&p);
            cur = self.nodes[p].parent;
        }
    }

    /// Rows currently visible, in display order.
    pub fn visible(&self) -> Vec<VisibleRow> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.nodes.len() {
            let n = &self.nodes[i];
            let expanded = !self.collapsed.contains(&i);
            out.push(VisibleRow { id: i, depth: n.depth, has_children: n.child_count > 0, expanded });
            i = if expanded { i + 1 } else { n.subtree_end };
        }
        out
    }

    /// Share of total runtime spent in this node alone (analyzed plans).
    pub fn time_share(&self, id: usize) -> Option<f64> {
        let total = self.total_ms.filter(|t| *t > 0.0)?;
        Some((self.nodes[id].excl_ms? / total).clamp(0.0, 1.0))
    }

    /// Share of the plan's cost spent in this node alone.
    pub fn cost_share(&self, id: usize) -> Option<f64> {
        let total = self.total_cost.filter(|t| *t > 0.0)?;
        Some((self.nodes[id].excl_cost? / total).clamp(0.0, 1.0))
    }

    /// Share used for the heat colouring: time when measured, else cost.
    pub fn heat_share(&self, id: usize) -> Option<f64> {
        self.time_share(id).or_else(|| self.cost_share(id))
    }
}

/// The row index of `id` among `visible`, if it is shown.
pub fn row_of(visible: &[VisibleRow], id: usize) -> Option<usize> {
    visible.binary_search_by_key(&id, |r| r.id).ok()
}

// ---------------------------------------------------------------- details

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    Plain,
    /// Rendered as SQL in the buffer font with syntax colours.
    Sql,
    /// Highlighted as a problem (misestimates, spills).
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub label: String,
    pub value: String,
    pub kind: PropKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub props: Vec<Prop>,
}

fn prop(label: impl Into<String>, value: impl Into<String>) -> Prop {
    Prop { label: label.into(), value: value.into(), kind: PropKind::Plain }
}

fn is_condition_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.ends_with(" cond")
        || k.ends_with("condition")
        || k.ends_with(" filter")
        || k.ends_with(" key")
        || k == "filter"
        || k == "output"
        || k.ends_with("sort_key")
        || k == "join_condition"
}

fn is_io_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.contains("blocks")
        || k.contains("memory")
        || k.contains("batches")
        || k.starts_with("sort ")
        || k.contains("disk")
        || k.contains("heap fetches")
        || k.contains("temp ")
        || k.starts_with("i/o")
        || k.contains("pages_accessed")
        || k.contains("engine_stats")
}

/// Everything about one node, grouped. Empty sections are omitted.
pub fn node_sections(tree: &PlanTree, plan: &Plan, id: usize) -> Vec<Section> {
    let Some(n) = node_at(&plan.root, tree.path_of(id)) else { return Vec::new() };
    let tn = tree.node(id);
    let loops = n.loops.unwrap_or(1.0).max(1.0);
    let mut out = Vec::new();

    let mut timing = Vec::new();
    if let Some(ex) = tn.excl_ms {
        let share = tree.time_share(id).map(|s| format!("  ({} of total)", fmt_share(s))).unwrap_or_default();
        timing.push(prop("Exclusive", format!("{}{share}", fmt_ms(ex))));
    }
    if let Some(inc) = tn.incl_ms {
        timing.push(prop("Inclusive", fmt_ms(inc)));
        if loops > 1.0 {
            timing.push(prop("Per loop", fmt_ms(inc / loops)));
        }
    }
    if let Some(l) = n.loops {
        timing.push(prop("Loops", fmt_rows(l)));
    }
    if !timing.is_empty() {
        out.push(Section { title: "Timing", props: timing });
    }

    let mut rows = Vec::new();
    if let Some(a) = n.actual_rows {
        let total = if loops > 1.0 {
            format!("{}  ({} × {})", fmt_rows(a * loops), fmt_rows(a), fmt_rows(loops))
        } else {
            fmt_rows(a)
        };
        rows.push(prop("Actual", total));
    }
    if let Some(e) = n.plan_rows {
        rows.push(prop("Estimated", format!("{} per loop", fmt_rows(e))));
    }
    if let Some(m) = misestimate(n.plan_rows, n.actual_rows).filter(|m| m.factor >= MISESTIMATE_THRESHOLD) {
        rows.push(Prop {
            label: "Misestimate".into(),
            value: format!("{} ({})", m.badge(), if m.under { "under-estimated" } else { "over-estimated" }),
            kind: PropKind::Warn,
        });
    }
    if let Some(r) = n.rows_removed_by_filter {
        rows.push(prop("Removed by filter", fmt_rows(r * loops)));
    }
    if !rows.is_empty() {
        out.push(Section { title: "Rows", props: rows });
    }

    let mut cost = Vec::new();
    if let Some(s) = n.startup_cost {
        cost.push(prop("Startup", fmt_cost(s)));
    }
    if let Some(t) = n.total_cost {
        cost.push(prop("Total", fmt_cost(t)));
    }
    if let Some(e) = tn.excl_cost {
        let share = tree.cost_share(id).map(|s| format!("  ({} of plan)", fmt_share(s))).unwrap_or_default();
        cost.push(prop("Exclusive", format!("{}{share}", fmt_cost(e))));
    }
    if !cost.is_empty() {
        out.push(Section { title: "Cost", props: cost });
    }

    let mut conds = Vec::new();
    if let Some(f) = &n.filter {
        conds.push(Prop { label: "Filter".into(), value: f.clone(), kind: PropKind::Sql });
    }
    for (k, v) in &n.extra {
        if is_condition_key(k) {
            conds.push(Prop { label: k.clone(), value: v.clone(), kind: PropKind::Sql });
        }
    }
    if !conds.is_empty() {
        out.push(Section { title: "Conditions", props: conds });
    }

    let mut io = Vec::new();
    match (n.shared_hit_blocks, n.shared_read_blocks) {
        (None, None) => {}
        (h, r) => {
            let (h, r) = (h.unwrap_or(0), r.unwrap_or(0));
            io.push(prop("Shared hit", format!("{}  ({})", group_digits(h), fmt_blocks_size(h))));
            io.push(prop("Shared read", format!("{}  ({})", group_digits(r), fmt_blocks_size(r))));
        }
    }
    let mut other = Vec::new();
    for (k, v) in &n.extra {
        if is_condition_key(k) {
            continue;
        }
        // PostgreSQL reports every counter even when it is zero; that is noise.
        if k.ends_with("Blocks") && v == "0" {
            continue;
        }
        if matches!(k.as_str(), "Async Capable" | "Parallel Aware") && v == "false" {
            continue;
        }
        let spill = k == "Hash Batches" && v.parse::<f64>().is_ok_and(|b| b > 1.0)
            || k == "Sort Method" && v.to_ascii_lowercase().contains("external");
        let p = Prop {
            label: k.clone(),
            value: v.clone(),
            kind: if spill { PropKind::Warn } else { PropKind::Plain },
        };
        if is_io_key(k) { io.push(p) } else { other.push(p) }
    }
    if !io.is_empty() {
        out.push(Section { title: "I/O and memory", props: io });
    }
    if !other.is_empty() {
        out.push(Section { title: "Other", props: other });
    }
    out
}

// ------------------------------------------------------------------- text

/// The plan as indented text for the clipboard.
pub fn plan_as_text(plan: &Plan, kind: DatabaseKind) -> String {
    let mut out = String::new();
    let mut header = Vec::new();
    if let Some(p) = plan.planning_time_ms {
        header.push(format!("Planning: {}", fmt_ms(p)));
    }
    if let Some(e) = plan.execution_time_ms {
        header.push(format!("Execution: {}", fmt_ms(e)));
    }
    if !plan.analyzed {
        header.push("Estimated only (not executed)".into());
    }
    if !header.is_empty() {
        out.push_str(&header.join(" · "));
        out.push('\n');
    }
    plan.root.walk(&mut |n, depth| {
        if depth > 0 {
            out.push_str(&"    ".repeat(depth - 1));
            out.push_str("  -> ");
        }
        out.push_str(&label(n, kind).full());
        let mut stats = Vec::new();
        if let Some(c) = n.total_cost {
            stats.push(format!("cost={}", fmt_cost(c)));
        }
        if let Some(r) = n.plan_rows {
            stats.push(format!("rows={}", fmt_rows(r)));
        }
        if !stats.is_empty() {
            out.push_str(&format!("  ({})", stats.join(" ")));
        }
        if plan.analyzed {
            let mut act = Vec::new();
            if let Some(t) = n.actual_total_ms {
                act.push(format!("time={}", fmt_ms(t)));
            }
            if let Some(r) = n.actual_rows {
                act.push(format!("rows={}", fmt_rows(r)));
            }
            if let Some(l) = n.loops {
                act.push(format!("loops={}", fmt_rows(l)));
            }
            if !act.is_empty() {
                out.push_str(&format!("  (actual {})", act.join(" ")));
            }
        }
        out.push('\n');
        let pad = "    ".repeat(depth) + "      ";
        if let Some(f) = &n.filter {
            out.push_str(&format!("{pad}Filter: {f}\n"));
        }
        for (k, v) in &n.extra {
            if is_condition_key(k) && k.as_str() != "Output" {
                out.push_str(&format!("{pad}{k}: {v}\n"));
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(t: &str) -> PlanNode {
        PlanNode { node_type: t.into(), ..Default::default() }
    }

    fn sample() -> Plan {
        // Limit
        // └ Hash Join
        //   ├ Seq Scan orders o
        //   └ Hash
        //     └ Index Scan customers
        let mut scan = n("Seq Scan");
        scan.relation = Some("orders".into());
        scan.alias = Some("o".into());
        scan.actual_total_ms = Some(4.0);
        scan.actual_rows = Some(100.0);
        scan.plan_rows = Some(5.0);
        scan.loops = Some(1.0);
        scan.total_cost = Some(30.0);
        scan.filter = Some("(status = 'open')".into());
        scan.extra.insert("Shared Dirtied Blocks".into(), "0".into());
        let mut idx = n("Index Scan");
        idx.relation = Some("customers".into());
        idx.index = Some("customers_pkey".into());
        idx.actual_total_ms = Some(1.0);
        idx.total_cost = Some(8.0);
        let mut hash = n("Hash");
        hash.actual_total_ms = Some(1.5);
        hash.total_cost = Some(9.0);
        hash.children = vec![idx];
        let mut join = n("Hash Join");
        join.actual_total_ms = Some(8.0);
        join.total_cost = Some(60.0);
        join.extra.insert("Hash Cond".into(), "(o.customer_id = c.id)".into());
        join.children = vec![scan, hash];
        let mut limit = n("Limit");
        limit.actual_total_ms = Some(10.0);
        limit.total_cost = Some(61.0);
        limit.children = vec![join];
        Plan {
            root: limit,
            analyzed: true,
            planning_time_ms: Some(0.3),
            execution_time_ms: Some(10.2),
            raw: String::new(),
        }
    }

    #[test]
    fn classification_across_dialects() {
        let f = |t: &str| classify(&n(t));
        assert_eq!(f("Seq Scan"), Family::Scan);
        assert_eq!(f("Bitmap Heap Scan"), Family::Scan);
        assert_eq!(f("Index Only Scan"), Family::Index);
        assert_eq!(f("Bitmap Index Scan"), Family::Index);
        assert_eq!(f("Hash Join"), Family::Join);
        assert_eq!(f("Nested Loop"), Family::Join);
        assert_eq!(f("Nested loop"), Family::Join);
        assert_eq!(f("Incremental Sort"), Family::Sort);
        assert_eq!(f("Filesort"), Family::Sort);
        assert_eq!(f("USE TEMP B-TREE FOR ORDER BY"), Family::Sort);
        assert_eq!(f("Aggregate"), Family::Aggregate);
        assert_eq!(f("Grouping operation"), Family::Aggregate);
        assert_eq!(f("USE TEMP B-TREE FOR GROUP BY"), Family::Aggregate);
        assert_eq!(f("Limit"), Family::Other);
        assert_eq!(f("Hash"), Family::Other);
        assert_eq!(f("ALL"), Family::Scan);
        assert_eq!(f("eq_ref"), Family::Index);
        assert_eq!(f("SEARCH"), Family::Index);
        assert_eq!(f("SCAN"), Family::Scan);
    }

    #[test]
    fn labels() {
        let mut s = n("Seq Scan");
        s.relation = Some("orders".into());
        s.alias = Some("o".into());
        let l = label(&s, DatabaseKind::Postgres);
        assert_eq!((l.title.as_str(), l.subtitle.as_str()), ("Seq Scan", "on orders o"));
        assert_eq!(l.full(), "Seq Scan  on orders o");
        s.alias = Some("orders".into());
        assert_eq!(label(&s, DatabaseKind::Postgres).subtitle, "on orders");

        let mut m = n("ALL");
        m.relation = Some("big".into());
        m.detail = Some("access type: ALL".into());
        let l = label(&m, DatabaseKind::MySql);
        assert_eq!((l.title.as_str(), l.subtitle.as_str()), ("Full table scan", "on big"));

        let mut q = n("SEARCH");
        q.relation = Some("o".into());
        q.index = Some("idx_o".into());
        q.detail = Some("SEARCH o USING INDEX idx_o (customer_id=?)".into());
        assert_eq!(label(&q, DatabaseKind::Sqlite).subtitle, "on o  using idx_o");

        let mut j = n("Hash Join");
        j.detail = Some("Inner join".into());
        assert_eq!(label(&j, DatabaseKind::Postgres).subtitle, "Inner join");
    }

    #[test]
    fn tree_ids_paths_and_subtrees() {
        let t = PlanTree::new(&sample());
        assert_eq!(t.len(), 5);
        assert_eq!(t.path_of(0), &[] as &[usize]);
        assert_eq!(t.path_of(2), &[0, 0]);
        assert_eq!(t.path_of(4), &[0, 1, 0]);
        assert_eq!(t.id_for_path(&[0, 1]), Some(3));
        assert_eq!(t.id_for_path(&[3]), None);
        assert_eq!(t.node(1).subtree_end, 5);
        assert_eq!(t.node(3).subtree_end, 5);
        assert_eq!(t.node(2).subtree_end, 3);
        // path -> id -> path round trips for every node
        for i in 0..t.len() {
            assert_eq!(t.id_for_path(t.path_of(i)), Some(i));
        }
    }

    #[test]
    fn exclusive_metrics_and_shares() {
        let t = PlanTree::new(&sample());
        // Hash Join: 8.0 - (4.0 + 1.5) = 2.5
        assert!((t.node(1).excl_ms.unwrap() - 2.5).abs() < 1e-9);
        assert_eq!(t.max_excl_ms, 4.0);
        assert!((t.time_share(2).unwrap() - 0.4).abs() < 1e-9);
        // Cost: join 60 - (30 + 9) = 21
        assert!((t.node(1).excl_cost.unwrap() - 21.0).abs() < 1e-9);
        assert!(t.heat_share(2).unwrap() > 0.39);
        let mut plan = sample();
        plan.analyzed = false;
        let t = PlanTree::new(&plan);
        assert_eq!(t.total_ms, None);
        assert!(t.time_share(0).is_none());
        assert!(t.heat_share(1).is_some(), "falls back to cost");
    }

    #[test]
    fn collapse_flattening_and_row_mapping() {
        let mut t = PlanTree::new(&sample());
        let ids = |t: &PlanTree| t.visible().iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(&t), vec![0, 1, 2, 3, 4]);
        t.toggle(3);
        assert_eq!(ids(&t), vec![0, 1, 2, 3]);
        assert!(!t.visible()[3].expanded);
        t.toggle(1);
        assert_eq!(ids(&t), vec![0, 1]);
        assert_eq!(row_of(&t.visible(), 4), None);
        // Revealing a hidden node expands only its ancestors.
        t.reveal(4);
        assert_eq!(ids(&t), vec![0, 1, 2, 3, 4]);
        t.collapse_all();
        assert_eq!(ids(&t), vec![0, 1]);
        assert!(t.any_collapsed());
        t.expand_all();
        assert_eq!(row_of(&t.visible(), 4), Some(4));
        // Leaves cannot be collapsed.
        t.toggle(2);
        assert_eq!(ids(&t).len(), 5);
    }

    #[test]
    fn details_group_properties_and_hide_noise() {
        let plan = sample();
        let t = PlanTree::new(&plan);
        let secs = node_sections(&t, &plan, 2);
        let title = |s: &str| secs.iter().find(|x| x.title == s);
        let timing = title("Timing").unwrap();
        assert_eq!(timing.props[0].label, "Exclusive");
        assert!(timing.props[0].value.starts_with("4.0 ms") && timing.props[0].value.contains("40%"));
        let rows = title("Rows").unwrap();
        assert!(
            rows.props.iter().any(|p| p.label == "Misestimate"
                && p.kind == PropKind::Warn
                && p.value.starts_with("▲ 20×"))
        );
        let conds = title("Conditions").unwrap();
        assert_eq!(conds.props[0].kind, PropKind::Sql);
        assert_eq!(conds.props[0].value, "(status = 'open')");
        // Zero-valued block counters are not shown.
        assert!(title("Other").is_none() && title("I/O and memory").is_none());
        let join = node_sections(&t, &plan, 1);
        let c = join.iter().find(|s| s.title == "Conditions").unwrap();
        assert_eq!(c.props[0].label, "Hash Cond");
    }

    #[test]
    fn spills_are_flagged_in_details() {
        let mut plan = sample();
        plan.root.children[0].children[0].extra.insert("Sort Method".into(), "external merge".into());
        plan.root.children[0].children[0].extra.insert("Sort Space Used".into(), "10240".into());
        let t = PlanTree::new(&plan);
        let secs = node_sections(&t, &plan, 2);
        let io = secs.iter().find(|s| s.title == "I/O and memory").unwrap();
        assert!(io.props.iter().any(|p| p.label == "Sort Method" && p.kind == PropKind::Warn));
    }

    #[test]
    fn text_export_is_indented_and_includes_conditions() {
        let text = plan_as_text(&sample(), DatabaseKind::Postgres);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("Planning: 0.30 ms · Execution: 10.2 ms"));
        assert!(lines[1].starts_with("Limit  (cost=61.00"));
        assert!(lines[2].starts_with("  -> Hash Join"), "{}", lines[2]);
        assert!(text.contains("Hash Cond: (o.customer_id = c.id)"));
        assert!(text.contains("Filter: (status = 'open')"));
        assert!(text.contains("(actual time=4.0 ms rows=100 loops=1)"));
        let deepest = lines.iter().find(|l| l.contains("Index Scan")).unwrap();
        assert!(deepest.starts_with("          -> Index Scan"), "{deepest:?}");
    }
}
