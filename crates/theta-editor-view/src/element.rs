//! The custom GPUI element that paints the editor.
//!
//! Phases:
//! * `request_layout`: fill the parent (absolutely positioned, like gpui-kit's own editor).
//! * `prepaint`: `EditorView::compute_frame` applies pending scroll/reveal requests, shapes the
//!   visible lines (through the line cache) and turns the editor state into plain draw lists
//!   (`Frame`); popups are registered as deferred draws so they escape the editor's bounds.
//! * `paint`: replays the draw lists in z-order and registers mouse + text-input handlers.
//!
//! Only visible lines (plus a small overscan) are ever touched, so cost is independent of
//! buffer size; the engine's per-line highlight cache and our shaped-line cache do the rest.

use crate::completion::{CompletionState, MAX_VISIBLE_ROWS};
use crate::geometry;
use crate::lines::LineEntry;
use crate::popup;
use crate::selection_geom::{self, PathCmd, Span};
use crate::status::StatementStatus;
use crate::util::{grapheme_extent, selection_is_real};
use crate::view::{Drag, EditorView, LayoutInfo};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, AvailableSpace, Bounds, ContentMask, Context, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels,
    Point, ScrollDelta, ScrollEvent, ShapedLine, SharedString, Style, TextAlign, TextRun, UnderlineStyle,
    Window, fill, point, px, relative, size,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use theta_editor::{CursorStyle as EdCursor, Keymap, Mode};
use theta_sql::Severity;
use theta_ui::{EditorColors, SyntaxColors};

pub(crate) const LANE: f32 = 12.0;
const GUTTER_GAP: f32 = 4.0;
const GUTTER_RIGHT_PAD: f32 = 12.0;
const SCROLLBAR_ZONE: f32 = 14.0;
const THUMB_W: f32 = 6.0;
const CORNER: f32 = 3.0;
/// Extra width drawn past the last character of a line whose newline is selected.
const NEWLINE_EXT: f32 = 0.5;

pub(crate) struct EditorElement {
    view: Entity<EditorView>,
}

impl EditorElement {
    pub fn new(view: Entity<EditorView>) -> Self {
        Self { view }
    }
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub error: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub accent: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub thumb: Hsla,
}

impl Palette {
    pub fn get(cx: &App) -> Self {
        let theme = cx.theme();
        let status = &theme.highlight_theme.style.status;
        Palette {
            error: status.error(cx),
            warning: status.warning(cx),
            info: status.info(cx),
            accent: theme.ring,
            success: theme.green,
            danger: theme.red,
            thumb: theme.scrollbar_thumb,
        }
    }

    pub fn severity(&self, s: Severity) -> Hsla {
        match s {
            Severity::Critical => self.error,
            Severity::Warning => self.warning,
            Severity::Info => self.info,
        }
    }
}

pub(crate) struct VLine {
    pub line: usize,
    /// Top y relative to the element's top (scroll applied).
    pub y: f32,
    pub entry: Rc<LineEntry>,
    pub number: ShapedLine,
}

pub(crate) enum CursorShape {
    Block {
        glyph: Option<Box<ShapedLine>>,
    },
    Bar,
    /// A 2px line under the cell (Vim Replace mode).
    Underline,
}

pub(crate) struct CursorDraw {
    pub bounds: Bounds<Pixels>,
    pub shape: CursorShape,
    pub primary: bool,
    /// Block drawn as an outline / bar drawn dim: the editor is unfocused or a prompt has the keys.
    pub inactive: bool,
}

pub(crate) struct Squiggle {
    pub origin: Point<Pixels>,
    pub width: Pixels,
    pub color: Hsla,
}

pub(crate) enum MarkKind {
    Status(StatementStatus),
    Diag(Severity),
}

pub(crate) struct Mark {
    /// Line top y relative to the element.
    pub y: f32,
    pub kind: MarkKind,
}

pub(crate) struct ScrollbarDraw {
    pub alpha: f32,
    pub v: Option<(Bounds<Pixels>, Bounds<Pixels>)>,
    pub h: Option<(Bounds<Pixels>, Bounds<Pixels>)>,
}

pub(crate) struct CompletionSpec {
    pub anchor: (f32, f32),
}

pub(crate) struct HoverSpec {
    pub anchor: (f32, f32),
    pub message: String,
    pub severity: Severity,
}

pub(crate) struct Frame {
    pub bounds: Bounds<Pixels>,
    pub colors: EditorColors,
    pub pal: Palette,
    pub line_h: f32,
    pub gutter_w: f32,
    pub text_bounds: Bounds<Pixels>,
    /// Window x of text column 0 with the horizontal scroll applied.
    pub text_origin_x: Pixels,
    pub lines: Vec<VLine>,
    pub active_line: Option<usize>,
    pub selection_paths: Vec<(Vec<Vec<PathCmd>>, Hsla)>,
    pub rects: Vec<(Bounds<Pixels>, Hsla)>,
    pub outlines: Vec<(Bounds<Pixels>, Hsla)>,
    pub cursors: Vec<CursorDraw>,
    pub squiggles: Vec<Squiggle>,
    pub marked_underlines: Vec<(Point<Pixels>, Pixels)>,
    pub guides: Vec<Bounds<Pixels>>,
    pub band: Option<(f32, f32)>,
    pub marks: Vec<Mark>,
    pub scrollbars: ScrollbarDraw,
    pub placeholder: Option<(ShapedLine, f32)>,
    pub completion: Option<CompletionSpec>,
    pub hover: Option<HoverSpec>,
    pub animating: bool,
    pub ime_cursor: Option<Bounds<Pixels>>,
}

pub(crate) struct Prepaint {
    frame: Frame,
    hitbox: Hitbox,
    text_hitbox: Hitbox,
    started: Instant,
    prepaint_us: f64,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style { position: gpui_kit::Position::Absolute, ..Style::default() };
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let started = Instant::now();
        let view = self.view.clone();
        let mut frame = view.update(cx, |v, cx| v.compute_frame(bounds, window, cx));
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let text_hitbox = window.insert_hitbox(frame.text_bounds, HitboxBehavior::Normal);

        // Popups are deferred draws: they paint above everything and are not clipped to the editor.
        let viewport = window.viewport_size();
        let win = (f32::from(viewport.width), f32::from(viewport.height));
        if let Some(spec) = frame.completion.take() {
            let element = view.update(cx, |v, cx| {
                let state = v.completion.as_ref()?;
                Some(popup::completion_popup(view.clone(), state, v.font_family.clone(), cx))
            });
            if let Some((mut element, dims)) = element {
                element.layout_as_root(
                    size(AvailableSpace::MinContent, AvailableSpace::MinContent),
                    window,
                    cx,
                );
                let p =
                    geometry::place_popup((spec.anchor.0, spec.anchor.1, frame.line_h), dims, win, 2.0, 8.0);
                window.defer_draw(element, point(px(p.x), px(p.y)), 2, None);
            }
        }
        if let Some(spec) = frame.hover.take() {
            let (mut element, dims) = popup::hover_popover(&spec.message, spec.severity, &frame.pal, cx);
            element.layout_as_root(size(AvailableSpace::MinContent, AvailableSpace::MinContent), window, cx);
            let p = geometry::place_popup((spec.anchor.0, spec.anchor.1, frame.line_h), dims, win, 4.0, 8.0);
            window.defer_draw(element, point(px(p.x), px(p.y)), 1, None);
        }
        let prepaint_us = started.elapsed().as_secs_f64() * 1e6;
        Prepaint { frame, hitbox, text_hitbox, started, prepaint_us }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint_start = Instant::now();
        let focus = self.view.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);

        let f = &prepaint.frame;
        paint_frame(f, window, cx);
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.text_hitbox);
        self.register_mouse(&prepaint.hitbox, window);

        // The IME candidate window follows the caret.
        if let Some(b) = f.ime_cursor {
            let moved = self.view.update(cx, |v, _| v.ime_anchor.replace(b) != Some(b));
            if moved && focus.is_focused(window) {
                window.invalidate_character_coordinates();
            }
        }
        if f.animating {
            window.request_animation_frame();
        }

        let paint_us = paint_start.elapsed().as_secs_f64() * 1e6;
        let total_prepaint = prepaint.prepaint_us;
        let _ = prepaint.started;
        self.view.update(cx, |v, _| {
            v.stats.frames += 1;
            v.stats.prepaint_us = total_prepaint;
            v.stats.paint_us = paint_us;
            v.stats.shaped = v.cache.shaped_this_frame;
            v.stats.cache_hits = v.cache.hits_this_frame;
            if perf_enabled() {
                eprintln!(
                    "editor-frame #{}: prepaint {:.0} us, paint {:.0} us, shaped {} / cached {} ({} lines visible)",
                    v.stats.frames,
                    total_prepaint,
                    paint_us,
                    v.stats.shaped,
                    v.stats.cache_hits,
                    v.last_viewport.1
                );
            }
        });
    }
}

fn perf_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("THETA_EDITOR_PERF").is_some_and(|v| v != "0" && !v.is_empty()))
}

impl EditorElement {
    fn register_mouse(&self, hitbox: &Hitbox, window: &mut Window) {
        let view = self.view.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hb.is_hovered(window) {
                return;
            }
            view.update(cx, |v, cx| v.element_mouse_down(ev, window, cx));
        });
        let view = self.view.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = hb.is_hovered(window);
            view.update(cx, |v, cx| v.element_mouse_move(ev, hovered, window, cx));
        });
        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left {
                return;
            }
            view.update(cx, |v, cx| v.on_mouse_up(cx));
        });
        let view = self.view.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hb.is_hovered(window) {
                return;
            }
            view.update(cx, |v, cx| {
                let lh = v.line_height_px();
                let mut d = match ev.delta {
                    ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
                    // Wheel notches: three lines each, as terminals and Zed do.
                    ScrollDelta::Lines(l) => (l.x * lh * 3.0, l.y * lh * 3.0),
                };
                if ev.modifiers.shift && d.0 == 0.0 {
                    d = (d.1, 0.0);
                }
                v.on_scroll(d.0, d.1, cx);
            });
            cx.stop_propagation();
        });
    }
}

impl EditorView {
    pub(crate) fn element_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.clone().focus(window, cx);
        // Scrollbars first: they overlay the text.
        if ev.button == MouseButton::Left
            && let Some(l) = self.layout.clone()
        {
            let over = |b: &Option<Bounds<Pixels>>| b.is_some_and(|b| b.contains(&ev.position));
            if over(&l.v_thumb) {
                let t = l.v_thumb.expect("checked");
                self.drag = Some(Drag::VThumb { grab: f32::from(ev.position.y - t.origin.y) });
                cx.notify();
                return;
            }
            if over(&l.h_thumb) {
                let t = l.h_thumb.expect("checked");
                self.drag = Some(Drag::HThumb { grab: f32::from(ev.position.x - t.origin.x) });
                cx.notify();
                return;
            }
            if over(&l.v_track) {
                let t = l.v_track.expect("checked");
                let frac = f32::from(ev.position.y - t.origin.y) / f32::from(t.size.height);
                self.scroll_y =
                    (frac * (l.max_scroll.1 + l.viewport.1) - l.viewport.1 * 0.5).clamp(0.0, l.max_scroll.1);
                self.scroll_activity = Some(Instant::now());
                cx.notify();
                return;
            }
            if over(&l.h_track) {
                let t = l.h_track.expect("checked");
                let frac = f32::from(ev.position.x - t.origin.x) / f32::from(t.size.width);
                self.scroll_x =
                    (frac * (l.max_scroll.0 + l.viewport.0) - l.viewport.0 * 0.5).clamp(0.0, l.max_scroll.0);
                self.scroll_activity = Some(Instant::now());
                cx.notify();
                return;
            }
        }
        if let Some(idx) = self.char_at(ev.position, cx) {
            self.on_mouse_down_text(idx, ev.button, ev.click_count, ev.modifiers, cx);
        }
    }

    pub(crate) fn element_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        hovered: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer = Some(ev.position);
        let Some(layout) = self.layout.clone() else { return };
        match self.drag {
            Some(Drag::VThumb { grab }) => {
                if let Some(track) = layout.v_track {
                    let th = layout.v_thumb.map_or(24.0, |t| f32::from(t.size.height));
                    let top = f32::from(ev.position.y - track.origin.y) - grab;
                    self.scroll_y = geometry::scroll_for_thumb(
                        top,
                        th,
                        f32::from(track.size.height),
                        layout.max_scroll.1,
                    );
                    cx.notify();
                }
                return;
            }
            Some(Drag::HThumb { grab }) => {
                if let Some(track) = layout.h_track {
                    let tw = layout.h_thumb.map_or(24.0, |t| f32::from(t.size.width));
                    let left = f32::from(ev.position.x - track.origin.x) - grab;
                    self.scroll_x = geometry::scroll_for_thumb(
                        left,
                        tw,
                        f32::from(track.size.width),
                        layout.max_scroll.0,
                    );
                    cx.notify();
                }
                return;
            }
            Some(Drag::Text) => {
                if ev.pressed_button == Some(MouseButton::Left) {
                    if let Some(idx) = self.char_at(ev.position, cx) {
                        self.on_mouse_drag_text(idx, cx);
                    }
                } else {
                    self.drag = None;
                }
                return;
            }
            None => {}
        }
        // Scrollbars appear when the pointer nears the right/bottom edge.
        let b = layout.bounds;
        let near = hovered
            && (ev.position.x > b.right() - px(SCROLLBAR_ZONE)
                || ev.position.y > b.bottom() - px(SCROLLBAR_ZONE));
        if near != self.near_edge {
            self.near_edge = near;
            cx.notify();
        }
        self.update_hover(ev.position, hovered, cx);
    }

    fn update_hover(&mut self, pos: Point<Pixels>, hovered: bool, cx: &mut Context<Self>) {
        use crate::view::{HOVER_DELAY, Hover};
        let Some(layout) = self.layout.clone() else { return };
        let in_text = hovered && pos.x >= layout.bounds.origin.x + px(layout.gutter_w);
        let found = if in_text {
            self.char_at(pos, cx).and_then(|idx| {
                // A squiggle only counts under the glyphs, not in the empty space after the line.
                let line = self.editor.char_to_line(idx);
                let entry = self.line_entry(line, cx);
                let x = f32::from(pos.x - layout.bounds.origin.x) - layout.gutter_w + self.scroll_x;
                if px(x) > entry.width + px(layout.cell_w) {
                    return None;
                }
                self.all_diagnostics()
                    .iter()
                    .position(|d| d.range.start <= idx && idx < d.range.end.max(d.range.start + 1))
            })
        } else {
            None
        };
        match (found, self.hover.as_ref()) {
            (None, None) => {}
            (None, Some(_)) => {
                self.hover = None;
                cx.notify();
            }
            (Some(i), Some(h)) if h.diag == i => {}
            (Some(i), _) => {
                self.hover = Some(Hover { diag: i, shown: false });
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_DELAY).await;
                    this.update(cx, |v, cx| {
                        if let Some(h) = &mut v.hover
                            && h.diag == i
                        {
                            h.shown = true;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Everything the element needs for one frame.
    pub(crate) fn compute_frame(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        self.refresh_style(cx);
        self.cache.begin_frame();
        let colors = EditorColors::get(cx);
        let pal = Palette::get(cx);
        let syntax = SyntaxColors::get(cx);
        let style = self.style.clone().expect("refresh_style sets it");
        if self.numbers_epoch != style.epoch {
            self.numbers.clear();
            self.numbers_epoch = style.epoch;
        }
        let line_h = self.line_height_px();
        let cell_w = self.cell_width(cx);
        let line_count = self.editor.line_count();
        let gutter_w = geometry::gutter_width(line_count, cell_w, LANE, GUTTER_GAP, GUTTER_RIGHT_PAD);
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let viewport = ((bw - gutter_w).max(1.0), bh.max(1.0));
        let rev = self.editor.revision();
        if self.longest.0 != rev {
            self.longest = (rev, 0.0);
        }
        let ox = f32::from(bounds.origin.x);
        let oy = f32::from(bounds.origin.y);
        let focused = self.focus_handle.is_focused(window);
        let mut animating = false;

        // ---- drag auto-scroll: the pointer outside the text area keeps scrolling and extending
        if self.drag == Some(Drag::Text) && self.drag_moved {
            let p = window.mouse_position();
            let (px_, py_) = (f32::from(p.x), f32::from(p.y));
            let dy = if py_ < oy {
                -geometry::autoscroll_step(oy - py_, line_h)
            } else if py_ > oy + bh {
                geometry::autoscroll_step(py_ - oy - bh, line_h)
            } else {
                0.0
            };
            let left = ox + gutter_w;
            let dx = if px_ < left {
                -geometry::autoscroll_step(left - px_, cell_w)
            } else if px_ > ox + bw {
                geometry::autoscroll_step(px_ - ox - bw, cell_w)
            } else {
                0.0
            };
            if dx != 0.0 || dy != 0.0 {
                self.scroll_x = (self.scroll_x + dx).max(0.0);
                self.scroll_y = (self.scroll_y + dy).max(0.0);
                let clamped = point(px(px_.clamp(left, ox + bw - 1.0)), px(py_.clamp(oy, oy + bh - 1.0)));
                if let Some(idx) = self.char_at(clamped, cx) {
                    self.editor.drag_to(idx);
                }
                animating = true;
            }
        }

        // ---- scroll requests and reveal
        let pending_scroll = self.pending_scroll.take();
        let reveal = std::mem::take(&mut self.pending_reveal);
        if pending_scroll.is_some() || reveal {
            let cursor = self.primary_cursor();
            let cl = self.editor.char_to_line(cursor);
            let col = cursor - self.editor.line_to_char(cl);
            let entry = self.line_entry(cl, cx);
            let x = f32::from(entry.x_of_col(col));
            let (_, end_b) = grapheme_extent(&entry.display, col);
            let w = (f32::from(entry.shaped.x_for_index(end_b)) - x).max(cell_w);
            // `longest` was reset by the edit and is only rebuilt from the visible lines
            // further down; without this the clamp below would use a width of 0 and throw
            // away the reveal (typing at the end of a long line never scrolled).
            self.longest.1 = self.longest.1.max(f32::from(entry.width));
            if let Some(req) = pending_scroll {
                self.scroll_y = geometry::scroll_for_request(req, cl, self.scroll_y, line_h, bh);
            }
            if reveal || pending_scroll.is_some() {
                let (nx, ny) =
                    geometry::reveal((self.scroll_x, self.scroll_y), cl, x, w, line_h, cell_w, viewport);
                self.scroll_x = nx;
                if pending_scroll.is_none() {
                    self.scroll_y = ny;
                }
            }
        }
        let (mx, my) = (
            geometry::max_scroll_x(self.longest.1, viewport.0, cell_w * (geometry::REVEAL_COLUMNS + 1.0)),
            geometry::max_scroll_y(line_count, line_h, viewport.1),
        );
        self.scroll_y = self.scroll_y.clamp(0.0, my);
        self.scroll_x = self.scroll_x.clamp(0.0, mx.max(0.0));

        // ---- visible lines
        let vis = geometry::visible_lines(self.scroll_y, line_h, bh, line_count);
        let shaped = geometry::shaped_lines(&vis, line_count);
        let cursor_line = self.editor.char_to_line(self.primary_cursor());
        let mut lines: Vec<VLine> = Vec::with_capacity(shaped.len());
        let mut widest = self.longest.1;
        for line in shaped.clone() {
            let entry = self.line_entry(line, cx);
            widest = widest.max(f32::from(entry.width));
            let active = line == cursor_line;
            let number = match self.numbers.get(&(line + 1, active)) {
                Some(n) => n.clone(),
                None => {
                    let text = (line + 1).to_string();
                    let color = if active { colors.active_line_number } else { colors.line_number };
                    let run = TextRun {
                        len: text.len(),
                        font: style.font.clone(),
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let n = self.text_system.shape_line(text.into(), px(self.font_size), &[run], None);
                    if self.numbers.len() > 4000 {
                        self.numbers.clear();
                    }
                    self.numbers.insert((line + 1, active), n.clone());
                    n
                }
            };
            lines.push(VLine { line, y: line as f32 * line_h - self.scroll_y, entry, number });
        }
        self.longest.1 = widest;
        let (mx, _) =
            (geometry::max_scroll_x(widest, viewport.0, cell_w * (geometry::REVEAL_COLUMNS + 1.0)), 0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, mx);

        if self.last_viewport != (vis.start, geometry::full_lines_in_viewport(line_h, bh)) {
            self.last_viewport = (vis.start, geometry::full_lines_in_viewport(line_h, bh));
            self.editor.set_viewport(self.last_viewport.0, self.last_viewport.1);
        }

        let text_left = ox + gutter_w - self.scroll_x;
        let text_bounds =
            Bounds::new(point(px(ox + gutter_w), bounds.origin.y), size(px(viewport.0), px(bh)));
        let by_line: HashMap<usize, &VLine> = lines.iter().map(|l| (l.line, l)).collect();
        let scroll_y = self.scroll_y;
        let line_top = move |line: usize| oy + line as f32 * line_h - scroll_y;

        // ---- selections, cursors, active line
        let keymap = self.editor.keymap();
        let mode = self.editor.mode();
        let prompt_open = self.editor.prompt().is_some();
        // The engine decides how its cursor looks (block / bar / underline) for every keymap.
        let cursor_style = self.editor.cursor_style();
        // Vim's block-wise Visual mode is one selection per line, but there is only one cursor.
        let single_cursor =
            keymap == Keymap::Vim && matches!(mode, Mode::Visual | Mode::VisualLine | Mode::VisualBlock);
        let block_visual = keymap == Keymap::Vim && mode == Mode::VisualBlock;
        let mut block_rows: Vec<(usize, Span)> = Vec::new();
        let first_char = self.editor.line_to_char(vis.start);
        // `line_to_char` clamps to the last line's start, so "past the last line" must be spelled out:
        // otherwise the cursor and selections of the final line are culled unless they start at column 0.
        let last_char =
            if vis.end >= line_count { self.editor.len_chars() } else { self.editor.line_to_char(vis.end) };
        let sels: Vec<_> = self.editor.selections().to_vec();
        let primary_idx = self.editor.primary_index();
        let lo = sels.partition_point(|s| s.to() < first_char);
        let active_line = (!selection_is_real(&self.editor, &self.editor.primary())).then_some(cursor_line);

        let mut selection_paths = Vec::new();
        let mut cursors = Vec::new();
        let mut ime_cursor = None;
        let sel_color = crate::util::visible_selection(colors.selection, colors.background);
        let sel_secondary = Hsla { a: sel_color.a * 0.55, ..sel_color };
        for (i, s) in sels.iter().enumerate().skip(lo) {
            if s.from() > last_char {
                break;
            }
            let primary = i == primary_idx;
            if selection_is_real(&self.editor, s) {
                let ed = &self.editor;
                let line_len = |l: usize| by_line.get(&l).map_or(0, |v| v.entry.display.char_count());
                let parts = selection_geom::decompose(
                    s.from(),
                    s.to(),
                    shaped.clone(),
                    &|c| ed.char_to_line(c),
                    &|l| ed.line_to_char(l),
                    &line_len,
                );
                let spans: Vec<Span> = parts
                    .iter()
                    .filter_map(|p| {
                        let v = by_line.get(&p.line)?;
                        let x0 = f32::from(v.entry.x_of_col(p.col_start));
                        let mut x1 = f32::from(v.entry.x_of_col(p.col_end));
                        if p.newline {
                            x1 += cell_w * NEWLINE_EXT;
                        }
                        Some(Span { x0: text_left + x0, x1: text_left + x1.max(x0 + 1.0) })
                    })
                    .collect();
                if block_visual {
                    // Rows of a rectangular block are separate selections; they are outlined
                    // together below so the corners of the rectangle are rounded, not each row's.
                    block_rows
                        .extend(parts.iter().filter_map(|p| by_line.get(&p.line).map(|_| p.line)).zip(spans));
                } else if let (Some(first), false) = (parts.first(), spans.is_empty()) {
                    let y0 = line_top(first.line);
                    let shapes = selection_geom::outline(&spans, y0, line_h, CORNER);
                    selection_paths.push((shapes, if primary { sel_color } else { sel_secondary }));
                }
            }
            // Cursor
            if single_cursor && !primary {
                continue;
            }
            let idx = self.editor.cursor_of(s);
            let cl = self.editor.char_to_line(idx);
            let Some(v) = by_line.get(&cl) else { continue };
            let col = idx - self.editor.line_to_char(cl);
            let x0 = f32::from(v.entry.x_of_col(col));
            let y = line_top(cl);
            if cursor_style == EdCursor::Underline {
                let (end_col, _) = grapheme_extent(&v.entry.display, col);
                let at_eol = col >= v.entry.display.char_count() || end_col == col;
                let (xa, xb) = (x0, if at_eol { x0 + cell_w } else { f32::from(v.entry.x_of_col(end_col)) });
                if primary {
                    ime_cursor =
                        Some(Bounds::new(point(px(text_left + xa), px(y)), size(px(xb - xa), px(line_h))));
                }
                cursors.push(CursorDraw {
                    bounds: Bounds::new(
                        point(px(text_left + xa), px(y + line_h - 3.0)),
                        size(px((xb - xa).max(cell_w * 0.5)), px(2.)),
                    ),
                    shape: CursorShape::Underline,
                    primary,
                    inactive: !focused || prompt_open,
                });
            } else if cursor_style == EdCursor::Bar {
                let draw = if focused && !prompt_open { self.blink_visible } else { true };
                let dim = !focused || prompt_open;
                // The primary caret's bounds feed the IME candidate window.
                if primary {
                    ime_cursor =
                        Some(Bounds::new(point(px(text_left + x0), px(y)), size(px(2.), px(line_h))));
                }
                if !draw {
                    continue;
                }
                let w = 2.0;
                cursors.push(CursorDraw {
                    bounds: Bounds::new(point(px(text_left + x0 - 1.0), px(y)), size(px(w), px(line_h))),
                    shape: CursorShape::Bar,
                    primary,
                    inactive: dim,
                });
            } else {
                let (end_col, end_b) = grapheme_extent(&v.entry.display, col);
                let at_eol = col >= v.entry.display.char_count() || end_col == col;
                let x1 = if at_eol { x0 + cell_w } else { f32::from(v.entry.shaped.x_for_index(end_b)) };
                let w = (x1 - x0).max(cell_w * 0.5);
                if primary {
                    ime_cursor = Some(Bounds::new(point(px(text_left + x0), px(y)), size(px(w), px(line_h))));
                }
                let hollow = !focused || prompt_open;
                let glyph = if !at_eol && !hollow {
                    let b = v.entry.display.byte_of_char(col);
                    let text = &v.entry.display.text[b..end_b];
                    if text.trim().is_empty() {
                        None
                    } else {
                        let run = TextRun {
                            len: text.len(),
                            font: style.font.clone(),
                            color: colors.background,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        Some(Box::new(self.text_system.shape_line(
                            SharedString::from(text.to_string()),
                            px(self.font_size),
                            &[run],
                            None,
                        )))
                    }
                } else {
                    None
                };
                cursors.push(CursorDraw {
                    bounds: Bounds::new(point(px(text_left + x0), px(y)), size(px(w), px(line_h))),
                    shape: CursorShape::Block { glyph },
                    primary,
                    inactive: hollow,
                });
            }
        }

        // Block-visual rows: one outline per run of consecutive lines.
        let mut i = 0;
        while i < block_rows.len() {
            let mut j = i + 1;
            while j < block_rows.len() && block_rows[j].0 == block_rows[j - 1].0 + 1 {
                j += 1;
            }
            let spans: Vec<Span> = block_rows[i..j].iter().map(|r| r.1).collect();
            let shapes = selection_geom::outline(&spans, line_top(block_rows[i].0), line_h, CORNER);
            selection_paths.push((shapes, sel_color));
            i = j;
        }

        // ---- search matches and matching bracket
        let mut rects: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
        let mut outlines: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
        // Vim's `hlsearch` keeps the matches painted after the prompt closes (`:noh` clears them
        // inside the engine, which then reports none).
        let searching = self.editor.prompt().is_some_and(|p| matches!(p.prefix, "/" | "?" | "find:"))
            || keymap == Keymap::Vim;
        let push_range = |this: &EditorView,
                          from: usize,
                          to: usize,
                          color: Hsla,
                          outline: bool,
                          rects: &mut Vec<(Bounds<Pixels>, Hsla)>,
                          outlines: &mut Vec<(Bounds<Pixels>, Hsla)>| {
            let l = this.editor.char_to_line(from);
            let Some(v) = by_line.get(&l) else { return };
            let start = this.editor.line_to_char(l);
            let (c0, c1) = (from - start, (to - start).min(v.entry.display.char_count()));
            let x0 = f32::from(v.entry.x_of_col(c0));
            let x1 = f32::from(v.entry.x_of_col(c1)).max(x0 + cell_w * 0.6);
            let b = Bounds::new(
                point(px(text_left + x0), px(line_top(l) + 1.0)),
                size(px(x1 - x0), px(line_h - 2.0)),
            );
            if outline { outlines.push((b, color)) } else { rects.push((b, color)) }
        };
        if searching {
            let matches = self.editor.search_matches();
            let start = matches.partition_point(|m| m.end <= first_char);
            let cur = self.editor.primary();
            let cur_from = if keymap == Keymap::Vim { self.primary_cursor() } else { cur.from() };
            for m in matches[start..].iter().take_while(|m| m.start < last_char) {
                let is_cur = m.start == cur_from;
                push_range(self, m.start, m.end, colors.search_match, false, &mut rects, &mut outlines);
                if is_cur {
                    push_range(self, m.start, m.end, pal.warning, true, &mut rects, &mut outlines);
                }
            }
        }
        let bracket_key = (rev, self.primary_cursor());
        let bracket = match self.bracket_cache {
            Some((k, v)) if k == bracket_key => v,
            _ => {
                let v = self.editor.matching_bracket();
                self.bracket_cache = Some((bracket_key, v));
                v
            }
        };
        if let Some((a, b)) = bracket {
            for c in [a, b] {
                push_range(self, c, c + 1, colors.matching_bracket, false, &mut rects, &mut outlines);
            }
        }
        // other uses of the name the cursor rests on
        let start = self.occurrences.partition_point(|m| m.end <= first_char);
        for m in self.occurrences[start..].iter().take_while(|m| m.start < last_char) {
            push_range(self, m.start, m.end, colors.occurrence, false, &mut rects, &mut outlines);
        }

        // ---- diagnostics: squiggles + gutter dots
        let mut squiggles = Vec::new();
        let mut worst: HashMap<usize, Severity> = HashMap::new();
        for d in self.all_diagnostics() {
            let (first, last) = (
                self.editor.char_to_line(d.range.start),
                self.editor.char_to_line(d.range.end.max(d.range.start)),
            );
            for l in first.max(shaped.start)..=last.min(shaped.end.saturating_sub(1)) {
                let e = worst.entry(l).or_insert(d.severity);
                if d.severity > *e {
                    *e = d.severity;
                }
                let Some(v) = by_line.get(&l) else { continue };
                let ls = self.editor.line_to_char(l);
                let n = v.entry.display.char_count();
                let c0 = if l == first { d.range.start - ls } else { 0 };
                let c1 = if l == last { (d.range.end - ls).min(n) } else { n };
                let x0 = f32::from(v.entry.x_of_col(c0.min(n)));
                let x1 = f32::from(v.entry.x_of_col(c1)).max(x0 + cell_w * 0.8);
                squiggles.push(Squiggle {
                    origin: point(px(text_left + x0), px(line_top(l) + line_h - 4.5)),
                    width: px(x1 - x0),
                    color: pal.severity(d.severity),
                });
            }
        }
        let mut marks: Vec<Mark> = Vec::new();
        let status_line = self.statement_status.clone().map(|(r, s)| (self.editor.char_to_line(r.start), s));
        for (l, sev) in &worst {
            if by_line.contains_key(l) && status_line.map(|(sl, _)| sl) != Some(*l) {
                marks.push(Mark { y: *l as f32 * line_h - self.scroll_y, kind: MarkKind::Diag(*sev) });
            }
        }
        if let Some((l, s)) = status_line
            && by_line.contains_key(&l)
        {
            if s == StatementStatus::Running {
                animating = true;
            }
            marks.push(Mark { y: l as f32 * line_h - self.scroll_y, kind: MarkKind::Status(s) });
        }

        // ---- statement band
        let band = self.statement_lines(cx).map(|(a, b)| {
            let top = (a as f32 * line_h - self.scroll_y).max(0.0);
            let bottom = ((b + 1) as f32 * line_h - self.scroll_y).min(bh);
            (top, bottom.max(top))
        });

        // ---- IME marked text
        let mut marked_underlines = Vec::new();
        if let Some(m) = self.marked.clone() {
            let (first, last) = (self.editor.char_to_line(m.start), self.editor.char_to_line(m.end));
            for l in first..=last {
                let Some(v) = by_line.get(&l) else { continue };
                let ls = self.editor.line_to_char(l);
                let n = v.entry.display.char_count();
                let c0 = if l == first { m.start - ls } else { 0 };
                let c1 = if l == last { (m.end - ls).min(n) } else { n };
                let x0 = f32::from(v.entry.x_of_col(c0.min(n)));
                let x1 = f32::from(v.entry.x_of_col(c1));
                marked_underlines.push((
                    point(px(text_left + x0), px(line_top(l) + line_h - 3.0)),
                    px((x1 - x0).max(1.0)),
                ));
            }
        }

        // ---- indent guides
        let guides = {
            let indents: Vec<(usize, bool)> = lines
                .iter()
                .map(|l| {
                    let t = &l.entry.display.text;
                    (t.bytes().take_while(|&b| b == b' ').count(), t.trim().is_empty())
                })
                .collect();
            let mut out = Vec::new();
            for (i, l) in lines.iter().enumerate() {
                let (mut ind, blank) = indents[i];
                if blank {
                    // Blank lines continue the shallower of the neighbouring indents so guides run through.
                    let up = indents[..i].iter().rev().find(|(_, b)| !*b).map(|(n, _)| *n);
                    let down = indents[i + 1..].iter().find(|(_, b)| !*b).map(|(n, _)| *n);
                    ind = match (up, down) {
                        (Some(a), Some(b)) => a.min(b),
                        _ => 0,
                    };
                }
                for level in 0..ind / 4 {
                    let x = text_left + (level * 4) as f32 * cell_w;
                    out.push(Bounds::new(point(px(x.floor()), px(oy + l.y)), size(px(1.), px(line_h))));
                }
            }
            out
        };

        // ---- scrollbars
        let (max_x, max_y) = (
            geometry::max_scroll_x(self.longest.1, viewport.0, cell_w * (geometry::REVEAL_COLUMNS + 1.0)),
            my,
        );
        let hovering_edge = self.near_edge;
        let alpha = self.scrollbar_alpha(hovering_edge);
        if alpha > 0.0 && alpha < 1.0 {
            animating = true;
        }
        let v_track = (max_y > 0.0).then(|| {
            let h = bh - if max_x > 0.0 { THUMB_W + 4.0 } else { 0.0 };
            Bounds::new(point(px(ox + bw - SCROLLBAR_ZONE), px(oy)), size(px(SCROLLBAR_ZONE), px(h)))
        });
        let h_track = (max_x > 0.0).then(|| {
            Bounds::new(
                point(px(ox + gutter_w), px(oy + bh - SCROLLBAR_ZONE)),
                size(px(viewport.0 - if max_y > 0.0 { SCROLLBAR_ZONE } else { 0.0 }), px(SCROLLBAR_ZONE)),
            )
        });
        let v_thumb = v_track.map(|t| {
            let (top, h) = geometry::thumb_metrics(self.scroll_y, max_y, bh, f32::from(t.size.height), 28.0);
            Bounds::new(point(px(ox + bw - THUMB_W - 3.0), t.origin.y + px(top)), size(px(THUMB_W), px(h)))
        });
        let h_thumb = h_track.map(|t| {
            let (left, w) =
                geometry::thumb_metrics(self.scroll_x, max_x, viewport.0, f32::from(t.size.width), 28.0);
            Bounds::new(point(t.origin.x + px(left), px(oy + bh - THUMB_W - 3.0)), size(px(w), px(THUMB_W)))
        });
        self.layout = Some(LayoutInfo {
            bounds,
            gutter_w,
            line_h,
            cell_w,
            viewport,
            max_scroll: (max_x, max_y),
            v_track,
            v_thumb,
            h_track,
            h_thumb,
        });

        // ---- placeholder
        let placeholder = (line_count == 1 && self.editor.len_chars() == 0 && !self.placeholder.is_empty())
            .then(|| {
                let text = self.placeholder.clone();
                let run = TextRun {
                    len: text.len(),
                    font: style.font.clone(),
                    color: colors.line_number,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                (self.text_system.shape_line(text, px(self.font_size), &[run], None), 0.0)
            });

        // ---- popups
        let completion = self.completion.as_ref().and_then(|c: &CompletionState| {
            let l = self.editor.char_to_line(c.replace.start);
            let ent = by_line.get(&l)?.entry.clone();
            let col = c.replace.start - self.editor.line_to_char(l);
            let x = text_left + f32::from(ent.x_of_col(col));
            let _ = MAX_VISIBLE_ROWS;
            Some(CompletionSpec { anchor: (x, line_top(l)) })
        });
        let hover = self.hover.as_ref().filter(|h| h.shown).and_then(|h| {
            let all = self.all_diagnostics();
            let d = all.get(h.diag)?;
            let l = self.editor.char_to_line(d.range.start);
            let p = self.pointer?;
            Some(HoverSpec {
                anchor: (f32::from(p.x), line_top(l)),
                message: d.message.clone(),
                severity: d.severity,
            })
        });

        let _ = syntax;
        Frame {
            bounds,
            colors,
            pal,
            line_h,
            gutter_w,
            text_bounds,
            text_origin_x: px(text_left),
            lines,
            active_line,
            selection_paths,
            rects,
            outlines,
            cursors,
            squiggles,
            marked_underlines,
            guides,
            band,
            marks,
            scrollbars: ScrollbarDraw { alpha, v: v_track.zip(v_thumb), h: h_track.zip(h_thumb) },
            placeholder,
            completion,
            hover,
            animating,
            ime_cursor,
        }
    }
}

fn build_path(cmds: &[PathCmd]) -> Option<gpui_kit::Path<Pixels>> {
    let mut b = PathBuilder::fill();
    let pt = |p: selection_geom::Pt| point(px(p.x), px(p.y));
    for c in cmds {
        match *c {
            PathCmd::Move(p) => b.move_to(pt(p)),
            PathCmd::Line(p) => b.line_to(pt(p)),
            PathCmd::Curve(to, ctrl) => b.curve_to(pt(to), pt(ctrl)),
        }
    }
    b.close();
    b.build().ok()
}

fn paint_frame(f: &Frame, window: &mut Window, cx: &mut App) {
    let origin = f.bounds.origin;
    let line_h = px(f.line_h);
    let width = f.bounds.size.width;
    let text_left = origin.x + px(f.gutter_w);

    // Active line band spans text and gutter.
    if let Some(al) = f.active_line
        && let Some(v) = f.lines.iter().find(|l| l.line == al)
    {
        window.paint_quad(fill(
            Bounds::new(point(origin.x, origin.y + px(v.y)), size(width, line_h)),
            f.colors.active_line,
        ));
    }

    // Gutter background, then the statement band on top of it.
    let gutter = Bounds::new(origin, size(px(f.gutter_w), f.bounds.size.height));
    window.paint_quad(fill(gutter, f.colors.gutter_background));
    if let Some((top, bottom)) = f.band {
        window.paint_quad(fill(
            Bounds::new(point(origin.x, origin.y + px(top)), size(px(f.gutter_w), px(bottom - top))),
            f.pal.accent.opacity(0.07),
        ));
        window.paint_quad(
            fill(
                Bounds::new(
                    point(origin.x + px(f.gutter_w - 2.0), origin.y + px(top)),
                    size(px(2.), px(bottom - top)),
                ),
                f.pal.accent.opacity(0.85),
            )
            .corner_radii(px(1.)),
        );
    }

    // ---- text area (clipped so horizontal scrolling never bleeds into the gutter)
    window.with_content_mask(Some(ContentMask { bounds: f.text_bounds }), |window| {
        for g in &f.guides {
            window.paint_quad(fill(*g, f.colors.indent_guide));
        }
        for (b, c) in &f.rects {
            window.paint_quad(fill(*b, *c).corner_radii(px(2.)));
        }
        for (shapes, color) in &f.selection_paths {
            for cmds in shapes {
                if let Some(path) = build_path(cmds) {
                    window.paint_path(path, *color);
                }
            }
        }
        if let Some((line, _)) = &f.placeholder {
            let _ = line.paint(point(text_left, origin.y), line_h, TextAlign::Left, None, window, cx);
        }
        for v in &f.lines {
            let _ = v.entry.shaped.paint(
                point(f.text_origin_x, origin.y + px(v.y)),
                line_h,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        for (b, c) in &f.outlines {
            window.paint_quad(gpui_kit::outline(*b, *c, gpui_kit::BorderStyle::Solid).corner_radii(px(2.)));
        }
        for (p, w) in &f.marked_underlines {
            window.paint_underline(
                *p,
                *w,
                &UnderlineStyle { thickness: px(1.), color: Some(f.colors.foreground), wavy: false },
            );
        }
        for s in &f.squiggles {
            window.paint_underline(
                s.origin,
                s.width,
                &UnderlineStyle { thickness: px(1.), color: Some(s.color), wavy: true },
            );
        }
        for c in &f.cursors {
            let base = if c.primary { f.colors.cursor } else { f.colors.cursor.opacity(0.6) };
            match &c.shape {
                CursorShape::Bar | CursorShape::Underline => {
                    let color = if c.inactive { base.opacity(0.5) } else { base };
                    window.paint_quad(fill(c.bounds, color).corner_radii(px(0.5)));
                }
                CursorShape::Block { glyph } if !c.inactive => {
                    window.paint_quad(fill(c.bounds, base).corner_radii(px(1.5)));
                    if let Some(g) = glyph {
                        let _ = g.paint(c.bounds.origin, line_h, TextAlign::Left, None, window, cx);
                    }
                }
                CursorShape::Block { .. } => {
                    window.paint_quad(
                        gpui_kit::outline(c.bounds, base, gpui_kit::BorderStyle::Solid).corner_radii(px(1.5)),
                    );
                }
            }
        }
    });

    // ---- gutter contents
    window.with_content_mask(Some(ContentMask { bounds: gutter }), |window| {
        let num_right = origin.x + px(f.gutter_w - GUTTER_RIGHT_PAD);
        for v in &f.lines {
            let w = v.number.width();
            let _ = v.number.paint(
                point(num_right - w, origin.y + px(v.y)),
                line_h,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        let cx_lane = origin.x + px(LANE / 2.0 + 2.0);
        for m in &f.marks {
            let cy = origin.y + px(m.y + f.line_h / 2.0);
            match &m.kind {
                MarkKind::Diag(s) => {
                    let c = f.pal.severity(*s);
                    let r = 3.0;
                    window.paint_quad(
                        fill(
                            Bounds::new(point(cx_lane - px(r), cy - px(r)), size(px(r * 2.), px(r * 2.))),
                            c,
                        )
                        .corner_radii(px(r)),
                    );
                }
                MarkKind::Status(StatementStatus::Ok) => {
                    paint_status_dot(window, cx_lane, cy, f.pal.success, Glyph::Check)
                }
                MarkKind::Status(StatementStatus::Failed) => {
                    paint_status_dot(window, cx_lane, cy, f.pal.danger, Glyph::Cross)
                }
                MarkKind::Status(StatementStatus::Running) => {
                    paint_spinner(window, cx_lane, cy, f.pal.accent)
                }
            }
        }
    });

    // ---- scrollbars
    if f.scrollbars.alpha > 0.0 {
        let a = f.scrollbars.alpha;
        for (track, thumb) in [f.scrollbars.v, f.scrollbars.h].into_iter().flatten() {
            let _ = track;
            window.paint_quad(fill(thumb, f.pal.thumb.opacity(0.75 * a)).corner_radii(px(THUMB_W / 2.0)));
        }
    }
}

enum Glyph {
    Check,
    Cross,
}

fn paint_status_dot(window: &mut Window, cx_: Pixels, cy: Pixels, color: Hsla, glyph: Glyph) {
    let r = 5.0;
    window.paint_quad(
        fill(Bounds::new(point(cx_ - px(r), cy - px(r)), size(px(r * 2.), px(r * 2.))), color)
            .corner_radii(px(r)),
    );
    let mut b = PathBuilder::stroke(px(1.4));
    let p = |dx: f32, dy: f32| point(cx_ + px(dx), cy + px(dy));
    match glyph {
        Glyph::Check => {
            b.move_to(p(-2.2, 0.2));
            b.line_to(p(-0.6, 1.8));
            b.line_to(p(2.4, -1.6));
        }
        Glyph::Cross => {
            b.move_to(p(-1.8, -1.8));
            b.line_to(p(1.8, 1.8));
            b.move_to(p(1.8, -1.8));
            b.line_to(p(-1.8, 1.8));
        }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, gpui_kit::white());
    }
}

fn paint_spinner(window: &mut Window, cx_: Pixels, cy: Pixels, color: Hsla) {
    // Eight dots on a ring; the bright one chases around (80 ms per step).
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| (d.as_millis() / 80) as usize);
    for i in 0..8usize {
        let ang = i as f32 / 8.0 * std::f32::consts::TAU;
        let age = (t + 8 - i) % 8;
        let a = 1.0 - age as f32 / 8.0;
        let r = 4.6;
        let (x, y) = (cx_ + px(ang.cos() * r), cy + px(ang.sin() * r));
        let d = 2.0 + 1.0 * a;
        window.paint_quad(
            fill(
                Bounds::new(point(x - px(d / 2.0), y - px(d / 2.0)), size(px(d), px(d))),
                color.opacity(0.3 + 0.7 * a),
            )
            .corner_radii(px(d / 2.0)),
        );
    }
}
