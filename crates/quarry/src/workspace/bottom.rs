//! Bottom panel: Results | Messages | Plan | Map | History.

use super::{BottomTab, Workspace};
use crate::actions::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use quarry_ui::{CHROME_TEXT_SIZE, EmptyState, KeyHint, PANEL_HEADER_HEIGHT, tool_button};

fn empty_state(tab: BottomTab) -> EmptyState {
    let modk = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    match tab {
        BottomTab::Results => EmptyState::new(IconName::Table, "Run a query to see results")
            .description("Rows returned by a statement appear here.")
            .key_hint(KeyHint::new(&format!("{modk}-enter"))),
        BottomTab::Messages => EmptyState::new(IconName::MessageSquare, "No messages")
            .description("Notices, warnings and statement outcomes are listed here."),
        BottomTab::Plan => EmptyState::new(IconName::Network, "No query plan")
            .description("Run Explain to see how the database executes a statement.")
            .key_hint(KeyHint::new(&format!("{modk}-e"))),
        BottomTab::Map => EmptyState::new(IconName::Map, "No geometry to display")
            .description("Result columns containing geometry are drawn on a map."),
        BottomTab::History => EmptyState::new(IconName::Clock, "No history yet")
            .description("Statements you run are kept here so you can run them again."),
    }
}

impl Workspace {
    pub(super) fn render_bottom_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // Converting here ends the borrow of `self`: an `impl IntoElement` return
        // would capture it and block the other panels built in `render`.
        self.render_bottom_panel_inner(window, cx).into_any_element()
    }

    fn render_bottom_panel_inner(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.state.bottom_tab;
        let content = match self.bottom_views.get(&current) {
            Some(view) => view.clone().into_any_element(),
            None => empty_state(current).into_any_element(),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(PANEL_HEADER_HEIGHT + px(2.))
                    .flex_shrink_0()
                    .px_1()
                    .items_end()
                    .justify_between()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(h_flex().h_full().gap_1().children(BottomTab::ALL.into_iter().map(|tab| {
                        let active = tab == current;
                        h_flex()
                            .id(tab.label())
                            .h_full()
                            .px_2p5()
                            .gap_1p5()
                            .items_center()
                            .cursor_default()
                            .border_b_2()
                            .border_color(if active { theme.ring } else { theme.transparent })
                            .child(
                                Icon::new(Self::icon_for_bottom(tab)).size(px(13.)).text_color(if active {
                                    theme.foreground
                                } else {
                                    theme.muted_foreground
                                }),
                            )
                            .child(
                                div()
                                    .text_size(CHROME_TEXT_SIZE)
                                    .font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                                    .text_color(if active {
                                        theme.foreground
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(tab.label()),
                            )
                            .when(!active, |d| d.hover(|s| s.bg(theme.list_hover)))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_bottom_tab(tab, cx)))
                    })))
                    .child(h_flex().h_full().items_center().child(
                        tool_button("bottom-close", IconName::X, "Close Panel").on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleBottomPanel), cx)
                        }),
                    )),
            )
            .child(div().flex_1().min_h_0().child(content))
    }
}
