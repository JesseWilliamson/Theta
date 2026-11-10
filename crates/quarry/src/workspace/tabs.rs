//! Flat Zed-style tab strip and the console content area.

use super::Workspace;
use crate::actions::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use quarry_ui::{CHROME_TEXT_SIZE, EmptyState, KeyHint, TAB_BAR_HEIGHT, tool_button};

impl Workspace {
    pub(super) fn render_tab_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // Converting here ends the borrow of `self`: an `impl IntoElement` return
        // would capture it and block the other panels built in `render`.
        self.render_tab_bar_inner(window, cx).into_any_element()
    }

    fn render_tab_bar_inner(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active_tab;
        let tabs: Vec<_> = self.tabs.iter().map(|t| (t.id, t.title.clone(), t.modified)).collect();

        h_flex()
            .h(TAB_BAR_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .relative()
            .bg(theme.tab_bar)
            .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(theme.border))
            .child(
                h_flex()
                    .id("tab-strip")
                    .h_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.tabs_scroll)
                    .children(tabs.into_iter().map(|(id, title, modified)| {
                        let is_active = active == Some(id);
                        let group: SharedString = format!("tab-{id}").into();
                        h_flex()
                            .id(("tab", id))
                            .group(group.clone())
                            .h_full()
                            .flex_shrink_0()
                            .min_w(px(110.))
                            .max_w(px(220.))
                            .pl_3()
                            .pr_1p5()
                            .gap_2()
                            .items_center()
                            .relative()
                            .border_r_1()
                            .border_color(theme.border)
                            .cursor_default()
                            .when(is_active, |d| {
                                // Hides the bar's bottom rule under the active tab so it
                                // reads as one surface with the editor below.
                                d.bg(theme.background).child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .bottom(px(-1.))
                                        .h(px(2.))
                                        .bg(theme.background),
                                )
                            })
                            .when(!is_active, |d| d.hover(|s| s.bg(theme.list_hover)))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.activate_tab(id, window, cx)),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _, window, cx| {
                                    this.request_close_tab(id, window, cx)
                                }),
                            )
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(13.))
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(CHROME_TEXT_SIZE + px(0.5))
                                    .text_color(if is_active {
                                        theme.foreground
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(title),
                            )
                            .child(
                                div()
                                    .id(("tab-close", id))
                                    .relative()
                                    .size(px(18.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .hover(|s| s.bg(theme.border))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.request_close_tab(id, window, cx);
                                    }))
                                    .when(modified, |d| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .size(px(7.))
                                                .rounded_full()
                                                .bg(theme.muted_foreground)
                                                .group_hover(group.clone(), |s| s.opacity(0.)),
                                        )
                                    })
                                    .child(
                                        div()
                                            .when(!is_active || modified, |d| d.invisible())
                                            .group_hover(group.clone(), |s| s.visible())
                                            .child(
                                                Icon::new(IconName::X)
                                                    .size(px(12.))
                                                    .text_color(theme.muted_foreground),
                                            ),
                                    ),
                            )
                    })),
            )
            .child(
                h_flex().h_full().flex_1().min_w(px(36.)).items_center().px_1().child(
                    tool_button("new-console", IconName::Plus, "New Console")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(NewConsole), cx)),
                ),
            )
    }

    pub(super) fn render_console_area(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // Converting here ends the borrow of `self`: an `impl IntoElement` return
        // would capture it and block the other panels built in `render`.
        self.render_console_area_inner(window, cx).into_any_element()
    }

    fn render_console_area_inner(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(tab) = self.active_console().cloned() else {
            if self.store.read(cx).configs().is_empty() {
                return self.render_welcome(cx);
            }
            return EmptyState::new(IconName::SquareTerminal, "No console open")
                .description("Open a console to write and run SQL against a connection.")
                .action(
                    gpui_kit::component::button::Button::new("empty-new-console")
                        .small()
                        .icon(Icon::new(IconName::Plus))
                        .label("New Console")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(NewConsole), cx)),
                )
                .key_hint(KeyHint::new(if cfg!(target_os = "macos") { "cmd-n" } else { "ctrl-n" }))
                .into_any_element();
        };
        if let Some(view) = self.console_views.get(&tab.id) {
            return view.clone().into_any_element();
        }
        let store = self.store.read(cx);
        let target = tab
            .connection_id
            .as_ref()
            .and_then(|id| store.config(id))
            .map(|c| format!("Bound to {}.", c.name))
            .unwrap_or_else(|| "Not bound to a connection yet.".into());
        EmptyState::new(IconName::Code, "SQL editor")
            .description(format!(
                "{target} The editor mounts here and runs statements with the shortcut below."
            ))
            .key_hint(KeyHint::new(if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" }))
            .into_any_element()
    }
}
