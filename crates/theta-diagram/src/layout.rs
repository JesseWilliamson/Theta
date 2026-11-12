//! Layered layout and orthogonal edge routing.
//!
//! The pipeline (Sugiyama's, in the order the steps appear below), and why each step is what it is:
//!
//! 1. **Components.** Tables that are not connected by any foreign key never share a layering, so each
//!    connected component is laid out on its own and the pieces are then packed in rows. One tangled picture
//!    of 100 unrelated tables would otherwise be a single enormous column.
//! 2. **Cycles.** Foreign keys can form cycles (`a -> b -> a`). The Eades-Lin-Smyth greedy ordering picks a
//!    node sequence that leaves few edges pointing backwards; those are drawn in the opposite direction for
//!    layering only. Every tie is broken by the lowest node index, so the result is a function of the input.
//! 3. **Layers.** Longest path from the referenced ("one") tables, so a table always sits to the right of every
//!    table it references; then tables that only others reference are pulled next to their first referrer
//!    so their edges stay short.
//! 4. **Virtual nodes.** An edge that spans several layers gets one invisible slot per layer it crosses. The
//!    slot keeps the edge's horizontal run clear of every box (boxes in a layer never overlap the slot) and
//!    lets ordering treat all edges as spanning exactly one layer.
//! 5. **Order within layers.** Barycenter sweeps down and up, then adjacent swaps while they remove crossings;
//!    the ordering with the fewest crossings seen wins. Crossings are counted with a Fenwick tree
//!    (`O(e log e)` per layer pair), so 100+ tables stay fast.
//! 6. **Vertical position.** Each item wants to sit where its edges' ports line up with its neighbours'
//!    (so an edge is a straight line when it can be). Resolving overlaps under that wish is isotonic
//!    regression, solved exactly by pool-adjacent-violators.
//! 7. **Channels.** The gaps between layers hold the vertical parts of edges. Each vertical part gets a track
//!    (greedy interval colouring), and the channel is as wide as its tracks need.
//!
//! Coordinates are `f32` "world" units; the viewer scales them. The text metrics are a [`Metrics`] value
//! (a monospace advance), because this crate has no fonts.

use crate::model::Graph;

#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    /// Advance of one character of the monospace font at zoom 1.
    pub char_w: f32,
    pub row_h: f32,
    pub header_h: f32,
    pub pad_x: f32,
    pub pad_bottom: f32,
    pub min_width: f32,
    /// Vertical space between two boxes of a layer.
    pub node_gap: f32,
    /// Height of the slot an edge reserves in a layer it crosses.
    pub slot_h: f32,
    /// Narrowest gap between two layers.
    pub min_channel: f32,
    /// Distance between two vertical edge parts that share a channel.
    pub track_gap: f32,
    /// Columns shown per table before the rest collapses into "+N more"; key columns are always shown.
    pub max_rows: usize,
    pub max_name_chars: usize,
    pub max_type_chars: usize,
    /// Space between packed components, and around the whole picture.
    pub component_gap: f32,
    pub margin: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            char_w: 7.2,
            row_h: 20.0,
            header_h: 28.0,
            pad_x: 10.0,
            pad_bottom: 4.0,
            min_width: 130.0,
            node_gap: 26.0,
            slot_h: 12.0,
            min_channel: 56.0,
            track_gap: 12.0,
            max_rows: 30,
            max_name_chars: 36,
            max_type_chars: 24,
            component_gap: 64.0,
            margin: 32.0,
        }
    }
}

/// Width, in characters, reserved left of a column name for its `PK` / `FK` marker.
pub const MARKER_CHARS: usize = 6;

/// The marker drawn before a column's name.
pub fn marker(c: &crate::model::GraphColumn) -> &'static str {
    match (c.pk, c.fk) {
        (true, true) => "PK FK",
        (true, false) => "PK",
        (false, true) => "FK",
        _ => "",
    }
}

/// The summary row of a table whose column list was cut short.
pub fn more_text(hidden: usize) -> String {
    format!("+{hidden} more column{}", if hidden == 1 { "" } else { "s" })
}

/// A column name as drawn (cut to `max` characters with an ellipsis).
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowLayout {
    /// Index into the node's `columns`.
    pub column: usize,
    /// Top of the row, in diagram coordinates.
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxLayout {
    pub node: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rows: Vec<RowLayout>,
    /// Columns left out of `rows`; they are summarised as one "+N more columns" row after the last.
    pub hidden: usize,
}

impl BoxLayout {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// The marker drawn where a composite foreign key attaches: a vertical bar beside the box spanning the
/// columns, with a tick to each of them. The edge itself leaves from the middle of the bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Bracket {
    pub x_edge: f32,
    pub x_out: f32,
    pub ticks: Vec<f32>,
}

impl Bracket {
    pub fn y_range(&self) -> (f32, f32) {
        (
            self.ticks.iter().cloned().fold(f32::MAX, f32::min),
            self.ticks.iter().cloned().fold(f32::MIN, f32::max),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeLayout {
    /// Index into `Graph::edges`.
    pub edge: usize,
    /// Axis-aligned polyline from one box to the other (or from a box back to itself).
    pub points: Vec<(f32, f32)>,
    /// The foreign key's own table ("many" side) is at the first point; otherwise at the last.
    pub owner_at_start: bool,
    pub start_bracket: Option<Bracket>,
    pub end_bracket: Option<Bracket>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// One per graph node, in node order.
    pub boxes: Vec<BoxLayout>,
    /// One per graph edge, in edge order.
    pub edges: Vec<EdgeLayout>,
    pub width: f32,
    pub height: f32,
}

impl Layout {
    /// Nodes left to right, then top to bottom: the order Tab walks through.
    pub fn reading_order(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.boxes.len()).collect();
        v.sort_by(|&a, &b| {
            let (p, q) = (&self.boxes[a], &self.boxes[b]);
            // Boxes of one layer are centred in their column, so their left edges differ a little: compare
            // the column by rounding to the nearest 8 units.
            let col = |x: f32| (x / 8.0).round() as i64;
            col(p.x).cmp(&col(q.x)).then(p.y.total_cmp(&q.y)).then(a.cmp(&b))
        });
        v
    }
}

/// Short line segments that end an edge: a crow's foot at the table that owns the foreign key (a single bar
/// when the key is unique, so at most one row points at the parent) and a bar at the referenced table.
pub fn end_marks(e: &EdgeLayout, unique: bool) -> Vec<((f32, f32), (f32, f32))> {
    let pts = &e.points;
    if pts.len() < 2 {
        return Vec::new();
    }
    let dir = |from: (f32, f32), to: (f32, f32)| {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let l = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
        (dx / l, dy / l)
    };
    let ends =
        [(pts[0], dir(pts[0], pts[1])), (pts[pts.len() - 1], dir(pts[pts.len() - 1], pts[pts.len() - 2]))];
    let mut out = Vec::new();
    for (i, (p, a)) in ends.into_iter().enumerate() {
        let n = (-a.1, a.0);
        let owner = (i == 0) == e.owner_at_start;
        let at = |t: f32, s: f32| (p.0 + a.0 * t + n.0 * s, p.1 + a.1 * t + n.1 * s);
        if owner && !unique {
            out.push((at(10.0, 0.0), at(0.0, 6.0)));
            out.push((at(10.0, 0.0), at(0.0, 0.0)));
            out.push((at(10.0, 0.0), at(0.0, -6.0)));
        } else {
            out.push((at(8.0, 5.0), at(8.0, -5.0)));
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ sizes

struct Size {
    w: f32,
    h: f32,
    rows: Vec<usize>,
    hidden: usize,
}

fn size_of(g: &Graph, n: usize, m: &Metrics) -> Size {
    let node = &g.nodes[n];
    let total = node.columns.len();
    let rows: Vec<usize> = if total <= m.max_rows {
        (0..total).collect()
    } else {
        let mut keep: Vec<bool> = node.columns.iter().map(|c| c.pinned()).collect();
        let mut shown = keep.iter().filter(|k| **k).count();
        for k in keep.iter_mut() {
            if shown >= m.max_rows {
                break;
            }
            if !*k {
                *k = true;
                shown += 1;
            }
        }
        (0..total).filter(|&i| keep[i]).collect()
    };
    let hidden = total - rows.len();
    let mut chars = 0usize;
    for &r in &rows {
        let c = &node.columns[r];
        let name = clip(&c.name, m.max_name_chars).chars().count();
        let ty = clip(&c.data_type, m.max_type_chars).chars().count();
        // marker column, name, a gap, type
        chars = chars.max(MARKER_CHARS + name + 3 + ty);
    }
    if hidden > 0 {
        chars = chars.max(MARKER_CHARS + more_text(hidden).len());
    }
    let title = clip(&g.label(n), m.max_name_chars + 8).chars().count() as f32 * m.char_w * 1.1;
    let w = (chars as f32 * m.char_w + 2.0 * m.pad_x).max(title + 2.0 * m.pad_x).max(m.min_width);
    let lines = rows.len() + usize::from(hidden > 0);
    let h = m.header_h + lines as f32 * m.row_h + m.pad_bottom;
    Size { w, h, rows, hidden }
}

// ------------------------------------------------------------------------------------------------ pipeline

/// Lay out the whole graph. Deterministic: the same graph and metrics give the same result.
pub fn layout(g: &Graph, m: &Metrics) -> Layout {
    let n = g.nodes.len();
    let sizes: Vec<Size> = (0..n).map(|i| size_of(g, i, m)).collect();

    // Connected components over foreign keys between distinct tables.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for e in g.edges.iter().filter(|e| e.from != e.to) {
        let (a, b) = (find(&mut parent, e.from), find(&mut parent, e.to));
        if a != b {
            parent[a.max(b)] = a.min(b);
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot_of_root = vec![usize::MAX; n];
    for i in 0..n {
        let r = find(&mut parent, i);
        if slot_of_root[r] == usize::MAX {
            slot_of_root[r] = groups.len();
            groups.push(Vec::new());
        }
        groups[slot_of_root[r]].push(i);
    }
    // Components with edges first, biggest first; then the tables that stand alone, alphabetically.
    groups.sort_by(|a, b| {
        (a.len() == 1).cmp(&(b.len() == 1)).then(b.len().cmp(&a.len())).then(a[0].cmp(&b[0]))
    });

    let mut locals: Vec<Local> = Vec::new();
    for grp in &groups {
        let edges: Vec<usize> = (0..g.edges.len())
            .filter(|&e| grp.contains(&g.edges[e].from) && grp.contains(&g.edges[e].to))
            .collect();
        locals.push(layout_component(g, m, &sizes, grp, &edges));
    }

    // Shelf packing.
    let area: f32 = locals.iter().map(|l| (l.w + m.component_gap) * (l.h + m.component_gap)).sum();
    let widest = locals.iter().map(|l| l.w).fold(0.0, f32::max);
    let target = widest.max((area * 1.6).sqrt());
    let (mut x, mut y, mut row_h, mut total_w) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut singles_started = false;
    let mut out = Layout {
        boxes: vec![BoxLayout { node: 0, x: 0.0, y: 0.0, w: 0.0, h: 0.0, rows: Vec::new(), hidden: 0 }; n],
        edges: vec![
            EdgeLayout {
                edge: 0,
                points: Vec::new(),
                owner_at_start: true,
                start_bracket: None,
                end_bracket: None
            };
            g.edges.len()
        ],
        width: 0.0,
        height: 0.0,
    };
    for (gi, l) in locals.into_iter().enumerate() {
        let single = groups[gi].len() == 1;
        if single && !singles_started {
            singles_started = true;
            if x > 0.0 {
                y += row_h + m.component_gap;
                x = 0.0;
                row_h = 0.0;
            }
        }
        if x > 0.0 && x + l.w > target {
            y += row_h + m.component_gap;
            x = 0.0;
            row_h = 0.0;
        }
        for mut b in l.boxes {
            b.x += x + m.margin;
            b.y += y + m.margin;
            for r in &mut b.rows {
                r.y += y + m.margin;
            }
            let node = b.node;
            out.boxes[node] = b;
        }
        for mut e in l.edges {
            let shift = |p: &mut (f32, f32)| {
                p.0 += x + m.margin;
                p.1 += y + m.margin;
            };
            e.points.iter_mut().for_each(shift);
            for br in [&mut e.start_bracket, &mut e.end_bracket].into_iter().flatten() {
                br.x_edge += x + m.margin;
                br.x_out += x + m.margin;
                br.ticks.iter_mut().for_each(|t| *t += y + m.margin);
            }
            let idx = e.edge;
            out.edges[idx] = e;
        }
        total_w = total_w.max(x + l.w);
        row_h = row_h.max(l.h);
        x += l.w + m.component_gap;
    }
    out.width = total_w + 2.0 * m.margin;
    out.height = y + row_h + 2.0 * m.margin;
    if n == 0 {
        out.width = 0.0;
        out.height = 0.0;
    }
    out
}

struct Local {
    boxes: Vec<BoxLayout>,
    edges: Vec<EdgeLayout>,
    w: f32,
    h: f32,
}

/// One virtual-graph item: a real table or an edge's slot in a layer.
struct Vn {
    /// Global node index for a real table.
    node: Option<usize>,
    layer: usize,
    h: f32,
}

/// Neighbour link: `(other item, other's port offset, my port offset)`; the desired top of mine is
/// `top[other] + other_off - my_off`.
type Link = (usize, f32, f32);

struct Chain {
    edge: usize,
    /// Items from the left table to the right one, both included.
    items: Vec<usize>,
    /// Port offsets from the top of the left and the right table.
    off_left: f32,
    off_right: f32,
    /// The FK owner is the left table.
    owner_left: bool,
}

fn layout_component(g: &Graph, m: &Metrics, sizes: &[Size], nodes: &[usize], edge_ids: &[usize]) -> Local {
    let k = nodes.len();
    let local_of = |global: usize| nodes.iter().position(|&x| x == global).expect("edge inside component");

    // -- cycles and layers
    let mut pairs: Vec<(usize, usize)> = edge_ids
        .iter()
        .filter(|&&e| g.edges[e].from != g.edges[e].to)
        .map(|&e| (local_of(g.edges[e].to), local_of(g.edges[e].from))) // referenced -> referencing
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    let order = greedy_order(k, &pairs);
    let mut rank = vec![0usize; k];
    for (i, &v) in order.iter().enumerate() {
        rank[v] = i;
    }
    let mut dag: Vec<(usize, usize)> =
        pairs.iter().map(|&(p, c)| if rank[p] < rank[c] { (p, c) } else { (c, p) }).collect();
    dag.sort_unstable();
    dag.dedup();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); k];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); k];
    for &(a, b) in &dag {
        preds[b].push(a);
        succs[a].push(b);
    }
    let mut layer = vec![0usize; k];
    for &v in &order {
        layer[v] = preds[v].iter().map(|&u| layer[u] + 1).max().unwrap_or(0);
    }
    for v in 0..k {
        if preds[v].is_empty()
            && let Some(first) = succs[v].iter().map(|&s| layer[s]).min()
        {
            layer[v] = first - 1;
        }
    }
    let mut used: Vec<usize> = layer.clone();
    used.sort_unstable();
    used.dedup();
    for l in layer.iter_mut() {
        *l = used.binary_search(l).expect("layer in use");
    }
    let n_layers = used.len().max(1);

    // -- virtual graph
    let mut vns: Vec<Vn> =
        (0..k).map(|i| Vn { node: Some(nodes[i]), layer: layer[i], h: sizes[nodes[i]].h }).collect();
    let mut chains: Vec<Chain> = Vec::new();
    let mut loops: Vec<usize> = Vec::new();
    for &e in edge_ids {
        let ed = &g.edges[e];
        if ed.from == ed.to {
            loops.push(e);
            continue;
        }
        let (f, t) = (local_of(ed.from), local_of(ed.to));
        let (left, right) = if layer[f] < layer[t] { (f, t) } else { (t, f) };
        let mut items = vec![left];
        for l in layer[left] + 1..layer[right] {
            vns.push(Vn { node: None, layer: l, h: m.slot_h });
            items.push(vns.len() - 1);
        }
        items.push(right);
        let off = |local: usize, cols: &[String]| port(g, m, &sizes[nodes[local]], nodes[local], cols).0;
        let (cols_left, cols_right) =
            if left == f { (&ed.from_cols, &ed.to_cols) } else { (&ed.to_cols, &ed.from_cols) };
        chains.push(Chain {
            edge: e,
            items,
            off_left: off(left, cols_left),
            off_right: off(right, cols_right),
            owner_left: left == f,
        });
    }
    let mut up: Vec<Vec<Link>> = vec![Vec::new(); vns.len()];
    let mut down: Vec<Vec<Link>> = vec![Vec::new(); vns.len()];
    for c in &chains {
        let last = c.items.len() - 1;
        for i in 0..last {
            let (u, v) = (c.items[i], c.items[i + 1]);
            let off_u = if i == 0 { c.off_left } else { m.slot_h / 2.0 };
            let off_v = if i + 1 == last { c.off_right } else { m.slot_h / 2.0 };
            up[v].push((u, off_u, off_v));
            down[u].push((v, off_v, off_u));
        }
    }

    // -- order within layers
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); n_layers];
    for v in 0..k {
        layers[layer[v]].push(v); // real tables by index
    }
    let mut pos = vec![0usize; vns.len()];
    for items in &layers {
        for (i, &v) in items.iter().enumerate() {
            pos[v] = i;
        }
    }
    // Dummies join their layer in an order given by the first sweep.
    for v in k..vns.len() {
        layers[vns[v].layer].push(v);
    }
    for items in &layers {
        for (i, &v) in items.iter().enumerate() {
            pos[v] = i;
        }
    }
    reduce_crossings(&mut layers, &mut pos, &up, &down);

    // -- vertical positions
    let mut top = vec![0.0f32; vns.len()];
    let gap =
        |a: usize, b: usize| if vns[a].node.is_none() && vns[b].node.is_none() { 6.0 } else { m.node_gap };
    for l in &layers {
        let mut y = 0.0;
        for (i, &v) in l.iter().enumerate() {
            if i > 0 {
                y += gap(l[i - 1], v);
            }
            top[v] = y;
            y += vns[v].h;
        }
    }
    for _ in 0..4 {
        for items in layers.iter().skip(1) {
            place_layer(items, &mut top, &up, &vns, &gap);
        }
        for l in (0..n_layers.saturating_sub(1)).rev() {
            place_layer(&layers[l], &mut top, &down, &vns, &gap);
        }
    }
    let min_top = top.iter().cloned().fold(f32::MAX, f32::min);
    if min_top.is_finite() {
        top.iter_mut().for_each(|t| *t -= min_top);
    }
    let height = layers.iter().filter_map(|l| l.last()).map(|&v| top[v] + vns[v].h).fold(0.0, f32::max);

    // -- channels
    let col_w: Vec<f32> = (0..n_layers)
        .map(|l| layers[l].iter().filter_map(|&v| vns[v].node.map(|nd| sizes[nd].w)).fold(0.0, f32::max))
        .collect();
    // Vertical segments per channel: (lo, hi, chain, link index).
    let mut segs: Vec<Vec<(f32, f32, usize, usize)>> = vec![Vec::new(); n_layers];
    let end_y = |v: usize, off: f32| top[v] + off;
    for (ci, c) in chains.iter().enumerate() {
        let last = c.items.len() - 1;
        for i in 0..last {
            let (u, v) = (c.items[i], c.items[i + 1]);
            let yu = end_y(u, if i == 0 { c.off_left } else { m.slot_h / 2.0 });
            let yv = end_y(v, if i + 1 == last { c.off_right } else { m.slot_h / 2.0 });
            if (yu - yv).abs() >= 0.05 {
                segs[vns[u].layer].push((yu.min(yv), yu.max(yv), ci, i));
            }
        }
    }
    let mut track_of: Vec<Vec<usize>> = chains.iter().map(|c| vec![usize::MAX; c.items.len() - 1]).collect();
    let mut n_tracks = vec![0usize; n_layers];
    for ch in 0..n_layers {
        let s = &mut segs[ch];
        s.sort_by(|a, b| {
            a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3))
        });
        let mut last_hi: Vec<f32> = Vec::new();
        for &(lo, hi, ci, li) in s.iter() {
            let t = match last_hi.iter().position(|&h| h + 4.0 < lo) {
                Some(t) => t,
                None => {
                    last_hi.push(f32::MIN);
                    last_hi.len() - 1
                }
            };
            last_hi[t] = hi;
            track_of[ci][li] = t;
        }
        n_tracks[ch] = last_hi.len();
    }
    // Self-loops stick out of the right side of their box; the channel after the box makes room.
    let loop_d = |i: usize| 14.0 + 8.0 * i as f32;
    let mut loops_of: Vec<Vec<usize>> = vec![Vec::new(); k];
    for &e in &loops {
        loops_of[local_of(g.edges[e].from)].push(e);
    }
    let mut reserve = vec![0.0f32; n_layers];
    for v in 0..k {
        if !loops_of[v].is_empty() {
            let spare = (col_w[layer[v]] - sizes[nodes[v]].w) / 2.0;
            let need = loop_d(loops_of[v].len() - 1) + 4.0 - spare;
            reserve[layer[v]] = reserve[layer[v]].max(need.max(0.0));
        }
    }
    let mut chan_w = vec![0.0f32; n_layers];
    for ch in 0..n_layers {
        chan_w[ch] = if ch + 1 == n_layers {
            reserve[ch]
        } else {
            reserve[ch] + m.min_channel.max((n_tracks[ch] + 1) as f32 * m.track_gap)
        };
    }
    let mut col_x = vec![0.0f32; n_layers];
    let mut chan_x = vec![0.0f32; n_layers];
    let mut x = 0.0;
    for l in 0..n_layers {
        col_x[l] = x;
        x += col_w[l];
        chan_x[l] = x;
        x += chan_w[l];
    }
    let width = x;
    let track_x = |ch: usize, t: usize| {
        let avail = chan_w[ch] - reserve[ch];
        chan_x[ch] + reserve[ch] + (t + 1) as f32 * avail / (n_tracks[ch] + 1) as f32
    };

    // -- boxes
    let mut boxes: Vec<BoxLayout> = Vec::with_capacity(k);
    for v in 0..k {
        let nd = nodes[v];
        let s = &sizes[nd];
        let bx = col_x[layer[v]] + (col_w[layer[v]] - s.w) / 2.0;
        let by = top[v];
        boxes.push(BoxLayout {
            node: nd,
            x: bx,
            y: by,
            w: s.w,
            h: s.h,
            rows: s
                .rows
                .iter()
                .enumerate()
                .map(|(i, &c)| RowLayout { column: c, y: by + m.header_h + i as f32 * m.row_h })
                .collect(),
            hidden: s.hidden,
        });
    }

    // -- edges
    let mut edges: Vec<EdgeLayout> = Vec::new();
    for (ci, c) in chains.iter().enumerate() {
        let ed = &g.edges[c.edge];
        let (lv, rv) = (c.items[0], *c.items.last().expect("chain has two ends"));
        let (lb, rb) = (&boxes[lv], &boxes[rv]);
        let last = c.items.len() - 1;
        let y_at = |i: usize| {
            top[c.items[i]]
                + if i == 0 {
                    c.off_left
                } else if i == last {
                    c.off_right
                } else {
                    m.slot_h / 2.0
                }
        };
        let mut pts = vec![(lb.right(), y_at(0))];
        for i in 0..last {
            let t = track_of[ci][i];
            if t != usize::MAX {
                let tx = track_x(vns[c.items[i]].layer, t);
                pts.push((tx, y_at(i)));
                pts.push((tx, y_at(i + 1)));
            }
        }
        pts.push((rb.x, y_at(last)));
        let (cols_l, cols_r) =
            if c.owner_left { (&ed.from_cols, &ed.to_cols) } else { (&ed.to_cols, &ed.from_cols) };
        edges.push(EdgeLayout {
            edge: c.edge,
            points: simplify(pts),
            owner_at_start: c.owner_left,
            start_bracket: bracket(g, m, &boxes[lv], cols_l, 1.0),
            end_bracket: bracket(g, m, &boxes[rv], cols_r, -1.0),
        });
    }
    for v in 0..k {
        for (i, &e) in loops_of[v].iter().enumerate() {
            let ed = &g.edges[e];
            let b = &boxes[v];
            let mut y1 = b.y + port(g, m, &sizes[nodes[v]], nodes[v], &ed.from_cols).0;
            let mut y2 = b.y + port(g, m, &sizes[nodes[v]], nodes[v], &ed.to_cols).0;
            if (y1 - y2).abs() < 4.0 {
                y1 -= 5.0;
                y2 += 5.0;
            }
            let d = b.right() + loop_d(i);
            edges.push(EdgeLayout {
                edge: e,
                points: vec![(b.right(), y1), (d, y1), (d, y2), (b.right(), y2)],
                owner_at_start: true,
                start_bracket: bracket(g, m, b, &ed.from_cols, 1.0),
                end_bracket: bracket(g, m, b, &ed.to_cols, 1.0),
            });
        }
    }
    Local { boxes, edges, w: width, h: height }
}

/// Port offset from the top of a box for a set of columns, and the visible row indices behind it. One
/// column: the middle of its row. Several: the middle of the span they cover. None visible: the header.
fn port(g: &Graph, m: &Metrics, size: &Size, node: usize, cols: &[String]) -> (f32, Vec<usize>) {
    let rows: Vec<usize> = cols
        .iter()
        .filter_map(|c| size.rows.iter().position(|&r| g.nodes[node].columns[r].name.eq_ignore_ascii_case(c)))
        .collect();
    let centre = |r: usize| m.header_h + r as f32 * m.row_h + m.row_h / 2.0;
    match (rows.iter().min(), rows.iter().max()) {
        (Some(&lo), Some(&hi)) => ((centre(lo) + centre(hi)) / 2.0, rows),
        _ => (m.header_h / 2.0, rows),
    }
}

fn bracket(g: &Graph, m: &Metrics, b: &BoxLayout, cols: &[String], side: f32) -> Option<Bracket> {
    let mut ys: Vec<f32> = cols
        .iter()
        .filter_map(|c| {
            b.rows
                .iter()
                .find(|r| g.nodes[b.node].columns[r.column].name.eq_ignore_ascii_case(c))
                .map(|r| r.y + m.row_h / 2.0)
        })
        .collect();
    ys.sort_by(|a, b| a.total_cmp(b));
    ys.dedup();
    if ys.len() < 2 {
        return None;
    }
    // `side` is +1 on a box's right edge and -1 on its left edge.
    let x_edge = if side > 0.0 { b.right() } else { b.x };
    Some(Bracket { x_edge, x_out: x_edge + side * 6.0, ticks: ys })
}

/// Snap nearly equal coordinates, then drop repeated and collinear points.
fn simplify(mut pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    for i in 1..pts.len() {
        let prev = pts[i - 1];
        if (pts[i].0 - prev.0).abs() < 0.05 {
            pts[i].0 = prev.0;
        }
        if (pts[i].1 - prev.1).abs() < 0.05 {
            pts[i].1 = prev.1;
        }
    }
    pts.dedup();
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for p in pts {
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            if (a.0 == b.0 && b.0 == p.0) || (a.1 == b.1 && b.1 == p.1) {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

// ------------------------------------------------------------------------------------------------ ordering

/// Eades-Lin-Smyth: a node sequence in which few edges point backwards.
fn greedy_order(k: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut outd = vec![0i64; k];
    let mut ind = vec![0i64; k];
    let mut out_adj: Vec<Vec<usize>> = vec![Vec::new(); k];
    let mut in_adj: Vec<Vec<usize>> = vec![Vec::new(); k];
    for &(a, b) in edges {
        outd[a] += 1;
        ind[b] += 1;
        out_adj[a].push(b);
        in_adj[b].push(a);
    }
    let mut gone = vec![false; k];
    let (mut left, mut right): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let mut remaining = k;
    let remove = |v: usize, gone: &mut [bool], outd: &mut [i64], ind: &mut [i64]| {
        gone[v] = true;
        for &w in &out_adj[v] {
            ind[w] -= 1;
        }
        for &u in &in_adj[v] {
            outd[u] -= 1;
        }
    };
    while remaining > 0 {
        if let Some(v) = (0..k).find(|&v| !gone[v] && outd[v] == 0) {
            remove(v, &mut gone, &mut outd, &mut ind);
            right.push(v);
        } else if let Some(v) = (0..k).find(|&v| !gone[v] && ind[v] == 0) {
            remove(v, &mut gone, &mut outd, &mut ind);
            left.push(v);
        } else {
            let v = (0..k)
                .filter(|&v| !gone[v])
                .max_by_key(|&v| (outd[v] - ind[v], std::cmp::Reverse(v)))
                .expect("a node remains");
            remove(v, &mut gone, &mut outd, &mut ind);
            left.push(v);
        }
        remaining -= 1;
    }
    right.reverse();
    left.extend(right);
    left
}

fn total_crossings(layers: &[Vec<usize>], pos: &[usize], up: &[Vec<Link>]) -> usize {
    let mut total = 0;
    for items in layers.iter().skip(1) {
        // Edges as (position above, position below), counted as inversions with a Fenwick tree.
        let mut es: Vec<(usize, usize)> =
            items.iter().flat_map(|&v| up[v].iter().map(move |&(u, _, _)| (pos[u], pos[v]))).collect();
        es.sort_unstable();
        let width = items.len() + 1;
        let mut tree = vec![0usize; width + 1];
        for (seen, &(_, b)) in es.iter().enumerate() {
            // Earlier edges with a larger lower position cross this one.
            let mut i = b + 1;
            let mut le = 0;
            while i > 0 {
                le += tree[i];
                i -= i & i.wrapping_neg();
            }
            total += seen - le;
            let mut i = b + 1;
            while i <= width {
                tree[i] += 1;
                i += i & i.wrapping_neg();
            }
        }
    }
    total
}

fn reduce_crossings(layers: &mut [Vec<usize>], pos: &mut [usize], up: &[Vec<Link>], down: &[Vec<Link>]) {
    let n = layers.len();
    let reindex = |layers: &[Vec<usize>], pos: &mut [usize]| {
        for l in layers {
            for (i, &v) in l.iter().enumerate() {
                pos[v] = i;
            }
        }
    };
    let sweep = |layers: &mut [Vec<usize>], pos: &mut [usize], l: usize, adj: &[Vec<Link>]| {
        let mut keyed: Vec<(f64, usize, usize)> = layers[l]
            .iter()
            .map(|&v| {
                let key = if adj[v].is_empty() {
                    pos[v] as f64
                } else {
                    adj[v].iter().map(|&(u, _, _)| pos[u] as f64).sum::<f64>() / adj[v].len() as f64
                };
                (key, pos[v], v)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (i, &(_, _, v)) in keyed.iter().enumerate() {
            layers[l][i] = v;
            pos[v] = i;
        }
    };
    let mut best = total_crossings(layers, pos, up);
    let mut best_layers = layers.to_vec();
    let mut stale = 0;
    for _ in 0..24 {
        if best == 0 || stale >= 4 {
            break;
        }
        for l in 1..n {
            sweep(layers, pos, l, up);
        }
        for l in (0..n.saturating_sub(1)).rev() {
            sweep(layers, pos, l, down);
        }
        transpose(layers, pos, up, down);
        let c = total_crossings(layers, pos, up);
        if c < best {
            best = c;
            best_layers = layers.to_vec();
            stale = 0;
        } else {
            stale += 1;
        }
    }
    layers.clone_from_slice(&best_layers);
    reindex(layers, pos);
}

/// Swap neighbours while that removes crossings with the layers on both sides.
fn transpose(layers: &mut [Vec<usize>], pos: &mut [usize], up: &[Vec<Link>], down: &[Vec<Link>]) {
    let pair = |pos: &[usize], u: usize, v: usize| -> (usize, usize) {
        // (crossings as they are, crossings if swapped) for u left of v.
        let (mut now, mut swapped) = (0, 0);
        for adj in [up, down] {
            for &(a, _, _) in &adj[u] {
                for &(b, _, _) in &adj[v] {
                    match pos[a].cmp(&pos[b]) {
                        std::cmp::Ordering::Greater => now += 1,
                        std::cmp::Ordering::Less => swapped += 1,
                        std::cmp::Ordering::Equal => {}
                    }
                }
            }
        }
        (now, swapped)
    };
    for _ in 0..6 {
        let mut improved = false;
        for items in layers.iter_mut() {
            for i in 0..items.len().saturating_sub(1) {
                let (u, v) = (items[i], items[i + 1]);
                let (now, swapped) = pair(pos, u, v);
                if swapped < now {
                    items.swap(i, i + 1);
                    pos[u] = i + 1;
                    pos[v] = i;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
}

/// Move the items of one layer to where their links want them, without overlap and in the same order.
fn place_layer(
    order: &[usize],
    top: &mut [f32],
    links: &[Vec<Link>],
    vns: &[Vn],
    gap: &impl Fn(usize, usize) -> f32,
) {
    if order.is_empty() {
        return;
    }
    let mut offset = Vec::with_capacity(order.len());
    let mut o = 0.0f32;
    for (i, &v) in order.iter().enumerate() {
        if i > 0 {
            o += gap(order[i - 1], v);
        }
        offset.push(o);
        o += vns[v].h;
    }
    // Wish minus offset; pool adjacent violators makes the sequence non-decreasing, which is exactly "no
    // overlap in this order".
    let wish: Vec<f32> = order
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let d = if links[v].is_empty() {
                top[v]
            } else {
                links[v].iter().map(|&(u, ou, ov)| top[u] + ou - ov).sum::<f32>() / links[v].len() as f32
            };
            d - offset[i]
        })
        .collect();
    let mut blocks: Vec<(f32, usize)> = Vec::new(); // (sum, count)
    for &w in &wish {
        blocks.push((w, 1));
        while blocks.len() >= 2 {
            let (b, a) = (blocks[blocks.len() - 1], blocks[blocks.len() - 2]);
            if a.0 / a.1 as f32 > b.0 / b.1 as f32 {
                blocks.pop();
                let l = blocks.len() - 1;
                blocks[l] = (a.0 + b.0, a.1 + b.1);
            } else {
                break;
            }
        }
    }
    let mut i = 0;
    for (sum, count) in blocks {
        let z = sum / count as f32;
        for _ in 0..count {
            top[order[i]] = z + offset[i];
            i += 1;
        }
    }
}
