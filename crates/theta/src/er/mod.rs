//! The ER diagram tab.
//!
//! What is drawn, and where, comes from `theta-diagram` (graph, layout, routing, SVG and Mermaid text); this
//! module owns the viewport (`camera`), the keys (`keys`), the selection and the fuzzy jump, and hands the
//! scene to `paint`. It never touches a connection: it is given a catalog and emits [`DiagramEvent`]s, which
//! `workspace/diagram_tabs.rs` turns into "open the table", "save the file" and notices.

pub mod camera;
pub(crate) mod keys;
mod paint;

use crate::connections::NoticeKind;
use camera::Camera;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Render, ScrollDelta, ScrollEvent, Styled as _, Window, canvas, div, px,
};
use keys::Cmd;
use paint::{Scene, Style};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use theta_core::Catalog;
use theta_diagram::{Graph, Layout, Metrics, Scope, from_catalog};
use theta_ui::fuzzy::fuzzy_filter;

gpui_kit::actions!(theta_er, [SelectNext, SelectPrev]);

/// Tab is bound to focus traversal by gpui-kit's `Root`, which wins over a key handler, so the diagram claims
/// it for its own context (as the table editor and the grid do).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui_kit::KeyBinding::new("tab", SelectNext, Some("ErDiagram")),
        gpui_kit::KeyBinding::new("shift-tab", SelectPrev, Some("ErDiagram")),
    ]);
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiagramEvent {
    OpenTable {
        schema: String,
        table: String,
    },
    /// Write `text` to a file the user picks.
    SaveFile {
        suggested: String,
        text: String,
        done: String,
    },
    Notice(NoticeKind, String),
    /// The user asked to read the catalog again.
    Reload,
}

/// The fuzzy jump box: what was typed and which match is highlighted.
struct Jump {
    query: String,
    cursor: usize,
}

pub struct DiagramView {
    conn_id: String,
    scope: Scope,
    graph: Arc<Graph>,
    layout: Arc<Layout>,
    metrics: Metrics,
    cam: Camera,
    selected: Option<usize>,
    /// Per node: shares an edge with the selected one.
    near: Vec<bool>,
    jump: Option<Jump>,
    focus: FocusHandle,
    /// Canvas bounds at the last paint, to map pointer positions and to fit.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<(f32, f32)>,
    /// Frames still allowed to wait for the first real bounds before fitting.
    fit_pending: u8,
}

impl EventEmitter<DiagramEvent> for DiagramView {}

impl Focusable for DiagramView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The advance of one monospace character at the base font size, measured, so layout boxes fit the text the
/// view will draw.
fn measure_char_w(window: &mut Window, font: &gpui_kit::Font) -> f32 {
    let probe = "MMMMMMMMMMMMMMMMMMMM";
    let run = gpui_kit::TextRun {
        len: probe.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let l = window.text_system().shape_line(probe.into(), px(paint::BASE_FONT), &[run], None);
    (f32::from(l.width) / probe.len() as f32).max(4.0)
}

fn mono_font(window: &Window, cx: &App) -> gpui_kit::Font {
    let mut ts = window.text_style();
    ts.font_family = cx.theme().mono_font_family.clone();
    ts.font()
}

fn hex(c: Hsla) -> String {
    let r = c.to_rgb();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(r.r), b(r.g), b(r.b))
}

impl DiagramView {
    pub fn new(
        conn_id: String,
        scope: Scope,
        catalog: &Catalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let font = mono_font(window, cx);
        let metrics = Metrics { char_w: measure_char_w(window, &font), ..Metrics::default() };
        let graph = Arc::new(from_catalog(catalog, &scope));
        let layout = Arc::new(theta_diagram::layout(&graph, &metrics));
        let mut v = DiagramView {
            conn_id,
            scope,
            graph,
            layout,
            metrics,
            cam: Camera::default(),
            selected: None,
            near: Vec::new(),
            jump: None,
            focus: cx.focus_handle(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            fit_pending: 6,
        };
        // A single-table diagram starts with that table selected: it is what was asked for.
        if let Scope::Table { schema, table } = &v.scope {
            v.selected = v.graph.node_index(schema, table);
            v.update_near();
        }
        v
    }

    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn title(&self) -> String {
        match &self.scope {
            Scope::Schema(s) => format!("Diagram: {s}"),
            Scope::Table { table, .. } => format!("Diagram: {table}"),
        }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn selected_label(&self) -> Option<String> {
        self.selected.map(|n| self.graph.label(n))
    }

    pub fn focus_handle_ref(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Rebuild from a fresh catalog, keeping the selection by name and the camera where it is.
    pub fn set_catalog(&mut self, catalog: &Catalog, cx: &mut Context<Self>) {
        let keep =
            self.selected.map(|n| (self.graph.nodes[n].schema.clone(), self.graph.nodes[n].name.clone()));
        self.graph = Arc::new(from_catalog(catalog, &self.scope));
        self.layout = Arc::new(theta_diagram::layout(&self.graph, &self.metrics));
        self.selected = keep.and_then(|(s, t)| self.graph.node_index(&s, &t));
        self.update_near();
        cx.notify();
    }

    fn update_near(&mut self) {
        self.near = vec![false; self.graph.nodes.len()];
        if let Some(n) = self.selected {
            for m in self.graph.neighbours(n) {
                self.near[m] = true;
            }
        }
    }

    fn view_size(&self) -> (f32, f32) {
        let b = self.bounds.get();
        (f32::from(b.size.width), f32::from(b.size.height))
    }

    fn fit(&mut self) {
        let (w, h) = self.view_size();
        if w > 1.0 && h > 1.0 {
            self.cam = Camera::fit((self.layout.width, self.layout.height), (w, h));
        }
    }

    fn box_rect(&self, n: usize) -> (f32, f32, f32, f32) {
        let b = &self.layout.boxes[n];
        (b.x, b.y, b.w, b.h)
    }

    fn select(&mut self, n: Option<usize>, cx: &mut Context<Self>) {
        self.selected = n;
        self.update_near();
        if let Some(n) = n {
            let (r, v) = (self.box_rect(n), self.view_size());
            self.cam.reveal(r, v);
        }
        cx.notify();
    }

    fn step_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let order = self.layout.reading_order();
        if order.is_empty() {
            return;
        }
        let at = self.selected.and_then(|s| order.iter().position(|&n| n == s));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
        };
        self.select(Some(order[next]), cx);
    }

    fn node_at(&self, sx: f32, sy: f32) -> Option<usize> {
        let (wx, wy) = self.cam.to_world(sx, sy);
        self.layout
            .boxes
            .iter()
            .rev()
            .find(|b| wx >= b.x && wx <= b.right() && wy >= b.y && wy <= b.bottom())
            .map(|b| b.node)
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        match self.selected {
            Some(n) => {
                let node = &self.graph.nodes[n];
                cx.emit(DiagramEvent::OpenTable { schema: node.schema.clone(), table: node.name.clone() });
            }
            None => cx.emit(DiagramEvent::Notice(
                NoticeKind::Info,
                "Select a table first (Tab, or / to jump to one)".into(),
            )),
        }
    }

    /// The matches of the jump box: all tables in reading order when nothing is typed, else best first.
    fn jump_matches(&self, query: &str) -> Vec<usize> {
        let labels: Vec<String> = (0..self.graph.nodes.len()).map(|i| self.graph.label(i)).collect();
        if query.is_empty() {
            return self.layout.reading_order().into_iter().take(10).collect();
        }
        fuzzy_filter(&labels, query, |s| s.as_str()).into_iter().map(|(i, _)| i).take(10).collect()
    }

    fn svg_text(&self, cx: &App) -> String {
        let t = cx.theme();
        let palette = theta_diagram::svg::Palette {
            background: hex(t.background),
            box_fill: hex(t.popover),
            header_fill: hex(t.secondary),
            border: hex(t.border),
            text: hex(t.foreground),
            muted: hex(t.muted_foreground),
            edge: hex(t.muted_foreground),
            key: hex(t.warning),
        };
        theta_diagram::svg::render(&self.graph, &self.layout, &self.metrics, &palette, &self.title())
    }

    pub(crate) fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.view_size();
        match cmd {
            Cmd::Pan(dx, dy) => self.cam.pan_screen(-dx, -dy),
            Cmd::ZoomIn => self.cam.zoom_at(1.25, view.0 / 2.0, view.1 / 2.0),
            Cmd::ZoomOut => self.cam.zoom_at(0.8, view.0 / 2.0, view.1 / 2.0),
            Cmd::ZoomReset => self.cam.set_zoom_centered(1.0, view),
            Cmd::Fit => self.fit(),
            Cmd::Next => self.step_selection(true, cx),
            Cmd::Prev => self.step_selection(false, cx),
            Cmd::Open => self.open_selected(cx),
            Cmd::Jump => self.jump = Some(Jump { query: String::new(), cursor: 0 }),
            Cmd::Cancel => {
                if self.jump.is_some() {
                    self.jump = None;
                } else if self.selected.is_some() {
                    self.select(None, cx);
                } else {
                    let _ = window;
                }
            }
            Cmd::SaveSvg => {
                let name = match &self.scope {
                    Scope::Schema(s) => s.clone(),
                    Scope::Table { table, .. } => table.clone(),
                };
                let text = self.svg_text(cx);
                cx.emit(DiagramEvent::SaveFile {
                    suggested: format!("{name}.svg"),
                    text,
                    done: "Exported SVG".into(),
                });
            }
            Cmd::CopyMermaid => {
                let text = theta_diagram::mermaid::render(&self.graph);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.emit(DiagramEvent::Notice(
                    NoticeKind::Success,
                    format!(
                        "Copied the diagram as Mermaid erDiagram text ({} table{})",
                        self.graph.nodes.len(),
                        if self.graph.nodes.len() == 1 { "" } else { "s" }
                    ),
                ));
            }
            Cmd::Reload => cx.emit(DiagramEvent::Reload),
        }
        cx.notify();
    }

    /// Tab in the jump box walks its list; elsewhere it walks the tables.
    fn tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(j) = &self.jump {
            let n = self.jump_matches(&j.query).len();
            let at = j.cursor;
            let cursor = if forward { (at + 1).min(n.saturating_sub(1)) } else { at.saturating_sub(1) };
            if let Some(j) = self.jump.as_mut() {
                j.cursor = cursor;
            }
            cx.notify();
            return;
        }
        self.run(if forward { Cmd::Next } else { Cmd::Prev }, window, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if let Some(j) = &self.jump {
            let (query, cursor) = (j.query.clone(), j.cursor);
            let matches = self.jump_matches(&query);
            let mut next = Jump { query, cursor };
            match key {
                "escape" => {
                    self.jump = None;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "enter" => {
                    self.jump = None;
                    if let Some(&n) = matches.get(cursor) {
                        self.select(Some(n), cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "down" | "tab" if !m.shift => next.cursor = (cursor + 1).min(matches.len().saturating_sub(1)),
                "up" | "tab" => next.cursor = cursor.saturating_sub(1),
                "backspace" => {
                    next.query.pop();
                    next.cursor = 0;
                }
                _ if !m.control && !m.platform => {
                    if let Some(t) = ev.keystroke.key_char.as_deref()
                        && !t.chars().any(|c| c.is_control())
                    {
                        next.query.push_str(t);
                        next.cursor = 0;
                    }
                }
                _ => return,
            }
            self.jump = Some(next);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // ctrl / cmd chords belong to the application (zoom of the editor font, tabs, palette).
        if m.control || m.platform || m.alt {
            return;
        }
        if let Some(cmd) = keys::command(key, m.shift) {
            self.run(cmd, window, cx);
            cx.stop_propagation();
        }
    }

    fn local(&self, p: gpui_kit::Point<Pixels>) -> (f32, f32) {
        let o = self.bounds.get().origin;
        (f32::from(p.x - o.x), f32::from(p.y - o.y))
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, cx: &mut Context<Self>) {
        let d = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(l) => (l.x * 40.0, l.y * 40.0),
        };
        if ev.modifiers.control || ev.modifiers.platform {
            let (sx, sy) = self.local(ev.position);
            let factor = (1.0f32 + 0.0015).powf(d.1.clamp(-120.0, 120.0));
            self.cam.zoom_at(factor, sx, sy);
        } else {
            self.cam.pan_screen(d.0, d.1);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn on_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let (sx, sy) = self.local(ev.position);
        self.drag = Some((sx, sy));
        let hit = self.node_at(sx, sy);
        if ev.click_count >= 2 {
            if hit.is_some() {
                self.selected = hit;
                self.open_selected(cx);
            }
            return;
        }
        if hit.is_some() {
            self.select(hit, cx);
        } else if self.selected.is_some() {
            self.select(None, cx);
        }
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let (sx, sy) = self.local(ev.position);
        if let Some((px0, py0)) = self.drag {
            self.cam.pan_screen(sx - px0, sy - py0);
            self.drag = Some((sx, sy));
            cx.notify();
        }
    }

    fn on_up(&mut self, _: &MouseUpEvent, _: &mut Context<Self>) {
        self.drag = None;
    }

    fn describe_selection(&self) -> Option<String> {
        let n = self.selected?;
        let node = &self.graph.nodes[n];
        let out = self.graph.edges.iter().filter(|e| e.from == n && e.to != n).count();
        let inn = self.graph.edges.iter().filter(|e| e.to == n && e.from != n).count();
        let cols = node.columns.len();
        Some(format!(
            "{} · {cols} column{} · references {out} table{} · referenced by {inn}",
            self.graph.label(n),
            if cols == 1 { "" } else { "s" },
            if out == 1 { "" } else { "s" },
        ))
    }
}

impl Render for DiagramView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // The first real bounds arrive with the first paint; fit once they are known.
        if self.fit_pending > 0 {
            let (w, h) = self.view_size();
            if w > 1.0 && h > 1.0 {
                self.fit();
                self.fit_pending = 0;
                if let Some(n) = self.selected {
                    let (r, v) = (self.box_rect(n), self.view_size());
                    self.cam.centre_on(r, v);
                    // Keep the fit zoom but centre on the table when the whole diagram is much larger.
                    if self.cam.zoom < 0.6 {
                        self.cam.zoom = 1.0;
                        self.cam.centre_on(r, v);
                    }
                }
            } else {
                // A `notify` from inside `render` does not schedule another frame, so wait a moment and ask
                // again from outside it.
                self.fit_pending -= 1;
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(40)).await;
                    this.update(cx, |_, cx| cx.notify()).ok();
                })
                .detach();
            }
        }
        let font = mono_font(window, cx);
        let style = Style {
            background: theme.background,
            box_fill: theme.popover,
            header_fill: theme.secondary,
            border: theme.border,
            text: theme.foreground,
            muted: theme.muted_foreground,
            edge: theme.muted_foreground.opacity(0.75),
            accent: theme.primary,
            key: theme.warning,
            font,
        };
        let (graph, layout, metrics) = (self.graph.clone(), self.layout.clone(), self.metrics.clone());
        let (cam, selected, near) = (self.cam, self.selected, self.near.clone());
        let bounds_cell = self.bounds.clone();
        let surface = canvas(
            move |b, _, _| {
                bounds_cell.set(b);
                b
            },
            move |_, b: Bounds<Pixels>, window, cx| {
                let scene = Scene {
                    graph: &graph,
                    layout: &layout,
                    metrics: &metrics,
                    cam,
                    selected,
                    near: &near,
                    style: &style,
                };
                paint::paint(window, cx, b, &scene);
            },
        )
        .size_full();

        let empty = self.graph.nodes.is_empty();
        let n_tables = self.graph.nodes.len();
        let n_edges = self.graph.edges.len();
        let counts = format!(
            "{n_tables} table{} · {n_edges} foreign key{}",
            if n_tables == 1 { "" } else { "s" },
            if n_edges == 1 { "" } else { "s" }
        );
        let zoom = format!("{:.0}%", self.cam.zoom * 100.0);
        let focused = self.focus.is_focused(window);
        let status = h_flex()
            .h(px(26.))
            .px_3()
            .gap_3()
            .items_center()
            .border_t_2()
            // The only focus mark the diagram needs: without it nothing says the keys reach it.
            .border_color(if focused { theme.primary } else { theme.border })
            .text_size(px(11.))
            .text_color(theme.muted_foreground)
            .child(div().flex_shrink_0().child(counts))
            .children(self.describe_selection().map(|s| div().flex_shrink_0().text_color(theme.foreground).child(s)))
            .children(self.graph.notes.iter().map(|n| div().flex_shrink_0().child(n.clone())))
            // The keys come last and give way first (they are also in the palette, "Diagram: ..."); a toast
            // at the bottom right then covers the zoom, not the hint.
            .child(div().flex_1().min_w_0().truncate().child(
                "hjkl pan · + - 0 zoom · f fit · tab select · / jump · enter open · s svg · m mermaid · r reload",
            ))
            .child(div().flex_shrink_0().child(zoom));

        let mut body = div()
            .id("er-diagram")
            .key_context("ErDiagram")
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| this.tab(true, window, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| this.tab(false, window, cx)))
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| this.on_scroll(ev, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, w, cx| this.on_down(ev, w, cx)),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_move(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev: &MouseUpEvent, _, cx| this.on_up(ev, cx)))
            .child(surface);
        if empty {
            body = body.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child(match &self.scope {
                        Scope::Schema(s) => format!("Schema {s} has no tables to draw"),
                        Scope::Table { table, .. } => format!("Table {table} is not in the loaded catalog"),
                    }),
            );
        }
        if let Some(j) = &self.jump {
            let matches = self.jump_matches(&j.query);
            body = body.child(
                v_flex().absolute().top(px(12.)).left_0().right_0().items_center().child(
                    v_flex()
                        .w(px(420.))
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.primary)
                        .rounded_md()
                        .shadow_md()
                        .child(
                            h_flex()
                                .h(px(30.))
                                .px_3()
                                .gap_2()
                                .items_center()
                                .text_size(px(13.))
                                .child(div().text_color(theme.muted_foreground).child("Jump to"))
                                .child(div().child(format!("{}▏", j.query))),
                        )
                        .children(matches.iter().enumerate().map(|(i, &n)| {
                            h_flex()
                                .h(px(24.))
                                .px_3()
                                .text_size(px(12.))
                                .when(i == j.cursor, |d| d.bg(theme.list_active))
                                .child(self.graph.label(n))
                        }))
                        .when(matches.is_empty(), |d| {
                            d.child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child("No table matches"),
                            )
                        }),
                ),
            );
        }
        v_flex().size_full().bg(theme.background).child(body).child(status)
    }
}
