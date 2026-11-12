//! 26px status bar: panel toggles on the left, editor/connection/result slots on the right.

use super::Workspace;
use crate::actions::*;
use crate::connections::ConnState;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use theta_ui::{CHROME_TEXT_SIZE, STATUS_BAR_HEIGHT, tool_button};

/// Colour of the mode chip. Helix and Vim share the scale: Normal is the accent, Insert is green,
/// selecting modes are warm (Visual yellow, V-LINE magenta, V-BLOCK cyan) and Replace is red.
fn mode_chip_color(mode: theta_editor::Mode, theme: &gpui_kit::component::Theme) -> gpui_kit::Hsla {
    use theta_editor::Mode;
    match mode {
        Mode::Insert => theme.success,
        Mode::Select | Mode::Visual => theme.warning,
        Mode::VisualLine => theme.magenta,
        Mode::VisualBlock => theme.cyan,
        Mode::Replace => theme.danger,
        Mode::Normal | Mode::Standard => theme.primary,
    }
}

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
        if let Some(elapsed) = self.running_elapsed(cx) {
            right.push(
                h_flex()
                    .px_2()
                    .gap_1p5()
                    .items_center()
                    .child(gpui_kit::component::spinner::Spinner::new().xsmall().color(theme.warning))
                    .child(
                        div()
                            .text_size(CHROME_TEXT_SIZE)
                            .text_color(theme.foreground)
                            .whitespace_nowrap()
                            .child(format!("Running {:.1} s", elapsed.as_secs_f32())),
                    )
                    .into_any_element(),
            );
        }
        if let Some(info) = self.status.result_info.clone() {
            right.push(if self.status.result_error {
                div()
                    .px_2()
                    .text_size(CHROME_TEXT_SIZE)
                    .text_color(theme.danger)
                    .whitespace_nowrap()
                    .child(info)
                    .into_any_element()
            } else {
                item(info)
            });
        }
        if let Some(st) = self.editor_status.clone().filter(|_| self.active_console().is_some()) {
            if st.selection_count > 1 {
                right.push(item(format!("{} selections", st.selection_count).into()));
            } else if st.selected_chars > 0 {
                right.push(item(format!("{} selected", st.selected_chars).into()));
            }
            right.push(item(format!("Ln {}, Col {}", st.line, st.column).into()));
            if !st.pending_keys.is_empty() {
                right.push(
                    div()
                        .px_2()
                        .text_size(CHROME_TEXT_SIZE)
                        .text_color(theme.warning)
                        .font_family(theta_ui::Settings::get(cx).buffer_font_family.clone())
                        .child(st.pending_keys.clone())
                        .into_any_element(),
                );
            }
            if let Some(reg) = st.recording {
                right.push(
                    div()
                        .px_2()
                        .text_size(CHROME_TEXT_SIZE)
                        .text_color(theme.danger)
                        .whitespace_nowrap()
                        .child(format!("recording @{reg}"))
                        .into_any_element(),
                );
            }
            if st.keymap != theta_editor::Keymap::Standard {
                let color = mode_chip_color(st.mode, &theme);
                right.push(
                    div()
                        .mx_1()
                        .px_1p5()
                        .rounded(px(3.))
                        .bg(color.opacity(0.2))
                        .text_size(CHROME_TEXT_SIZE)
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(st.mode_label())
                        .into_any_element(),
                );
            }
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
