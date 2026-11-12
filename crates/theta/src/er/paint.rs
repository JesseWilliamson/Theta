//! Painting a [`theta_diagram::Layout`]: the one place that turns the pure geometry into GPUI quads, paths
//! and shaped text. Everything here is colours and scale; where things go was decided by the layout.

use super::camera::Camera;
use gpui_kit::{
    App, BorderStyle, Bounds, Corners, Font, Hsla, PathBuilder, Pixels, SharedString, TextAlign, TextRun,
    Window, fill, point, px, quad, size,
};
use theta_diagram::layout::{end_marks, marker, more_text};
use theta_diagram::{Graph, Layout, Metrics, clip};

/// The theme colours a diagram uses, resolved once per frame.
#[derive(Clone)]
pub struct Style {
    pub background: Hsla,
    pub box_fill: Hsla,
    pub header_fill: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub edge: Hsla,
    pub accent: Hsla,
    pub key: Hsla,
    pub font: Font,
}

pub struct Scene<'a> {
    pub graph: &'a Graph,
    pub layout: &'a Layout,
    pub metrics: &'a Metrics,
    pub cam: Camera,
    pub selected: Option<usize>,
    /// Per node: adjacent to the selection.
    pub near: &'a [bool],
    pub style: &'a Style,
}

/// Below this zoom a table is a block: its text would be a few pixels high and shaping it is wasted work.
pub const TEXT_ZOOM: f32 = 0.42;
/// The font size, in world units, the metrics were measured at.
pub const BASE_FONT: f32 = 12.0;

fn shape(
    window: &mut Window,
    text: &str,
    size: f32,
    color: Hsla,
    font: &Font,
    bold: bool,
) -> gpui_kit::ShapedLine {
    let mut f = font.clone();
    if bold {
        f.weight = gpui_kit::FontWeight::BOLD;
    }
    let run = TextRun {
        len: text.len(),
        font: f,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(SharedString::from(text.to_string()), px(size), &[run], None)
}

pub fn paint(window: &mut Window, cx: &mut App, bounds: Bounds<Pixels>, s: &Scene) {
    let cam = s.cam;
    let st = s.style;
    let at = |wx: f32, wy: f32| {
        let (x, y) = cam.to_screen(wx, wy);
        bounds.origin + point(px(x), px(y))
    };
    let (vw, vh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (wx0, wy0) = cam.to_world(0.0, 0.0);
    let (wx1, wy1) = cam.to_world(vw, vh);
    let visible = |x: f32, y: f32, w: f32, h: f32| x <= wx1 && x + w >= wx0 && y <= wy1 && y + h >= wy0;
    window.paint_quad(fill(bounds, st.background));

    let line = |window: &mut Window, pts: &[(f32, f32)], width: f32, color: Hsla| {
        let Some(first) = pts.first() else { return };
        let mut b = PathBuilder::stroke(px(width));
        b.move_to(at(first.0, first.1));
        for p in &pts[1..] {
            b.line_to(at(p.0, p.1));
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };

    // Edges first so boxes cover the few places a line touches them. The selection's edges go last of the
    // edges, so they are on top of the grey ones.
    let touches = |e: &theta_diagram::EdgeLayout| {
        s.selected.is_some_and(|n| s.graph.edges[e.edge].from == n || s.graph.edges[e.edge].to == n)
    };
    for pass in [false, true] {
        for e in s.layout.edges.iter().filter(|e| touches(e) == pass) {
            let (minx, maxx) =
                e.points.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
            let (miny, maxy) =
                e.points.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
            if !visible(minx - 20.0, miny - 20.0, maxx - minx + 40.0, maxy - miny + 40.0) {
                continue;
            }
            let ge = &s.graph.edges[e.edge];
            let (color, width) = if pass {
                (st.accent, 2.4 * cam.zoom.clamp(0.6, 1.3))
            } else if s.selected.is_some() {
                (st.edge.opacity(0.35), 1.4 * cam.zoom.clamp(0.6, 1.3))
            } else {
                (st.edge, 1.4 * cam.zoom.clamp(0.6, 1.3))
            };
            if ge.optional {
                for seg in dashes(&e.points, 7.0, 4.0) {
                    line(window, &seg, width, color);
                }
            } else {
                line(window, &e.points, width, color);
            }
            for (a, b) in end_marks(e, ge.unique) {
                line(window, &[a, b], width, color);
            }
            for br in [&e.start_bracket, &e.end_bracket].into_iter().flatten() {
                let (y0, y1) = br.y_range();
                line(
                    window,
                    &[(br.x_edge, y0), (br.x_out, y0), (br.x_out, y1), (br.x_edge, y1)],
                    width,
                    color,
                );
                for t in &br.ticks {
                    line(window, &[(br.x_edge, *t), (br.x_out, *t)], width, color);
                }
            }
        }
    }

    let m = s.metrics;
    let detail = cam.zoom >= TEXT_ZOOM;
    let font_px = BASE_FONT * cam.zoom;
    for b in &s.layout.boxes {
        if !visible(b.x, b.y, b.w, b.h) {
            continue;
        }
        let sel = s.selected == Some(b.node);
        let near = s.near.get(b.node).copied().unwrap_or(false);
        let dim = s.selected.is_some() && !sel && !near;
        let fade = |c: Hsla| if dim { c.opacity(0.45) } else { c };
        let (border, bw) = if sel {
            (st.accent, 2.0)
        } else if near {
            (st.accent.opacity(0.7), 1.5)
        } else {
            (fade(st.border), 1.0)
        };
        let tl = at(b.x, b.y);
        let r = px(4.0 * cam.zoom.max(0.5));
        let bbox = Bounds { origin: tl, size: size(px(b.w * cam.zoom), px(b.h * cam.zoom)) };
        window.paint_quad(quad(bbox, r, fade(st.box_fill), px(bw), border, BorderStyle::Solid));
        let hbox = Bounds {
            origin: tl + point(px(1.0), px(1.0)),
            size: size(px(b.w * cam.zoom - 2.0), px(m.header_h * cam.zoom - 1.0)),
        };
        let top = Corners { top_left: r, top_right: r, bottom_right: px(0.0), bottom_left: px(0.0) };
        window.paint_quad(quad(hbox, top, fade(st.header_fill), px(0.0), st.border, BorderStyle::Solid));
        if !detail {
            continue;
        }
        let node = &s.graph.nodes[b.node];
        let line_h = px(m.row_h * cam.zoom);
        let title = clip(&s.graph.label(b.node), m.max_name_chars + 8);
        let t = shape(window, &title, font_px * 1.05, fade(st.text), &st.font, true);
        let ty = at(b.x + m.pad_x, b.y + (m.header_h - BASE_FONT * 1.4) / 2.0);
        let _ = t.paint(ty, px(BASE_FONT * 1.4 * cam.zoom), TextAlign::Left, None, window, cx);
        let marker_w = theta_diagram::layout::MARKER_CHARS as f32 * m.char_w;
        // A thin divider under the header and a highlight band under the selected table's key rows is more
        // noise than help; rows are separated by spacing alone.
        for row in &b.rows {
            let c = &node.columns[row.column];
            let o = at(b.x + m.pad_x, row.y + (m.row_h - BASE_FONT * 1.3) / 2.0);
            let mk = marker(c);
            if !mk.is_empty() {
                let t = shape(window, mk, font_px * 0.8, fade(st.key), &st.font, true);
                let _ =
                    t.paint(o + point(px(0.0), px(font_px * 0.1)), line_h, TextAlign::Left, None, window, cx);
            }
            let t = shape(window, &clip(&c.name, m.max_name_chars), font_px, fade(st.text), &st.font, c.pk);
            let _ = t.paint(
                o + point(px(marker_w * cam.zoom), px(0.0)),
                line_h,
                TextAlign::Left,
                None,
                window,
                cx,
            );
            let ty = shape(
                window,
                &clip(&c.data_type, m.max_type_chars),
                font_px,
                fade(st.muted),
                &st.font,
                false,
            );
            let right = at(b.x + b.w - m.pad_x, row.y + (m.row_h - BASE_FONT * 1.3) / 2.0);
            let _ = ty.paint(right - point(ty.width, px(0.0)), line_h, TextAlign::Left, None, window, cx);
        }
        if b.hidden > 0 {
            let y = b.y + m.header_h + b.rows.len() as f32 * m.row_h + (m.row_h - BASE_FONT * 1.3) / 2.0;
            let t = shape(window, &more_text(b.hidden), font_px, fade(st.muted), &st.font, false);
            let _ = t.paint(at(b.x + m.pad_x + marker_w, y), line_h, TextAlign::Left, None, window, cx);
        }
    }

    // At overview zoom the selected table's name floats over it, since the boxes carry no text there.
    if !detail
        && let Some(n) = s.selected
        && let Some(b) = s.layout.boxes.get(n)
    {
        let label = s.graph.label(n);
        let t = shape(window, &label, 12.0, st.text, &st.font, true);
        let o = at(b.x, b.y) - point(px(0.0), px(22.0));
        let pad = px(5.0);
        let bg = Bounds { origin: o - point(pad, px(2.0)), size: size(t.width + pad * 2.0, px(20.0)) };
        window.paint_quad(quad(bg, px(3.0), st.header_fill, px(1.0), st.accent, BorderStyle::Solid));
        let _ = t.paint(o, px(16.0), TextAlign::Left, None, window, cx);
    }
}

/// A polyline cut into dashes, as separate polylines.
pub fn dashes(pts: &[(f32, f32)], dash: f32, gap: f32) -> Vec<Vec<(f32, f32)>> {
    let mut out: Vec<Vec<(f32, f32)>> = Vec::new();
    let (mut on, mut left) = (true, dash);
    let mut cur: Vec<(f32, f32)> = Vec::new();
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if len == 0.0 {
            continue;
        }
        let (dx, dy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let mut pos = 0.0;
        if on && cur.is_empty() {
            cur.push(a);
        }
        while pos < len {
            let step = left.min(len - pos);
            pos += step;
            left -= step;
            let p = (a.0 + dx * pos, a.1 + dy * pos);
            if on {
                cur.push(p);
            }
            if left <= 0.0 {
                if on {
                    out.push(std::mem::take(&mut cur));
                }
                on = !on;
                left = if on { dash } else { gap };
                if on {
                    cur.push(p);
                }
            }
        }
    }
    if on && cur.len() > 1 {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::dashes;

    #[test]
    fn dashes_cover_roughly_the_dash_share_of_a_line() {
        let d = dashes(&[(0.0, 0.0), (110.0, 0.0)], 7.0, 4.0);
        assert_eq!(d.len(), 10);
        assert_eq!(d[0], vec![(0.0, 0.0), (7.0, 0.0)]);
        assert_eq!(d[1][0], (11.0, 0.0));
    }

    #[test]
    fn dashes_follow_a_bend() {
        let d = dashes(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 7.0, 1.0);
        // The second dash starts at 8 on the first leg and turns the corner at 10.
        assert!(d.iter().any(|seg| seg.len() >= 3), "{d:?}");
    }
}
