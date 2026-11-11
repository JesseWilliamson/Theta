//! `Render` for `EditorView`: the focus/key/action shell, the context menu, the bottom prompt
//! bar and the which-key box. The text itself is the custom element in `element.rs`.

use crate::actions::*;
use crate::element::EditorElement;
use crate::view::{EditorView, HINT_DELAY};
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div, px,
};
use quarry_editor::Effect;
use quarry_ui::{CHROME_TEXT_SIZE, EditorColors, KeyHint};
use std::time::Duration;

impl EditorView {
    fn which_key(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (title, hints) = self.editor.key_hints()?;
        let since = self.hints_since?;
        let immediate = self.editor.pending_keys() == "space";
        if !immediate && since.elapsed() < HINT_DELAY {
            return None;
        }
        let theme = cx.theme();
        let rows = if hints.len() > 8 { hints.len().div_ceil(2) } else { hints.len() };
        let mut columns: Vec<Vec<AnyElement>> = Vec::new();
        for chunk in hints.chunks(rows.max(1)) {
            columns.push(
                chunk
                    .iter()
                    .map(|h| {
                        h_flex()
                            .h(px(22.))
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .min_w(px(34.))
                                    .justify_center()
                                    .child(KeyHint::from_label(h.key.clone())),
                            )
                            .child(
                                div()
                                    .text_size(CHROME_TEXT_SIZE)
                                    .text_color(theme.popover_foreground)
                                    .whitespace_nowrap()
                                    .child(h.description.clone()),
                            )
                            .into_any_element()
                    })
                    .collect(),
            );
        }
        Some(
            v_flex()
                .id("which-key")
                .absolute()
                .right(px(18.))
                .bottom(px(14.))
                .occlude()
                .gap_1()
                .px_3()
                .py_2()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_md()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(title),
                )
                .child(
                    h_flex()
                        .gap_6()
                        .items_start()
                        .children(columns.into_iter().map(|c| v_flex().gap(px(1.)).children(c))),
                )
                .into_any_element(),
        )
    }

    fn bottom_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let colors = EditorColors::get(cx);
        let h = self.line_height_px() + 6.0;
        let base = h_flex()
            .id("editor-bottom-line")
            .h(px(h))
            .flex_shrink_0()
            .px_3()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .font_family(self.font_family.clone())
            .text_size(px(self.font_size));
        if let Some(p) = self.editor.prompt() {
            let before: String = p.text.chars().take(p.cursor).collect();
            let after: String = p.text.chars().skip(p.cursor).collect();
            let search = matches!(p.prefix, "/" | "?" | "find:");
            let counter = search.then(|| {
                let matches = self.editor.search_matches();
                let from = self.editor.primary().from();
                let cur = matches.iter().position(|m| m.start >= from).map(|i| i + 1).unwrap_or(0);
                (cur, matches.len())
            });
            let bar_h = (self.line_height_px() - 4.0).max(8.0);
            return Some(
                base.child(
                    h_flex()
                        .items_center()
                        .child(div().mr(px(6.)).text_color(theme.ring).child(p.prefix.to_string()))
                        .child(div().whitespace_nowrap().text_color(colors.foreground).child(before))
                        .child(div().w(px(2.)).h(px(bar_h)).bg(colors.cursor))
                        .child(div().whitespace_nowrap().text_color(colors.foreground).child(after)),
                )
                .when_some(counter, |d, (cur, total)| {
                    let none = total == 0;
                    d.child(
                        div()
                            .text_size(CHROME_TEXT_SIZE)
                            .text_color(if none { theme.danger } else { theme.muted_foreground })
                            .child(if none { "no matches".to_string() } else { format!("{cur}/{total}") }),
                    )
                })
                .into_any_element(),
            );
        }
        None
    }

    /// Engine status text ("yanked 2 lines", command errors) for a few seconds. It floats over
    /// the bottom edge instead of taking a row, so showing and hiding it never shifts the text.
    fn message_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.editor.prompt().is_some() {
            return None;
        }
        let (msg, _) = self.message.as_ref()?;
        let theme = cx.theme();
        Some(
            h_flex()
                .id("editor-message")
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(self.line_height_px() + 6.0))
                .px_3()
                .items_center()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .font_family(self.font_family.clone())
                .text_size(px(self.font_size))
                .text_color(theme.muted_foreground)
                .child(div().whitespace_nowrap().overflow_hidden().child(msg.clone()))
                .into_any_element(),
        )
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ensure_bindings(cx);
        if std::mem::take(&mut self.pending_initial_diagnostics)
            && let Some(p) = self.diagnostics_provider_clone()
        {
            let rev = self.editor.revision();
            self.spawn_diagnostics(p, rev, Duration::ZERO, cx);
        }
        let colors = EditorColors::get(cx);
        let view = cx.entity();
        let focus = self.focus_handle.clone();
        let has_selection = self.editor.selected_text().is_some();
        let which_key = self.which_key(cx);
        let bottom = self.bottom_bar(cx);
        let message = self.message_bar(cx);

        div()
            .id("quarry-editor")
            .key_context(key_context(
                self.editor.keymap(),
                self.editor.mode(),
                self.editor.prompt().is_some(),
                self.completion.is_some(),
                self.editor.keymap() == quarry_editor::Keymap::Vim
                    && self.editor.prompt().is_none()
                    && !self.editor.pending_keys().is_empty(),
            ))
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .on_key_down(cx.listener(|this, ev, window, cx| this.on_key_down(ev, window, cx)))
            .on_action(cx.listener(|this, _: &EditorCopy, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &EditorCut, window, cx| this.cut_selection(window, cx)))
            .on_action(cx.listener(|this, _: &EditorPaste, window, cx| this.paste_clipboard(window, cx)))
            .on_action(cx.listener(|this, _: &EditorRunStatement, _, cx| {
                this.emit_effect(Effect::RunStatementAtCursor, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorRunSelection, _, cx| {
                let e = this.run_effect();
                this.emit_effect(e, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorExplain, _, cx| {
                this.emit_effect(Effect::ExplainStatement { analyze: false }, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorFormat, _, cx| {
                let e = if this.editor.selected_text().is_some() {
                    Effect::FormatSelection
                } else {
                    Effect::FormatDocument
                };
                this.emit_effect(e, cx)
            }))
            .context_menu(move |menu, _window, _cx| {
                let _ = &view;
                menu.action_context(focus.clone())
                    .menu_with_icon_and_disabled("Cut", IconName::Copy, Box::new(EditorCut), !has_selection)
                    .menu_with_icon_and_disabled("Copy", IconName::Copy, Box::new(EditorCopy), !has_selection)
                    .menu_with_icon("Paste", IconName::Copy, Box::new(EditorPaste))
                    .separator()
                    .menu_with_icon("Run Statement", IconName::Play, Box::new(EditorRunStatement))
                    .menu_with_icon_and_disabled(
                        "Run Selection",
                        IconName::Play,
                        Box::new(EditorRunSelection),
                        !has_selection,
                    )
                    .menu_with_icon("Explain", IconName::ChartPie, Box::new(EditorExplain))
                    .menu_with_icon("Format", IconName::Menu, Box::new(EditorFormat))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(EditorElement::new(cx.entity()))
                    .children(message)
                    .children(which_key),
            )
            .children(bottom)
    }
}
