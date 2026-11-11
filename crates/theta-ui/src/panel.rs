//! Panel chrome shared by every view: header, empty state, toolbar buttons.

use crate::keyhint::KeyHint;
use crate::tokens::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window, div, px,
};

/// A 22px ghost icon button with a tooltip: the only button style used in
/// panel headers, the title bar and the status bar, so they all match.
pub fn tool_button(id: impl Into<ElementId>, icon: IconName, tooltip: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().with_size(TOOL_BUTTON_SIZE).icon(Icon::new(icon)).tooltip(tooltip)
}

/// 28px header: medium-weight 12px title on the left, icon buttons on the right.
#[derive(IntoElement)]
pub struct PanelHeader {
    title: SharedString,
    actions: Vec<AnyElement>,
    leading: Option<AnyElement>,
}

impl PanelHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), actions: Vec::new(), leading: None }
    }
    /// Right-aligned action (usually a [`tool_button`]); call repeatedly.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
    /// Element shown before the title (e.g. an icon).
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .h(PANEL_HEADER_HEIGHT)
            .flex_shrink_0()
            .px_3()
            .gap_1()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .overflow_hidden()
                    .when_some(self.leading, |this, l| this.child(l))
                    .child(
                        div()
                            .text_size(CHROME_TEXT_SIZE)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .truncate()
                            .child(self.title),
                    ),
            )
            .child(h_flex().gap_0p5().items_center().children(self.actions))
    }
}

use gpui_kit::prelude::FluentBuilder as _;

/// Centered icon + title + explanation (+ optional action and key hint) used
/// for every empty, loading-less or failed region.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Option<Icon>,
    title: SharedString,
    description: Option<SharedString>,
    action: Option<AnyElement>,
    key_hint: Option<KeyHint>,
    tone_error: bool,
}

impl EmptyState {
    pub fn new(icon: IconName, title: impl Into<SharedString>) -> Self {
        Self {
            icon: Some(Icon::new(icon)),
            title: title.into(),
            description: None,
            action: None,
            key_hint: None,
            tone_error: false,
        }
    }
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
    pub fn action(mut self, button: impl IntoElement) -> Self {
        self.action = Some(button.into_any_element());
        self
    }
    pub fn key_hint(mut self, hint: KeyHint) -> Self {
        self.key_hint = Some(hint);
        self
    }
    /// Tint the icon with the error colour (failed states).
    pub fn error(mut self) -> Self {
        self.tone_error = true;
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let icon_color = if self.tone_error { theme.danger } else { theme.muted_foreground.opacity(0.8) };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .py_4()
            .when_some(self.icon, |this, icon| this.child(icon.size(px(26.)).text_color(icon_color)))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .text_center()
                    .child(self.title),
            )
            .when_some(self.description, |this, d| {
                this.child(
                    div()
                        .max_w(px(320.))
                        .text_size(CHROME_TEXT_SIZE)
                        .line_height(px(17.))
                        .text_color(theme.muted_foreground)
                        .text_center()
                        .child(d),
                )
            })
            .when(self.action.is_some() || self.key_hint.is_some(), |this| {
                this.child(
                    h_flex()
                        .mt_1()
                        .gap_2()
                        .items_center()
                        .when_some(self.action, |t, a| t.child(a))
                        .when_some(self.key_hint, |t, k| t.child(k)),
                )
            })
    }
}
