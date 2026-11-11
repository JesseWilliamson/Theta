//! Popups painted next to the text: the completion list and the diagnostic hover card.
//! Both are ordinary GPUI elements handed to `Window::defer_draw`, so they float above the
//! editor and are not clipped to it.

use crate::completion::{CompletionState, highlight_segments};
use crate::element::Palette;
use crate::view::EditorView;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, FontWeight, HighlightStyle, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, Styled as _, StyledText, div, px,
};
use theta_sql::{CompletionKind, Severity};
use theta_ui::SyntaxColors;

pub(crate) const ROW_H: f32 = 24.0;
const FOOTER_H: f32 = 30.0;
const LIST_PAD: f32 = 4.0;

pub(crate) fn kind_look(kind: CompletionKind, cx: &App) -> (IconName, Hsla) {
    let t = cx.theme();
    let s = SyntaxColors::get(cx);
    match kind {
        CompletionKind::Keyword => (IconName::Code, s.keyword),
        CompletionKind::Table => (IconName::Table2, t.blue),
        CompletionKind::View => (IconName::Eye, t.cyan),
        CompletionKind::Column => (IconName::Columns3, t.green),
        CompletionKind::Function => (IconName::SquareFunction, t.yellow),
        CompletionKind::Schema => (IconName::Database, t.red),
        CompletionKind::Alias => (IconName::AtSign, t.cyan_light),
        CompletionKind::Join => (IconName::GitMerge, t.magenta),
        CompletionKind::Snippet => (IconName::Scissors, t.muted_foreground),
    }
}

/// Pixel size of the popup for the current state (what `place_popup` needs before layout).
pub(crate) fn completion_size(state: &CompletionState) -> (f32, f32) {
    let vis = state.visible();
    let rows = vis.len() as f32;
    let (mut label_w, mut detail_w) = (0.0f32, 0.0f32);
    for it in &state.items[vis] {
        label_w = label_w.max(it.label.chars().count() as f32 * 7.4);
        detail_w = detail_w.max(it.detail.as_ref().map_or(0, |d| d.chars().count().min(28)) as f32 * 6.4);
    }
    let w = (50.0 + label_w + detail_w + 16.0).clamp(300.0, 560.0);
    let footer = if state.footer().is_some() { FOOTER_H } else { 0.0 };
    (w, rows * ROW_H + LIST_PAD * 2.0 + footer + 2.0)
}

pub(crate) fn completion_popup(
    view: Entity<EditorView>,
    state: &CompletionState,
    buffer_font: SharedString,
    cx: &App,
) -> (AnyElement, (f32, f32)) {
    let theme = cx.theme();
    let (w, h) = completion_size(state);
    let vis = state.visible();
    let mut rows = Vec::with_capacity(vis.len());
    for ix in vis.clone() {
        let item = &state.items[ix];
        let selected = ix == state.selected;
        let (icon, color) = kind_look(item.kind, cx);
        // Matched characters of the typed prefix, as byte ranges into the label.
        let mut ranges = Vec::new();
        let mut at = 0usize;
        for (text, matched) in highlight_segments(&item.label, &state.prefix) {
            if matched {
                ranges.push((
                    at..at + text.len(),
                    HighlightStyle {
                        color: Some(theme.ring),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                ));
            }
            at += text.len();
        }
        let label = StyledText::new(item.label.clone()).with_highlights(ranges);
        let (v1, v2) = (view.clone(), view.clone());
        rows.push(
            h_flex()
                .id(("completion-row", ix))
                .h(px(ROW_H))
                .mx_1()
                .px_2()
                .gap_2()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .when(selected, |d| d.bg(theme.list_active))
                .on_mouse_move(move |_, _, cx| {
                    v1.update(cx, |v, cx| v.select_completion(ix, cx));
                })
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    v2.update(cx, |v, cx| {
                        v.select_completion(ix, cx);
                        v.accept_completion(window, cx);
                    });
                    cx.stop_propagation();
                })
                .child(Icon::new(icon).size(px(14.)).text_color(color))
                .child(
                    // The name is what you are choosing by: it keeps its width and the detail gives way.
                    div()
                        .flex_shrink_0()
                        .max_w(px(360.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.))
                        .text_color(theme.foreground)
                        .child(label),
                )
                .when_some(
                    item.detail.clone().filter(|d| !(selected && state.footer() == Some(d.as_str()))),
                    |d, text| {
                        d.child(
                            div()
                                .ml_auto()
                                .min_w_0()
                                .max_w(px(210.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(text),
                        )
                    },
                ),
        );
    }
    let wheel = view.clone();
    let footer = state.footer().map(|t| t.to_string());
    let el = v_flex()
        .id("completion-popup")
        .w(px(w))
        .h(px(h))
        .occlude()
        .overflow_hidden()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_md()
        .on_scroll(move |ev, _, cx| {
            let dy = f32::from(ev.delta.pixel_delta(px(ROW_H)).y);
            let rows = if dy.abs() < 1.0 { 0 } else { -(dy / ROW_H).round() as isize };
            let rows = if rows == 0 && dy != 0.0 { -dy.signum() as isize } else { rows };
            wheel.update(cx, |v, cx| {
                if let Some(c) = &mut v.completion {
                    c.scroll_rows(rows);
                    cx.notify();
                }
            });
            cx.stop_propagation();
        })
        .child(v_flex().py(px(LIST_PAD)).children(rows))
        .when_some(footer, |d, text| {
            d.child(
                h_flex()
                    .h(px(FOOTER_H))
                    .px_3()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.border)
                    .font_family(buffer_font)
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(text),
            )
        })
        .into_any_element();
    (el, (w, h))
}

pub(crate) fn hover_popover(
    message: &str,
    severity: Severity,
    pal: &Palette,
    cx: &App,
) -> (AnyElement, (f32, f32)) {
    let theme = cx.theme();
    let chars = message.chars().count() as f32;
    let w = (chars * 6.3 + 38.0).clamp(140.0, 440.0);
    let lines = ((chars * 6.3) / (w - 38.0)).ceil().max(1.0);
    let h = lines * 18.0 + 14.0;
    let color = pal.severity(severity);
    let el = h_flex()
        .id("diagnostic-hover")
        .w(px(w))
        .min_h(px(h))
        .occlude()
        .gap_2()
        .px_3()
        .py(px(6.))
        .items_start()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_md()
        .child(div().mt(px(5.)).size(px(8.)).flex_shrink_0().rounded_full().bg(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(theme.popover_foreground)
                .child(message.to_string()),
        )
        .into_any_element();
    (el, (w, h))
}
