//! 26px status bar: panel toggles on the left, editor/connection/result slots on the right.

use super::Workspace;
use crate::actions::*;
use crate::connections::ConnState;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, h_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use quarry_ui::{CHROME_TEXT_SIZE, STATUS_BAR_HEIGHT, tool_button};

impl Workspace {
    pub(super) fn render_status_bar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let item = |text: SharedString| -> AnyElement {
            div()
                .px_2()
                .text_size(CHROME_TEXT_SIZE)
                .text_color(muted)
                .whitespace_nowrap()
                .child(text)
                .into_any_element()
        };

        let mut right: Vec<AnyElement> = Vec::new();
        if let Some(info) = self.status.result_info.clone() {
            right.push(item(info));
        }
        if let Some(n) = self.status.selection_count.filter(|n| *n > 0) {
            right.push(item(format!("{n} selected").into()));
        }
        if self.active_console().is_some() {
            let (l, c) = self.status.cursor.unwrap_or((1, 1));
            right.push(item(format!("Ln {l}, Col {c}").into()));
        }
        if let Some(mode) = self.status.editor_mode.clone() {
            right.push(item(mode));
        }
        if let Some(id) = self.active_connection_id(cx) {
            let store = self.store.read(cx);
            if let Some(config) = store.config(&id) {
                let (color, text) = match store.state(&id) {
                    ConnState::Connected { server, .. } => {
                        (theme.success, format!("{} · {}", config.name, server.version))
                    }
                    ConnState::Connecting => (theme.warning, format!("{} · connecting…", config.name)),
                    ConnState::Failed(_) => (theme.danger, format!("{} · connection failed", config.name)),
                    ConnState::Disconnected => {
                        (muted.opacity(0.5), format!("{} · disconnected", config.name))
                    }
                };
                right.push(
                    h_flex()
                        .px_2()
                        .gap_1p5()
                        .items_center()
                        .child(div().size(px(6.)).rounded_full().bg(color))
                        .child(
                            div()
                                .text_size(CHROME_TEXT_SIZE)
                                .text_color(muted)
                                .whitespace_nowrap()
                                .child(text),
                        )
                        .into_any_element(),
                );
            }
        }

        h_flex()
            .h(STATUS_BAR_HEIGHT)
            .flex_shrink_0()
            .px_1()
            .items_center()
            .justify_between()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_0p5()
                    .items_center()
                    .child(
                        tool_button("status-explorer", IconName::PanelLeft, "Toggle Explorer")
                            .selected(self.state.explorer_open)
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleExplorer), cx)),
                    )
                    .child(
                        tool_button("status-bottom", IconName::PanelBottom, "Toggle Bottom Panel")
                            .selected(self.state.bottom_open)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleBottomPanel), cx)
                            }),
                    ),
            )
            .child(h_flex().items_center().children(right))
            .into_any_element()
    }
}
