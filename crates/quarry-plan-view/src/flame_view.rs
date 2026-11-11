//! Flame (icicle) graph painted on a canvas, and the raw-output view.

use crate::flame::FlameMetric;
use crate::format::*;
use crate::model::{Family, label, node_at};
use crate::panel::PlanPanel;
use crate::ui::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Editor;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnchoredPositionMode, AnyElement, BorderStyle, Bounds, Context, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, TextAlign, TextRun, Window, anchored,
    canvas, deferred, div, point, px, quad, relative, size,
};
use quarry_ui::tool_button;

const MIN_ROW: f32 = 18.0;
const MAX_ROW: f32 = 26.0;
/// Blocks narrower than this are not drawn (and cannot be hit).
const MIN_BLOCK_PX: f32 = 2.0;
/// Blocks narrower than this get no label.
const MIN_LABEL_PX: f32 = 34.0;

struct Block {
    id: usize,
    depth: usize,
    x: f32,
    w: f32,
    label: SharedString,
    color: Hsla,
    sev: Option<Hsla>,
}

fn shape(
    window: &mut Window,
    text: &str,
    font: &gpui_kit::Font,
    size: Pixels,
    color: Hsla,
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

/// Shape `text` to fit `avail`, cutting it with an ellipsis when needed. Starts
/// from a width-proportional guess so a label costs two or three shapings.
fn fit_label(
    window: &mut Window,
    text: &str,
    font: &gpui_kit::Font,
    size: Pixels,
    color: Hsla,
    avail: Pixels,
) -> Option<gpui_kit::ShapedLine> {
    let full = shape(window, text, font, size, color);
    if full.width <= avail {
        return Some(full);
    }
    let cuts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let per_char = f32::from(full.width) / cuts.len().max(1) as f32;
    let mut n = ((f32::from(avail) / per_char.max(1.0)) as usize).saturating_sub(1).min(cuts.len() - 1);
    let build = |n: usize| format!("{}…", &text[..cuts[n]]);
    let mut best: Option<gpui_kit::ShapedLine> = None;
    for _ in 0..4 {
        if n == 0 {
            break;
        }
        let line = shape(window, &build(n), font, size, color);
        if line.width <= avail {
            best = Some(line);
            if n + 1 < cuts.len() {
                n += 1;
                continue;
            }
            break;
        }
        if best.is_some() {
            break;
        }
        n -= 1;
    }
    best
}

impl PlanPanel {
    fn flame_row_height(&self, max_depth: usize) -> f32 {
        let avail = f32::from(self.flame_bounds.get().size.height).max(1.0);
        let h = if avail > 40.0 { (avail - 0.0) / (max_depth + 1) as f32 } else { MAX_ROW };
        h.clamp(MIN_ROW, MAX_ROW).floor()
    }

    pub(crate) fn render_flame(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let l = self.loaded.as_ref().expect("flame renders with a plan");
        let plan = l.input.plan.clone();
        let max_depth = l.flame.max_depth;
        let row_h = self.flame_row_height(max_depth);
        let total_h = row_h * (max_depth + 1) as f32;
        let metric = l.flame.metric;

        let blocks: Vec<Block> = l
            .flame
            .rects
            .iter()
            .filter_map(|r| {
                let node = node_at(&plan.root, l.tree.path_of(r.id))?;
                Some(Block {
                    id: r.id,
                    depth: r.depth,
                    x: r.x,
                    w: r.w,
                    label: label(node, l.input.kind).full().into(),
                    color: family_color(l.tree.node(r.id).family, cx),
                    sev: l.node_sev.get(&r.id).map(|s| severity_color(*s, cx)),
                })
            })
            .collect();

        let selected = self.selected;
        let hovered = self.flame_hover;
        let highlighted = self.hover_node;
        let (fg, primary, warn, border) = (theme.foreground, theme.primary, theme.warning, theme.border);
        let ui_font = theme.font_family.clone();
        let bounds_cell = self.flame_bounds.clone();
        let paint_cell = self.flame_bounds.clone();

        let canvas_el = canvas(
            move |bounds, _, _| {
                bounds_cell.set(bounds);
                bounds
            },
            move |_, bounds: Bounds<Pixels>, window, cx| {
                let width = f32::from(bounds.size.width);
                let mut style = window.text_style();
                style.font_family = ui_font.clone();
                let font = style.font();
                let fsize = px(11.5);
                for b in &blocks {
                    let w_px = b.w * width;
                    if w_px < MIN_BLOCK_PX {
                        continue;
                    }
                    let origin = bounds.origin + point(px(b.x * width), px(b.depth as f32 * row_h));
                    let r = Bounds { origin, size: size(px((w_px - 1.0).max(1.0)), px(row_h - 2.0)) };
                    let is_sel = selected == Some(b.id);
                    let is_hot = hovered == Some(b.id) || highlighted == Some(b.id);
                    let fill = b.color.opacity(if is_sel {
                        0.5
                    } else if is_hot {
                        0.42
                    } else {
                        0.28
                    });
                    let edge = if is_sel {
                        primary
                    } else if highlighted == Some(b.id) {
                        warn
                    } else {
                        b.color.opacity(if is_hot { 1.0 } else { 0.75 })
                    };
                    let bw = if is_sel { 2.0 } else { 1.0 };
                    window.paint_quad(quad(r, px(3.), fill, px(bw), edge, BorderStyle::Solid));
                    let mut text_right = r.size.width - px(6.0);
                    if let Some(sev) = b.sev.filter(|_| w_px >= 22.0) {
                        let dot = Bounds {
                            origin: r.origin + point(r.size.width - px(9.0), px(row_h / 2.0 - 4.0)),
                            size: size(px(6.), px(6.)),
                        };
                        window.paint_quad(quad(dot, px(3.), sev, px(0.), border, BorderStyle::Solid));
                        text_right -= px(10.0);
                    }
                    if w_px >= MIN_LABEL_PX {
                        let avail = text_right - px(6.0);
                        if let Some(line) = fit_label(window, &b.label, &font, fsize, fg, avail) {
                            let y = r.origin.y + (r.size.height - px(14.0)) / 2.0;
                            let _ = line.paint(
                                point(r.origin.x + px(6.0), y),
                                px(14.0),
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    }
                }
                paint_cell.set(bounds);
            },
        )
        .absolute()
        .size_full();

        let hit = {
            let layout_rects = l.flame.clone();
            let cell = self.flame_bounds.clone();
            move |pos: gpui_kit::Point<Pixels>| -> Option<usize> {
                let b = cell.get();
                let width = f32::from(b.size.width);
                if width <= 0.0 {
                    return None;
                }
                let local = pos - b.origin;
                let (x, y) = (f32::from(local.x), f32::from(local.y));
                if x < 0.0 || y < 0.0 {
                    return None;
                }
                layout_rects.hit(x / width, (y / row_h) as usize, width, MIN_BLOCK_PX)
            }
        };
        let hit_move = hit.clone();

        let tooltip = self.flame_hover.and_then(|id| self.flame_tooltip(id, row_h, total_h, cx));

        let legend = h_flex().gap(px(10.)).items_center().children(Family::ALL.into_iter().map(|f| {
            h_flex()
                .gap(px(4.))
                .items_center()
                .child(div().size(px(8.)).rounded(px(2.)).bg(family_color(f, cx).opacity(0.7)))
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(f.label()))
        }));

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(24.))
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.table_head)
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child("Flame graph"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(match metric {
                                        FlameMetric::Time => theme.muted_foreground,
                                        _ => theme.warning,
                                    })
                                    .child(metric.caption()),
                            ),
                    )
                    .child(legend),
            )
            .child(
                v_flex().id("plan-flame-scroll").flex_1().min_h_0().overflow_y_scrollbar().child(
                    div()
                        .id("plan-flame-area")
                        .relative()
                        .w_full()
                        .h(px(total_h))
                        .flex_shrink_0()
                        .child(canvas_el)
                        .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                            let now = hit_move(ev.position);
                            if now != this.flame_hover {
                                this.flame_hover = now;
                                cx.notify();
                            }
                        }))
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if !*hovered && this.flame_hover.is_some() {
                                this.flame_hover = None;
                                cx.notify();
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                window.focus(&this.focus, cx);
                                if let Some(id) = hit(ev.position) {
                                    this.select_node(id, true, cx);
                                }
                            }),
                        )
                        .children(tooltip),
                ),
            )
            .into_any_element()
    }

    /// Floating card for the hovered block, anchored to the block (not the
    /// pointer) so moving inside a block does not re-render.
    fn flame_tooltip(
        &self,
        id: usize,
        row_h: f32,
        total_h: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let l = self.loaded.as_ref()?;
        let rect = l.flame.rect_for(id)?;
        let node = node_at(&l.input.plan.root, l.tree.path_of(id))?;
        let tn = l.tree.node(id);
        let lab = label(node, l.input.kind);
        let width = f32::from(self.flame_bounds.get().size.width);
        const TIP_W: f32 = 270.0;
        let left = (rect.x * width).clamp(0.0, (width - TIP_W - 8.0).max(0.0));
        let below = (rect.depth as f32) * row_h < total_h / 2.0 || rect.depth == 0;

        let line = |k: &str, v: String| {
            h_flex()
                .gap(px(8.))
                .justify_between()
                .child(div().text_color(theme.muted_foreground).child(k.to_string()))
                .child(div().font_family(theme.mono_font_family.clone()).child(v))
        };
        let analyzed = l.input.plan.analyzed;
        let mut rows = v_flex().gap(px(2.)).text_size(px(11.5));
        if analyzed {
            if let Some(i) = tn.incl_ms {
                rows = rows.child(line("Inclusive", fmt_ms(i)));
            }
            if let Some(e) = tn.excl_ms {
                let share =
                    l.tree.time_share(id).map(|s| format!("  ({})", fmt_share(s))).unwrap_or_default();
                rows = rows.child(line("Exclusive", format!("{}{share}", fmt_ms(e))));
            }
            if let Some(a) = node.actual_rows {
                let loops = node.loops.unwrap_or(1.0).max(1.0);
                rows = rows.child(line(
                    "Rows",
                    format!(
                        "{} (est {})",
                        fmt_rows(a * loops),
                        node.plan_rows.map(fmt_rows).unwrap_or_else(|| "–".into())
                    ),
                ));
            }
            if let Some(n) = node.loops.filter(|n| *n > 1.0) {
                rows = rows.child(line("Loops", fmt_rows(n)));
            }
        } else {
            if let Some(c) = node.total_cost {
                rows = rows.child(line("Cost", fmt_cost(c)));
            }
            if let Some(r) = node.plan_rows {
                rows = rows.child(line("Estimated rows", fmt_rows(r)));
            }
        }
        let card = v_flex()
            .w(px(TIP_W))
            .p(px(8.))
            .gap(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .shadow_lg()
            .child(
                v_flex()
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(lab.title))
                    .when(!lab.subtitle.is_empty(), |d| {
                        d.child(
                            div().text_size(px(11.5)).text_color(theme.muted_foreground).child(lab.subtitle),
                        )
                    }),
            )
            .child(rows);
        // On the window overlay layer, clamped to the window: a short panel (or a block at
        // the panel's edge) never clips the card.
        let (y, anchor) = if below {
            ((rect.depth + 1) as f32 * row_h + 4.0, Anchor::TopLeft)
        } else {
            (rect.depth as f32 * row_h - 4.0, Anchor::BottomLeft)
        };
        Some(
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Local)
                    .position(point(px(left), px(y)))
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    pub(crate) fn render_raw(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let l = self.loaded.as_ref().expect("raw renders with a plan");
        let raw = l.input.plan.raw.clone();
        let is_json = raw.trim_start().starts_with(['{', '[']);
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(24.))
                    .flex_shrink_0()
                    .pl_3()
                    .pr_2()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.table_head)
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child(if is_json { "Raw server output · JSON" } else { "Raw server output" }),
                    )
                    .child(
                        tool_button("plan-copy-raw", IconName::Copy, "Copy raw output")
                            .on_click(cx.listener(move |this, _, _, cx| this.copy_text(raw.clone(), cx))),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.raw_editor)
                        .readonly(true)
                        .bordered(false)
                        .appearance(false)
                        .h(relative(1.0)),
                ),
            )
            .into_any_element()
    }
}
