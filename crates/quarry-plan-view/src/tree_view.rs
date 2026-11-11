//! The tree-table view: a virtualized list of 24px rows with fixed numeric
//! columns and a flexing Operation column.

use crate::format::*;
use crate::model::{VisibleRow, label, node_at};
use crate::panel::PlanPanel;
use crate::ui::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, uniform_list,
};
use quarry_ui::Settings;

pub(crate) const ROW_H: f32 = 24.0;
const INDENT: f32 = 14.0;
const W_DOT: f32 = 16.0;
const W_TIME: f32 = 150.0;
const W_ROWS: f32 = 204.0;
const W_COST: f32 = 112.0;
const W_COST_EST: f32 = 190.0;
const W_LOOPS: f32 = 56.0;
const W_BUF: f32 = 128.0;
/// Narrowest Operation column worth showing; below it columns are dropped.
const MIN_OP: f32 = 200.0;

/// Which optional columns fit. Time and Rows are always shown; the rest go in
/// reverse order of importance as the tree gets narrower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub time: bool,
    pub cost: bool,
    pub loops: bool,
    pub buffers: bool,
}

/// An estimate-only plan has no times, loops or buffers to show, so those columns
/// are left out entirely instead of being a wall of dashes.
pub fn columns_for(tree_width: f32, analyzed: bool) -> Columns {
    if !analyzed {
        return Columns { time: false, cost: true, loops: false, buffers: false };
    }
    let mut c = Columns { time: true, cost: true, loops: true, buffers: true };
    let used = |c: &Columns| {
        W_DOT
            + W_TIME
            + W_ROWS
            + if c.cost { W_COST } else { 0.0 }
            + if c.loops { W_LOOPS } else { 0.0 }
            + if c.buffers { W_BUF } else { 0.0 }
    };
    for drop in [0, 1, 2] {
        if tree_width - used(&c) >= MIN_OP {
            break;
        }
        match drop {
            0 => c.buffers = false,
            1 => c.loops = false,
            _ => c.cost = false,
        }
    }
    c
}

impl PlanPanel {
    pub(crate) fn render_tree(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let analyzed_plan = self.loaded.as_ref().is_some_and(|l| l.input.plan.analyzed);
        let cols = columns_for(self.tree_width(), analyzed_plan);
        let Some(l) = self.loaded.as_ref() else { return div().into_any_element() };
        let analyzed = l.input.plan.analyzed;
        let count = l.visible.len();

        let head = |text: &'static str, w: f32, right: bool| {
            div().w(px(w)).flex_shrink_0().px_2().when(right, |d| d.flex().justify_end()).child(text)
        };
        let header = h_flex()
            .h(px(24.))
            .flex_shrink_0()
            .w_full()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.table_head)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.muted_foreground)
            .child(div().w(px(W_DOT)).flex_shrink_0())
            .child(div().flex_1().min_w_0().px_2().child("Operation"))
            .when(cols.time, |d| d.child(head("Time (exclusive)", W_TIME, false)))
            .child(head(if analyzed { "Rows" } else { "Estimated rows" }, W_ROWS, false))
            .when(cols.cost, |d| d.child(head("Cost", if analyzed { W_COST } else { W_COST_EST }, false)))
            .when(cols.loops, |d| d.child(head("Loops", W_LOOPS, true)))
            .when(cols.buffers, |d| d.child(head("Buffers", W_BUF, true)))
            .child(div().w(px(8.)).flex_shrink_0());

        let list = uniform_list(
            "plan-tree-rows",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let rows: Vec<VisibleRow> = match this.loaded.as_ref() {
                    Some(l) => {
                        l.visible[range.start.min(l.visible.len())..range.end.min(l.visible.len())].to_vec()
                    }
                    None => Vec::new(),
                };
                rows.into_iter().map(|r| this.tree_row(r, cols, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.tree_scroll)
        .size_full();

        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .child(Scrollbar::vertical(&self.tree_scroll).viewport_from_layout()),
            )
            .into_any_element()
    }

    fn tree_row(&self, vr: VisibleRow, cols: Columns, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let l = self.loaded.as_ref().expect("rows render with a plan");
        let plan = &l.input.plan;
        let id = vr.id;
        let Some(node) = node_at(&plan.root, l.tree.path_of(id)) else { return div().into_any_element() };
        let tn = l.tree.node(id);
        let lab = label(node, l.input.kind);
        let analyzed = plan.analyzed;
        let selected = self.selected == Some(id);
        let highlighted = self.hover_node == Some(id);
        let fam = family_color(tn.family, cx);
        let sev = l.node_sev.get(&id).copied();
        let num_size = px(Settings::get(cx).buffer_font_size.clamp(11.0, 12.5));

        // ---- operation cell
        let guides = (0..vr.depth).map(|k| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(6.0 + k as f32 * INDENT + 7.0))
                .w(px(1.))
                .bg(theme.border.opacity(0.5))
        });
        let chevron = div()
            .id(("plan-chev", id))
            .w(px(INDENT))
            .h(px(ROW_H))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .when(vr.has_children, |d| {
                d.cursor_pointer()
                    .child(
                        Icon::new(if vr.expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                            .size(px(12.))
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_node(id, cx);
                    }))
            });
        let op = h_flex()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .pl(px(6.0 + vr.depth as f32 * INDENT))
            .gap(px(5.))
            .items_center()
            .children(guides)
            .child(chevron)
            .child(Icon::new(family_icon(tn.family)).size(px(13.)).text_color(fam).flex_shrink_0())
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .child(lab.title.clone()),
            )
            .when(!lab.subtitle.is_empty(), |d| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(lab.subtitle.clone()),
                )
            });

        // ---- numeric cells
        let num = |text: String| {
            div().font_family(mono.clone()).text_size(num_size).whitespace_nowrap().child(text)
        };
        let muted = theme.muted_foreground;

        let time_cell = {
            let share = l.tree.heat_share(id);
            let excl = if analyzed { tn.excl_ms } else { None };
            let bar_share = if analyzed { l.tree.time_share(id) } else { l.tree.cost_share(id) };
            let tip = match (analyzed, tn.incl_ms, tn.excl_ms) {
                (true, Some(inc), Some(ex)) => format!(
                    "Inclusive {} · exclusive {}{}",
                    fmt_ms(inc),
                    fmt_ms(ex),
                    l.tree
                        .time_share(id)
                        .map(|s| format!(" ({} of total)", fmt_share(s)))
                        .unwrap_or_default()
                ),
                (true, _, _) => "No timing reported for this step".to_string(),
                _ => "Estimated only. Run Explain Analyze for timings".to_string(),
            };
            h_flex()
                .id(("plan-time", id))
                .w(px(W_TIME))
                .flex_shrink_0()
                .px_2()
                .gap(px(6.))
                .items_center()
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(div().w(px(12.)).flex_shrink_0().when(l.slowest == Some(id) && analyzed, |d| {
                    d.child(Icon::new(IconName::Flame).size(px(12.)).text_color(theme.danger))
                }))
                .child(div().w(px(60.)).flex_shrink_0().flex().justify_end().child(match excl {
                    Some(ms) => num(fmt_ms(ms)).into_any_element(),
                    None => num("–".into()).text_color(muted.opacity(0.6)).into_any_element(),
                }))
                .child(match (bar_share, share) {
                    (Some(s), Some(h)) => div()
                        .w(px(52.))
                        .flex_shrink_0()
                        .h(px(5.))
                        .rounded(px(2.))
                        .bg(theme.muted.opacity(0.8))
                        .child(
                            div()
                                .h_full()
                                .w(px(((s as f32) * 52.0).max(if s > 0.0 { 2.0 } else { 0.0 })))
                                .rounded(px(2.))
                                .bg(heat_color(heat_level(h), cx))
                                .when(!analyzed, |b| b.opacity(0.55)),
                        )
                        .into_any_element(),
                    _ => div().w(px(52.)).into_any_element(),
                })
        };

        let rows_cell = {
            let loops = node.loops.unwrap_or(1.0).max(1.0);
            let mis =
                misestimate(node.plan_rows, node.actual_rows).filter(|m| m.factor >= MISESTIMATE_THRESHOLD);
            let tip = match (analyzed, node.actual_rows, node.plan_rows) {
                (true, Some(a), est) => format!(
                    "{} rows ({} per loop × {} loop{}){}",
                    fmt_rows(a * loops),
                    fmt_rows(a),
                    fmt_rows(loops),
                    if loops == 1.0 { "" } else { "s" },
                    est.map(|e| format!(" · planner expected {} per loop", fmt_rows(e))).unwrap_or_default()
                ),
                (_, _, Some(e)) => format!("Planner estimate: {} rows", fmt_rows(e)),
                _ => "No row figures reported".to_string(),
            };
            h_flex()
                .id(("plan-rows", id))
                .w(px(W_ROWS))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .flex_shrink_0()
                .px_2()
                .gap(px(8.))
                .items_center()
                .child(div().w(px(54.)).flex_shrink_0().flex().justify_end().child(match node.actual_rows {
                    Some(a) if analyzed => num(fmt_rows(a * loops)).into_any_element(),
                    _ => match node.plan_rows {
                        Some(e) => num(format!("~{}", fmt_rows(e))).text_color(muted).into_any_element(),
                        None => num("–".into()).text_color(muted.opacity(0.6)).into_any_element(),
                    },
                }))
                .child(div().w(px(56.)).flex_shrink_0().child(match (node.plan_rows, analyzed) {
                    (Some(e), true) => {
                        num(format!("est {}", fmt_rows(e))).text_color(muted.opacity(0.85)).into_any_element()
                    }
                    _ => div().into_any_element(),
                }))
                .child(match mis.filter(|_| analyzed) {
                    Some(m) => {
                        let c = if m.factor >= 100.0 { theme.danger } else { theme.warning };
                        div()
                            .px(px(4.))
                            .rounded(px(3.))
                            .bg(c.opacity(0.16))
                            .text_color(c)
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_nowrap()
                            .child(m.badge())
                            .flex_shrink_0()
                            .into_any_element()
                    }
                    None => div().into_any_element(),
                })
        };

        let bar_w = if analyzed { 36.0 } else { 84.0 };
        let cost_cell = h_flex()
            .w(px(if analyzed { W_COST } else { W_COST_EST }))
            .flex_shrink_0()
            .px_2()
            .gap(px(6.))
            .items_center()
            .child(div().w(px(56.)).flex_shrink_0().flex().justify_end().child(match node.total_cost {
                Some(c) => num(fmt_cost(c)).into_any_element(),
                None => num("–".into()).text_color(muted.opacity(0.6)).into_any_element(),
            }))
            .child(match l.tree.cost_share(id) {
                Some(s) => div()
                    .w(px(bar_w))
                    .h(px(if analyzed { 3.0 } else { 5.0 }))
                    .rounded(px(2.))
                    .bg(theme.muted.opacity(0.8))
                    .child(
                        div()
                            .h_full()
                            .w(px((s as f32 * bar_w).max(if s > 0.0 { 1.5 } else { 0.0 })))
                            .rounded(px(2.))
                            // Without timings the cost bar carries the heat colouring.
                            .bg(if analyzed {
                                theme.primary.opacity(0.7)
                            } else {
                                heat_color(heat_level(s), cx)
                            }),
                    )
                    .into_any_element(),
                None => div().w(px(bar_w)).into_any_element(),
            });

        let loops_cell =
            div().w(px(W_LOOPS)).flex_shrink_0().px_2().flex().justify_end().child(match node.loops {
                Some(n) if analyzed => {
                    let t = num(fmt_rows(n));
                    if n > 1.0 {
                        t.text_color(theme.foreground).into_any_element()
                    } else {
                        t.text_color(muted).into_any_element()
                    }
                }
                _ => num("–".into()).text_color(muted.opacity(0.6)).into_any_element(),
            });

        let buf_tip = match (node.shared_hit_blocks, node.shared_read_blocks) {
            (None, None) => "No buffer figures reported".to_string(),
            (h, r) => format!(
                "Shared buffers: {} hit ({}), {} read from disk ({})",
                group_digits(h.unwrap_or(0)),
                fmt_blocks_size(h.unwrap_or(0)),
                group_digits(r.unwrap_or(0)),
                fmt_blocks_size(r.unwrap_or(0))
            ),
        };
        let buf_cell = div()
            .id(("plan-buf", id))
            .tooltip(move |window, cx| Tooltip::new(buf_tip.clone()).build(window, cx))
            .w(px(W_BUF))
            .flex_shrink_0()
            .px_2()
            .flex()
            .justify_end()
            .child(match (node.shared_hit_blocks, node.shared_read_blocks) {
                (None, None) => num("–".into()).text_color(muted.opacity(0.6)).into_any_element(),
                (h, r) => {
                    let (h, r) = (h.unwrap_or(0), r.unwrap_or(0));
                    num(format!("{} / {}", fmt_rows(h as f64), fmt_rows(r as f64)))
                        .text_color(if r > 0 { theme.warning } else { theme.foreground })
                        .into_any_element()
                }
            });

        let dot = div()
            .w(px(W_DOT))
            .flex_shrink_0()
            .flex()
            .justify_center()
            .when_some(sev, |d, s| d.child(div().size(px(7.)).rounded_full().bg(severity_color(s, cx))));

        h_flex()
            .id(("plan-row", id))
            .h(px(ROW_H))
            .w_full()
            .items_center()
            .border_l_2()
            .border_color(if selected { theme.primary } else { gpui_kit::transparent_black() })
            .when(selected, |d| d.bg(theme.primary.opacity(0.18)))
            .when(!selected && highlighted, |d| d.bg(theme.warning.opacity(0.14)))
            .when(!selected && !highlighted, |d| d.hover(|h| h.bg(theme.table_hover)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.select_node(id, true, cx);
                if ev.click_count() == 2 {
                    this.toggle_node(id, cx);
                }
            }))
            .child(dot)
            .child(op)
            .when(cols.time, |d| d.child(time_cell))
            .child(rows_cell)
            .when(cols.cost, |d| d.child(cost_cell))
            .when(cols.loops, |d| d.child(loops_cell))
            .when(cols.buffers, |d| d.child(buf_cell))
            .child(div().w(px(8.)).flex_shrink_0())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_trees_drop_optional_columns_in_order() {
        let all = columns_for(1400.0, true);
        assert!(all.time && all.cost && all.loops && all.buffers);
        let mid = columns_for(W_DOT + W_TIME + W_ROWS + W_COST + W_LOOPS + MIN_OP + 1.0, true);
        assert!(mid.cost && mid.loops && !mid.buffers);
        let small = columns_for(W_DOT + W_TIME + W_ROWS + W_COST + MIN_OP + 1.0, true);
        assert!(small.cost && !small.loops && !small.buffers);
        let tiny = columns_for(400.0, true);
        assert!(tiny.time && !tiny.cost && !tiny.loops && !tiny.buffers);
    }

    #[test]
    fn estimate_only_plans_have_no_measured_columns() {
        let c = columns_for(1400.0, false);
        assert!(!c.time && !c.loops && !c.buffers && c.cost);
    }
}
