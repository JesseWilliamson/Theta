//! The insights column: always visible next to every view, because the findings
//! are the main thing a plan panel has to say.

use crate::panel::{PlanEvent, PlanPanel};
use crate::sqlcolor::{Palette, highlights};
use crate::ui::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, div, px,
};
use quarry_core::DatabaseKind;
use quarry_sql::Insight;
use quarry_ui::{EditorColors, SyntaxColors, tool_button};

/// A SQL fragment in the buffer font with the theme's syntax colours.
pub(crate) fn sql_text(sql: &str, kind: DatabaseKind, cx: &gpui_kit::App) -> StyledText {
    let palette = Palette::from(SyntaxColors::get(cx));
    StyledText::new(SharedString::from(sql.to_string())).with_highlights(highlights(sql, kind, &palette))
}

impl PlanPanel {
    pub(crate) fn render_insights(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let l = self.loaded.as_ref().expect("insights render with a plan");
        let analyzed = l.input.plan.analyzed;
        let count = l.input.insights.len();
        let cards: Vec<AnyElement> = (0..count).map(|i| self.insight_card(i, false, cx)).collect();

        let header = h_flex()
            .h(px(24.))
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.table_head)
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child("Insights"),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground.opacity(0.8))
                    .child(if count == 0 { String::new() } else { count.to_string() }),
            );

        let hint = (!analyzed).then(|| {
            v_flex()
                .p(px(8.))
                .gap(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted.opacity(0.35))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .items_center()
                        .child(Icon::new(IconName::Zap).size(px(14.)).text_color(theme.warning))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Run Explain Analyze for real timings"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.))
                        .text_color(theme.muted_foreground)
                        .child("These are planner estimates. Timings, row counts and misestimates need the statement to run."),
                )
                .child(
                    h_flex().child(
                        Button::new("hint-analyze")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Zap))
                            .label("Explain Analyze")
                            .tooltip("Executes the statement (rolled back for writes)")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PlanEvent::Rerun { analyze: true }))),
                    ),
                )
        });

        let ok_row = (count == 0).then(|| {
            h_flex()
                .px(px(4.))
                .py(px(6.))
                .gap(px(6.))
                .items_center()
                .child(Icon::new(IconName::CircleCheck).size(px(14.)).text_color(theme.success))
                .child(
                    div().text_size(px(12.)).text_color(theme.muted_foreground).child("No problems detected"),
                )
        });

        v_flex()
            .size_full()
            .child(header)
            .child(
                v_flex()
                    .id("plan-insights-scroll")
                    .flex_1()
                    .min_h_0()
                    .p(px(8.))
                    .gap(px(8.))
                    .overflow_y_scrollbar()
                    .children(hint)
                    .children(ok_row)
                    .children(cards),
            )
            .into_any_element()
    }

    /// One finding. `expanded` shows the full explanation (details pane); in the
    /// column the explanation is clamped until the card is selected.
    pub(crate) fn insight_card(&self, i: usize, expanded: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let l = self.loaded.as_ref().expect("cards render with a plan");
        let ins: &Insight = &l.input.insights[i];
        let node = l.insight_node[i];
        let color = severity_color(ins.severity, cx);
        let selected = self.selected_insight == Some(i);
        let kind = l.input.kind;
        let ed = EditorColors::get(cx);
        let full = expanded || selected;

        let suggestion = ins.suggestion.clone().map(|sql| {
            if !is_runnable_sql(&sql) {
                // Advice such as "Add the join predicate, e.g. JOIN … ON …" is text, not a statement.
                return h_flex()
                    .gap(px(6.))
                    .items_start()
                    .child(Icon::new(IconName::Lightbulb).size(px(13.)).text_color(theme.warning))
                    .child(div().flex_1().min_w_0().text_size(px(11.5)).line_height(px(16.)).child(sql))
                    .into_any_element();
            }
            let (sql_open, sql_copy) = (sql.clone(), sql.clone());
            v_flex()
                .gap(px(4.))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(ed.background)
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(11.5))
                        .line_height(px(16.))
                        .child(sql_text(&sql, kind, cx)),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new(("plan-open-sugg", i))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Terminal))
                                .label("Open in Console")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(PlanEvent::OpenSuggestion(sql_open.clone()));
                                })),
                        )
                        .child(tool_button(("plan-copy-sugg", i), IconName::Copy, "Copy SQL").on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.copy_text(sql_copy.clone(), cx);
                            }),
                        )),
                )
                .into_any_element()
        });

        v_flex()
            .id(("plan-insight", i))
            .p(px(8.))
            .gap(px(5.))
            .rounded(px(6.))
            .border_1()
            .border_color(if selected { color.opacity(0.7) } else { theme.border })
            .bg(if selected { color.opacity(0.08) } else { theme.muted.opacity(0.35) })
            .when(node.is_some(), |d| d.cursor_pointer())
            .hover(|h| h.border_color(color.opacity(0.5)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hover_node = node;
                } else if this.hover_node == node {
                    this.hover_node = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.select_insight(i, cx);
            }))
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_start()
                    .child(
                        div()
                            .pt(px(1.))
                            .child(Icon::new(severity_icon(ins.severity)).size(px(14.)).text_color(color)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(ins.title.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .line_height(px(16.))
                    .text_color(theme.muted_foreground)
                    .when(!full, |d| d.line_clamp(3).text_ellipsis())
                    .child(ins.detail.clone()),
            )
            .children(suggestion)
            .into_any_element()
    }
}

/// Whether a suggestion is an executable statement (worth "Open in Console")
/// rather than advice in prose.
pub(crate) fn is_runnable_sql(s: &str) -> bool {
    let first = s.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(
        first.as_str(),
        "CREATE" | "ANALYZE" | "SET" | "ALTER" | "DROP" | "VACUUM" | "REINDEX" | "OPTIMIZE"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_statements_are_runnable() {
        assert!(is_runnable_sql("CREATE INDEX idx_a ON a (b);"));
        assert!(is_runnable_sql("  analyze orders;"));
        assert!(is_runnable_sql("SET work_mem = '64MB';"));
        assert!(!is_runnable_sql("Add the join predicate, e.g. JOIN … ON a.key = b.key"));
        assert!(!is_runnable_sql(""));
    }
}
