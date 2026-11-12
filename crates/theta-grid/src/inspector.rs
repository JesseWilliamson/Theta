//! The right-hand cell inspector.

use crate::fold::{JsonDoc, Row, RowFold, toggle};
use crate::format::raw_text;
use crate::grid::ResultGrid;
use crate::imagekind::ImageKind;
use crate::inspect::{Body, Inspect, JsonTok};
use crate::render::Palette;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, FontWeight, HighlightStyle, Image, ImageFormat, ImageSource,
    InteractiveElement as _, IntoElement, ListHorizontalSizingBehavior, ObjectFit, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, StyledText,
    UniformListScrollHandle, div, img, px, uniform_list,
};
use std::collections::BTreeSet;
use std::sync::Arc;
use theta_core::TypeKind;
use theta_core::Value;
use theta_ui::{CHROME_TEXT_SIZE, PanelHeader, Settings, SyntaxColors, tool_button};

/// The cell viewer's derived state: everything that is computed once per cursor move instead of once
/// per frame. Rebuilt by `ResultGrid::sync_inspector`.
#[derive(Default)]
pub(crate) struct ViewerState {
    /// A decodable picture, ready for `img`. Built from the cell's bytes (at most `MAX_IMAGE_BYTES`).
    pub image: Option<Arc<Image>>,
    pub doc: Option<Arc<JsonDoc>>,
    /// Lines of the pretty JSON that are folded.
    pub folds: BTreeSet<usize>,
    /// The rows `doc` shows under `folds`.
    pub rows: Arc<Vec<Row>>,
    /// One entry per line of the hex dump.
    pub hex: Arc<Vec<SharedString>>,
    pub scroll: UniformListScrollHandle,
}

impl ViewerState {
    pub(crate) fn build(info: Option<&Inspect>, value: Option<&Value>) -> Self {
        let mut st = ViewerState::default();
        match (info.map(|i| &i.body), value) {
            (Some(Body::Json { text, spans }), _) => {
                let doc = Arc::new(JsonDoc::new(text, spans));
                st.rows = Arc::new(doc.visible(&st.folds));
                st.doc = Some(doc);
            }
            (Some(Body::Hex(dump)), _) => {
                st.hex = Arc::new(dump.lines().map(|l| SharedString::from(l.to_string())).collect());
            }
            (Some(Body::Image(info)), Some(Value::Bytes(bytes))) => {
                let format = match info.kind {
                    ImageKind::Png => ImageFormat::Png,
                    ImageKind::Jpeg => ImageFormat::Jpeg,
                    ImageKind::Gif => ImageFormat::Gif,
                    ImageKind::Webp => ImageFormat::Webp,
                };
                st.image = Some(Arc::new(Image::from_bytes(format, bytes.clone())));
            }
            _ => {}
        }
        st
    }

    pub(crate) fn fold_all(&mut self, collapse: bool) {
        let Some(doc) = &self.doc else { return };
        self.folds = if collapse { doc.collapse_all() } else { BTreeSet::new() };
        self.rows = Arc::new(doc.visible(&self.folds));
    }

    fn toggle_fold(&mut self, line: usize) {
        let Some(doc) = &self.doc else { return };
        toggle(&mut self.folds, line);
        self.rows = Arc::new(doc.visible(&self.folds));
    }
}

pub(crate) const INSPECTOR_W: f32 = 340.0;

impl ResultGrid {
    pub(crate) fn render_inspector(&self, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let settings = Settings::get(cx);
        let mono: SharedString = settings.buffer_font_family.clone().into();
        let fsize = settings.buffer_font_size - 1.0;
        let syn = SyntaxColors::get(cx);
        let rs = self.result.as_ref();
        let sel = self.sel;

        let is_json = self.viewer.doc.as_ref().is_some_and(|d| d.foldable().next().is_some());
        let header = PanelHeader::new("Cell viewer")
            .when(is_json, |h| {
                h.action(
                    tool_button("viewer-collapse", IconName::ChevronsDownUp, "Fold all ([)")
                        .on_click(cx.listener(|this, _, _, cx| this.fold_json(true, cx))),
                )
                .action(
                    tool_button("viewer-expand", IconName::ChevronsUpDown, "Unfold all (])")
                        .on_click(cx.listener(|this, _, _, cx| this.fold_json(false, cx))),
                )
            })
            .action(tool_button("inspector-copy", IconName::Copy, "Copy value (Y)").on_click(cx.listener(
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
                    Body::Json { .. } => json_view(self, &mono, fsize, p, &syn, cx),
                    Body::Hex(_) => hex_view(self, &mono, fsize, p, cx),
                    Body::Image(_) => image_view(self, p),
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

/// Pretty JSON, virtualised by line, with a fold chevron on every multi-line container. A line is
/// not wrapped: scrolling sideways reads a long string better than a wrapped brace tower does, and
/// the plain-text rendering of the same value is one `Copy` away.
fn json_view(
    grid: &ResultGrid,
    mono: &SharedString,
    fsize: f32,
    p: Palette,
    syn: &SyntaxColors,
    cx: &mut Context<ResultGrid>,
) -> AnyElement {
    let row_h = fsize * 1.5;
    let (key, string, number, literal, punct) =
        (syn.parameter, syn.string, syn.number, syn.constant, syn.punctuation);
    let color = move |t: JsonTok| match t {
        JsonTok::Key => key,
        JsonTok::Str => string,
        JsonTok::Number => number,
        JsonTok::Literal => literal,
        JsonTok::Punct => punct,
    };
    let count = grid.viewer.rows.len();
    let mono = mono.clone();
    let list = uniform_list(
        "viewer-json",
        count,
        cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
            let Some(doc) = this.viewer.doc.clone() else { return Vec::new() };
            let rows = this.viewer.rows.clone();
            let end = range.end.min(rows.len());
            let start = range.start.min(end);
            rows[start..end]
                .iter()
                .map(|row| {
                    let line = &doc.lines[row.line];
                    let mut text = line.text.clone();
                    let mut highlights: Vec<_> = line
                        .spans
                        .iter()
                        .map(|(r, t)| {
                            (r.clone(), HighlightStyle { color: Some(color(*t)), ..Default::default() })
                        })
                        .collect();
                    if let RowFold::Folded { closer, .. } = &row.fold {
                        let from = text.len();
                        text.push_str(&format!(" … {closer}"));
                        highlights.push((
                            from..text.len(),
                            HighlightStyle { color: Some(p.muted), ..Default::default() },
                        ));
                    }
                    let n = row.line;
                    let chevron = match &row.fold {
                        RowFold::None => div().w(px(14.)).flex_shrink_0().into_any_element(),
                        fold => {
                            let icon = if matches!(fold, RowFold::Open) {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            };
                            div()
                                .id(("viewer-fold", n))
                                .w(px(14.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(p.muted)
                                .hover(|s| s.text_color(p.fg))
                                .child(Icon::new(icon).size_3())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.viewer.toggle_fold(n);
                                    cx.notify();
                                }))
                                .into_any_element()
                        }
                    };
                    h_flex()
                        .h(px(row_h))
                        .flex_shrink_0()
                        .items_center()
                        .child(
                            div()
                                .w(px(30.))
                                .flex_shrink_0()
                                .pr_1()
                                .text_right()
                                .text_color(p.muted.opacity(0.7))
                                .child(format!("{}", n + 1)),
                        )
                        .child(chevron)
                        .child(
                            div()
                                .whitespace_nowrap()
                                .child(StyledText::new(text).with_highlights(highlights)),
                        )
                })
                .collect()
        }),
    )
    .track_scroll(&grid.viewer.scroll)
    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    .size_full();
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .font_family(mono)
        .text_size(px(fsize))
        .line_height(px(row_h))
        .child(list)
        .child(Scrollbar::vertical(&grid.viewer.scroll).viewport_from_layout())
        .into_any_element()
}

/// The hex dump: `offset  bytes  |ascii|`, one virtual list row per line.
fn hex_view(
    grid: &ResultGrid,
    mono: &SharedString,
    fsize: f32,
    p: Palette,
    cx: &mut Context<ResultGrid>,
) -> AnyElement {
    let row_h = fsize * 1.5;
    let size = fsize - 1.0;
    let lines = grid.viewer.hex.clone();
    let list = uniform_list(
        "viewer-hex",
        lines.len(),
        cx.processor(move |this, range: std::ops::Range<usize>, _window, _cx| {
            let lines = this.viewer.hex.clone();
            let end = range.end.min(lines.len());
            let start = range.start.min(end);
            lines[start..end]
                .iter()
                .map(|l| {
                    // Split the three columns so each can take its own colour: the offset is
                    // chrome, the bytes are the data, the ASCII gutter is a hint.
                    let (offset, rest) = l.split_at(8.min(l.len()));
                    let (bytes, ascii) = rest.split_once(" |").unwrap_or((rest, ""));
                    h_flex()
                        .h(px(row_h))
                        .flex_shrink_0()
                        .items_center()
                        .whitespace_nowrap()
                        .child(div().text_color(p.muted).child(offset.to_string()))
                        .child(div().ml_2().child(bytes.trim_start().to_string()))
                        .child(div().ml_2().text_color(p.muted).child(format!("|{ascii}")))
                })
                .collect()
        }),
    )
    .track_scroll(&grid.viewer.scroll)
    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    .size_full();
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .font_family(mono.clone())
        .text_size(px(size))
        .line_height(px(row_h))
        .child(list)
        .child(Scrollbar::vertical(&grid.viewer.scroll).viewport_from_layout())
        .into_any_element()
}

/// A picture, scaled down to fit the pane (never up: a 16-pixel icon stays crisp and small).
fn image_view(grid: &ResultGrid, p: Palette) -> AnyElement {
    let Some(image) = grid.viewer.image.clone() else {
        return div().text_color(p.muted).child("The picture could not be prepared.").into_any_element();
    };
    v_flex()
        .flex_1()
        .min_h_0()
        .gap_2()
        .child(
            div()
                .id("viewer-image")
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .border_1()
                .border_color(p.border.opacity(0.6))
                .bg(p.head_bg)
                .overflow_hidden()
                .child(
                    img(ImageSource::Image(image)).object_fit(ObjectFit::ScaleDown).max_w_full().max_h_full(),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::{Cell, Selection};
    use crate::types::GridMeta;
    use gpui_kit::TestAppContext;
    use theta_core::{Column, ResultSet, TypeKind};

    fn png() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend(8u32.to_be_bytes());
        v.extend(8u32.to_be_bytes());
        v
    }

    fn grid(cx: &mut TestAppContext) -> (gpui_kit::Entity<ResultGrid>, &mut gpui_kit::VisualTestContext) {
        cx.update(theta_ui::init);
        let (g, vcx) = cx.add_window_view(ResultGrid::new);
        let col = |n: &str, kind| Column { name: n.into(), type_name: n.into(), kind, origin: None };
        let rs = Arc::new(ResultSet {
            columns: vec![col("json", TypeKind::Json), col("bytes", TypeKind::Bytes)],
            rows: vec![
                vec![Value::Json(r#"{"a":{"b":[1,2]},"c":1}"#.into()), Value::Bytes(png())],
                vec![Value::Null, Value::Bytes(vec![1, 2, 3])],
            ],
            truncated: false,
        });
        g.update_in(vcx, |g, window, cx| g.set_result(rs, GridMeta::new(), window, cx));
        (g, vcx)
    }

    #[gpui_kit::test]
    fn the_viewer_follows_the_cursor(cx: &mut TestAppContext) {
        let (g, vcx) = grid(cx);
        g.update_in(vcx, |g, window, cx| {
            g.set_selection(Some(Selection::single(Cell::new(0, 0))), false, cx);
            g.toggle_cell_viewer(cx);
            assert!(g.cell_viewer_open());
            g.sync_inspector(window, cx);
            assert!(g.viewer.doc.is_some() && g.viewer.image.is_none());

            // Moving the cursor marks the viewer stale; the next sync rebuilds it for the new cell.
            g.set_selection(Some(Selection::single(Cell::new(0, 1))), false, cx);
            g.sync_inspector(window, cx);
            assert!(g.viewer.doc.is_none() && g.viewer.image.is_some(), "a PNG header is a picture");

            g.set_selection(Some(Selection::single(Cell::new(1, 1))), false, cx);
            g.sync_inspector(window, cx);
            assert!(g.viewer.image.is_none());
            assert_eq!(g.viewer.hex.len(), 1, "three bytes are one dump line");
        });
    }

    #[gpui_kit::test]
    fn folds_apply_and_reset_when_the_cell_changes(cx: &mut TestAppContext) {
        let (g, vcx) = grid(cx);
        g.update_in(vcx, |g, window, cx| {
            g.set_selection(Some(Selection::single(Cell::new(0, 0))), false, cx);
            g.toggle_cell_viewer(cx);
            g.sync_inspector(window, cx);
            let all = g.viewer.rows.len();
            g.fold_json(true, cx);
            assert!(g.viewer.rows.len() < all, "the nested containers are folded");
            g.fold_json(false, cx);
            assert_eq!(g.viewer.rows.len(), all);
            g.fold_json(true, cx);
            g.set_selection(Some(Selection::single(Cell::new(0, 1))), false, cx);
            g.sync_inspector(window, cx);
            g.set_selection(Some(Selection::single(Cell::new(0, 0))), false, cx);
            g.sync_inspector(window, cx);
            assert_eq!(g.viewer.rows.len(), all, "a fresh cell starts unfolded");
        });
    }

    #[gpui_kit::test]
    fn copy_value_puts_the_raw_value_on_the_clipboard(cx: &mut TestAppContext) {
        let (g, vcx) = grid(cx);
        let copied = g.update_in(vcx, |g, _, cx| {
            g.copy_cell_value(cx);
            g.set_selection(Some(Selection::single(Cell::new(0, 0))), false, cx);
            g.copy_cell_value(cx)
        });
        assert!(copied);
        let text = vcx.read_from_clipboard().and_then(|c| c.text());
        assert_eq!(text.as_deref(), Some(r#"{"a":{"b":[1,2]},"c":1}"#));
    }
}
