//! The Chart tab: a chart of the active result, drawn with GPUI's own painting primitives.
//!
//! All decisions (which columns by default, grouping and aggregation, bucketing, ticks, scales, hit
//! testing, the readout text) are in `theta-chart`, tested without a window. This file owns the controls,
//! turns a [`theta_chart::Layout`] into quads, paths and shaped text, and forwards the pointer and the
//! keyboard to the layout's hit testing.
//!
//! **Why not an existing chart widget.** `gpui-component` ships one, but its axes, legends and hover
//! behaviour are fixed, it has no notion of aggregation or bucketing, and it would put the numbers on screen
//! where nothing here could test them; the plan view and the map already paint with `canvas`, so the chart
//! does too.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, BorderStyle, Bounds, Context, Entity, FocusHandle, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseMoveEvent, ParentElement as _, PathBuilder, Pixels, Render, SharedString,
    Styled as _, Subscription, TextAlign, TextRun, Window, canvas, div, fill, point, px, quad, size,
};
use std::cell::Cell;
use std::f64::consts::FRAC_PI_2;
use std::rc::Rc;
use std::sync::Arc;
use theta_chart::layout::{Rect, XScale};
use theta_chart::spec::{is_numeric, is_plottable};
use theta_chart::{Aggregate, ChartData, ChartKind, ChartSpec, Layout, Readout, Target, build, suggest};
use theta_core::ResultSet;
use theta_ui::{CHROME_TEXT_SIZE, EmptyState};

const NONE: &str = "None";

pub struct ChartPanel {
    result: Option<Arc<ResultSet>>,
    spec: Option<ChartSpec>,
    data: Result<ChartData, String>,
    x_select: Entity<SelectState<Vec<String>>>,
    series_select: Entity<SelectState<Vec<String>>>,
    agg_select: Entity<SelectState<Vec<String>>>,
    /// Plot size at the last paint, so pointer events can be mapped without a second layout pass over the
    /// window; the layout itself is recomputed from it (it is cheap and pure).
    plot_bounds: Rc<Cell<Bounds<Pixels>>>,
    hover: Option<Target>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl ChartPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let select = |cx: &mut Context<Self>, window: &mut Window, items: Vec<String>| {
            cx.new(|cx| SelectState::new(items, Some(IndexPath::new(0)), window, cx))
        };
        let x_select = select(cx, window, Vec::new());
        let series_select = select(cx, window, vec![NONE.to_string()]);
        let agg_select = select(cx, window, Aggregate::ALL.iter().map(|a| a.label().to_string()).collect());
        let subs = vec![
            cx.subscribe_in(&x_select, window, |this: &mut Self, _, _: &SelectEvent<Vec<String>>, _, cx| {
                this.controls_changed(cx)
            }),
            cx.subscribe_in(
                &series_select,
                window,
                |this: &mut Self, _, _: &SelectEvent<Vec<String>>, _, cx| this.controls_changed(cx),
            ),
            cx.subscribe_in(
                &agg_select,
                window,
                |this: &mut Self, _, _: &SelectEvent<Vec<String>>, _, cx| this.controls_changed(cx),
            ),
        ];
        Self {
            result: None,
            spec: None,
            data: Err(String::new()),
            x_select,
            series_select,
            agg_select,
            plot_bounds: Rc::new(Cell::new(Bounds::default())),
            hover: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.result = None;
        self.spec = None;
        self.data = Err(String::new());
        self.hover = None;
        cx.notify();
    }

    /// Show `rs`. The same result again (switching tabs back and forth) keeps the user's choices.
    pub fn set_result(&mut self, rs: Arc<ResultSet>, window: &mut Window, cx: &mut Context<Self>) {
        if self.result.as_ref().is_some_and(|r| Arc::ptr_eq(r, &rs)) {
            return;
        }
        self.result = Some(rs.clone());
        self.spec = suggest(&rs);
        self.hover = None;
        let x_items: Vec<String> =
            rs.columns.iter().filter(|c| is_plottable(c)).map(|c| c.name.clone()).collect();
        let mut series_items = vec![NONE.to_string()];
        series_items.extend(x_items.iter().cloned());
        let (x_at, s_at, a_at) = match &self.spec {
            Some(s) => (
                self.plottable_position(&rs, s.x).unwrap_or(0),
                s.series.and_then(|c| self.plottable_position(&rs, c)).map_or(0, |i| i + 1),
                Aggregate::ALL.iter().position(|a| *a == s.aggregate).unwrap_or(0),
            ),
            None => (0, 0, 0),
        };
        self.x_select.update(cx, |s, cx| {
            s.set_items(x_items, window, cx);
            s.set_selected_index(Some(IndexPath::new(x_at)), window, cx);
        });
        self.series_select.update(cx, |s, cx| {
            s.set_items(series_items, window, cx);
            s.set_selected_index(Some(IndexPath::new(s_at)), window, cx);
        });
        self.agg_select.update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(a_at)), window, cx));
        self.rebuild(cx);
    }

    /// Position of column `col` among the plottable columns (the order of the X and series dropdowns).
    fn plottable_position(&self, rs: &ResultSet, col: usize) -> Option<usize> {
        rs.columns.iter().enumerate().filter(|(_, c)| is_plottable(c)).position(|(i, _)| i == col)
    }

    fn plottable_column(&self, rs: &ResultSet, position: usize) -> Option<usize> {
        rs.columns.iter().enumerate().filter(|(_, c)| is_plottable(c)).nth(position).map(|(i, _)| i)
    }

    /// The dropdowns changed: read them back into the spec.
    fn controls_changed(&mut self, cx: &mut Context<Self>) {
        let Some(rs) = self.result.clone() else { return };
        let x = self.x_select.read(cx).selected_index(cx).and_then(|i| self.plottable_column(&rs, i.row));
        // Row 0 of the series dropdown is "None"; the columns follow it.
        let series = self
            .series_select
            .read(cx)
            .selected_index(cx)
            .and_then(|i| i.row.checked_sub(1))
            .and_then(|p| self.plottable_column(&rs, p));
        let agg = self
            .agg_select
            .read(cx)
            .selected_index(cx)
            .map(|i| Aggregate::ALL[i.row.min(Aggregate::ALL.len() - 1)]);
        let Some(spec) = self.spec.as_mut() else { return };
        if let Some(x) = x {
            spec.x = x;
        }
        spec.series = series;
        if let Some(a) = agg {
            spec.aggregate = a;
        }
        self.rebuild(cx);
    }

    fn set_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if let Some(s) = self.spec.as_mut() {
            s.kind = kind;
            self.rebuild(cx);
        }
    }

    fn toggle_y(&mut self, col: usize, cx: &mut Context<Self>) {
        let Some(s) = self.spec.as_mut() else { return };
        if let Some(i) = s.ys.iter().position(|&c| c == col) {
            // The last one stays: a chart with no Y is not a chart.
            if s.ys.len() > 1 {
                s.ys.remove(i);
            }
        } else {
            s.ys.push(col);
            s.ys.sort_unstable();
        }
        self.rebuild(cx);
    }

    fn cycle_aggregate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.spec.as_mut() else { return };
        let at = Aggregate::ALL.iter().position(|a| *a == s.aggregate).unwrap_or(0);
        s.aggregate = Aggregate::ALL[(at + 1) % Aggregate::ALL.len()];
        let at = (at + 1) % Aggregate::ALL.len();
        self.agg_select.update(cx, |sel, cx| sel.set_selected_index(Some(IndexPath::new(at)), window, cx));
        self.rebuild(cx);
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.hover = None;
        self.data = match (&self.result, &self.spec) {
            (Some(rs), Some(spec)) => build(rs, spec).map_err(|e| e.to_string()),
            (Some(_), None) => {
                Err("Nothing in this result can be charted: it has no text, date or number column.".into())
            }
            _ => Err(String::new()),
        };
        cx.notify();
    }

    fn layout(&self) -> Option<Layout> {
        let data = self.data.as_ref().ok()?;
        let b = self.plot_bounds.get();
        let (w, h) = (f64::from(f32::from(b.size.width)), f64::from(f32::from(b.size.height)));
        (w > 10.0 && h > 10.0).then(|| Layout::new(data, w, h))
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let b = self.plot_bounds.get();
        let (x, y) = (
            f64::from(f32::from(ev.position.x - b.origin.x)),
            f64::from(f32::from(ev.position.y - b.origin.y)),
        );
        let hit = match (self.layout(), self.data.as_ref().ok()) {
            (Some(l), Some(d)) => l.hit(d, x, y),
            _ => None,
        };
        if hit != self.hover {
            self.hover = hit;
            cx.notify();
        }
    }

    /// Arrow keys walk the points: left / right along the series, up / down across series.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        if m.control || m.platform || m.alt {
            return;
        }
        let Ok(data) = &self.data else {
            return;
        };
        let key = ev.keystroke.key.as_str();
        let kind = match key {
            "1" => Some(ChartKind::Bar),
            "2" => Some(ChartKind::Line),
            "3" => Some(ChartKind::Area),
            "4" => Some(ChartKind::Scatter),
            "5" => Some(ChartKind::Pie),
            _ => None,
        };
        if let Some(k) = kind {
            self.set_kind(k, cx);
            cx.stop_propagation();
            return;
        }
        if key == "a" && !m.shift {
            self.cycle_aggregate(window, cx);
            cx.stop_propagation();
            return;
        }
        let (dp, ds): (isize, isize) = match key {
            "left" | "h" => (-1, 0),
            "right" | "l" => (1, 0),
            "up" | "k" => (0, -1),
            "down" | "j" => (0, 1),
            "escape" => {
                if self.hover.take().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                }
                return;
            }
            _ => return,
        };
        let cur = self.hover.unwrap_or(Target { series: 0, point: 0 });
        let series = (cur.series as isize + ds).clamp(0, data.series.len() as isize - 1) as usize;
        let n = data.series[series].points.len();
        let point = if self.hover.is_none() {
            0
        } else {
            (cur.point as isize + dp).clamp(0, n as isize - 1) as usize
        };
        // Moving to another series keeps the x: the nearest point by x position.
        let point = if ds != 0 {
            let x = data.series[cur.series].points.get(cur.point).map_or(0.0, |p| p.x);
            data.series[series]
                .points
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1.x - x).abs().total_cmp(&(b.1.x - x).abs()))
                .map_or(0, |(i, _)| i)
        } else {
            point
        };
        self.hover = Some(Target { series, point });
        cx.notify();
        cx.stop_propagation();
    }
}

/// One colour per series, from the theme's own hues so a theme change recolours the chart.
fn palette(cx: &gpui_kit::App) -> Vec<Hsla> {
    let t = cx.theme();
    vec![t.blue, t.green, t.yellow, t.magenta, t.cyan, t.red]
}

fn series_color(colors: &[Hsla], i: usize) -> Hsla {
    colors[i % colors.len()]
}

fn shape(
    window: &mut Window,
    text: &str,
    size: Pixels,
    color: Hsla,
    font: &gpui_kit::Font,
) -> gpui_kit::ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(SharedString::from(text.to_string()), size, &[run], None)
}

struct Style {
    fg: Hsla,
    muted: Hsla,
    grid: Hsla,
    axis: Hsla,
    panel: Hsla,
    border: Hsla,
    colors: Vec<Hsla>,
    font: gpui_kit::Font,
}

const LINE_H: f32 = 14.0;

/// Paint `data` into `bounds` according to `layout`.
fn paint_chart(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    bounds: Bounds<Pixels>,
    data: &ChartData,
    layout: &Layout,
    hover: Option<Target>,
    st: &Style,
) {
    let at = |x: f64, y: f64| bounds.origin + point(px(x as f32), px(y as f32));
    let rect = |r: Rect| Bounds { origin: at(r.x, r.y), size: size(px(r.w as f32), px(r.h as f32)) };
    let label_size = px(11.0);
    let stroke = |window: &mut Window, pts: &[(f64, f64)], width: f32, color: Hsla| {
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
    let text = |window: &mut Window, cx: &mut gpui_kit::App, s: &str, x: f64, y: f64, color: Hsla| -> f64 {
        let line = shape(window, s, label_size, color, &st.font);
        let w = f64::from(f32::from(line.width));
        let _ = line.paint(at(x, y), px(LINE_H), TextAlign::Left, None, window, cx);
        w
    };
    let measure = |window: &mut Window, s: &str| {
        f64::from(f32::from(shape(window, s, label_size, st.fg, &st.font).width))
    };
    let p = layout.plot;
    let readout = hover.and_then(|t| layout.readout(data, t));

    if data.kind == ChartKind::Pie {
        paint_pie(window, cx, bounds, layout, hover, st, &text, &measure);
    } else {
        // Highlight the hovered slot of a bar chart behind the bars.
        if let (Some(t), XScale::Band(b), ChartKind::Bar) = (hover, &layout.x, data.kind) {
            let i = data.series[t.series].points[t.point].x as usize;
            window.paint_quad(fill(
                rect(Rect { x: b.start(i), y: p.y, w: b.width(), h: p.h }),
                st.grid.opacity(0.35),
            ));
        }
        for t in &layout.y_ticks {
            stroke(window, &[(p.x, t.at), (p.x + p.w, t.at)], 1.0, st.grid);
            let w = measure(window, &t.label);
            text(window, cx, &t.label, p.x - 8.0 - w, t.at - 7.0, st.muted);
        }
        stroke(window, &[(p.x, p.y + p.h), (p.x + p.w, p.y + p.h)], 1.0, st.axis);
        stroke(window, &[(p.x, p.y), (p.x, p.y + p.h)], 1.0, st.axis);
        if layout.baseline < p.y + p.h - 1.0 {
            stroke(window, &[(p.x, layout.baseline), (p.x + p.w, layout.baseline)], 1.0, st.axis);
        }
        // x labels, dropping any that would touch the previous one.
        let mut last_end = f64::NEG_INFINITY;
        for t in &layout.x_ticks {
            let w = measure(window, &t.label);
            let x = (t.at - w / 2.0).clamp(p.x - 24.0, p.x + p.w + 12.0 - w);
            if x < last_end + 10.0 {
                continue;
            }
            last_end = x + w;
            stroke(window, &[(t.at, p.y + p.h), (t.at, p.y + p.h + 4.0)], 1.0, st.axis);
            text(window, cx, &t.label, x, p.y + p.h + 8.0, st.muted);
        }

        let n = data.series.len();
        match data.kind {
            ChartKind::Bar => {
                for (si, s) in data.series.iter().enumerate() {
                    let color = series_color(&st.colors, si);
                    for pt in &s.points {
                        let r = layout.bar_rect(si, n, pt);
                        let hot =
                            hover.is_some_and(|t| t.series == si && data.series[si].points[t.point] == *pt);
                        window.paint_quad(quad(
                            rect(r),
                            px(2.),
                            color.opacity(if hot { 1.0 } else { 0.85 }),
                            px(0.),
                            color,
                            BorderStyle::Solid,
                        ));
                    }
                }
            }
            ChartKind::Line | ChartKind::Area => {
                for (si, s) in data.series.iter().enumerate() {
                    let color = series_color(&st.colors, si);
                    let pts: Vec<(f64, f64)> = s.points.iter().map(|q| layout.point_px(q)).collect();
                    if data.kind == ChartKind::Area && pts.len() >= 2 {
                        let mut b = PathBuilder::fill();
                        b.move_to(at(pts[0].0, layout.baseline));
                        for q in &pts {
                            b.line_to(at(q.0, q.1));
                        }
                        b.line_to(at(pts[pts.len() - 1].0, layout.baseline));
                        b.close();
                        if let Ok(path) = b.build() {
                            window.paint_path(path, color.opacity(0.22));
                        }
                    }
                    stroke(window, &pts, 2.0, color);
                    if pts.len() <= 80 {
                        for q in &pts {
                            let d = 3.0;
                            window.paint_quad(quad(
                                rect(Rect { x: q.0 - d, y: q.1 - d, w: d * 2.0, h: d * 2.0 }),
                                px(3.),
                                color,
                                px(1.5),
                                st.panel,
                                BorderStyle::Solid,
                            ));
                        }
                    }
                }
            }
            ChartKind::Scatter => {
                for (si, s) in data.series.iter().enumerate() {
                    let color = series_color(&st.colors, si);
                    for q in &s.points {
                        let (x, y) = layout.point_px(q);
                        let d = 3.5;
                        window.paint_quad(quad(
                            rect(Rect { x: x - d, y: y - d, w: d * 2.0, h: d * 2.0 }),
                            px(4.),
                            color.opacity(0.7),
                            px(1.0),
                            color,
                            BorderStyle::Solid,
                        ));
                    }
                }
            }
            ChartKind::Pie => {}
        }
        // Guide and markers for the hovered position.
        if let (Some(r), Some(t)) = (&readout, hover) {
            if matches!(data.kind, ChartKind::Line | ChartKind::Area) {
                stroke(window, &[(r.anchor.0, p.y), (r.anchor.0, p.y + p.h)], 1.0, st.muted.opacity(0.6));
            }
            for (si, _, _) in &r.rows {
                let s = &data.series[*si];
                let pt = if *si == t.series {
                    s.points[t.point]
                } else {
                    // The point this row's value belongs to: same x for categories, nearest otherwise.
                    let x = data.series[t.series].points[t.point].x;
                    *s.points
                        .iter()
                        .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
                        .unwrap_or(&s.points[0])
                };
                if data.kind != ChartKind::Bar {
                    let (x, y) = layout.point_px(&pt);
                    window.paint_quad(quad(
                        rect(Rect { x: x - 5.0, y: y - 5.0, w: 10.0, h: 10.0 }),
                        px(5.),
                        series_color(&st.colors, *si),
                        px(2.0),
                        st.fg,
                        BorderStyle::Solid,
                    ));
                }
            }
        }
    }

    if let Some(r) = readout {
        paint_readout(window, cx, bounds, &r, st, &text, &measure);
    }
}

type TextFn<'a> = &'a dyn Fn(&mut Window, &mut gpui_kit::App, &str, f64, f64, Hsla) -> f64;
type MeasureFn<'a> = &'a dyn Fn(&mut Window, &str) -> f64;

#[allow(clippy::too_many_arguments)]
fn paint_pie(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    bounds: Bounds<Pixels>,
    layout: &Layout,
    hover: Option<Target>,
    st: &Style,
    text: TextFn,
    measure: MeasureFn,
) {
    let Some((cx0, cy0, r)) = layout.pie else { return };
    let at = |x: f64, y: f64| bounds.origin + point(px(x as f32), px(y as f32));
    for sl in &layout.slices {
        let color = series_color(&st.colors, sl.point);
        let hot = hover.is_some_and(|t| t.point == sl.point);
        let grow = if hot { 4.0 } else { 0.0 };
        let mut b = PathBuilder::fill();
        b.move_to(at(cx0, cy0));
        let steps = (((sl.end - sl.start) / 0.04).ceil() as usize).max(2);
        for i in 0..=steps {
            let a = sl.start + (sl.end - sl.start) * i as f64 / steps as f64 - FRAC_PI_2;
            b.line_to(at(cx0 + (r + grow) * a.cos(), cy0 + (r + grow) * a.sin()));
        }
        b.close();
        if let Ok(path) = b.build() {
            window.paint_path(path, color.opacity(if hot { 1.0 } else { 0.9 }));
        }
        // A thin gap between slices, in the panel colour.
        let mut e = PathBuilder::stroke(px(1.5));
        e.move_to(at(cx0, cy0));
        let a = sl.start - FRAC_PI_2;
        e.line_to(at(cx0 + (r + grow) * a.cos(), cy0 + (r + grow) * a.sin()));
        if let Ok(path) = e.build() {
            window.paint_path(path, st.panel);
        }
        if sl.fraction >= 0.07 {
            let label = format!("{:.0}%", sl.fraction * 100.0);
            let mid = (sl.start + sl.end) / 2.0 - FRAC_PI_2;
            let w = measure(window, &label);
            text(
                window,
                cx,
                &label,
                cx0 + r * 0.62 * mid.cos() - w / 2.0,
                cy0 + r * 0.62 * mid.sin() - 7.0,
                st.panel,
            );
        }
    }
}

fn paint_readout(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    bounds: Bounds<Pixels>,
    r: &Readout,
    st: &Style,
    text: TextFn,
    measure: MeasureFn,
) {
    let pad = 8.0;
    let rows: Vec<String> = r.rows.iter().map(|(_, n, v)| format!("{n}: {v}")).collect();
    let width = std::iter::once(measure(window, &r.title))
        .chain(rows.iter().map(|s| measure(window, s) + 14.0))
        .fold(0.0f64, f64::max)
        + pad * 2.0;
    let height = (rows.len() + 1) as f64 * f64::from(LINE_H) + pad * 2.0 - 2.0;
    let (bw, bh) = (f64::from(f32::from(bounds.size.width)), f64::from(f32::from(bounds.size.height)));
    // Beside the point, flipped to its other side when it would leave the plot.
    let mut x = r.anchor.0 + 14.0;
    if x + width > bw - 4.0 {
        x = r.anchor.0 - 14.0 - width;
    }
    let y = (r.anchor.1 - height / 2.0).clamp(4.0, (bh - height - 4.0).max(4.0));
    let at = |x: f64, y: f64| bounds.origin + point(px(x as f32), px(y as f32));
    window.paint_quad(quad(
        Bounds { origin: at(x, y), size: size(px(width as f32), px(height as f32)) },
        px(5.),
        st.panel,
        px(1.),
        st.border,
        BorderStyle::Solid,
    ));
    text(window, cx, &r.title, x + pad, y + pad - 1.0, st.muted);
    for (i, ((si, _, _), line)) in r.rows.iter().zip(&rows).enumerate() {
        let ty = y + pad - 1.0 + (i + 1) as f64 * f64::from(LINE_H);
        window.paint_quad(fill(
            Bounds { origin: at(x + pad, ty + 4.0), size: size(px(8.), px(8.)) },
            series_color(&st.colors, *si),
        ));
        text(window, cx, line, x + pad + 14.0, ty, st.fg);
    }
}

impl Render for ChartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(rs) = self.result.clone() else {
            return EmptyState::new(gpui_kit::assets::IconName::ChartColumn, "Nothing to chart")
                .description("Run a query; its result is charted here.")
                .into_any_element();
        };
        let Some(spec) = self.spec.clone() else {
            return EmptyState::new(gpui_kit::assets::IconName::ChartColumn, "Nothing to chart")
                .description("This result has no text, date or number column to plot.")
                .into_any_element();
        };
        let colors = palette(cx);
        let kinds = h_flex().gap_0p5().children(ChartKind::ALL.into_iter().enumerate().map(|(i, k)| {
            Button::new(("chart-kind", i))
                .small()
                .ghost()
                .label(k.label())
                .selected(spec.kind == k)
                .on_click(cx.listener(move |this, _, _, cx| this.set_kind(k, cx)))
        }));
        let y_chips = h_flex().gap_0p5().flex_wrap().children(
            rs.columns
                .iter()
                .enumerate()
                .filter(|(_, c)| is_numeric(c) || spec.aggregate == Aggregate::Count)
                .map(|(i, c)| {
                    let on = spec.ys.contains(&i);
                    Button::new(("chart-y", i))
                        .small()
                        .ghost()
                        .label(c.name.clone())
                        .selected(on)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_y(i, cx)))
                }),
        );
        let caption =
            |t: &'static str| div().text_size(gpui_kit::px(11.)).text_color(theme.muted_foreground).child(t);
        let controls = h_flex()
            .px_2()
            .py_1()
            .gap_3()
            .flex_wrap()
            .items_center()
            .text_size(CHROME_TEXT_SIZE)
            .child(kinds)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(caption("X"))
                    .child(div().w(px(150.)).child(Select::new(&self.x_select).small())),
            )
            .child(h_flex().gap_1().items_center().child(caption("Y")).child(y_chips))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(caption("Series"))
                    .child(div().w(px(130.)).child(Select::new(&self.series_select).small())),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(caption("Aggregate"))
                    .child(div().w(px(100.)).child(Select::new(&self.agg_select).small())),
            );

        let body = match &self.data {
            Err(msg) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px_4()
                .text_color(theme.muted_foreground)
                .child(msg.clone())
                .into_any_element(),
            Ok(data) => {
                let legend_items: Vec<(Hsla, String)> = if data.kind == ChartKind::Pie {
                    data.categories
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (series_color(&colors, i), c.clone()))
                        .collect()
                } else {
                    data.series
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (series_color(&colors, i), s.name.clone()))
                        .collect()
                };
                let legend = h_flex().px_3().gap_3().flex_wrap().text_size(CHROME_TEXT_SIZE).children(
                    legend_items.into_iter().map(|(c, n)| {
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(div().size(px(9.)).rounded_sm().bg(c))
                            .child(div().text_color(theme.muted_foreground).child(n))
                    }),
                );
                let style = Style {
                    fg: theme.foreground,
                    muted: theme.muted_foreground,
                    grid: theme.border.opacity(0.55),
                    axis: theme.muted_foreground.opacity(0.6),
                    panel: theme.popover,
                    border: theme.border,
                    colors,
                    font: {
                        let mut ts = window.text_style();
                        ts.font_family = theme.font_family.clone();
                        ts.font()
                    },
                };
                let (data_for_paint, hover) = (data.clone(), self.hover);
                let bounds_cell = self.plot_bounds.clone();
                let notes = data.notes.clone();
                let titles = format!("{}  ·  {}", data.y_title, data.x_title);
                let plot = canvas(
                    {
                        let bounds_cell = bounds_cell.clone();
                        move |bounds, _, _| {
                            bounds_cell.set(bounds);
                            bounds
                        }
                    },
                    move |_, bounds: Bounds<Pixels>, window, cx| {
                        let (w, h) = (
                            f64::from(f32::from(bounds.size.width)),
                            f64::from(f32::from(bounds.size.height)),
                        );
                        if w > 10.0 && h > 10.0 {
                            let layout = Layout::new(&data_for_paint, w, h);
                            paint_chart(window, cx, bounds, &data_for_paint, &layout, hover, &style);
                        }
                    },
                )
                .size_full();
                v_flex()
                    .size_full()
                    .child(legend)
                    .child(
                        div()
                            .id("chart-plot")
                            .flex_1()
                            .min_h_0()
                            .mx_2()
                            .relative()
                            .track_focus(&self.focus)
                            .on_key_down(
                                cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                                    this.on_key(ev, window, cx)
                                }),
                            )
                            .on_mouse_move(
                                cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_move(ev, cx)),
                            )
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    window.focus(&this.focus, cx);
                                }),
                            )
                            .child(plot),
                    )
                    .child(
                        h_flex()
                            .px_3()
                            .pb_1()
                            .gap_3()
                            .text_size(gpui_kit::px(11.))
                            .text_color(theme.muted_foreground)
                            .child(div().child(titles))
                            .children(notes.into_iter().map(|n| div().text_color(theme.warning).child(n))),
                    )
                    .into_any_element()
            }
        };
        v_flex().size_full().child(controls).child(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use theta_core::{Column, TypeKind, Value};

    fn result() -> Arc<ResultSet> {
        let col = |n: &str, kind| Column { name: n.into(), type_name: String::new(), kind, origin: None };
        Arc::new(ResultSet {
            columns: vec![
                col("country", TypeKind::Text),
                col("n", TypeKind::Integer),
                col("m", TypeKind::Integer),
            ],
            rows: vec![
                vec![Value::Text("SE".into()), Value::Int(3), Value::Int(4)],
                vec![Value::Text("NO".into()), Value::Int(5), Value::Int(6)],
                vec![Value::Text("SE".into()), Value::Int(7), Value::Int(8)],
            ],
            truncated: false,
        })
    }

    #[gpui_kit::test]
    fn a_two_column_result_charts_at_once_and_the_controls_change_it(cx: &mut TestAppContext) {
        cx.update(theta_ui::init);
        let (panel, vcx) = cx.add_window_view(ChartPanel::new);
        panel.update_in(vcx, |p, window, cx| {
            p.set_result(result(), window, cx);
            let d = p.data.as_ref().expect("a chart without touching a control");
            assert_eq!(
                (d.kind, d.categories.clone()),
                (ChartKind::Bar, vec!["SE".to_string(), "NO".to_string()])
            );
            assert_eq!(d.series.len(), 2, "both numeric columns are charted");
            assert_eq!(d.series[0].points[0].y, 10.0, "SE: 3 + 7");

            p.toggle_y(2, cx);
            assert_eq!(p.data.as_ref().unwrap().series.len(), 1);
            p.toggle_y(1, cx);
            assert_eq!(p.data.as_ref().unwrap().series.len(), 1, "the last Y column cannot be switched off");

            p.set_kind(ChartKind::Pie, cx);
            assert_eq!(p.data.as_ref().unwrap().kind, ChartKind::Pie);
            p.cycle_aggregate(window, cx);
            assert_eq!(p.spec.as_ref().unwrap().aggregate, Aggregate::Avg);
            assert_eq!(p.data.as_ref().unwrap().series[0].points[0].y, 5.0, "SE: (3 + 7) / 2 as m");

            // The same result again keeps the choices; a new one starts from the suggestion.
            p.set_result(result(), window, cx);
            assert_eq!(p.spec.as_ref().unwrap().kind, ChartKind::Bar, "a new result resets the spec");
            p.clear(cx);
            assert!(p.data.is_err() && p.spec.is_none());
        });
    }

    #[gpui_kit::test]
    fn the_same_result_keeps_the_users_choices(cx: &mut TestAppContext) {
        cx.update(theta_ui::init);
        let (panel, vcx) = cx.add_window_view(ChartPanel::new);
        let rs = result();
        panel.update_in(vcx, |p, window, cx| {
            p.set_result(rs.clone(), window, cx);
            p.set_kind(ChartKind::Line, cx);
            p.set_result(rs.clone(), window, cx);
            assert_eq!(p.spec.as_ref().unwrap().kind, ChartKind::Line);
        });
    }

    #[gpui_kit::test]
    fn a_result_with_nothing_plottable_says_so(cx: &mut TestAppContext) {
        cx.update(theta_ui::init);
        let (panel, vcx) = cx.add_window_view(ChartPanel::new);
        let rs = Arc::new(ResultSet {
            columns: vec![Column {
                name: "j".into(),
                type_name: "jsonb".into(),
                kind: TypeKind::Json,
                origin: None,
            }],
            rows: vec![vec![Value::Json("{}".into())]],
            truncated: false,
        });
        panel.update_in(vcx, |p, window, cx| {
            p.set_result(rs, window, cx);
            assert!(p.spec.is_none());
            assert!(p.data.as_ref().unwrap_err().contains("no text, date or number"));
        });
    }
}
