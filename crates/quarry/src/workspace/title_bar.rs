//! Title bar: app menu, connection switcher on the left; run/cancel,
//! transaction state and settings on the right.

use super::Workspace;
use crate::actions::*;
use crate::connections::ConnState;
use crate::explorer::db_icon;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use quarry_ui::{CHROME_TEXT_SIZE, DbKindIcon, tool_button};

impl Workspace {
    pub(super) fn render_title_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // Converting here ends the borrow of `self`: an `impl IntoElement` return
        // would capture it and block the other panels built in `render`.
        self.render_title_bar_inner(window, cx).into_any_element()
    }

    fn render_title_bar_inner(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let conn_id = self.active_connection_id(cx);
        let (name, kind, state, color) = {
            let store = self.store.read(cx);
            match conn_id.as_ref().and_then(|id| store.config(id)) {
                Some(c) => {
                    (Some(c.name.clone()), Some(c.kind), Some(store.state(&c.id).clone()), c.color.clone())
                }
                None => (None, None, None, None),
            }
        };
        let dot = match &state {
            Some(ConnState::Connected { .. }) => Some(theme.success),
            Some(ConnState::Connecting) => Some(theme.warning),
            Some(ConnState::Failed(_)) => Some(theme.danger),
            _ => None,
        };
        let tint =
            color.as_deref().and_then(|h| h.strip_prefix('#')).and_then(|h| u32::from_str_radix(h, 16).ok());
        let running = self.running;
        let transaction = self.txn_open_for_active(cx);

        let switcher = h_flex()
            .id("connection-switcher")
            .h(px(24.))
            .px_2()
            .gap_1p5()
            .items_center()
            .rounded(px(4.))
            .cursor_default()
            .hover(|s| s.bg(theme.list_hover))
            .on_click(
                cx.listener(|_, _, window, cx| window.dispatch_action(Box::new(OpenConnectionPicker), cx)),
            )
            .tooltip(|window, cx| Tooltip::new("Switch connection").build(window, cx))
            .when_some(tint, |d, c| d.child(div().w(px(3.)).h(px(12.)).rounded(px(1.5)).bg(gpui_kit::rgb(c))))
            .map_icon(kind)
            .child(
                div()
                    .text_size(CHROME_TEXT_SIZE + px(0.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if name.is_some() { theme.foreground } else { theme.muted_foreground })
                    .child(name.unwrap_or_else(|| "No connection".to_string())),
            )
            .when_some(dot, |d, c| d.child(div().size(px(6.)).rounded_full().bg(c)))
            .child(Icon::new(IconName::ChevronsUpDown).size(px(12.)).text_color(theme.muted_foreground));

        TitleBar::new().bg(theme.title_bar).child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Button::new("app-menu")
                                .ghost()
                                .with_size(quarry_ui::TOOL_BUTTON_SIZE)
                                .icon(Icon::new(IconName::Menu))
                                .dropdown_menu(|menu, _, _| {
                                    menu.menu("New Console", Box::new(NewConsole))
                                        .menu("New Connection…", Box::new(NewConnection))
                                        .separator()
                                        .menu("Command Palette…", Box::new(OpenCommandPalette))
                                        .menu("Go to Table…", Box::new(OpenTablePicker))
                                        .separator()
                                        .menu("Quit", Box::new(Quit))
                                }),
                        )
                        .child(div().w(px(1.)).h(px(14.)).bg(theme.border))
                        .child(switcher),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .pr_2()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .when(transaction, |d| {
                            d.child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        h_flex()
                                            .h(px(22.))
                                            .px_2()
                                            .gap_1()
                                            .items_center()
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(theme.warning.opacity(0.5))
                                            .bg(theme.warning.opacity(0.18))
                                            .text_size(CHROME_TEXT_SIZE)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.warning)
                                            .child(Icon::new(IconName::TriangleAlert).size(px(12.)))
                                            .child("TXN open"),
                                    )
                                    .child(
                                        Button::new("txn-commit")
                                            .small()
                                            .success()
                                            .outline()
                                            .icon(Icon::new(IconName::Check))
                                            .label("Commit")
                                            .tooltip("Commit Transaction")
                                            .on_click(|_, window, cx| {
                                                window.dispatch_action(Box::new(CommitTransaction), cx)
                                            }),
                                    )
                                    .child(
                                        Button::new("txn-rollback")
                                            .small()
                                            .danger()
                                            .outline()
                                            .icon(Icon::new(IconName::Undo2))
                                            .label("Roll Back")
                                            .tooltip("Roll Back Transaction")
                                            .on_click(|_, window, cx| {
                                                window.dispatch_action(Box::new(RollbackTransaction), cx)
                                            }),
                                    ),
                            )
                        })
                        .child(
                            Button::new("run")
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::Play).text_color(theme.success))
                                .label("Run")
                                .tooltip("Run Statement")
                                .disabled(running)
                                .on_click(|_, window, cx| window.dispatch_action(Box::new(RunStatement), cx)),
                        )
                        .child(
                            Button::new("cancel")
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::Square).text_color(if running {
                                    theme.danger
                                } else {
                                    theme.muted_foreground
                                }))
                                .tooltip("Cancel Query")
                                .disabled(!running)
                                .on_click(|_, window, cx| window.dispatch_action(Box::new(CancelQuery), cx)),
                        )
                        .child(div().w(px(1.)).h(px(14.)).mx_1().bg(theme.border))
                        .child(tool_button("settings", IconName::Settings, "Settings").dropdown_menu(
                            |menu, _, _| {
                                menu.menu("Settings…", Box::new(OpenSettings))
                                    .menu("Select Theme…", Box::new(SelectTheme))
                                    .menu("Toggle Keymap (Helix / Standard)", Box::new(ToggleKeymap))
                                    .menu("Import Zed Theme…", Box::new(ImportZedTheme))
                                    .separator()
                                    .menu("Increase Editor Font Size", Box::new(ZoomIn))
                                    .menu("Decrease Editor Font Size", Box::new(ZoomOut))
                                    .menu("Reset Editor Font Size", Box::new(ZoomReset))
                                    .separator()
                                    .menu("Open Settings File", Box::new(OpenSettingsFile))
                            },
                        )),
                ),
        )
    }
}

trait MapIcon: Sized {
    fn map_icon(self, kind: Option<quarry_core::DatabaseKind>) -> Self;
}

impl MapIcon for gpui_kit::Stateful<gpui_kit::Div> {
    fn map_icon(self, kind: Option<quarry_core::DatabaseKind>) -> Self {
        match kind {
            Some(k) => self.child(DbKindIcon::new(db_icon(k)).size(px(14.))),
            None => self.child(Icon::new(IconName::Database).size(px(14.))),
        }
    }
}
