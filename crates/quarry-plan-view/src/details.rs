//! The details pane: every property of the selected node, grouped, plus the
//! insights attached to it.

use crate::insights::sql_text;
use crate::model::{PropKind, label, node_sections};
use crate::panel::PlanPanel;
use crate::ui::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    div, px,
};
use quarry_ui::{EditorColors, data_surface, tool_button};

impl PlanPanel {
    pub(crate) fn render_details(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ed = EditorColors::get(cx);
        let Some(l) = self.loaded.as_ref() else { return div().into_any_element() };
        let Some(id) = self.selected.filter(|i| *i < l.tree.len()) else { return div().into_any_element() };
        let Some(node) = crate::model::node_at(&l.input.plan.root, l.tree.path_of(id)) else {
            return div().into_any_element();
        };
        let lab = label(node, l.input.kind);
        let fam = l.tree.node(id).family;
        let kind = l.input.kind;
        let sections = node_sections(&l.tree, &l.input.plan, id);
        let mono = theme.mono_font_family.clone();
        let own: Vec<usize> =
            (0..l.input.insights.len()).filter(|&i| l.insight_node[i] == Some(id)).collect();
        let cards: Vec<AnyElement> = own.into_iter().map(|i| self.insight_card(i, true, cx)).collect();

        let header = v_flex()
            .flex_shrink_0()
            .px_3()
            .py(px(6.))
            .gap(px(2.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(22.))
                    .gap(px(6.))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_center()
                            .min_w_0()
                            .child(
                                Icon::new(family_icon(fam)).size(px(14.)).text_color(family_color(fam, cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(lab.title.clone()),
                            ),
                    )
                    .child(tool_button("plan-details-close", IconName::X, "Close details (Esc)").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.details_open = false;
                            cx.notify();
                        }),
                    )),
            )
            .when(!lab.subtitle.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(15.))
                        .text_color(theme.muted_foreground)
                        .child(lab.subtitle.clone()),
                )
            });

        let mut body = v_flex()
            .id(("plan-details-scroll", id))
            .flex_1()
            .min_h_0()
            .p_3()
            .gap(px(12.))
            .overflow_y_scrollbar();
        if !cards.is_empty() {
            body = body.child(v_flex().gap(px(8.)).children(cards));
        }
        for sec in sections {
            let rows = sec.props.into_iter().map(|p| match p.kind {
                PropKind::Sql => v_flex()
                    .gap(px(3.))
                    .child(
                        div().text_size(px(11.5)).text_color(theme.muted_foreground).child(p.label.clone()),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(ed.background)
                            .font_family(mono.clone())
                            .text_size(px(11.5))
                            .line_height(px(16.))
                            .child(sql_text(&p.value, kind, cx)),
                    )
                    .into_any_element(),
                other => h_flex()
                    .gap(px(8.))
                    .items_start()
                    .child(
                        div()
                            .w(px(108.))
                            .flex_shrink_0()
                            .text_size(px(11.5))
                            .line_height(px(16.))
                            .text_color(theme.muted_foreground)
                            .child(p.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(mono.clone())
                            .text_size(px(11.5))
                            .line_height(px(16.))
                            .when(other == PropKind::Warn, |d| {
                                d.text_color(theme.warning).font_weight(FontWeight::MEDIUM)
                            })
                            .child(p.value.clone()),
                    )
                    .into_any_element(),
            });
            body = body.child(
                v_flex()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(sec.title.to_uppercase()),
                    )
                    .children(rows),
            );
        }
        v_flex().size_full().bg(data_surface(cx)).child(header).child(body).into_any_element()
    }
}
