//! Record view: the current row laid out as a two-column list (column name and type on the left, the
//! full value on the right). It is another way of looking at the grid's own selection: the row is
//! `selection.cursor.row` and the highlighted field is `selection.cursor.col`, so editing, staging,
//! copying and the inspector all keep working unchanged.
//!
//! The list is virtualised with GPUI's variable-height `list`, because a wrapped value is taller than a
//! row and a 500-column row must not build 500 elements.

use crate::format::{CellKind, cell_text, format_count, raw_text};
use crate::grid::{ROW_H, ResultGrid};
use crate::inspect::{Body, inspect_value};
use crate::pending::{CellState, RowState};
use crate::render::{Palette, palette};
use crate::selection::{Cell, Selection};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ListAlignment, ListOffset, ListState,
    MouseButton, ParentElement as _, SharedString, Styled as _, div, list, px,
};
use theta_core::{Column, Value};
use theta_ui::{CHROME_TEXT_SIZE, Settings, tool_button};

/// Longest value text laid out in one field. A record shows complete values, but a multi-megabyte
/// blob would make the list unusable; the note says how much is hidden and the inspector has the rest.
pub const MAX_FIELD_CHARS: usize = 4_000;
/// JSON larger than this is shown as raw text: pretty-printing it on every frame would be wasteful.
const MAX_PRETTY_JSON: usize = 20_000;
/// Width of the name column.
const NAME_W: f32 = 240.0;

pub(crate) struct RecordState {
    pub list: ListState,
}

/// What one field shows.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldText {
    pub text: String,
    pub kind: CellKind,
    /// "… 12,345 more characters" when the text was cut.
    pub note: Option<String>,
}

pub fn field_text(value: &Value) -> FieldText {
    let cut = |s: String| -> (String, Option<String>) {
        let n = s.chars().count();
        if n <= MAX_FIELD_CHARS {
            (s, None)
        } else {
            let head: String = s.chars().take(MAX_FIELD_CHARS).collect();
            (head, Some(format!("… {} more characters", format_count(n - MAX_FIELD_CHARS))))
        }
    };
    match value {
        Value::Json(s) => {
            let text = if s.len() <= MAX_PRETTY_JSON {
                match inspect_value(value).body {
                    Body::Json { text, .. } => text,
                    _ => s.clone(),
                }
            } else {
                s.clone()
            };
            let (text, note) = cut(text);
            FieldText { text, kind: CellKind::Json, note }
        }
        Value::Text(s) if !s.is_empty() => {
            let (text, note) = cut(s.clone());
            FieldText { text, kind: CellKind::Text, note }
        }
        other => {
            let c = cell_text(other);
            match other {
                // Dates, arrays, UUIDs and the like: the display form is already complete.
                Value::Array(_) | Value::Unknown(_) | Value::Uuid(_) | Value::Interval(_) => {
                    let (text, note) = cut(raw_text(other));
                    FieldText { text, kind: c.kind, note }
                }
                _ => FieldText { text: c.text, kind: c.kind, note: None },
            }
        }
    }
}

/// `12 / 600`, or `0 / 0` for an empty result. `row` is zero-based.
pub fn row_counter(row: usize, total: usize) -> String {
    if total == 0 { "0 / 0".into() } else { format!("{} / {}", format_count(row + 1), format_count(total)) }
}

/// The row as `name<TAB>value` lines (NULL as an empty value), ready to paste into a spreadsheet.
pub fn row_text(columns: &[Column], values: &[&Value]) -> String {
    columns
        .iter()
        .zip(values)
        .map(|(c, v)| format!("{}\t{}", c.name, raw_text(v).replace('\t', " ").replace('\n', "\\n")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `scroll_to_reveal_item` works from item heights, which are only known for items that have been laid
/// out; for a far jump (End on a 500-field row) they are estimates and it lands in the wrong place.
/// An item that is not currently laid out is put at the top instead, which is exact.
pub(crate) fn reveal_item(list: &ListState, ix: usize) {
    match list.bounds_for_item(ix) {
        Some(_) => list.scroll_to_reveal_item(ix),
        None => list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) }),
    }
}

impl ResultGrid {
    pub fn is_record_view(&self) -> bool {
        self.record.is_some()
    }

    pub fn toggle_record_view(&mut self, cx: &mut Context<Self>) {
        if self.record.is_some() {
            self.record = None;
        } else {
            let n = self.n_cols();
            self.record = Some(RecordState { list: ListState::new(n, ListAlignment::Top, px(300.)) });
            if self.sel.is_none() && self.n_rows() > 0 && n > 0 {
                self.sel = Some(Selection::single(Cell::new(0, 0)));
                self.refresh_summary_pub();
                cx.emit(crate::types::GridEvent::SelectionChanged);
            }
            self.record_reveal_field();
        }
        cx.notify();
    }

    /// Called when a new result arrives: the list must be rebuilt for the new column count.
    pub(crate) fn record_reset(&mut self) {
        let n = self.n_cols();
        if let Some(r) = &self.record {
            r.list.reset(n);
        }
    }

    /// Keep the highlighted field on screen and re-measure after the row changed.
    pub(crate) fn record_reveal_field(&self) {
        if let (Some(r), Some(s)) = (&self.record, self.sel) {
            r.list.remeasure();
            reveal_item(&r.list, s.cursor.col);
        }
    }

    /// Move to the previous (`-1`) or next (`1`) row, or to the first / last with `i64::MIN` / `MAX`.
    pub(crate) fn record_step(&mut self, delta: i64, cx: &mut Context<Self>) {
        let (nr, nc) = (self.n_rows(), self.n_cols());
        if nr == 0 || nc == 0 {
            return;
        }
        let cur = self.sel.map_or(Cell::new(0, 0), |s| s.cursor);
        let row = (cur.row as i64).saturating_add(delta).clamp(0, nr as i64 - 1) as usize;
        self.set_selection(Some(Selection::single(Cell::new(row, cur.col.min(nc - 1)))), false, cx);
    }

    /// Copy the whole row (`name<TAB>value` per line).
    pub(crate) fn copy_record_row(&mut self, cx: &mut Context<Self>) {
        let (Some(rs), Some(sel)) = (&self.result, self.sel) else { return };
        let values: Vec<&Value> = (0..rs.columns.len()).map(|c| self.value_at(sel.cursor.row, c)).collect();
        let text = row_text(&rs.columns, &values);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        cx.emit(crate::types::GridEvent::Copied { cells: values.len() });
    }

    pub(crate) fn copy_record_value(&mut self, cx: &mut Context<Self>) {
        let Some(sel) = self.sel else { return };
        let text = raw_text(self.value_at(sel.cursor.row, sel.cursor.col));
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        cx.emit(crate::types::GridEvent::Copied { cells: 1 });
    }

    pub(crate) fn render_record(&mut self, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let nr = self.n_rows();
        let nc = self.n_cols();
        let cur = self.sel.map_or(0, |s| s.cursor.row.min(nr.saturating_sub(1)));
        let can_prev = cur > 0;
        let can_next = cur + 1 < nr;
        let nav = h_flex()
            .h(px(28.))
            .flex_shrink_0()
            .px_2()
            .gap_1()
            .items_center()
            .justify_between()
            .bg(p.bar_bg)
            .border_b_1()
            .border_color(p.border.opacity(0.6))
            .text_size(CHROME_TEXT_SIZE)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        tool_button("rec-prev", IconName::ChevronLeft, "Previous row (Up, k)")
                            .disabled(!can_prev)
                            .on_click(cx.listener(|this, _, _, cx| this.record_step(-1, cx))),
                    )
                    .child(div().min_w(px(64.)).text_center().child(row_counter(cur, nr)))
                    .child(
                        tool_button("rec-next", IconName::ChevronRight, "Next row (Down, j)")
                            .disabled(!can_next)
                            .on_click(cx.listener(|this, _, _, cx| this.record_step(1, cx))),
                    )
                    .child(div().ml_2().text_color(p.muted).child(format!(
                        "{} column{}",
                        format_count(nc),
                        if nc == 1 { "" } else { "s" }
                    ))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("rec-copy-value")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Copy))
                            .label("Value")
                            .tooltip("Copy the highlighted value (Ctrl+C)")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_record_value(cx))),
                    )
                    .child(
                        Button::new("rec-copy-row")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Copy))
                            .label("Row")
                            .tooltip("Copy the whole row as name and value lines (Shift+Y)")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_record_row(cx))),
                    ),
            );
        let body: AnyElement = match (&self.record, nr) {
            (Some(r), n) if n > 0 && nc > 0 => {
                let state = r.list.clone();
                div()
                    .id("record-body")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        list(state, cx.processor(|this, c: usize, _, cx| this.render_record_field(c, cx)))
                            .size_full(),
                    )
                    .into_any_element()
            }
            _ => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted)
                .child("No rows")
                .into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .justify_start()
            .child(nav)
            .child(body)
            .into_any_element()
    }

    fn render_record_field(&mut self, c: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let s = Settings::get(cx);
        let mono: SharedString = s.buffer_font_family.clone().into();
        let fsize = s.buffer_font_size - 1.0;
        let Some(sel) = self.sel else { return div().into_any_element() };
        let row = sel.cursor.row;
        let Some(col) = self.result.as_ref().and_then(|r| r.columns.get(c)).cloned() else {
            return div().into_any_element();
        };
        let rr = self.row_ref(row);
        let (row_state, cell_state) = match &self.pending {
            Some(pd) => (pd.row_state(rr), pd.cell_state(rr, c)),
            None => (RowState::Normal, CellState::Normal),
        };
        let selected = sel.cursor.col == c;
        let editing = self.editor.as_ref().is_some_and(|e| e.col == c && e.row == rr);
        let value = self.value_at(row, c).clone();
        let ft = field_text(&value);
        let is_key = self.is_key_column(c);
        let reference = self.is_reference_column(c);

        let shown: AnyElement = if editing {
            let ed = self.editor.as_ref().expect("editing implies an editor");
            let err = ed.error.clone();
            v_flex()
                .w_full()
                .gap_0p5()
                .child(
                    div()
                        .key_context("GridCellEditor")
                        .w_full()
                        .h(px(ROW_H - 2.))
                        .flex()
                        .items_center()
                        .bg(p.bg)
                        .border_1()
                        .border_color(if err.is_some() { p.danger } else { p.accent })
                        .child(
                            Input::new(&ed.input)
                                .with_size(px(fsize / 0.875))
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false),
                        ),
                )
                .when_some(err, |d, e| d.child(div().text_size(px(11.)).text_color(p.danger).child(e)))
                .into_any_element()
        } else {
            let text: SharedString = ft.text.clone().into();
            let value_el = match ft.kind {
                CellKind::Null => div().italic().text_color(p.muted).child("NULL").into_any_element(),
                CellKind::EmptyText => div().text_color(p.muted).child("''").into_any_element(),
                CellKind::Bool(b) => {
                    div().text_color(if b { p.green } else { p.red }).child(text).into_any_element()
                }
                CellKind::Bytes => div().text_color(p.muted).child(text).into_any_element(),
                CellKind::Geometry => h_flex()
                    .gap(px(5.))
                    .items_center()
                    .child(Icon::new(IconName::MapPin).size(px(11.)).text_color(p.accent).flex_shrink_0())
                    .child(div().child(text))
                    .into_any_element(),
                _ => div().child(text).into_any_element(),
            };
            v_flex()
                .w_full()
                .min_w_0()
                .child(value_el)
                .when_some(ft.note.clone(), |d, n| {
                    d.child(div().text_color(p.muted).text_size(px(11.)).child(n))
                })
                .into_any_element()
        };

        let tint = match (row_state, cell_state) {
            (RowState::Deleted, _) => Some(p.deleted),
            (RowState::New, _) => Some(p.added),
            (_, CellState::Modified) => Some(p.modified),
            _ => None,
        };
        h_flex()
            .id(("rec-field", c))
            .w_full()
            .min_h(px(ROW_H))
            .items_start()
            .font_family(mono)
            .text_size(px(fsize))
            .border_b_1()
            .border_l_2()
            .border_color(p.line)
            .when_some(tint, |d, t| d.bg(t))
            .when(selected, |d| d.bg(p.sel_fill).border_color(p.accent))
            .when(row_state == RowState::Deleted, |d| d.line_through())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    if this.editor.is_some() && !this.commit_edit(crate::editing::After::Stay, window, cx) {
                        return;
                    }
                    let row = this.sel.map_or(0, |s| s.cursor.row);
                    this.set_selection(Some(Selection::single(Cell::new(row, c))), false, cx);
                    if ev.click_count >= 2 {
                        this.activate_cursor(false, window, cx);
                    }
                }),
            )
            .child(
                h_flex()
                    .w(px(NAME_W))
                    .flex_shrink_0()
                    .gap_2()
                    .items_center()
                    .px_2()
                    .min_h(px(ROW_H))
                    .overflow_hidden()
                    .child(
                        div()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .when(is_key, |d| d.text_color(p.accent))
                            .child(SharedString::from(col.name.clone())),
                    )
                    .when(reference, |d| {
                        d.child(Icon::new(IconName::Link).size(px(11.)).text_color(p.muted).flex_shrink_0())
                    })
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .child(SharedString::from(col.type_name.clone())),
                    ),
            )
            .child(div().flex_1().min_w_0().px_2().py(px(3.)).child(shown))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::TypeKind;

    fn col(name: &str) -> Column {
        Column { name: name.into(), type_name: "text".into(), kind: TypeKind::Text, origin: None }
    }

    #[test]
    fn counter() {
        assert_eq!(row_counter(11, 600), "12 / 600");
        assert_eq!(row_counter(0, 0), "0 / 0");
        assert_eq!(row_counter(999, 12_345), "1,000 / 12,345");
    }

    #[test]
    fn json_is_pretty_printed() {
        let f = field_text(&Value::Json(r#"{"a":1,"b":[true,null]}"#.into()));
        assert_eq!(f.kind, CellKind::Json);
        assert!(f.text.contains("\n  \"a\": 1"), "{}", f.text);
        assert!(f.note.is_none());
    }

    #[test]
    fn huge_json_stays_raw_and_is_cut() {
        let big = format!("[{}]", "1,".repeat(20_000));
        let f = field_text(&Value::Json(big));
        assert!(!f.text.contains('\n'));
        assert_eq!(f.text.chars().count(), MAX_FIELD_CHARS);
        assert!(f.note.unwrap().starts_with("… "));
    }

    #[test]
    fn long_text_is_complete_up_to_the_cap_with_real_newlines() {
        let f = field_text(&Value::Text("line one\nline two".into()));
        assert_eq!(f.text, "line one\nline two");
        let f = field_text(&Value::Text("é".repeat(MAX_FIELD_CHARS + 5)));
        assert_eq!(f.note.as_deref(), Some("… 5 more characters"));
    }

    #[test]
    fn null_empty_and_geometry_kinds() {
        assert_eq!(field_text(&Value::Null).kind, CellKind::Null);
        assert_eq!(field_text(&Value::Text(String::new())).kind, CellKind::EmptyText);
        assert_eq!(field_text(&Value::Bool(true)).kind, CellKind::Bool(true));
        assert_eq!(field_text(&Value::Geometry(vec![1, 2, 3])).kind, CellKind::Geometry);
    }

    #[test]
    fn row_text_is_name_tab_value() {
        let cols = [col("id"), col("note"), col("x")];
        let vals = [Value::Int(7), Value::Text("a\tb\nc".into()), Value::Null];
        let refs: Vec<&Value> = vals.iter().collect();
        assert_eq!(row_text(&cols, &refs), "id\t7\nnote\ta b\\nc\nx\t");
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::agg_ui::AggState;
    use crate::types::GridMeta;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Arc;
    use theta_core::{ResultSet, TypeKind};

    fn grid(cx: &mut TestAppContext, rows: usize) -> (gpui_kit::Entity<ResultGrid>, &mut VisualTestContext) {
        cx.update(|cx| theta_ui::init(cx));
        let (g, vcx) = cx.add_window_view(|window, cx| ResultGrid::new(window, cx));
        let columns = ["a", "b", "c"]
            .iter()
            .map(|n| Column {
                name: (*n).into(),
                type_name: "int4".into(),
                kind: TypeKind::Integer,
                origin: None,
            })
            .collect();
        let rows = (0..rows as i64)
            .map(|i| {
                vec![Value::Int(i), Value::Int(i * 10), if i % 2 == 0 { Value::Null } else { Value::Int(1) }]
            })
            .collect();
        let rs = Arc::new(ResultSet { columns, rows, truncated: false });
        g.update_in(vcx, |g, window, cx| g.set_result(rs, GridMeta::new(), window, cx));
        (g, vcx)
    }

    #[gpui_kit::test]
    fn record_view_steps_through_rows_and_fields(cx: &mut TestAppContext) {
        let (g, vcx) = grid(cx, 5);
        g.update_in(vcx, |g, _, cx| {
            g.toggle_record_view(cx);
            assert!(g.is_record_view());
            assert_eq!(g.sel.unwrap().cursor, Cell::new(0, 0));
            g.record_step(1, cx);
            g.record_step(1, cx);
            assert_eq!(g.sel.unwrap().cursor.row, 2);
            g.record_step(-100, cx);
            assert_eq!(g.sel.unwrap().cursor.row, 0, "clamped at the first row");
            g.record_step(i64::MAX, cx);
            assert_eq!(g.sel.unwrap().cursor.row, 4, "clamped at the last row");
            g.toggle_record_view(cx);
            assert!(!g.is_record_view());
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn aggregates_follow_the_selection(cx: &mut TestAppContext) {
        let (g, vcx) = grid(cx, 4);
        g.update_in(vcx, |g, _, cx| {
            g.select_all(cx);
            g.toggle_aggregates(cx);
            let AggState::Ready(s) = &g.agg else { panic!("small selections are computed at once") };
            assert_eq!((s.cells, s.nulls, s.count), (12, 2, 10));
            assert_eq!(s.sum, Some(0. + 1. + 2. + 3. + 0. + 10. + 20. + 30. + 1. + 1.));
            g.set_selection(Some(Selection::single(Cell::new(1, 1))), false, cx);
            let AggState::Ready(s) = &g.agg else { panic!() };
            assert_eq!((s.cells, s.min.as_deref()), (1, Some("10")));
            g.toggle_aggregates(cx);
            assert!(!g.aggregates_open());
        });
    }
}
