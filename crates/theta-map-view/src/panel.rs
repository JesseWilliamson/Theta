//! The GPUI entity: toolbar, canvas, overlays, input handling.

use crate::core::{Core, grid_spec};
use crate::layout::card_origin;
use crate::measure_tool::format_units;
use crate::model::{MapEvent, MapFeature, MapLayer};
use crate::layers::{Legend, LegendBody};
use crate::palette::layer_color;
use crate::props::card_rows;
use crate::scene::{Colors, paint_map};
use crate::world::{Mode, lon_lat_to_world, world_to_lon_lat};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Anchor, AnchoredPositionMode, anchored, deferred, point};
use gpui_kit::{
    AnyElement, App, AsyncApp, ClipboardItem, Context, CursorStyle, DispatchPhase, EventEmitter, FocusHandle,
    FontWeight, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PinchEvent, Render, ScrollDelta,
    ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, canvas, div, hsla,
    px,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use theta_geo::{Coord, Feature, Geometry, describe, to_wkt};
use theta_ui::{CHROME_TEXT_SIZE, EmptyState, PANEL_HEADER_HEIGHT, Settings, data_surface, tool_button};

/// Pointer travel (px) below which a press-release is a click rather than a drag.
const CLICK_SLOP: f64 = 4.0;
/// Pixels panned per arrow / hjkl key press.
const KEY_PAN_PX: f64 = 90.0;
/// Zoom levels per wheel line and per pixel of precise (trackpad) scrolling.
const WHEEL_LEVELS_PER_LINE: f64 = 0.15;
const WHEEL_LEVELS_PER_PX: f64 = 1.0 / 240.0;

struct ContextMenu {
    /// Where the menu opened (viewport px).
    at: [f64; 2],
    /// Pointer position in the data's own coordinates (lon/lat or planar).
    coord: Coord,
    /// Feature under the pointer when it opened.
    feature: Option<usize>,
}

/// Counters of the tile and aggregation workers (see `MapPanel::take_layer_perf`).
#[derive(Clone, Copy, Debug, Default)]
pub struct LayerPerf {
    /// Tiles finished since the last call.
    pub tiles_built: u32,
    /// Tiles in the cache.
    pub tiles_cached: usize,
    /// Memory the cached tiles use.
    pub tile_cache_bytes: usize,
    /// Milliseconds the latest aggregation took on its worker thread.
    pub last_agg_ms: f64,
    /// Tiles the current view still waits for.
    pub tiles_missing: usize,
}

/// The map viewer entity.
pub struct MapPanel {
    pub(crate) core: Rc<RefCell<Core>>,
    focus: FocusHandle,
    pub(crate) layers_open: bool,
    /// Layer shown in the Layers panel.
    pub(crate) sel_layer: usize,
    menu: Option<ContextMenu>,
    /// A data set is being projected and indexed on a worker.
    preparing: Option<usize>,
    /// Counts builds so a slow one never replaces a newer data set.
    build_gen: u64,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<MapEvent> for MapPanel {}

/// The basemap in force: a test hook (`THETA_TILE_URL_OVERRIDE`) wins over settings.
fn current_basemap(cx: &App) -> crate::basemap::Basemap {
    let mut b = crate::basemap::selected(Settings::get(cx));
    if let Some(url) = std::env::var("THETA_TILE_URL_OVERRIDE").ok().filter(|s| !s.trim().is_empty()) {
        b.template = Some(url);
    }
    b
}

impl MapPanel {
    /// A panel with no content (shows the empty state).
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let core = Rc::new(RefCell::new(Core::new(theta_ui::settings::DEFAULT_TILE_URL.to_string())));
        core.borrow_mut().apply_basemap(&current_basemap(cx));
        let rx = core.borrow().tiles.loader.results.clone();
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            while let Ok(result) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.core.borrow_mut().tiles.accept(result) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let (vrx, arx) = {
            let c = core.borrow();
            (c.ls.pool.results.clone(), c.ls.agg_worker.results.clone())
        };
        let vtask = cx.spawn(async move |this, cx: &mut AsyncApp| {
            while let Ok(done) = vrx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.core.borrow_mut().accept_tile(done) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let atask = cx.spawn(async move |this, cx: &mut AsyncApp| {
            while let Ok(done) = arx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.core.borrow_mut().accept_agg(done) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut tasks = vec![task, vtask, atask];
        if std::env::var_os("THETA_MAP_STATS").is_some() {
            // Benchmark aid: paint timing and worker counters once a second on stderr.
            tasks.push(cx.spawn(async move |this, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                    let alive = this.update(cx, |this, _| {
                        let (frames, avg, max) = this.take_paint_stats();
                        let lp = this.take_layer_perf();
                        if frames > 0 || lp.tiles_built > 0 {
                            eprintln!(
                                "map: {frames} frames, avg {avg:.2} ms, max {max:.2} ms; tiles built {} cached {} ({} KB) missing {}; aggregation {:.0} ms",
                                lp.tiles_built,
                                lp.tiles_cached,
                                lp.tile_cache_bytes / 1024,
                                lp.tiles_missing,
                                lp.last_agg_ms
                            );
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }));
        }
        MapPanel {
            core,
            focus: cx.focus_handle(),
            layers_open: false,
            sel_layer: 0,
            menu: None,
            preparing: None,
            build_gen: 0,
            _tasks: tasks,
        }
    }

    /// Replace the content and fit the camera to it.
    ///
    /// Projecting, indexing and summarising the features is the slow part (seconds for a
    /// few hundred thousand rows), so it runs on a background thread and the window stays
    /// responsive; the previous content stays up, with a "Preparing" chip, until it is done.
    pub fn set_features(
        &mut self,
        layers: Vec<MapLayer>,
        features: Arc<Vec<MapFeature>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.build_gen += 1;
        let gen_ = self.build_gen;
        self.preparing = Some(features.len());
        self.layers_open = false;
        self.menu = None;
        let task = cx.background_executor().spawn(async move { crate::core::build(layers, features) });
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let built = task.await;
            this.update(cx, |this, cx| {
                if this.build_gen != gen_ {
                    return;
                }
                this.preparing = None;
                this.sel_layer = 0;
                this.core.borrow_mut().install(built);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Remove all content.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.build_gen += 1;
        self.preparing = None;
        self.core.borrow_mut().clear();
        self.layers_open = false;
        self.menu = None;
        cx.notify();
    }

    /// Highlight the feature(s) of a result row and optionally fly to them.
    pub fn select_row(&mut self, row: Option<usize>, zoom_to: bool, cx: &mut Context<Self>) {
        self.core.borrow_mut().select_row(row, zoom_to);
        cx.notify();
    }

    /// `(frames, average ms, max ms)` of the paint work since the last call (CPU time spent
    /// building the scene, not GPU time); for benchmarks and the demo's stats line.
    pub fn take_paint_stats(&self) -> (u32, f64, f64) {
        let mut core = self.core.borrow_mut();
        let (n, total, max) = std::mem::take(&mut core.paint_stats);
        (n, if n > 0 { total / f64::from(n) } else { 0.0 }, max)
    }

    /// Background work counters since the last call, for benchmarks and the demo's stats line.
    pub fn take_layer_perf(&self) -> LayerPerf {
        let mut core = self.core.borrow_mut();
        let built = std::mem::take(&mut core.ls.perf.tiles_built);
        LayerPerf {
            tiles_built: built,
            tiles_cached: core.ls.tiles.len(),
            tile_cache_bytes: core.ls.tiles.bytes(),
            last_agg_ms: core.ls.perf.last_agg_ms,
            tiles_missing: core.ls.tiles_missing,
        }
    }

    /// The focus handle of the map surface (keyboard shortcuts work while it is focused).
    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }

    // ---- commands -------------------------------------------------------------------

    fn fit(&mut self, cx: &mut Context<Self>) {
        self.core.borrow_mut().fit_all();
        cx.notify();
    }

    fn zoom(&mut self, delta: f64, cx: &mut Context<Self>) {
        let mut core = self.core.borrow_mut();
        let s = core.camera.size();
        core.zoom_by(delta, [s[0] / 2.0, s[1] / 2.0]);
        drop(core);
        cx.notify();
    }

    fn toggle_measure(&mut self, cx: &mut Context<Self>) {
        let mut core = self.core.borrow_mut();
        core.measuring = !core.measuring;
        let metric = core.metric();
        core.measure = crate::measure_tool::Measurement::new(metric);
        core.hover = None;
        drop(core);
        self.menu = None;
        cx.notify();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let text = crate::export::geojson(&self.core.borrow());
        cx.emit(MapEvent::ExportGeoJson(text));
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        cx.emit(MapEvent::CopyText(text));
    }

    // ---- input ----------------------------------------------------------------------

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        if k.modifiers.shift && k.key == "l" {
            // Shift-L opens the Layers panel (plain L pans right in the Helix keymap).
            if !self.core.borrow().is_empty() {
                self.layers_open = !self.layers_open;
                self.menu = None;
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let modal = Settings::get(cx).keymap.is_modal();
        let key = k.key.as_str();
        let ch = k.key_char.as_deref().unwrap_or("");
        let handled = match key {
            "escape" => {
                self.escape();
                true
            }
            "left" => self.pan_key(-KEY_PAN_PX, 0.0),
            "right" => self.pan_key(KEY_PAN_PX, 0.0),
            "up" => self.pan_key(0.0, -KEY_PAN_PX),
            "down" => self.pan_key(0.0, KEY_PAN_PX),
            "h" if modal => self.pan_key(-KEY_PAN_PX, 0.0),
            "l" if modal => self.pan_key(KEY_PAN_PX, 0.0),
            "k" if modal => self.pan_key(0.0, -KEY_PAN_PX),
            "j" if modal => self.pan_key(0.0, KEY_PAN_PX),
            "0" => {
                self.core.borrow_mut().fit_all();
                true
            }
            "m" => {
                self.toggle_measure(cx);
                true
            }
            _ if key == "=" || key == "+" || ch == "+" => {
                self.zoom(1.0, cx);
                true
            }
            _ if key == "-" || ch == "-" => {
                self.zoom(-1.0, cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn pan_key(&mut self, dx: f64, dy: f64) -> bool {
        self.core.borrow_mut().nudge(dx, dy);
        true
    }

    fn escape(&mut self) {
        if self.menu.take().is_some() || std::mem::take(&mut self.layers_open) {
            return;
        }
        let mut core = self.core.borrow_mut();
        if core.measuring {
            core.measuring = false;
            let metric = core.metric();
            core.measure = crate::measure_tool::Measurement::new(metric);
        } else {
            core.selected_row = None;
            core.pinned = None;
        }
    }

    fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        origin: [f64; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let p = [f64::from(ev.position.x) - origin[0], f64::from(ev.position.y) - origin[1]];
        self.menu = None;
        self.layers_open = false;
        let mut core = self.core.borrow_mut();
        match ev.button {
            MouseButton::Left => {
                core.anim.cancel();
                core.drag = Some(crate::core::Drag { last: p, travel: 0.0 });
                if ev.click_count >= 2 {
                    if core.measuring {
                        core.measure.close();
                    } else {
                        core.zoom_by(1.0, p);
                    }
                }
            }
            MouseButton::Right => {
                let feature = if core.measuring { None } else { core.hit_at(p) };
                let coord = core.camera.unproject(p);
                drop(core);
                self.menu = Some(ContextMenu { at: p, coord, feature });
            }
            _ => {}
        }
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, origin: [f64; 2], hovered: bool, cx: &mut Context<Self>) {
        let p = [f64::from(ev.position.x) - origin[0], f64::from(ev.position.y) - origin[1]];
        let mut core = self.core.borrow_mut();
        if let Some(d) = core.drag.as_mut() {
            if ev.pressed_button == Some(MouseButton::Left) {
                let (dx, dy) = (p[0] - d.last[0], p[1] - d.last[1]);
                d.last = p;
                d.travel += dx.hypot(dy);
                if d.travel > CLICK_SLOP {
                    core.camera.pan_by(dx, dy);
                    core.hover = None;
                }
                core.pointer = Some(p);
                drop(core);
                cx.notify();
                return;
            }
            core.drag = None;
        }
        if !hovered {
            if core.pointer.take().is_some() || core.hover.take().is_some() {
                drop(core);
                cx.notify();
            }
            return;
        }
        core.pointer = Some(p);
        let hit = if core.measuring { None } else { core.hit_at(p) };
        core.hover = hit;
        drop(core);
        cx.notify();
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, origin: [f64; 2], cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left {
            return;
        }
        let p = [f64::from(ev.position.x) - origin[0], f64::from(ev.position.y) - origin[1]];
        let mut core = self.core.borrow_mut();
        let Some(d) = core.drag.take() else { return };
        if d.travel > CLICK_SLOP || ev.click_count >= 2 {
            return;
        }
        if core.measuring {
            let c = core.measure_coord_at(p);
            core.measure.push(c);
            drop(core);
            cx.notify();
            return;
        }
        if let Some((_, c)) = core.cluster_under(p)
            && c.count > 1
        {
            core.zoom_to_cluster(&c);
            drop(core);
            cx.notify();
            return;
        }
        match core.hit_at(p) {
            Some(i) => {
                let row = core.prepared.features[i].row;
                core.selected_row = Some(row);
                core.pinned = Some(crate::core::Pinned { feature: i, at: p });
                drop(core);
                cx.emit(MapEvent::FeatureClicked { row });
            }
            None => {
                core.selected_row = None;
                core.pinned = None;
            }
        }
        cx.notify();
    }

    fn scroll(&mut self, ev: &ScrollEvent, origin: [f64; 2], cx: &mut Context<Self>) {
        let p = [f64::from(ev.position.x) - origin[0], f64::from(ev.position.y) - origin[1]];
        let dy = match ev.delta {
            ScrollDelta::Lines(l) => f64::from(l.y) * WHEEL_LEVELS_PER_LINE,
            ScrollDelta::Pixels(d) => f64::from(d.y) * WHEEL_LEVELS_PER_PX,
        };
        if dy == 0.0 {
            return;
        }
        self.core.borrow_mut().zoom_by(dy, p);
        cx.notify();
    }

    fn pinch(&mut self, ev: &PinchEvent, origin: [f64; 2], cx: &mut Context<Self>) {
        let p = [f64::from(ev.position.x) - origin[0], f64::from(ev.position.y) - origin[1]];
        // `delta` is a relative scale change (0.1 = +10%), i.e. log2(1 + d) levels.
        let levels = f64::from(1.0 + ev.delta).max(0.01).log2();
        self.core.borrow_mut().zoom_by(levels, p);
        cx.notify();
    }
}

// ---- rendering ------------------------------------------------------------------------

pub(crate) struct Theme {
    pub(crate) bg: Hsla,
    pub(crate) fg: Hsla,
    pub(crate) muted_fg: Hsla,
    pub(crate) border: Hsla,
    pub(crate) popover: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) warning: Hsla,
}

pub(crate) fn chip(t: &Theme) -> gpui_kit::Div {
    h_flex()
        .gap_1p5()
        .items_center()
        .px_2()
        .h(px(22.))
        .rounded(px(5.))
        .border_1()
        .border_color(t.border)
        .bg(t.popover.opacity(0.94))
        .text_size(px(11.))
        .text_color(t.fg)
}

pub(crate) fn muted_text(t: &Theme, s: impl Into<SharedString>) -> gpui_kit::Div {
    div().text_size(px(11.)).text_color(t.muted_fg).child(s.into())
}

fn menu_item(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    t: &Theme,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .text_size(px(12.))
        .text_color(if enabled { t.fg } else { t.muted_fg.opacity(0.6) })
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(t.hover)))
        .child(label)
}

/// Text with a 1px halo in the background colour, legible on any tile colour.
fn haloed(t: &Theme, text: String, color: Hsla) -> AnyElement {
    let halo = t.bg.opacity(0.9);
    let copy = |dx: f32, dy: f32| {
        div()
            .absolute()
            .left(px(dx))
            .top(px(dy))
            .whitespace_nowrap()
            .text_size(px(10.))
            .text_color(halo)
            .child(text.clone())
    };
    div()
        .relative()
        .child(copy(-1., 0.))
        .child(copy(1., 0.))
        .child(copy(0., -1.))
        .child(copy(0., 1.))
        .child(div().relative().whitespace_nowrap().text_size(px(10.)).text_color(color).child(text.clone()))
        .into_any_element()
}

/// The scale bar: label over a 1px rule with a small tick at each end, bottom-right, `above`
/// px up from the bottom edge (clear of the attribution).
fn scale_bar(t: &Theme, bar_px: f32, label: String, above: f32) -> AnyElement {
    let ink = t.fg.opacity(0.8);
    let rule = |w: f32, h: f32, x: f32, y: f32, c: Hsla| {
        div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(c)
    };
    let halo = t.bg.opacity(0.55);
    v_flex()
        .absolute()
        .right(px(8.))
        .bottom(px(above))
        .items_end()
        .gap(px(1.))
        .child(haloed(t, label, t.fg.opacity(0.8)))
        .child(
            div()
                .relative()
                .w(px(bar_px + 2.))
                .h(px(6.))
                // Halo first (3px wider than the ink), then the ink on top.
                .child(rule(bar_px + 2., 3., 0., 2., halo))
                .child(rule(3., 6., 0., 0., halo))
                .child(rule(3., 6., bar_px - 1., 0., halo))
                .child(rule(bar_px, 1., 1., 3., ink))
                .child(rule(1., 5., 1., 0., ink))
                .child(rule(1., 5., bar_px, 0., ink)),
        )
        .into_any_element()
}

/// Show `el` at `at` (px from the map's top-left) on the window-level overlay layer, clamped
/// to stay inside the window: a hover card, menu or popover is then never clipped by the
/// bottom panel's bounds, however short the panel is dragged.
pub(crate) fn floating(at: [f64; 2], el: impl IntoElement) -> AnyElement {
    // `Local` positions are relative to where the anchored element sits in the layout, so it is
    // pinned to the top-left corner of the map area by a zero-size absolute wrapper: otherwise it
    // sits after the map canvas and every card and menu lands a canvas-height too low.
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_0()
        .child(
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Local)
                    .position(point(px(at[0] as f32), px(at[1] as f32)))
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(el),
            )
            .with_priority(2),
        )
        .into_any_element()
}

impl MapPanel {
    fn toolbar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let core = self.core.borrow();
        let has_data = !core.is_empty();
        let geographic = core.prepared.mode == Mode::Geographic;
        let (basemap, dim, measuring) = (core.basemap, core.dim, core.measuring);
        let basemap_name = core.basemap_info.name.clone();
        let dark_basemap = core.basemap_info.dark;
        let n_visible = core.prepared.features.iter().filter(|f| core.layer_visible(f)).count();
        drop(core);
        let sep = || div().w(px(1.)).h(px(14.)).mx_1().bg(t.border);
        h_flex()
            .h(PANEL_HEADER_HEIGHT)
            .flex_shrink_0()
            .px_3()
            .gap_0p5()
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(CHROME_TEXT_SIZE)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.fg)
                            .child("Map"),
                    )
                    .when(has_data, |d| {
                        d.child(muted_text(
                            t,
                            format!("{n_visible} feature{}", if n_visible == 1 { "" } else { "s" }),
                        ))
                    }),
            )
            .child(
                tool_button("map-fit", IconName::Maximize, "Fit to data (0)")
                    .disabled(!has_data)
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
            .child(
                tool_button("map-zoom-in", IconName::ZoomIn, "Zoom in (+)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.0, cx))),
            )
            .child(
                tool_button("map-zoom-out", IconName::ZoomOut, "Zoom out (-)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(-1.0, cx))),
            )
            .child(sep())
            .child(
                tool_button("map-basemap", IconName::Map, format!("Basemap: {basemap_name}"))
                    .disabled(!geographic)
                    .selected(basemap && geographic)
                    .dropdown_menu(move |mut menu, _, cx| {
                        let current = crate::basemap::selected(Settings::get(cx)).id;
                        for b in crate::basemap::all(Settings::get(cx)) {
                            let id = b.id.clone();
                            menu = menu.item(
                                PopupMenuItem::new(b.name.clone()).checked(b.id == current).on_click(
                                    move |_, _, cx| {
                                        let id = id.clone();
                                        Settings::update(cx, move |s| s.map_basemap = id);
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            )
            .child(
                tool_button(
                    "map-dim",
                    IconName::Contrast,
                    if dim { "Don't dim tiles in dark themes" } else { "Dim tiles in dark themes" },
                )
                .disabled(!geographic || !basemap || dark_basemap)
                .selected(dim && geographic && basemap && !dark_basemap)
                .on_click(cx.listener(|this, _, _, cx| {
                    let mut c = this.core.borrow_mut();
                    c.dim = !c.dim;
                    drop(c);
                    cx.notify();
                })),
            )
            .child(sep())
            .child(
                tool_button("map-measure", IconName::Ruler, "Measure distance and area (m)")
                    .disabled(!has_data)
                    .selected(measuring)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_measure(cx))),
            )
            .child(
                tool_button("map-layers", IconName::Layers, "Layers")
                    .disabled(!has_data)
                    .selected(self.layers_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layers_open = !this.layers_open;
                        this.menu = None;
                        cx.notify();
                    })),
            )
            .child(
                tool_button("map-export", IconName::Download, "Export GeoJSON")
                    .disabled(!has_data)
                    .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
            )
    }

    fn feature_card(&self, core: &Core, id: usize, t: &Theme, pinned: bool) -> AnyElement {
        let f = &core.prepared.features[id];
        let src: &MapFeature = &core.source[f.src];
        let title = src.label.clone().unwrap_or_else(|| format!("Row {}", src.row + 1));
        let (rows, more) = card_rows(&src.properties);
        let layer = core.layers.get(f.layer);
        let color = layer_color(f.layer, true);
        let swatch = hsla(color.h, color.s, color.l, 1.0);
        v_flex()
            .w(px(CARD_W as f32))
            .gap_1()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(if pinned { t.accent.opacity(0.6) } else { t.border })
            .bg(t.popover)
            .shadow_md()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(div().size(px(8.)).rounded_full().flex_shrink_0().bg(swatch))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.fg)
                            .truncate()
                            .child(title),
                    )
                    .when(core.layers.len() > 1, |d| {
                        d.children(
                            layer.map(|l| {
                                div().ml_auto().flex_shrink_0().child(muted_text(t, l.name.clone()))
                            }),
                        )
                    }),
            )
            .child(muted_text(t, describe(&src.feature)))
            .when(!rows.is_empty(), |d| {
                d.child(div().h(px(1.)).my_0p5().bg(t.border)).children(rows.into_iter().map(|(k, v)| {
                    h_flex()
                        .gap_2()
                        .items_baseline()
                        .child(div().w(px(80.)).flex_shrink_0().truncate().child(muted_text(t, k)))
                        .child(
                            div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(t.fg).child(v),
                        )
                }))
            })
            .when(more > 0, |d| d.child(muted_text(t, format!("+{more} more"))))
            .into_any_element()
    }

    fn overlays(&self, t: &Theme, size: [f64; 2], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let core = self.core.borrow();
        let mut out: Vec<AnyElement> = Vec::new();

        // Warning / status chips, top-left.
        let mut chips = v_flex().absolute().top(px(8.)).left(px(8.)).gap_1();
        if core.prepared.skipped > 0 {
            let n = core.prepared.skipped;
            chips = chips.child(
                chip(t)
                    .border_color(t.warning.opacity(0.6))
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.)).text_color(t.warning))
                    .child(format!("{n} feature{} skipped: unknown SRID", if n == 1 { "" } else { "s" })),
            );
        }
        if let Some(n) = self.preparing {
            chips = chips.child(chip(t).child(format!("Preparing {n} feature{}\u{2026}", if n == 1 { "" } else { "s" })));
        } else if core.ls.tiles_missing > 0 || core.agg_pending() {
            chips = chips.child(chip(t).child("Updating layers\u{2026}"));
        }
        if core.prepared.mode == Mode::Geographic && core.basemap && core.tiles.offline() {
            chips = chips.child(
                chip(t)
                    .child(Icon::new(IconName::WifiOff).size(px(12.)).text_color(t.muted_fg))
                    .child("Basemap unavailable \u{2014} offline?")
                    .child(Button::new("map-retry").ghost().xsmall().label("Retry").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.core.borrow_mut().tiles.retry();
                            cx.notify();
                        },
                    ))),
            );
        }
        if core.measuring && core.measure.points.is_empty() {
            chips = chips.child(
                chip(t).child("Click to add points \u{00b7} double-click to close \u{00b7} Esc to exit"),
            );
        }
        out.push(chips.into_any_element());

        // Grid labels.
        if core.grid_shown {
            let spec = grid_spec(&core.camera);
            for l in spec.vertical.iter().filter(|l| l.at > 24.0 && l.at < size[0] - 24.0) {
                out.push(
                    div()
                        .absolute()
                        .left(px(l.at as f32 + 3.0))
                        .bottom(px(2.))
                        .text_size(px(10.))
                        .text_color(t.muted_fg.opacity(0.9))
                        .child(l.label.clone())
                        .into_any_element(),
                );
            }
            for l in spec.horizontal.iter().filter(|l| l.at > 14.0 && l.at < size[1] - 40.0) {
                out.push(
                    div()
                        .absolute()
                        .top(px(l.at as f32 - 13.0))
                        .left(px(3.))
                        .text_size(px(10.))
                        .text_color(t.muted_fg.opacity(0.9))
                        .child(l.label.clone())
                        .into_any_element(),
                );
            }
        }

        // Attribution (always shown with a basemap) in the bottom-right corner, and a quiet
        // scale bar just above it: no box, a 1px rule with end ticks and a haloed label, so
        // it reads over light and dark tiles without drawing the eye.
        let attribution = (core.prepared.mode == Mode::Geographic && core.basemap)
            .then(|| crate::basemap::attribution_text(&core.basemap_info))
            .filter(|s| !s.is_empty());
        if let Some(mpp) = core.camera.meters_per_pixel() {
            let (bar_px, label) = theta_geo::nice_scale_bar(mpp, 100.0);
            if bar_px > 1.0 {
                let above = if attribution.is_some() { 20. } else { 6. };
                out.push(scale_bar(t, bar_px as f32, label, above));
            }
        }
        if let Some(text) = attribution {
            out.push(
                div()
                    .absolute()
                    .right(px(0.))
                    .bottom(px(0.))
                    .max_w(px((size[0] - 8.0).max(80.0) as f32))
                    .px_1p5()
                    .py(px(1.))
                    .rounded_tl(px(4.))
                    .bg(t.bg.opacity(0.75))
                    .text_size(px(10.))
                    .text_color(t.muted_fg)
                    .child(text)
                    .into_any_element(),
            );
        }

        // Measure labels.
        if !core.measure.points.is_empty() {
            let m = &core.measure;
            let screen: Vec<[f64; 2]> = m.points.iter().map(|c| core.measure_screen(*c)).collect();
            let segs = m.segments();
            for (i, d) in segs.iter().enumerate() {
                let (a, b) = (screen[i], screen[(i + 1) % screen.len()]);
                if (a[0] - b[0]).hypot(a[1] - b[1]) < 46.0 {
                    continue;
                }
                let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                out.push(
                    div()
                        .absolute()
                        .left(px(mid[0] as f32 - 28.))
                        .top(px(mid[1] as f32 - 18.))
                        .child(chip(t).h(px(18.)).px_1().text_size(px(10.)).child(m.format_len(*d)))
                        .into_any_element(),
                );
            }
            if screen.len() >= 2 {
                let mut summary = format!("Total {}", m.format_len(m.total()));
                if let Some(a) = m.area() {
                    summary.push_str(&format!("  \u{00b7}  Area {}", m.format_area(a)));
                }
                out.push(
                    div()
                        .absolute()
                        .bottom(px(30.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(chip(t).font_weight(FontWeight::MEDIUM).child(summary))
                        .into_any_element(),
                );
            }
        }

        // Cards.
        let hovered = core.hover.filter(|h| core.pinned.is_none_or(|p| p.feature != *h));
        if let (Some(h), Some(p)) = (hovered, core.pointer)
            && core.drag.is_none()
            && self.menu.is_none()
        {
            let o = card_origin(p, [CARD_W, card_height(&core, h)], size, 14.0);
            out.push(floating(o, self.feature_card(&core, h, t, false)));
        }
        if let Some(p) = core.pinned.filter(|p| p.feature < core.prepared.features.len()) {
            let o = card_origin(p.at, [CARD_W, card_height(&core, p.feature)], size, 14.0);
            out.push(floating(o, self.feature_card(&core, p.feature, t, true)));
        }

        // Layers popover.
        if self.layers_open {
            drop(core);
            out.push(self.layers_popover(t, size, cx));
            return self.overlays_tail(t, size, out, cx);
        }

        self.overlays_tail_with(t, size, out, cx, core)
    }

    fn overlays_tail(&self, t: &Theme, size: [f64; 2], out: Vec<AnyElement>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let core = self.core.borrow();
        self.overlays_tail_with(t, size, out, cx, core)
    }

    fn overlays_tail_with(
        &self,
        t: &Theme,
        size: [f64; 2],
        mut out: Vec<AnyElement>,
        cx: &mut Context<Self>,
        core: std::cell::Ref<'_, Core>,
    ) -> Vec<AnyElement> {
        // Legends, bottom-left.
        let legends = core.legends();
        if !legends.is_empty() {
            out.push(legend_overlay(t, &legends));
        }
        // Hover value of a hexagon or grid cell.
        if let Some(p) = core.pointer
            && core.drag.is_none()
            && self.menu.is_none()
            && core.hover.is_none()
            && let Some(text) = core.bin_hover(p)
        {
            let o = card_origin(p, [180.0, 24.0], size, 14.0);
            out.push(floating(o, chip(t).child(text)));
        }
        // Context menu.
        if let Some(m) = &self.menu {
            let coord = m.coord;
            let geographic = core.prepared.mode == Mode::Geographic;
            let feature = m.feature;
            let o = card_origin(m.at, [190.0, 82.0], size, 2.0);
            let coord_text = crate::export::coord_text(coord, geographic);
            let wkt_point =
                geographic.then(|| to_wkt(&Feature::new(Geometry::Point(Some(coord)), Some(4326)), Some(6)));
            let wkt_point = wkt_point
                .unwrap_or_else(|| to_wkt(&Feature::new(Geometry::Point(Some(coord)), None), Some(3)));
            let feature_wkt =
                feature.map(|i| to_wkt(&core.source[core.prepared.features[i].src].feature, None));
            let has_feature = feature_wkt.is_some();
            out.push(floating(
                o,
                v_flex()
                    .w(px(190.))
                    .p_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .shadow_md()
                    .occlude()
                    .child(menu_item("map-cm-coords", "Copy coordinates", true, t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.menu = None;
                            this.copy(coord_text.clone(), cx);
                            cx.notify();
                        },
                    )))
                    .child(menu_item("map-cm-point", "Copy as WKT point", true, t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.menu = None;
                            this.copy(wkt_point.clone(), cx);
                            cx.notify();
                        },
                    )))
                    .child(menu_item("map-cm-feature", "Copy feature WKT", has_feature, t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            if let Some(w) = feature_wkt.clone() {
                                this.copy(w, cx);
                            }
                            cx.notify();
                        }),
                    )),
            ));
        }
        out
    }

    fn status_bar(&self, t: &Theme) -> impl IntoElement {
        let core = self.core.borrow();
        let geographic = core.prepared.mode == Mode::Geographic;
        let pointer = core.pointer.map(|p| {
            let c = core.camera.unproject(p);
            if geographic {
                format!("Lon {:.5}  Lat {:.5}", c[0], c[1])
            } else {
                format!("X {}  Y {}", format_units(c[0]), format_units(c[1]))
            }
        });
        let zoom = if geographic {
            format!("Zoom {:.1}", core.camera.level())
        } else {
            let t = core.camera.transform();
            format!("{} units/px", format_units(1.0 / t.scale[0].abs()))
        };
        let n = core.prepared.features.iter().filter(|f| core.layer_visible(f)).count();
        let mut parts: Vec<String> = Vec::new();
        parts.push(pointer.unwrap_or_else(|| "\u{2014}".into()));
        parts.push(zoom);
        parts.push(format!("{n} feature{}", if n == 1 { "" } else { "s" }));
        if !core.prepared.crs_note.is_empty() {
            parts.push(core.prepared.crs_note.clone());
        }
        h_flex()
            .h(px(22.))
            .flex_shrink_0()
            .px_3()
            .items_center()
            .border_t_1()
            .border_color(t.border)
            .gap_3()
            .overflow_hidden()
            .children(parts.into_iter().enumerate().map(|(i, p)| {
                div()
                    .flex_shrink_0()
                    .when(i == 0, |d| d.min_w(px(150.)))
                    .text_size(px(11.))
                    .text_color(t.muted_fg)
                    .whitespace_nowrap()
                    .child(p)
            }))
    }
}

fn rgb_hsla(c: [u8; 3]) -> Hsla {
    Hsla::from(gpui_kit::Rgba { r: f32::from(c[0]) / 255.0, g: f32::from(c[1]) / 255.0, b: f32::from(c[2]) / 255.0, a: 1.0 })
}

/// Legends of colour-by-column and aggregation layers, bottom-left.
fn legend_overlay(t: &Theme, legends: &[Legend]) -> AnyElement {
    v_flex()
        .absolute()
        .left(px(8.))
        .bottom(px(8.))
        .gap_1()
        .children(legends.iter().map(|l| {
            let body = match &l.body {
                LegendBody::Classes(entries) => v_flex()
                    .gap(px(3.))
                    .children(entries.iter().map(|(c, label)| {
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(div().size(px(10.)).rounded(px(2.)).flex_shrink_0().bg(rgb_hsla(*c)))
                            .child(
                                div().text_size(px(11.)).text_color(t.fg).whitespace_nowrap().truncate().child(label.clone()),
                            )
                    }))
                    .into_any_element(),
                LegendBody::Gradient { colors, low, high } => {
                    let n = colors.len().max(1);
                    v_flex()
                        .gap(px(2.))
                        .child(h_flex().w(px(150.)).h(px(8.)).rounded(px(2.)).overflow_hidden().children(
                            colors.iter().map(|c| div().h_full().flex_1().bg(rgb_hsla(*c))).take(n),
                        ))
                        .child(
                            h_flex()
                                .w(px(150.))
                                .justify_between()
                                .child(muted_text(t, low.clone()))
                                .child(muted_text(t, high.clone())),
                        )
                        .into_any_element()
                }
            };
            v_flex()
                .gap_1()
                .p_2()
                .max_w(px(220.))
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .bg(t.popover.opacity(0.94))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.fg)
                        .truncate()
                        .child(l.title.clone()),
                )
                .child(body)
        }))
        .into_any_element()
}

const CARD_W: f64 = 260.0;

/// Estimated rendered card height, for placing it before it has been measured.
fn card_height(core: &Core, id: usize) -> f64 {
    let src = &core.source[core.prepared.features[id].src];
    let rows = src.properties.len().min(crate::props::MAX_ROWS);
    let more = usize::from(src.properties.len() > crate::props::MAX_ROWS);
    60.0 + 17.0 * (rows + more) as f64 + if rows > 0 { 8.0 } else { 0.0 }
}

impl Render for MapPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let surface = data_surface(cx);
        let t = Theme {
            bg: surface,
            fg: theme.foreground,
            muted_fg: theme.muted_foreground,
            border: theme.border,
            popover: theme.popover,
            hover: theme.list_hover,
            accent: theme.primary,
            warning: theme.warning,
        };
        let colors = Colors {
            bg: surface,
            fg: theme.foreground,
            placeholder: surface.blend(theme.foreground.opacity(0.035)),
            accent: theme.primary,
            dark: theme.is_dark(),
        };
        let _ = window;
        let basemap = current_basemap(cx);
        self.core.borrow_mut().apply_basemap(&basemap);

        let empty = self.core.borrow().is_empty();
        let size = self.core.borrow().camera.size();

        let root = v_flex()
            .id("theta-map")
            .size_full()
            .bg(t.bg)
            .key_context("ThetaMap")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.key_down(ev, cx)))
            .child(self.toolbar(&t, cx));

        if empty {
            return root
                .child(div().flex_1().child(
                    EmptyState::new(IconName::MapPinOff, "No geometry in this result").description(
                        "Run a query that returns a geometry or geography column (PostGIS, MySQL spatial) or WKT/GeoJSON text.",
                    ),
                ))
                .into_any_element();
        }

        let core = self.core.clone();
        let entity = cx.entity().downgrade();
        let surface = canvas(
            move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, cx| {
                let origin = [f64::from(bounds.origin.x), f64::from(bounds.origin.y)];
                let sz = [f64::from(bounds.size.width), f64::from(bounds.size.height)];
                {
                    let mut c = core.borrow_mut();
                    if sz[0] > 1.0 && sz[1] > 1.0 {
                        if c.camera.size() != sz {
                            c.camera.set_size(sz);
                        }
                        if c.pending_fit {
                            c.pending_fit = false;
                            c.camera = c.fit_camera(false, sz);
                        }
                        c.size_known = true;
                    }
                    let more = c.step_anim(Instant::now());
                    let grid_before = c.grid_shown;
                    let updating_before = c.ls.tiles_missing > 0 || c.agg_pending();
                    let started = Instant::now();
                    paint_map(&mut c, bounds, &colors, window, cx);
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    c.paint_stats = (c.paint_stats.0 + 1, c.paint_stats.1 + ms, c.paint_stats.2.max(ms));
                    // Labels are rendered before paint from `grid_shown`; one more frame fixes them.
                    // The "Updating layers" chip is rendered from the previous paint's state: one more
                    // frame when that changed keeps it from lingering after the work is done.
                    let updating = c.ls.tiles_missing > 0 || c.agg_pending();
                    if more || c.grid_shown != grid_before || updating != updating_before {
                        window.request_animation_frame();
                    }
                    let cursor = if c.measuring {
                        CursorStyle::Crosshair
                    } else if c.drag.as_ref().is_some_and(|d| d.travel > CLICK_SLOP) {
                        CursorStyle::ClosedHand
                    } else if c.hover.is_some() || c.pointer.is_some_and(|p| c.cluster_under(p).is_some()) {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::OpenHand
                    };
                    window.set_cursor_style(cursor, &hitbox);
                }
                let (e, hb) = (entity.clone(), hitbox.clone());
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                        e.update(cx, |this, cx| this.mouse_down(ev, origin, window, cx)).ok();
                    }
                });
                let (e, hb) = (entity.clone(), hitbox.clone());
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let hovered = hb.is_hovered(window);
                        e.update(cx, |this, cx| this.mouse_move(ev, origin, hovered, cx)).ok();
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.mouse_up(ev, origin, cx)).ok();
                    }
                });
                let (e, hb) = (entity.clone(), hitbox.clone());
                window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb.should_handle_scroll(window) {
                        e.update(cx, |this, cx| this.scroll(ev, origin, cx)).ok();
                    }
                });
                let (e, hb) = (entity.clone(), hitbox.clone());
                window.on_mouse_event(move |ev: &PinchEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb.should_handle_scroll(window) {
                        e.update(cx, |this, cx| this.pinch(ev, origin, cx)).ok();
                    }
                });
            },
        )
        .size_full();

        let overlays = self.overlays(&t, size, cx);
        root.child(
            div()
                .id("map-area")
                .flex_1()
                .min_h_0()
                .relative()
                .overflow_hidden()
                .child(surface)
                .children(overlays),
        )
        .child(self.status_bar(&t))
        .into_any_element()
    }
}

#[allow(dead_code)]
fn _unused(_: (f64, f64)) -> Coord {
    world_to_lon_lat(lon_lat_to_world([0.0, 0.0]))
}
