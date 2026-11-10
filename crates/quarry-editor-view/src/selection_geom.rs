//! Selection geometry: a char range -> per-line spans -> rounded outline polygons.
//!
//! Zed paints one filled path per selection whose corners are rounded and whose line
//! boundaries "merge": where two consecutive lines are both selected the outline follows the
//! union of their rectangles instead of drawing independent boxes. That needs two steps, both
//! here and both GPUI-free so they can be unit-tested:
//!
//! 1. [`decompose`] turns `from..to` into one `(line, col_start, col_end, through_newline)`
//!    per line (the newline counts as selected, drawn as a small extension past the last
//!    character).
//! 2. [`outline`] takes the pixel spans of consecutive lines and builds the outline of their
//!    union as path commands with rounded corners.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineSel {
    pub line: usize,
    /// First selected char column on this line.
    pub col_start: usize,
    /// One past the last selected char column (the line length when the newline is included).
    pub col_end: usize,
    /// The selection continues past this line's newline.
    pub newline: bool,
}

/// Decompose the half-open char range `from..to` into line selections, restricted to the
/// `visible` lines (the lines outside are skipped, which is why this takes closures: a 50 000
/// line selection must not touch 50 000 lines).
///
/// A selection ending exactly at the start of a line stops on the previous line, marked as
/// running through its newline, matching how Helix and Zed draw `x` line selections.
pub fn decompose(
    from: usize,
    to: usize,
    visible: Range<usize>,
    line_of: &dyn Fn(usize) -> usize,
    line_start: &dyn Fn(usize) -> usize,
    line_len: &dyn Fn(usize) -> usize,
) -> Vec<LineSel> {
    if from >= to {
        return Vec::new();
    }
    let first = line_of(from);
    let mut last = line_of(to);
    let mut ends_in_newline = false;
    if last > first && line_start(last) == to {
        last -= 1;
        ends_in_newline = true;
    }
    let lo = first.max(visible.start);
    let hi = last.min(visible.end.saturating_sub(1));
    if visible.is_empty() || lo > hi {
        return Vec::new();
    }
    (lo..=hi)
        .map(|line| {
            let start = line_start(line);
            let len = line_len(line);
            let col_start = if line == first { from - start } else { 0 };
            let (col_end, newline) =
                if line < last || ends_in_newline { (len, true) } else { ((to - start).min(len), false) };
            LineSel { line, col_start: col_start.min(len), col_end, newline }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    Move(Pt),
    Line(Pt),
    /// Quadratic curve: `to`, control point.
    Curve(Pt, Pt),
}

/// Pixel extent of one line of a selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub x0: f32,
    pub x1: f32,
}

/// Split consecutive spans into runs that overlap horizontally: the union of two rows that
/// only touch at a corner is two shapes, not one self-intersecting polygon.
fn connected_runs(spans: &[Span]) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut start = 0;
    for i in 1..spans.len() {
        let (a, b) = (spans[i - 1], spans[i]);
        let overlap = a.x1.min(b.x1) - a.x0.max(b.x0);
        if overlap <= 0.0 {
            runs.push(start..i);
            start = i;
        }
    }
    if start < spans.len() {
        runs.push(start..spans.len());
    }
    runs
}

fn polygon(spans: &[Span], y0: f32, lh: f32) -> Vec<Pt> {
    let n = spans.len();
    let y = |i: usize| y0 + i as f32 * lh;
    let mut v: Vec<Pt> = Vec::with_capacity(n * 4);
    // Right edge, top to bottom.
    v.push(Pt { x: spans[0].x1, y: y(0) });
    for i in 0..n {
        v.push(Pt { x: spans[i].x1, y: y(i + 1) });
        if i + 1 < n {
            v.push(Pt { x: spans[i + 1].x1, y: y(i + 1) });
        }
    }
    // Left edge, bottom to top.
    v.push(Pt { x: spans[n - 1].x0, y: y(n) });
    for i in (0..n).rev() {
        v.push(Pt { x: spans[i].x0, y: y(i) });
        if i > 0 {
            v.push(Pt { x: spans[i - 1].x0, y: y(i) });
        }
    }
    // Drop repeated points and collinear middles (equal x on consecutive rows).
    let mut out: Vec<Pt> = Vec::with_capacity(v.len());
    for p in v {
        if out.last().is_some_and(|q| q.x == p.x && q.y == p.y) {
            continue;
        }
        out.push(p);
    }
    if out.len() > 1 && out[0] == out[out.len() - 1] {
        out.pop();
    }
    let mut cleaned: Vec<Pt> = Vec::with_capacity(out.len());
    for i in 0..out.len() {
        let (p, c, nx) = (out[(i + out.len() - 1) % out.len()], out[i], out[(i + 1) % out.len()]);
        let collinear = (p.x == c.x && c.x == nx.x) || (p.y == c.y && c.y == nx.y);
        if !collinear {
            cleaned.push(c);
        }
    }
    cleaned
}

fn toward(from: Pt, to: Pt, dist: f32) -> Pt {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= f32::EPSILON {
        return from;
    }
    Pt { x: from.x + dx / len * dist, y: from.y + dy / len * dist }
}

fn round_polygon(poly: &[Pt], radius: f32) -> Vec<PathCmd> {
    let n = poly.len();
    let dist = |a: Pt, b: Pt| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let mut cmds = Vec::with_capacity(n * 2 + 1);
    for i in 0..n {
        let (prev, cur, next) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
        let r = radius.min(dist(prev, cur) / 2.0).min(dist(cur, next) / 2.0).max(0.0);
        let a = toward(cur, prev, r);
        let b = toward(cur, next, r);
        if i == 0 {
            cmds.push(PathCmd::Move(a));
        } else {
            cmds.push(PathCmd::Line(a));
        }
        cmds.push(PathCmd::Curve(b, cur));
    }
    cmds.push(PathCmd::Line(match cmds[0] {
        PathCmd::Move(p) => p,
        _ => unreachable!("outline starts with a Move"),
    }));
    cmds
}

/// Outline(s) of the union of consecutive rows `spans` (row `i` occupies
/// `y0 + i*lh .. y0 + (i+1)*lh`), corners rounded with `radius`. Each inner `Vec` is one
/// closed shape.
pub fn outline(spans: &[Span], y0: f32, lh: f32, radius: f32) -> Vec<Vec<PathCmd>> {
    if spans.is_empty() {
        return Vec::new();
    }
    connected_runs(spans)
        .into_iter()
        .filter_map(|run| {
            let poly = polygon(&spans[run.clone()], y0 + run.start as f32 * lh, lh);
            (poly.len() >= 3).then(|| round_polygon(&poly, radius))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<&'static str> {
        vec!["select 1", "", "from users u", "where x"]
    }

    fn starts(ls: &[&str]) -> Vec<usize> {
        let mut s = 0;
        ls.iter()
            .map(|l| {
                let r = s;
                s += l.chars().count() + 1;
                r
            })
            .collect()
    }

    fn run(from: usize, to: usize, vis: Range<usize>) -> Vec<LineSel> {
        let ls = lines();
        let st = starts(&ls);
        let line_of = |c: usize| st.iter().rposition(|&s| s <= c).unwrap();
        decompose(from, to, vis, &line_of, &|l| st[l], &|l| ls[l].chars().count())
    }

    #[test]
    fn single_line_selection() {
        let r = run(2, 5, 0..4);
        assert_eq!(r, vec![LineSel { line: 0, col_start: 2, col_end: 5, newline: false }]);
    }

    #[test]
    fn multi_line_selection_includes_newlines() {
        // "lect 1\n\nfrom" : from col 2 of line 0 to col 4 of line 2
        let to = starts(&lines())[2] + 4;
        let r = run(2, to, 0..4);
        assert_eq!(
            r,
            vec![
                LineSel { line: 0, col_start: 2, col_end: 8, newline: true },
                LineSel { line: 1, col_start: 0, col_end: 0, newline: true },
                LineSel { line: 2, col_start: 0, col_end: 4, newline: false },
            ]
        );
    }

    #[test]
    fn selection_ending_at_line_start_runs_through_the_newline() {
        // `x` in Helix on line 0 selects 0..9 (through the newline).
        let r = run(0, 9, 0..4);
        assert_eq!(r, vec![LineSel { line: 0, col_start: 0, col_end: 8, newline: true }]);
    }

    #[test]
    fn only_visible_lines_are_produced() {
        let r = run(0, starts(&lines())[3] + 3, 1..3);
        assert_eq!(r.iter().map(|l| l.line).collect::<Vec<_>>(), vec![1, 2]);
        assert!(run(0, 5, 2..4).is_empty());
        assert!(run(3, 3, 0..4).is_empty());
    }

    #[test]
    fn rectangle_outline_has_four_rounded_corners() {
        let shapes = outline(&[Span { x0: 10.0, x1: 50.0 }], 0.0, 20.0, 3.0);
        assert_eq!(shapes.len(), 1);
        let curves = shapes[0].iter().filter(|c| matches!(c, PathCmd::Curve(..))).count();
        assert_eq!(curves, 4);
        assert!(matches!(shapes[0][0], PathCmd::Move(_)));
    }

    #[test]
    fn equal_edges_merge_into_one_rectangle() {
        // Two rows with identical extents are one 4-corner shape, not 8 corners.
        let shapes = outline(&[Span { x0: 0.0, x1: 40.0 }, Span { x0: 0.0, x1: 40.0 }], 0.0, 20.0, 3.0);
        assert_eq!(shapes.len(), 1);
        let curves = shapes[0].iter().filter(|c| matches!(c, PathCmd::Curve(..))).count();
        assert_eq!(curves, 4);
    }

    #[test]
    fn staircase_has_concave_corners() {
        // First row long, second row short on the right: an L shape = 6 corners.
        let shapes = outline(&[Span { x0: 0.0, x1: 80.0 }, Span { x0: 0.0, x1: 30.0 }], 0.0, 20.0, 3.0);
        assert_eq!(shapes.len(), 1);
        let curves = shapes[0].iter().filter(|c| matches!(c, PathCmd::Curve(..))).count();
        assert_eq!(curves, 6);
    }

    #[test]
    fn disjoint_rows_split_into_two_shapes() {
        let shapes = outline(&[Span { x0: 50.0, x1: 90.0 }, Span { x0: 0.0, x1: 30.0 }], 0.0, 20.0, 3.0);
        assert_eq!(shapes.len(), 2);
    }

    #[test]
    fn radius_is_limited_by_short_edges() {
        // A 4px-wide span cannot take a 10px radius: the curve control offsets stay within half the edge.
        let shapes = outline(&[Span { x0: 0.0, x1: 4.0 }], 0.0, 20.0, 10.0);
        for c in &shapes[0] {
            if let PathCmd::Move(p) | PathCmd::Line(p) | PathCmd::Curve(p, _) = c {
                assert!(p.x >= -0.001 && p.x <= 4.001 && p.y >= -0.001 && p.y <= 20.001);
            }
        }
    }

    #[test]
    fn outline_stays_inside_the_union_bounds() {
        let spans = [Span { x0: 5.0, x1: 80.0 }, Span { x0: 0.0, x1: 60.0 }, Span { x0: 0.0, x1: 20.0 }];
        for shape in outline(&spans, 100.0, 20.0, 3.0) {
            for c in shape {
                let (PathCmd::Move(p) | PathCmd::Line(p) | PathCmd::Curve(p, _)) = c;
                assert!((0.0..=80.0).contains(&p.x) && (100.0..=160.0).contains(&p.y), "{p:?}");
            }
        }
    }
}
