//! Icicle layout: the root spans the full width and each node's children split
//! its span in proportion to their weight. Positions are fractions of the full
//! width so layout is done once per plan and only the pixel filtering depends on
//! the window size.

use crate::model::PlanTree;
use quarry_core::{Plan, PlanNode};

/// What block widths mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlameMetric {
    /// Inclusive measured time.
    Time,
    /// Planner total cost (non-analyzed plans).
    Cost,
    /// Equal share per leaf, for plans that carry neither (SQLite).
    Structure,
}

impl FlameMetric {
    pub fn caption(self) -> &'static str {
        match self {
            FlameMetric::Time => "by time",
            FlameMetric::Cost => "by estimated cost",
            FlameMetric::Structure => "by structure (no timings or costs reported)",
        }
    }
}

fn count_with(n: &PlanNode, has: &dyn Fn(&PlanNode) -> bool) -> (usize, usize) {
    let mut with = 0;
    let mut all = 0;
    n.walk(&mut |x, _| {
        all += 1;
        with += has(x) as usize;
    });
    (with, all)
}

/// Time when the plan was executed and most nodes report it, else cost, else structure.
pub fn choose_metric(plan: &Plan) -> FlameMetric {
    let (timed, all) = count_with(&plan.root, &|n| n.actual_total_ms.is_some());
    let root_time = plan.root.actual_total_ms.or(plan.execution_time_ms).unwrap_or(0.0);
    if plan.analyzed && root_time > 0.0 && timed * 2 >= all && timed > 1 {
        return FlameMetric::Time;
    }
    let (costed, _) = count_with(&plan.root, &|n| n.total_cost.is_some());
    if plan.root.total_cost.is_some_and(|c| c > 0.0) && costed > 1 {
        return FlameMetric::Cost;
    }
    FlameMetric::Structure
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlameRect {
    pub id: usize,
    pub depth: usize,
    /// Left edge and width as fractions of the full width.
    pub x: f32,
    pub w: f32,
}

#[derive(Debug, Clone)]
pub struct FlameLayout {
    pub metric: FlameMetric,
    pub rects: Vec<FlameRect>,
    pub max_depth: usize,
}

fn child_ids(tree: &PlanTree, id: usize) -> Vec<usize> {
    let end = tree.node(id).subtree_end;
    let mut out = Vec::with_capacity(tree.node(id).child_count);
    let mut c = id + 1;
    while c < end {
        out.push(c);
        c = tree.node(c).subtree_end;
    }
    out
}

pub fn layout(plan: &Plan, tree: &PlanTree, metric: FlameMetric) -> FlameLayout {
    let n = tree.len();
    // Per-node weight under the metric; a node without its own figure takes its
    // children's sum so a gap in the data does not erase a whole subtree.
    let mut nodes: Vec<&PlanNode> = Vec::with_capacity(n);
    plan.root.walk(&mut |x, _| nodes.push(x));
    let kids: Vec<Vec<usize>> = (0..n).map(|i| child_ids(tree, i)).collect();
    let mut weight = vec![0.0f64; n];
    for id in (0..n).rev() {
        let own = match metric {
            FlameMetric::Time => nodes[id].actual_total_ms,
            FlameMetric::Cost => nodes[id].total_cost,
            FlameMetric::Structure => None,
        };
        let sum: f64 = kids[id].iter().map(|&c| weight[c]).sum();
        weight[id] = match metric {
            FlameMetric::Structure => {
                if kids[id].is_empty() {
                    1.0
                } else {
                    sum
                }
            }
            _ => own.map(|o| o.max(0.0)).unwrap_or(sum),
        };
    }
    if weight[0] <= 0.0 && metric != FlameMetric::Structure {
        // Nothing measurable under this metric: fall back rather than draw nothing.
        return layout(plan, tree, FlameMetric::Structure);
    }
    let mut rects = Vec::with_capacity(n);
    let mut max_depth = 0;
    let mut stack = vec![(0usize, 0.0f32, 1.0f32)];
    while let Some((id, x, w)) = stack.pop() {
        let depth = tree.node(id).depth;
        max_depth = max_depth.max(depth);
        rects.push(FlameRect { id, depth, x, w });
        // Children can sum to more than the parent (parallel workers, loops);
        // scale them down so they always fit beneath it.
        let sum: f64 = kids[id].iter().map(|&c| weight[c]).sum();
        let denom = weight[id].max(sum);
        if denom <= 0.0 {
            continue;
        }
        let mut cx = x;
        let mut spans = Vec::with_capacity(kids[id].len());
        for &c in &kids[id] {
            let cw = (w as f64 * weight[c] / denom) as f32;
            if cw > 0.0 {
                spans.push((c, cx, cw));
            }
            cx += cw;
        }
        stack.extend(spans.into_iter().rev());
    }
    rects.sort_by_key(|r| r.id);
    FlameLayout { metric, rects, max_depth }
}

impl FlameLayout {
    /// Blocks at least `min_px` wide when the graph is `width_px` wide. A block
    /// is never wider than its parent, so dropping thin ones drops their whole
    /// subtree consistently.
    pub fn visible(&self, width_px: f32, min_px: f32) -> impl Iterator<Item = &FlameRect> {
        self.rects.iter().filter(move |r| r.w * width_px >= min_px)
    }

    /// The block under fractional position `fx` at row `depth`.
    pub fn hit(&self, fx: f32, depth: usize, width_px: f32, min_px: f32) -> Option<usize> {
        self.visible(width_px, min_px).find(|r| r.depth == depth && fx >= r.x && fx < r.x + r.w).map(|r| r.id)
    }

    pub fn rect_for(&self, id: usize) -> Option<&FlameRect> {
        self.rects.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(t: &str, ms: f64, kids: Vec<PlanNode>) -> PlanNode {
        PlanNode {
            node_type: t.into(),
            actual_total_ms: Some(ms),
            total_cost: Some(ms * 10.0),
            children: kids,
            ..Default::default()
        }
    }

    fn plan(root: PlanNode, analyzed: bool) -> Plan {
        Plan { root, analyzed, planning_time_ms: None, execution_time_ms: None, raw: String::new() }
    }

    fn sample() -> Plan {
        plan(
            node("A", 10.0, vec![node("B", 6.0, vec![node("D", 3.0, vec![])]), node("C", 2.0, vec![])]),
            true,
        )
    }

    #[test]
    fn children_split_the_parent_proportionally() {
        let p = sample();
        let t = PlanTree::new(&p);
        let l = layout(&p, &t, FlameMetric::Time);
        assert_eq!(l.metric, FlameMetric::Time);
        assert_eq!(l.max_depth, 2);
        let r = |id| *l.rect_for(id).unwrap();
        assert_eq!((r(0).x, r(0).w), (0.0, 1.0));
        assert!((r(1).w - 0.6).abs() < 1e-6 && r(1).x == 0.0);
        assert!((r(3).x - 0.6).abs() < 1e-6 && (r(3).w - 0.2).abs() < 1e-6);
        // D is half of B and starts where B does.
        assert!((r(2).w - 0.3).abs() < 1e-6 && r(2).x == 0.0);
        assert_eq!(l.rects.len(), 4);
    }

    #[test]
    fn oversubscribed_children_are_scaled_to_fit() {
        // Parallel workers: children report more time than the parent.
        let p = plan(node("P", 4.0, vec![node("X", 4.0, vec![]), node("Y", 4.0, vec![])]), true);
        let t = PlanTree::new(&p);
        let l = layout(&p, &t, FlameMetric::Time);
        let (x, y) = (l.rect_for(1).unwrap(), l.rect_for(2).unwrap());
        assert!((x.w - 0.5).abs() < 1e-6 && (y.x - 0.5).abs() < 1e-6 && (y.x + y.w - 1.0).abs() < 1e-6);
    }

    #[test]
    fn min_width_drops_thin_blocks_and_hit_testing_respects_it() {
        let p = plan(node("A", 1000.0, vec![node("B", 999.0, vec![]), node("tiny", 1.0, vec![])]), true);
        let t = PlanTree::new(&p);
        let l = layout(&p, &t, FlameMetric::Time);
        assert_eq!(l.visible(1000.0, 2.0).count(), 2, "0.1% of 1000px is 1px: dropped");
        assert_eq!(l.visible(1000.0, 0.5).count(), 3);
        assert_eq!(l.hit(0.5, 1, 1000.0, 2.0), Some(1));
        assert_eq!(l.hit(0.9995, 1, 1000.0, 2.0), None);
        assert_eq!(l.hit(0.9995, 1, 1000.0, 0.5), Some(2));
        assert_eq!(l.hit(0.2, 0, 1000.0, 2.0), Some(0));
    }

    #[test]
    fn metric_selection() {
        assert_eq!(choose_metric(&sample()), FlameMetric::Time);
        let mut est = sample();
        est.analyzed = false;
        assert_eq!(choose_metric(&est), FlameMetric::Cost);
        // SQLite: analysed, but only the root has a time and nothing has a cost.
        let mut root =
            PlanNode { node_type: "QUERY".into(), actual_total_ms: Some(5.0), ..Default::default() };
        root.children = vec![PlanNode { node_type: "SCAN".into(), ..Default::default() }];
        assert_eq!(choose_metric(&plan(root, true)), FlameMetric::Structure);
    }

    #[test]
    fn structure_layout_gives_leaves_equal_width() {
        let root = PlanNode {
            node_type: "Q".into(),
            children: vec![
                PlanNode { node_type: "a".into(), ..Default::default() },
                PlanNode {
                    node_type: "b".into(),
                    children: vec![
                        PlanNode { node_type: "c".into(), ..Default::default() },
                        PlanNode { node_type: "d".into(), ..Default::default() },
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let p = plan(root, false);
        let t = PlanTree::new(&p);
        let l = layout(&p, &t, FlameMetric::Structure);
        let w = |id| l.rect_for(id).unwrap().w;
        assert!((w(1) - 1.0 / 3.0).abs() < 1e-6 && (w(2) - 2.0 / 3.0).abs() < 1e-6);
        assert!((w(3) - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn unmeasurable_metric_falls_back_to_structure() {
        let root = PlanNode {
            node_type: "Q".into(),
            children: vec![PlanNode { node_type: "a".into(), ..Default::default() }],
            ..Default::default()
        };
        let p = plan(root, true);
        let t = PlanTree::new(&p);
        assert_eq!(layout(&p, &t, FlameMetric::Time).metric, FlameMetric::Structure);
    }

    #[test]
    fn handles_three_hundred_nodes() {
        // A chain with fan-out: 1 + 19 + 19*15 = 305 nodes.
        let leaf = |i: usize| node(&format!("leaf{i}"), 0.5, vec![]);
        let mid = |i: usize| node(&format!("mid{i}"), 9.0, (0..15).map(leaf).collect());
        let p = plan(node("root", 200.0, (0..19).map(mid).collect()), true);
        let t = PlanTree::new(&p);
        assert!(t.len() > 300);
        let l = layout(&p, &t, FlameMetric::Time);
        assert_eq!(l.rects.len(), t.len());
        assert!(l.rects.iter().all(|r| r.x >= 0.0 && r.x + r.w <= 1.0 + 1e-5));
    }
}
