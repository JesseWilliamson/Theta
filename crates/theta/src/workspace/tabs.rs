//! Flat Zed-style tab strip and the console content area.

use super::Workspace;
use crate::actions::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render,
    ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, linear_color_stop,
    linear_gradient, px,
};
use theta_ui::{CHROME_TEXT_SIZE, EmptyState, KeyHint, TAB_BAR_HEIGHT, data_surface, tool_button};

/// Payload of a tab being dragged within the strip.
#[derive(Clone)]
struct TabDrag {
    id: u64,
    title: SharedString,
}

/// What follows the pointer while dragging a tab.
struct TabGhost(SharedString);

impl Render for TabGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .h(px(26.))
            .px_3()
            .gap_2()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .text_size(CHROME_TEXT_SIZE + px(0.5))
            .text_color(theme.foreground)
            .child(Icon::new(IconName::SquareTerminal).size(px(13.)).text_color(theme.muted_foreground))
            .child(self.0.clone())
    }
}

/// The tab order after dragging `dragged` onto `target`: the dragged tab takes the target's slot,
/// so it lands before it when moving left and after it when moving right.
pub(super) fn reordered(order: &[u64], dragged: u64, target: u64) -> Option<Vec<u64>> {
    let from = order.iter().position(|&t| t == dragged)?;
    let to = order.iter().position(|&t| t == target)?;
    if from == to {
        return None;
    }
    let mut next = order.to_vec();
    let id = next.remove(from);
    next.insert(to, id);
    Some(next)
}

impl Workspace {
    /// Move tab `dragged` into the slot of `target` and persist the new order.
    pub(super) fn move_tab(&mut self, dragged: u64, target: Option<u64>, cx: &mut Context<Self>) {
        let order: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        let next = match target {
            Some(t) => reordered(&order, dragged, t),
            // Dropped on the empty part of the strip: to the end.
            None => order.last().and_then(|&last| reordered(&order, dragged, last)),
        };
        let Some(next) = next else { return };
        let mut tabs = std::mem::take(&mut self.tabs);
        tabs.sort_by_key(|t| next.iter().position(|&id| id == t.id));
        self.tabs = tabs;
        self.save_all_consoles(cx);
        cx.notify();
    }

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
        let surface = data_surface(cx);
        let active = self.active_tab;
        let tabs: Vec<_> = self.tabs.iter().map(|t| (t.id, t.title.clone(), t.modified)).collect();
        let order: std::rc::Rc<Vec<u64>> = std::rc::Rc::new(tabs.iter().map(|t| t.0).collect());

        // Bring a newly activated tab into view (the strip scrolls, it does not wrap).
        if self.revealed_tab != active {
            self.revealed_tab = active;
            if let Some(ix) = active.and_then(|a| order.iter().position(|&t| t == a)) {
                self.tabs_scroll.scroll_to_item(ix);
            }
        }
        let max = self.tabs_scroll.max_offset().x;
        let offset = -self.tabs_scroll.offset().x;
        let overflowing = max > px(1.);
        let fade_left = overflowing && offset > px(1.);
        let fade_right = overflowing && offset < max - px(1.);
        let menu_tabs: Vec<(u64, SharedString)> = tabs.iter().map(|t| (t.0, t.1.clone())).collect();
        let workspace = cx.entity().downgrade();

        let fade = |left: bool| {
            let (from, to) = (theme.tab_bar, theme.tab_bar.opacity(0.));
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(28.))
                .when(left, |d| d.left_0())
                .when(!left, |d| d.right_0())
                .bg(linear_gradient(
                    if left { 90. } else { 270. },
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ))
        };

        h_flex()
            .h(TAB_BAR_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .relative()
            .bg(theme.tab_bar)
            .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(theme.border))
            .child(
                div()
                    .relative()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .child(
                        h_flex()
                            .id("tab-strip")
                            .h_full()
                            .min_w_0()
                            .overflow_x_scroll()
                            .track_scroll(&self.tabs_scroll)
                            // The wheel scrolls tabs sideways: a vertical wheel has nothing else to do here.
                            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                                let d = e.delta.pixel_delta(px(24.));
                                if d.y.abs() > d.x.abs() {
                                    let mut o = this.tabs_scroll.offset();
                                    let max = this.tabs_scroll.max_offset().x;
                                    o.x = (o.x + d.y).clamp(-max, px(0.));
                                    this.tabs_scroll.set_offset(o);
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .children(tabs.into_iter().map(|(id, title, modified)| {
                                let is_active = active == Some(id);
                                let group: SharedString = format!("tab-{id}").into();
                                let ghost_title = title.clone();
                                let (order_a, order_b) = (order.clone(), order.clone());
                                let accent = theme.ring;
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
                                        d.bg(surface).child(
                                            div()
                                                .absolute()
                                                .left_0()
                                                .right_0()
                                                .bottom(px(-1.))
                                                .h(px(2.))
                                                .bg(surface),
                                        )
                                    })
                                    .when(!is_active, |d| d.hover(|s| s.bg(theme.list_hover)))
                                    .on_drag(TabDrag { id, title: ghost_title }, |d, _, _, cx| {
                                        let title = d.title.clone();
                                        cx.new(|_| TabGhost(title))
                                    })
                                    // The drop marker sits on the side the dragged tab will land on.
                                    .drag_over::<TabDrag>(move |s, d, _, _| {
                                        let from = order_a.iter().position(|&t| t == d.id);
                                        let to = order_a.iter().position(|&t| t == id);
                                        match (from, to) {
                                            (Some(f), Some(t)) if f > t => {
                                                s.border_l_2().border_color(accent)
                                            }
                                            (Some(f), Some(t)) if f < t => {
                                                s.border_r_2().border_color(accent)
                                            }
                                            _ => s,
                                        }
                                    })
                                    .on_drop(cx.listener(move |this, d: &TabDrag, _, cx| {
                                        let _ = &order_b;
                                        this.move_tab(d.id, Some(id), cx)
                                    }))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.activate_tab(id, window, cx)
                                    }))
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
                    .when(fade_left, |d| d.child(fade(true)))
                    .when(fade_right, |d| d.child(fade(false))),
            )
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w(if overflowing { px(68.) } else { px(36.) })
                    .items_center()
                    .px_1()
                    .gap_0p5()
                    // Dropping on the empty part of the strip sends the tab to the end.
                    .drag_over::<TabDrag>(move |s, _, _, _| s.bg(gpui_kit::transparent_black()))
                    .on_drop(cx.listener(|this, d: &TabDrag, _, cx| this.move_tab(d.id, None, cx)))
                    .child(
                        tool_button("new-console", IconName::Plus, "New Console")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewConsole), cx)),
                    )
                    .when(overflowing, |d| {
                        d.child(tool_button("tabs-menu", IconName::ChevronDown, "All tabs").dropdown_menu(
                            move |menu, _, _| {
                                let mut menu = menu.max_h(px(360.)).scrollable(true);
                                for (id, title) in &menu_tabs {
                                    let (id, ws) = (*id, workspace.clone());
                                    menu = menu.item(
                                        PopupMenuItem::new(title.clone())
                                            .checked(active == Some(id))
                                            .on_click(move |_, window, cx| {
                                                ws.update(cx, |this, cx| this.activate_tab(id, window, cx))
                                                    .ok();
                                            }),
                                    );
                                }
                                menu
                            },
                        ))
                    }),
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

#[cfg(test)]
mod tests {
    use super::reordered;

    #[test]
    fn dragging_takes_the_targets_slot() {
        let o = [1, 2, 3, 4];
        assert_eq!(reordered(&o, 1, 3), Some(vec![2, 3, 1, 4]), "moving right lands after the target");
        assert_eq!(reordered(&o, 4, 2), Some(vec![1, 4, 2, 3]), "moving left lands before the target");
        assert_eq!(reordered(&o, 2, 2), None);
        assert_eq!(reordered(&o, 9, 2), None);
    }
}
