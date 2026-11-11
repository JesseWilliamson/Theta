//! `PlanPanel`: state, events, keyboard handling and the panel's overall layout.
//! The individual views live in sibling modules as further `impl PlanPanel` blocks.

use crate::flame::{FlameLayout, choose_metric, layout};
use crate::format::*;
use crate::model::{PlanTree, VisibleRow, plan_as_text, row_of};
use crate::ui::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _,
    UniformListScrollHandle, Window, canvas, div, px, relative,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use theta_core::{DatabaseKind, Plan};
use theta_sql::{Insight, PlanSummary, Severity};
use theta_ui::{EmptyState, KeyHint, Settings, data_surface, tool_button};

/// Everything the panel needs to show one plan.
#[derive(Clone)]
pub struct PlanInput {
    pub plan: Arc<Plan>,
    pub summary: PlanSummary,
    pub insights: Vec<Insight>,
    pub kind: DatabaseKind,
    pub statement: String,
}

#[derive(Clone, Debug)]
pub enum PlanEvent {
    /// The user chose "Open in Console" on an insight's suggested SQL.
    OpenSuggestion(String),
    CopyText(String),
    /// Run the statement again with or without ANALYZE.
    Rerun {
        analyze: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Tree,
    Flame,
    Raw,
}

impl ViewMode {
    pub fn parse(s: &str) -> Option<ViewMode> {
        match s.to_ascii_lowercase().as_str() {
            "tree" => Some(ViewMode::Tree),
            "flame" => Some(ViewMode::Flame),
            "raw" => Some(ViewMode::Raw),
            _ => None,
        }
    }
}

pub(crate) struct Loaded {
    pub input: PlanInput,
    pub tree: PlanTree,
    pub visible: Vec<VisibleRow>,
    pub flame: FlameLayout,
    /// Worst insight severity attached to each node id.
    pub node_sev: HashMap<usize, Severity>,
    /// Node id each insight points at (`None` when its path no longer resolves).
    pub insight_node: Vec<Option<usize>>,
    /// The step the analysis names as the slowest, marked in the tree.
    pub slowest: Option<usize>,
}

pub struct PlanPanel {
    pub(crate) focus: FocusHandle,
    pub(crate) loaded: Option<Loaded>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) empty_hints: Vec<SharedString>,
    pub(crate) view: ViewMode,
    pub(crate) selected: Option<usize>,
    pub(crate) hover_node: Option<usize>,
    pub(crate) selected_insight: Option<usize>,
    pub(crate) details_open: bool,
    pub(crate) tree_scroll: UniformListScrollHandle,
    pub(crate) raw_editor: Entity<EditorState>,
    /// Panel width as last painted; the layout adapts to it (columns, panes).
    pub(crate) width: f32,
    pub(crate) flame_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(crate) flame_hover: Option<usize>,
}

impl EventEmitter<PlanEvent> for PlanPanel {}

/// Pane widths. Below `WIDE` the details pane overlays the tree instead of
/// shrinking it, because the tree needs ~500px to show its numeric columns.
pub(crate) const INSIGHTS_W_WIDE: f32 = 296.0;
pub(crate) const INSIGHTS_W_NARROW: f32 = 256.0;
pub(crate) const DETAILS_W: f32 = 340.0;
pub(crate) const WIDE: f32 = 1180.0;

impl PlanPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let raw_editor = cx.new(|cx| EditorState::new(window, cx).language("json").line_number(false));
        raw_editor.update(cx, |s, cx| s.set_readonly(true, cx));
        Self {
            focus: cx.focus_handle(),
            loaded: None,
            loading: false,
            error: None,
            empty_hints: Vec::new(),
            view: ViewMode::Tree,
            selected: None,
            hover_node: None,
            selected_insight: None,
            details_open: false,
            tree_scroll: UniformListScrollHandle::new(),
            raw_editor,
            width: window.viewport_size().width.into(),
            flame_bounds: Rc::new(Cell::new(Bounds::default())),
            flame_hover: None,
        }
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }

    pub fn set_plan(&mut self, input: PlanInput, window: &mut Window, cx: &mut Context<Self>) {
        let tree = PlanTree::new(&input.plan);
        let visible = tree.visible();
        let flame = layout(&input.plan, &tree, choose_metric(&input.plan));
        let insight_node: Vec<Option<usize>> =
            input.insights.iter().map(|i| tree.id_for_path(&i.node_path)).collect();
        let mut node_sev: HashMap<usize, Severity> = HashMap::new();
        for (i, id) in insight_node.iter().enumerate() {
            if let Some(id) = id {
                let s = input.insights[i].severity;
                node_sev.entry(*id).and_modify(|e| *e = (*e).max(s)).or_insert(s);
            }
        }
        let slowest = input.summary.slowest_node_path.as_deref().and_then(|p| tree.id_for_path(p));
        let raw = pretty_raw(&input.plan.raw);
        let language = if raw.trim_start().starts_with(['{', '[']) { "json" } else { "text" };
        self.raw_editor.update(cx, |s, cx| {
            s.set_highlighter(language, cx);
            s.set_value(raw, window, cx);
        });
        self.loaded = Some(Loaded { input, tree, visible, flame, node_sev, insight_node, slowest });
        self.loading = false;
        self.error = None;
        self.selected = None;
        self.hover_node = None;
        self.selected_insight = None;
        self.details_open = false;
        self.flame_hover = None;
        self.tree_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// While loading, the previous plan stays on screen but dimmed.
    /// The statement whose plan is on show, if any.
    pub fn shown_statement(&self) -> Option<&str> {
        self.loaded.as_ref().map(|l| l.input.statement.as_str())
    }

    /// True while a plan is being fetched.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.loading = loading;
        if loading {
            self.error = None;
        }
        cx.notify();
    }

    pub fn set_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message);
        self.loading = false;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.loaded = None;
        self.error = None;
        self.loading = false;
        self.selected = None;
        self.hover_node = None;
        self.selected_insight = None;
        self.details_open = false;
        cx.notify();
    }

    /// Key hints shown under "No plan yet" (already formatted, e.g. `space e`,
    /// `Ctrl+Shift+E`); the application knows its own bindings.
    pub fn set_empty_hint(&mut self, hints: Vec<String>, cx: &mut Context<Self>) {
        self.empty_hints = hints.into_iter().map(SharedString::from).collect();
        cx.notify();
    }

    pub fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    /// Select a node by tree path, revealing it. Used by the demo and tests.
    pub fn select_path(&mut self, path: &[usize], cx: &mut Context<Self>) {
        if let Some(id) = self.loaded.as_ref().and_then(|l| l.tree.id_for_path(path)) {
            self.select_node(id, true, cx);
        }
    }

    pub(crate) fn select_node(&mut self, id: usize, open_details: bool, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        if id >= l.tree.len() {
            return;
        }
        l.tree.reveal(id);
        l.visible = l.tree.visible();
        if let Some(row) = row_of(&l.visible, id) {
            self.tree_scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        self.selected = Some(id);
        if open_details {
            self.details_open = true;
        }
        cx.notify();
    }

    /// Select an insight card by index and reveal its node in the tree.
    pub fn select_insight(&mut self, i: usize, cx: &mut Context<Self>) {
        self.selected_insight = Some(i);
        if let Some(id) = self.loaded.as_ref().and_then(|l| l.insight_node.get(i).copied().flatten()) {
            if self.view == ViewMode::Raw {
                self.view = ViewMode::Tree;
            }
            self.select_node(id, true, cx);
        }
        cx.notify();
    }

    pub(crate) fn toggle_node(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(l) = self.loaded.as_mut() {
            l.tree.toggle(id);
            l.visible = l.tree.visible();
            cx.notify();
        }
    }

    fn expand_collapse_all(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        if l.tree.any_collapsed() {
            l.tree.expand_all();
        } else {
            l.tree.collapse_all();
            // Keep the selection visible: fall back to its nearest shown ancestor.
            if let Some(mut s) = self.selected {
                while row_of(&l.tree.visible(), s).is_none() {
                    match l.tree.node(s).parent {
                        Some(p) => s = p,
                        None => break,
                    }
                }
                self.selected = Some(s);
            }
        }
        l.visible = l.tree.visible();
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_ref() else { return };
        if l.visible.is_empty() {
            return;
        }
        let cur = self.selected.and_then(|s| row_of(&l.visible, s));
        let next = match cur {
            None => 0,
            Some(r) => (r as isize + delta).clamp(0, l.visible.len() as isize - 1) as usize,
        };
        let id = l.visible[next].id;
        self.select_node(id, false, cx);
    }

    fn jump_selection(&mut self, to_end: bool, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_ref() else { return };
        let Some(row) = (if to_end { l.visible.last() } else { l.visible.first() }) else { return };
        let id = row.id;
        self.select_node(id, false, cx);
    }

    fn collapse_or_parent(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        let Some(s) = self.selected else { return };
        if l.tree.node(s).child_count > 0 && l.tree.is_expanded(s) {
            l.tree.set_expanded(s, false);
            l.visible = l.tree.visible();
            cx.notify();
        } else if let Some(p) = l.tree.node(s).parent {
            self.select_node(p, false, cx);
        }
    }

    fn expand_or_child(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        let Some(s) = self.selected else { return };
        if l.tree.node(s).child_count == 0 {
            return;
        }
        if !l.tree.is_expanded(s) {
            l.tree.set_expanded(s, true);
            l.visible = l.tree.visible();
            cx.notify();
        } else {
            self.select_node(s + 1, false, cx);
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        if m.control || m.alt || m.platform || self.loaded.is_none() || self.view == ViewMode::Raw {
            return;
        }
        let modal = Settings::get(cx).keymap.is_modal();
        let key = ev.keystroke.key.as_str();
        let key = match (modal, key) {
            (true, "j") => "down",
            (true, "k") => "up",
            (true, "h") => "left",
            (true, "l") => "right",
            _ => key,
        };
        match key {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(8, cx),
            "pageup" => self.move_selection(-8, cx),
            "home" => self.jump_selection(false, cx),
            "end" => self.jump_selection(true, cx),
            "left" => self.collapse_or_parent(cx),
            "right" => self.expand_or_child(cx),
            "enter" => {
                if self.selected.is_none() {
                    self.move_selection(0, cx);
                }
                self.details_open = !self.details_open && self.selected.is_some();
                cx.notify();
            }
            "escape" if self.details_open => {
                self.details_open = false;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn copy_text(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        cx.emit(PlanEvent::CopyText(text));
    }

    fn insights_width(&self) -> f32 {
        if self.width >= WIDE { INSIGHTS_W_WIDE } else { INSIGHTS_W_NARROW }
    }

    /// Width the tree table actually gets, which decides which columns it shows.
    pub(crate) fn tree_width(&self) -> f32 {
        let mut w = self.width - self.insights_width();
        if self.details_open && self.selected.is_some() && self.width >= WIDE {
            w -= DETAILS_W;
        }
        w
    }
}

/// Pretty-print JSON output; anything else (MySQL 8's text tree, SQLite's rows)
/// is shown as the server sent it.
pub(crate) fn pretty_raw(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()),
        Err(_) => raw.to_string(),
    }
}

// ------------------------------------------------------------------ render

impl Render for PlanPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let weak = cx.entity().downgrade();
        // Measure our own width so columns and panes adapt to the dock, not the window.
        let measure = canvas(
            move |bounds, _, cx| {
                let w: f32 = bounds.size.width.into();
                weak.update(cx, |this, cx| {
                    if (this.width - w).abs() > 0.5 {
                        this.width = w;
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let body: gpui_kit::AnyElement = if self.loaded.is_some() && self.error.is_none() {
            self.render_loaded(window, cx)
        } else if let Some(msg) = self.error.clone() {
            EmptyState::new(IconName::CircleAlert, "Couldn't explain the statement")
                .description(msg)
                .error()
                .into_any_element()
        } else if self.loading {
            EmptyState::new(IconName::Timer, "Explaining…")
                .description("Waiting for the database to produce a plan.")
                .into_any_element()
        } else {
            let mut e = EmptyState::new(IconName::ChartNoAxesGantt, "No plan yet")
                .description("Explain the statement under the cursor to see how the database runs it.");
            if !self.empty_hints.is_empty() {
                let hints = self.empty_hints.clone();
                e = e.action(h_flex().gap_1p5().children(hints.into_iter().map(KeyHint::from_label)));
            }
            e.into_any_element()
        };

        let dim = self.loading && self.loaded.is_some() && self.error.is_none();
        v_flex()
            .id("plan-panel")
            .key_context("PlanPanel")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(data_surface(cx))
            .text_color(theme.foreground)
            .child(measure)
            .when(self.loaded.is_some() && self.error.is_none(), |this| {
                this.child(self.render_header(window, cx))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(self.loaded.is_some(), |d| d.border_t_1().border_color(theme.border))
                    .when(dim, |d| d.opacity(0.45))
                    .child(body),
            )
            .when(self.loading, |this| {
                this.child(div().absolute().top_0().left_0().w_full().h(px(2.)).overflow_hidden().child(
                    div().absolute().top_0().h_full().w(relative(0.25)).bg(theme.primary).with_animation(
                        "plan-progress",
                        Animation::new(Duration::from_millis(1100)).repeat(),
                        |bar, t| bar.left(relative(t * 0.75)),
                    ),
                ))
            })
    }
}

impl PlanPanel {
    fn render_header(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let l = self.loaded.as_ref().expect("header only renders with a plan");
        let plan = &l.input.plan;
        let narrow = self.width < WIDE;

        let mut left = h_flex().gap_1p5().items_center().min_w_0().overflow_hidden();
        if plan.analyzed {
            let exec = l.input.summary.total_ms.or(plan.execution_time_ms);
            left = left.child(chip(
                Some((IconName::Timer, theme.primary)),
                match (exec, plan.planning_time_ms) {
                    (Some(e), Some(p)) if !narrow => format!("{} exec · {} planning", fmt_ms(e), fmt_ms(p)),
                    (Some(e), _) => format!("{} exec", fmt_ms(e)),
                    (None, _) => "executed".to_string(),
                },
                cx,
            ));
        } else {
            left =
                left.child(chip(Some((IconName::Info, theme.warning)), "Estimated only — not executed", cx));
        }
        if let Some(c) = l.input.summary.total_cost {
            left = left.child(chip(
                Some((IconName::Gauge, theme.muted_foreground)),
                format!("cost {}", fmt_cost(c)),
                cx,
            ));
        }
        left = left.child(chip(
            Some((IconName::Network, theme.muted_foreground)),
            format!(
                "{} node{}",
                l.input.summary.node_count,
                if l.input.summary.node_count == 1 { "" } else { "s" }
            ),
            cx,
        ));
        for sev in [Severity::Critical, Severity::Warning, Severity::Info] {
            let n = l.input.insights.iter().filter(|i| i.severity == sev).count();
            if n > 0 {
                left =
                    left.child(chip(Some((severity_icon(sev), severity_color(sev, cx))), n.to_string(), cx));
            }
        }

        let seg = |mode: ViewMode, label: &'static str, cx: &mut Context<Self>| {
            let active = self.view == mode;
            let t = cx.theme().clone();
            let data = data_surface(cx);
            div()
                .id(label)
                .h(px(20.))
                .px(px(9.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .text_size(px(11.5))
                .cursor_pointer()
                .font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                .text_color(if active { t.foreground } else { t.muted_foreground })
                .when(active, |d| d.bg(data).border_1().border_color(t.border))
                .when(!active, |d| d.hover(|h| h.text_color(t.foreground)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.view = mode;
                    cx.notify();
                }))
        };
        let segmented = h_flex()
            .p(px(1.))
            .gap(px(1.))
            .rounded(px(5.))
            .bg(theme.muted.opacity(0.7))
            .flex_shrink_0()
            .child(seg(ViewMode::Tree, "Tree", cx))
            .child(seg(ViewMode::Flame, "Flame", cx))
            .child(seg(ViewMode::Raw, "Raw", cx));

        // The statement leads the copied text as SQL comments so a pasted plan says what it explains.
        let plan_text = format!(
            "{}{}",
            l.input.statement.trim().lines().map(|line| format!("-- {line}\n")).collect::<String>(),
            plan_as_text(plan, l.input.kind)
        );
        let tree_view = self.view == ViewMode::Tree;
        let any_collapsed = l.tree.any_collapsed();

        h_flex()
            .h(px(28.))
            .flex_shrink_0()
            .px_2()
            .gap_2()
            .items_center()
            .justify_between()
            .bg(theme.tab_bar)
            .child(left)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .flex_shrink_0()
                    .child(segmented)
                    .child(div().w(px(1.)).h(px(14.)).mx_1().bg(theme.border))
                    .child(
                        Button::new("plan-explain")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Play))
                            .label("Explain")
                            .tooltip("Estimate only; the statement is not executed")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(PlanEvent::Rerun { analyze: false })),
                            ),
                    )
                    .child(
                        Button::new("plan-analyze")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Zap).text_color(theme.warning))
                            .label(if narrow { "Analyze" } else { "Explain Analyze" })
                            .tooltip("Executes the statement (rolled back for writes)")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PlanEvent::Rerun { analyze: true }))),
                    )
                    .child(
                        tool_button("plan-copy", IconName::Copy, "Copy plan as text").on_click(
                            cx.listener(move |this, _, _, cx| this.copy_text(plan_text.clone(), cx)),
                        ),
                    )
                    .when(tree_view, |d| {
                        d.child(
                            tool_button(
                                "plan-expand",
                                if any_collapsed {
                                    IconName::ChevronsUpDown
                                } else {
                                    IconName::ChevronsDownUp
                                },
                                if any_collapsed { "Expand all" } else { "Collapse all" },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.expand_collapse_all(cx))),
                        )
                    }),
            )
    }

    fn render_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let iw = self.insights_width();
        let wide = self.width >= WIDE;
        let show_details = self.details_open && self.selected.is_some() && self.view != ViewMode::Raw;
        let main = match self.view {
            ViewMode::Tree => self.render_tree(window, cx),
            ViewMode::Flame => self.render_flame(window, cx),
            ViewMode::Raw => self.render_raw(cx),
        };
        let details = show_details.then(|| self.render_details(cx));
        let (overlay, side) = if wide { (None, details) } else { (details, None) };
        h_flex()
            .size_full()
            .child(
                div()
                    .w(px(iw))
                    .h_full()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(theme.border)
                    .child(self.render_insights(cx)),
            )
            .child(div().relative().flex_1().min_w_0().h_full().child(main).when_some(overlay, |d, pane| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .w(px(DETAILS_W))
                        .border_l_1()
                        .border_color(theme.border)
                        .bg(data_surface(cx))
                        .shadow_lg()
                        .child(pane),
                )
            }))
            .when_some(side, |d, pane| {
                d.child(
                    div()
                        .w(px(DETAILS_W))
                        .h_full()
                        .flex_shrink_0()
                        .border_l_1()
                        .border_color(theme.border)
                        .child(pane),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_json_is_pretty_printed_and_text_is_left_alone() {
        assert_eq!(pretty_raw("[{\"a\":1}]"), "[\n  {\n    \"a\": 1\n  }\n]");
        assert_eq!(pretty_raw("-> Table scan on t"), "-> Table scan on t");
    }

    #[test]
    fn view_modes_parse() {
        assert_eq!(ViewMode::parse("Flame"), Some(ViewMode::Flame));
        assert_eq!(ViewMode::parse("tree"), Some(ViewMode::Tree));
        assert_eq!(ViewMode::parse("raw"), Some(ViewMode::Raw));
        assert_eq!(ViewMode::parse("x"), None);
    }
}
