//! The right-hand cell inspector.

use crate::format::raw_text;
use crate::grid::ResultGrid;
use crate::inspect::{Body, JsonTok};
use crate::render::Palette;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, div, px,
};
use theta_core::TypeKind;
use theta_ui::{CHROME_TEXT_SIZE, PanelHeader, Settings, SyntaxColors, tool_button};

pub(crate) const INSPECTOR_W: f32 = 340.0;

impl ResultGrid {
    pub(crate) fn render_inspector(&self, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let settings = Settings::get(cx);
        let mono: SharedString = settings.buffer_font_family.clone().into();
        let fsize = settings.buffer_font_size - 1.0;
        let syn = SyntaxColors::get(cx);
        let rs = self.result.as_ref();
        let sel = self.sel;

        let header = PanelHeader::new("Inspector")
            .action(tool_button("inspector-copy", IconName::Copy, "Copy value").on_click(cx.listener(
                |this, _, _, cx| {
                    if let Some(v) = this.cursor_value() {
                        let text = raw_text(v);
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        cx.emit(crate::GridEvent::Copied { cells: 1 });
                    }
                },
            )))
            .action(
                tool_button("inspector-close", IconName::X, "Close inspector")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_inspector(cx))),
            );

        let mut content = v_flex().flex_1().min_h_0().gap_2().px_3().pb_3();
        match (rs, sel, &self.inspector) {
            (Some(rs), Some(s), Some(info)) => {
                let col = &rs.columns[s.cursor.col.min(rs.columns.len().saturating_sub(1))];
                content = content.child(
                    v_flex()
                        .gap_0p5()
                        .child(div().font_weight(FontWeight::MEDIUM).truncate().child(col.name.clone()))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_size(px(11.))
                                .text_color(p.muted)
                                .child(div().font_family(mono.clone()).child(col.type_name.clone()))
                                .child(div().child(format!("row {}", s.cursor.row + 1)))
                                .when(!info.stats.is_empty(), |d| {
                                    d.child(div().child(format!("· {}", info.stats)))
                                }),
                        ),
                );
                let body: AnyElement = match &info.body {
                    Body::Null => div().italic().text_color(p.muted).child("NULL").into_any_element(),
                    Body::Text(_) => text_area(self, &mono, fsize, p),
                    Body::Json { text, spans } => {
                        let color = |t: JsonTok| match t {
                            JsonTok::Key => syn.parameter,
                            JsonTok::Str => syn.string,
                            JsonTok::Number => syn.number,
                            JsonTok::Literal => syn.constant,
                            JsonTok::Punct => syn.punctuation,
                        };
                        let highlights = spans
                            .iter()
                            .map(|(r, t)| {
                                (r.clone(), HighlightStyle { color: Some(color(*t)), ..Default::default() })
                            })
                            .collect::<Vec<_>>();
                        div()
                            .id("inspector-json")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .font_family(mono.clone())
                            .text_size(px(fsize))
                            .line_height(px(fsize * 1.5))
                            .child(StyledText::new(text.clone()).with_highlights(highlights))
                            .into_any_element()
                    }
                    Body::Hex(dump) => div()
                        .id("inspector-hex")
                        .flex_1()
                        .min_h_0()
                        .overflow_scroll()
                        .font_family(mono.clone())
                        .text_size(px(fsize - 1.0))
                        .whitespace_nowrap()
                        .children(dump.lines().map(|l| div().h(px(fsize * 1.5)).child(l.to_string())))
                        .into_any_element(),
                    Body::Geometry { facts, .. } => v_flex()
                        .flex_1()
                        .min_h_0()
                        .gap_2()
                        .child(v_flex().gap_1().children(facts.iter().map(|(k, v)| {
                            h_flex()
                                .gap_2()
                                .items_start()
                                .child(div().w(px(64.)).flex_shrink_0().text_color(p.muted).child(*k))
                                .child(div().flex_1().min_w_0().child(v.clone()))
                        })))
                        .child(
                            Button::new("inspector-map")
                                .small()
                                .primary()
                                .icon(Icon::new(IconName::MapPin))
                                .label("Show on Map")
                                .on_click(cx.listener(|this, _, _, cx| this.show_on_map(false, cx))),
                        )
                        .child(div().text_size(px(11.)).text_color(p.muted).child("EWKT"))
                        .child(text_area(self, &mono, fsize, p))
                        .into_any_element(),
                };
                content = content.child(div().flex_1().min_h_0().flex().flex_col().child(body));
                if let Some(note) = &info.note {
                    content =
                        content.child(div().text_size(px(11.)).text_color(p.warning).child(note.clone()));
                }
                if col.kind == TypeKind::Geometry && !matches!(info.body, Body::Geometry { .. }) {
                    content = content.child(div().text_color(p.muted).child("Not a valid geometry value."));
                }
            }
            _ => {
                content = content.child(
                    div()
                        .pt_4()
                        .text_color(p.muted)
                        .text_center()
                        .child("Select a cell to inspect its value."),
                );
            }
        }

        v_flex()
            .id("grid-inspector")
            .w(px(INSPECTOR_W))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(p.border.opacity(0.8))
            .bg(cx.theme().sidebar)
            .text_size(CHROME_TEXT_SIZE)
            .child(header)
            .child(content)
            .into_any_element()
    }
}

/// Read-only selectable text (soft-wrapped, monospace).
fn text_area(grid: &ResultGrid, mono: &SharedString, fsize: f32, _p: Palette) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .font_family(mono.clone())
        .text_size(px(fsize))
        .child(Textarea::new(&grid.inspector_text).readonly(true).appearance(false).bordered(false).h_full())
        .into_any_element()
}
