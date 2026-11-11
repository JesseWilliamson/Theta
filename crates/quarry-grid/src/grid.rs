//! `ResultGrid`: state and behaviour. Rendering lives in `render.rs`, keyboard
//! and pointer handling in `input.rs`, cell editing in `editing.rs`.

use crate::compare::{SortKey, cycle_sort};
use crate::copy::build_tsv;
use crate::filter::FilterQuery;
use crate::format::{self, CellKind, display_chars};
use crate::inspect::{Inspect, inspect_value};
use crate::layout::{self, ColumnLayout};
use crate::pending::{Pending, RowRef};
use crate::scroll::GridScroll;
use crate::selection::{Cell, CellRange, Motion, Selection};
use crate::summary::{NumericAgg, selection_summary};
use crate::types::*;
use crate::view::{BACKGROUND_THRESHOLD, build_view};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Pixels,
    SharedString, Subscription, Task, Window, px,
};
use quarry_core::{ResultSet, TypeKind, Value};
use quarry_ui::Settings;
use std::cell::Cell as StdCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) const ROW_H: f32 = 24.0;
pub(crate) const HEADER_H: f32 = 26.0;
/// Rows beyond which a selection's numeric aggregate is skipped: summing six
/// million cells on every click would be noticeable.
const AGGREGATE_LIMIT: usize = 1_000_000;

static NULL: Value = Value::Null;

#[derive(Clone)]
pub(crate) struct CachedCell {
    pub text: SharedString,
    pub kind: CellKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drag {
    Cells,
    Rows,
    Cols,
}

pub(crate) struct Resize {
    pub col: usize,
    pub start_x: f32,
    pub start_w: f32,
}

pub(crate) struct CellEditor {
    pub row: RowRef,
    pub col: usize,
    pub input: Entity<InputState>,
    pub error: Option<SharedString>,
    pub _subs: Vec<Subscription>,
}

/// Render-path timing, enabled with `QUARRY_GRID_TIMING=1`. It measures only
/// the time spent building the element tree in `render` (what this crate
/// controls); GPUI's own layout/paint is outside it.
#[derive(Default)]
pub(crate) struct RenderStats {
    pub enabled: bool,
    pub frames: u32,
    pub total: std::time::Duration,
    pub worst: std::time::Duration,
    pub cells: usize,
    pub first: Option<std::time::Instant>,
}

pub struct ResultGrid {
    pub(crate) focus: FocusHandle,
    pub(crate) result: Option<Arc<ResultSet>>,
    pub(crate) meta: GridMeta,
    /// Original row numbers in display order; `None` is the identity order.
    pub(crate) view: Option<Vec<u32>>,
    pub(crate) sorts: Vec<SortKey>,
    pub(crate) filter_input: Entity<InputState>,
    pub(crate) filter_open: bool,
    pub(crate) filter_query: Option<FilterQuery>,
    view_gen: u64,
    view_task: Option<Task<()>>,
    pub(crate) view_busy: bool,
    pub(crate) cols: ColumnLayout,
    pub(crate) gutter_w: f32,
    pub(crate) char_w: f32,
    font_key: (SharedString, u32),
    pub(crate) scroll: GridScroll,
    pub(crate) body_bounds: Rc<StdCell<Bounds<Pixels>>>,
    pub(crate) sel: Option<Selection>,
    pub(crate) drag: Option<Drag>,
    pub(crate) hover_row: Option<usize>,
    pub(crate) summary: Option<String>,
    pub(crate) pending: Option<Pending>,
    pub(crate) editor: Option<CellEditor>,
    pub(crate) submit_error: Option<String>,
    pub(crate) inspector_open: bool,
    pub(crate) inspector_text: Entity<TextareaState>,
    pub(crate) inspector: Option<Inspect>,
    pub(crate) inspector_dirty: bool,
    cache: HashMap<(u32, u32), CachedCell>,
    pub(crate) loading: bool,
    /// Body width before an inspector toggle; the cursor is re-revealed once the
    /// measured width differs.
    pub(crate) reveal_pending: Option<f32>,
    pub(crate) modal_g: bool,
    pub(crate) modal_visual: bool,
    pub(crate) resize: Option<Resize>,
    pub(crate) stats: RenderStats,
    pub(crate) _subs: Vec<Subscription>,
}

impl EventEmitter<GridEvent> for ResultGrid {}

impl ResultGrid {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::actions::bind_keys(cx);
        let filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter rows…  (column:text to narrow)"));
        let inspector_text = cx.new(|cx| TextareaState::new(window, cx));
        let subs = vec![cx.subscribe_in(&filter_input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.filter_changed(cx);
            } else if matches!(ev, InputEvent::PressEnter { .. }) {
                window.focus(&this.focus, cx);
            }
        })];
        Self {
            focus: cx.focus_handle(),
            result: None,
            meta: GridMeta::default(),
            view: None,
            sorts: Vec::new(),
            filter_input,
            filter_open: false,
            filter_query: None,
            view_gen: 0,
            view_task: None,
            view_busy: false,
            cols: ColumnLayout::default(),
            gutter_w: 40.0,
            char_w: 7.5,
            font_key: (SharedString::default(), 0),
            scroll: GridScroll::default(),
            body_bounds: Rc::new(StdCell::new(Bounds::default())),
            sel: None,
            drag: None,
            hover_row: None,
            summary: None,
            pending: None,
            editor: None,
            submit_error: None,
            inspector_open: false,
            inspector_text,
            inspector: None,
            inspector_dirty: false,
            cache: HashMap::new(),
            loading: false,
            reveal_pending: None,
            modal_g: false,
            modal_visual: false,
            resize: None,
            stats: RenderStats {
                enabled: std::env::var_os("QUARRY_GRID_TIMING").is_some(),
                ..Default::default()
            },
            _subs: subs,
        }
    }

    // ---------------------------------------------------------------- public API

    /// Show `result`, resetting sort, filter, selection and pending edits.
    pub fn set_result(
        &mut self,
        result: Arc<ResultSet>,
        meta: GridMeta,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.measure_font(window, cx);
        self.view_task = None;
        self.view_gen += 1;
        self.view_busy = false;
        self.view = None;
        self.sorts.clear();
        self.filter_query = None;
        self.filter_open = false;
        self.filter_input.update(cx, |s, cx| s.set_value("", window, cx));
        self.sel = None;
        self.drag = None;
        self.hover_row = None;
        self.summary = None;
        self.editor = None;
        self.submit_error = None;
        self.modal_g = false;
        self.modal_visual = false;
        self.cache.clear();
        self.pending = meta.editable.clone().map(|t| Pending::new(t, result.columns.len()));
        self.cols = ColumnLayout::new(self.initial_widths(&result, &meta));
        self.gutter_w = layout::gutter_width(result.rows.len(), self.char_w);
        self.scroll.set(0.0, 0.0);
        self.result = Some(result);
        self.meta = meta;
        self.loading = false;
        self.inspector = None;
        self.inspector_dirty = true;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.result = None;
        self.meta = GridMeta::default();
        self.view = None;
        self.view_task = None;
        self.view_gen += 1;
        self.sorts.clear();
        self.filter_query = None;
        self.sel = None;
        self.pending = None;
        self.editor = None;
        self.summary = None;
        self.submit_error = None;
        self.cache.clear();
        self.loading = false;
        self.inspector = None;
        cx.notify();
    }

    /// Dim the previous result and show a slim progress bar. Layout is unchanged.
    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        if self.loading != loading {
            self.loading = loading;
            cx.notify();
        }
    }

    /// Scroll the body by a pixel delta (positive = right/down). Used by the
    /// demo's scripted benchmark; users scroll with wheel and keyboard.
    pub fn scroll_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        if self.scroll.scroll_by(dx, dy) {
            cx.notify();
        }
    }

    pub fn result(&self) -> Option<&Arc<ResultSet>> {
        self.result.as_ref()
    }

    /// Select (and scroll to) the row that is row `row` of the result set, whatever the current
    /// sort or filter shows. Does nothing when a filter hides it. Does not take focus, so a
    /// click on the map can drive the grid without leaving the map tab.
    pub fn select_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(result) = &self.result else { return };
        let Some(display) = crate::view::display_position(self.view.as_deref(), result.rows.len(), row)
        else {
            return;
        };
        if self.n_cols() == 0 {
            return;
        }
        self.set_selection(Some(Selection::row(display, 0)), true, cx);
    }

    pub fn has_pending_changes(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| !p.is_empty())
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }

    /// Footer text for the current selection, e.g. `3 rows × 2 cols · sum 1,234.5 · avg 411.5`.
    pub fn selection_summary(&self) -> Option<String> {
        self.summary.clone()
    }

    /// The rows the user sees (sort/filter applied, staged edits included,
    /// staged deletes excluded), restricted to the selected rectangle when
    /// `selection_only`.
    pub fn export_view(&self, selection_only: bool) -> ResultSet {
        let Some(rs) = &self.result else { return ResultSet::default() };
        let nrows = self.n_rows();
        let ncols = rs.columns.len();
        let (rows, cols) = match (selection_only, self.sel) {
            (true, Some(sel)) => {
                let r = sel.range(nrows, ncols);
                (r.r0..r.r1 + 1, r.c0..r.c1 + 1)
            }
            _ => (0..nrows, 0..ncols),
        };
        let mut out = ResultSet {
            columns: rs.columns[cols.clone()].to_vec(),
            rows: Vec::with_capacity(rows.len()),
            truncated: rs.truncated && !selection_only,
        };
        for r in rows {
            if self.is_deleted(r) {
                continue;
            }
            out.rows.push(cols.clone().map(|c| self.value_at(r, c).clone()).collect());
        }
        out
    }

    /// The submit succeeded: fold the edits into the displayed data.
    pub fn changes_applied(&mut self, cx: &mut Context<Self>) {
        let (Some(result), Some(pending)) = (self.result.as_mut(), self.pending.as_mut()) else { return };
        pending.apply_to(Arc::make_mut(result));
        self.submit_error = None;
        self.editor = None;
        self.cache.clear();
        self.gutter_w = layout::gutter_width(result.rows.len(), self.char_w);
        self.recompute_view(cx);
        self.inspector_dirty = true;
        cx.notify();
    }

    /// The submit failed: keep the staged edits and show `message` in the footer.
    pub fn changes_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.submit_error = Some(message);
        cx.notify();
    }

    // ------------------------------------------------------------- data access

    pub(crate) fn n_view(&self) -> usize {
        match (&self.view, &self.result) {
            (Some(v), _) => v.len(),
            (None, Some(r)) => r.rows.len(),
            _ => 0,
        }
    }

    pub(crate) fn n_new(&self) -> usize {
        self.pending.as_ref().map_or(0, |p| p.new_row_count())
    }

    pub(crate) fn n_rows(&self) -> usize {
        self.n_view() + self.n_new()
    }

    pub(crate) fn n_cols(&self) -> usize {
        self.result.as_ref().map_or(0, |r| r.columns.len())
    }

    pub(crate) fn row_ref(&self, r: usize) -> RowRef {
        let nv = self.n_view();
        if r < nv {
            RowRef::Orig(self.view.as_ref().map_or(r, |v| v[r] as usize))
        } else {
            RowRef::New(r - nv)
        }
    }

    pub(crate) fn is_deleted(&self, r: usize) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.row_state(self.row_ref(r)) == crate::pending::RowState::Deleted)
    }

    pub(crate) fn value_at(&self, r: usize, c: usize) -> &Value {
        let Some(rs) = &self.result else { return &NULL };
        let rr = self.row_ref(r);
        match &self.pending {
            Some(p) => p.effective(rs, rr, c),
            None => match rr {
                RowRef::Orig(i) => rs.rows.get(i).and_then(|row| row.get(c)).unwrap_or(&NULL),
                RowRef::New(_) => &NULL,
            },
        }
    }

    pub(crate) fn cell_text(&mut self, r: usize, c: usize) -> CachedCell {
        let key = match self.row_ref(r) {
            RowRef::Orig(i) => (i as u32, c as u32),
            RowRef::New(i) => (0x8000_0000 | i as u32, c as u32),
        };
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let ct = format::cell_text(self.value_at(r, c));
        let cell = CachedCell { text: ct.text.into(), kind: ct.kind };
        // Bounded: a long scroll through 6M cells must not retain them all.
        if self.cache.len() > 30_000 {
            self.cache.clear();
        }
        self.cache.insert(key, cell.clone());
        cell
    }

    pub(crate) fn column_kind(&self, c: usize) -> TypeKind {
        self.result.as_ref().and_then(|r| r.columns.get(c)).map_or(TypeKind::Other, |col| col.kind)
    }

    pub(crate) fn is_reference_column(&self, c: usize) -> bool {
        self.meta.reference_columns.contains(&c)
    }

    pub(crate) fn is_key_column(&self, c: usize) -> bool {
        self.meta.editable.as_ref().is_some_and(|t| t.key_columns.contains(&c))
    }

    // ------------------------------------------------------------------ layout

    pub(crate) fn measure_font(&mut self, _window: &mut Window, cx: &App) {
        let s = Settings::get(cx);
        let key = (SharedString::from(s.buffer_font_family.clone()), (s.buffer_font_size * 10.0) as u32);
        if key == self.font_key {
            return;
        }
        let size = px(s.buffer_font_size - 1.0);
        let ts = cx.text_system();
        let font = gpui_kit::font(key.0.clone());
        let id = ts.resolve_font(&font);
        if let Ok(adv) = ts.advance(id, size, 'm') {
            self.char_w = f32::from(adv.width);
        }
        self.font_key = key;
    }

    fn initial_widths(&self, rs: &ResultSet, meta: &GridMeta) -> Vec<f32> {
        let key_cols: &[usize] = meta.editable.as_ref().map_or(&[], |t| &t.key_columns);
        rs.columns
            .iter()
            .enumerate()
            .map(|(c, col)| {
                let mut px = header_px(&col.name, &col.type_name);
                if key_cols.contains(&c) || meta.reference_columns.contains(&c) {
                    px += 16.0;
                }
                let longest = rs
                    .rows
                    .iter()
                    .take(layout::SAMPLE_ROWS)
                    .map(|r| r.get(c).map_or(4, display_chars))
                    .max()
                    .unwrap_or(4);
                layout::auto_width(px, longest, self.char_w)
            })
            .collect()
    }

    pub(crate) fn autofit_column(&mut self, col: usize, cx: &mut Context<Self>) {
        let Some(rs) = self.result.clone() else { return };
        let Some(c) = rs.columns.get(col) else { return };
        let mut longest =
            (0..self.n_rows().min(1000)).map(|r| display_chars(self.value_at(r, col))).max().unwrap_or(4);
        // Also cover what is on screen, which may be far from the first rows.
        let (_, sy) = self.scroll.xy();
        let vis = layout::visible_rows(sy, 600.0, ROW_H, self.n_rows(), 0);
        longest = longest.max(vis.map(|r| display_chars(self.value_at(r, col))).max().unwrap_or(0));
        let mut px = header_px(&c.name, &c.type_name);
        if self.is_key_column(col) || self.is_reference_column(col) {
            px += 16.0;
        }
        self.cols.set_width(col, layout::auto_width(px, longest, self.char_w));
        cx.notify();
    }

    pub(crate) fn body_size(&self) -> (f32, f32) {
        let b = self.body_bounds.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w > 10.0 && h > 10.0 { (w, h) } else { (800.0, 400.0) }
    }

    // --------------------------------------------------------- sort and filter

    pub(crate) fn toggle_sort(&mut self, col: usize, additive: bool, cx: &mut Context<Self>) {
        self.sorts = cycle_sort(&self.sorts, col, additive);
        self.recompute_view(cx);
    }

    pub(crate) fn set_sort(&mut self, col: usize, descending: Option<bool>, cx: &mut Context<Self>) {
        self.sorts = descending.map(|d| vec![SortKey { column: col, descending: d }]).unwrap_or_default();
        self.recompute_view(cx);
    }

    pub(crate) fn filter_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.filter_input.read(cx).value();
        let columns = self.result.as_ref().map(|r| r.columns.clone()).unwrap_or_default();
        let q = FilterQuery::parse(&text, &columns);
        if q != self.filter_query {
            self.filter_query = q;
            self.recompute_view(cx);
        }
    }

    pub(crate) fn open_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_open = true;
        let input = self.filter_input.clone();
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_open = false;
        self.filter_input.update(cx, |s, cx| s.set_value("", window, cx));
        if self.filter_query.take().is_some() {
            self.recompute_view(cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn recompute_view(&mut self, cx: &mut Context<Self>) {
        let Some(result) = self.result.clone() else { return };
        self.view_gen += 1;
        let generation = self.view_gen;
        if self.filter_query.is_none() && self.sorts.is_empty() {
            self.view_task = None;
            self.view_busy = false;
            self.apply_view(None, cx);
            return;
        }
        let (filter, sorts) = (self.filter_query.clone(), self.sorts.clone());
        if result.rows.len() <= BACKGROUND_THRESHOLD {
            self.view_task = None;
            self.view_busy = false;
            let v = build_view(&result, filter.as_ref(), &sorts);
            self.apply_view(Some(v), cx);
            return;
        }
        // Large result: sort an index vector on the background executor; a
        // newer request supersedes (drops) this task via the generation check.
        self.view_busy = true;
        self.view_task = Some(cx.spawn(async move |this, cx| {
            let v = cx.background_spawn(async move { build_view(&result, filter.as_ref(), &sorts) }).await;
            this.update(cx, |g, cx| {
                if g.view_gen == generation {
                    g.view_busy = false;
                    g.apply_view(Some(v), cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn apply_view(&mut self, view: Option<Vec<u32>>, cx: &mut Context<Self>) {
        self.view = view;
        let (nr, nc) = (self.n_rows(), self.n_cols());
        if let Some(sel) = self.sel {
            self.sel = (nr > 0 && nc > 0).then(|| sel.clamped(nr, nc));
        }
        self.hover_row = None;
        self.cache.clear();
        self.refresh_summary();
        self.inspector_dirty = true;
        cx.notify();
    }

    // ---------------------------------------------------------------- selection

    pub(crate) fn current_range(&self) -> Option<CellRange> {
        self.sel.map(|s| s.range(self.n_rows(), self.n_cols()))
    }

    pub(crate) fn set_selection(&mut self, sel: Option<Selection>, reveal: bool, cx: &mut Context<Self>) {
        if self.sel == sel {
            return;
        }
        self.sel = sel;
        self.refresh_summary();
        self.inspector_dirty = true;
        if reveal && let Some(s) = sel {
            self.reveal(s.cursor);
        }
        cx.emit(GridEvent::SelectionChanged);
        cx.notify();
    }

    pub(crate) fn move_selection(&mut self, motion: Motion, extend: bool, cx: &mut Context<Self>) {
        let (nr, nc) = (self.n_rows(), self.n_cols());
        if nr == 0 || nc == 0 {
            return;
        }
        let next = match self.sel {
            Some(s) => s.moved(motion, extend, nr, nc),
            None => Selection::single(Cell::new(0, 0)),
        };
        self.set_selection(Some(next), true, cx);
    }

    pub(crate) fn page_rows(&self) -> usize {
        ((self.body_size().1 / ROW_H) as usize).saturating_sub(1).max(1)
    }

    /// Scroll just enough to show `cell`.
    pub(crate) fn reveal(&mut self, cell: Cell) {
        let (bw, bh) = self.body_size();
        let (sx, sy) = self.scroll.xy();
        let ny = layout::scroll_to_reveal(sy, bh, cell.row as f32 * ROW_H, (cell.row + 1) as f32 * ROW_H);
        let nx = layout::scroll_to_reveal(sx, bw, self.cols.left(cell.col), self.cols.left(cell.col + 1));
        self.scroll.set(nx, ny);
    }

    pub(crate) fn refresh_summary_pub(&mut self) {
        self.refresh_summary();
    }

    pub(crate) fn cache_clear(&mut self) {
        self.cache.clear();
    }

    fn refresh_summary(&mut self) {
        let Some(range) = self.current_range() else {
            self.summary = None;
            return;
        };
        let agg = (range.cell_count() <= AGGREGATE_LIMIT && range.cell_count() > 1).then(|| {
            let mut a = NumericAgg::default();
            'outer: for r in range.r0..=range.r1 {
                for c in range.c0..=range.c1 {
                    a.add(self.value_at(r, c));
                    if a.mixed {
                        break 'outer;
                    }
                }
            }
            a
        });
        self.summary = Some(selection_summary(range.rows(), range.cols(), agg));
    }

    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        let (nr, nc) = (self.n_rows(), self.n_cols());
        if nr > 0 && nc > 0 {
            self.set_selection(Some(Selection::all(nr, nc)), false, cx);
        }
    }

    /// The cell under the pointer (window coordinates) in the body, clamped to
    /// the grid so a drag past the edge keeps extending.
    pub(crate) fn body_cell_at(&self, pos: gpui_kit::Point<Pixels>) -> Option<Cell> {
        let (nr, nc) = (self.n_rows(), self.n_cols());
        if nr == 0 || nc == 0 {
            return None;
        }
        let b = self.body_bounds.get();
        let (sx, sy) = self.scroll.xy();
        let x = f32::from(pos.x - b.origin.x) + sx;
        let y = f32::from(pos.y - b.origin.y) + sy;
        let row = ((y / ROW_H).floor().max(0.0) as usize).min(nr - 1);
        let col = self.cols.column_at(x.max(0.0)).unwrap_or(if x < 0.0 { 0 } else { nc - 1 });
        Some(Cell::new(row, col))
    }

    // --------------------------------------------------------------------- copy

    fn range_rows(&self, range: CellRange) -> Vec<Vec<Value>> {
        (range.r0..=range.r1)
            .map(|r| (range.c0..=range.c1).map(|c| self.value_at(r, c).clone()).collect())
            .collect()
    }

    pub(crate) fn copy_selection(&mut self, with_headers: bool, cx: &mut Context<Self>) {
        let (Some(range), Some(rs)) = (self.current_range(), self.result.clone()) else { return };
        let headers: Vec<String> = rs.columns[range.c0..=range.c1].iter().map(|c| c.name.clone()).collect();
        let text = build_tsv(with_headers.then_some(headers.as_slice()), &self.range_rows(range));
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.emit(GridEvent::Copied { cells: range.cell_count() });
    }

    pub(crate) fn copy_column_name(&mut self, cx: &mut Context<Self>) {
        let (Some(range), Some(rs)) = (self.current_range(), self.result.clone()) else { return };
        let names: Vec<&str> = rs.columns[range.c0..=range.c1].iter().map(|c| c.name.as_str()).collect();
        cx.write_to_clipboard(ClipboardItem::new_string(names.join("\t")));
        cx.emit(GridEvent::Copied { cells: names.len() });
    }

    pub(crate) fn request_export(
        &mut self,
        format: ExportFormatChoice,
        selection_only: bool,
        to_clipboard: bool,
        cx: &mut Context<Self>,
    ) {
        cx.emit(GridEvent::Export(ExportRequest { format, selection_only, to_clipboard }));
    }

    // ---------------------------------------------------------------- inspector

    pub(crate) fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        self.inspector_open = !self.inspector_open;
        self.inspector_dirty = true;
        // The body is re-measured only after the next paint; re-reveal the
        // cursor cell once its width has changed.
        self.reveal_pending = Some(f32::from(self.body_bounds.get().size.width));
        cx.notify();
    }

    /// Recompute the inspector content when the cursor cell or its value changed.
    pub(crate) fn sync_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inspector_dirty || !self.inspector_open {
            return;
        }
        self.inspector_dirty = false;
        let content = self.sel.map(|s| inspect_value(self.value_at(s.cursor.row, s.cursor.col)));
        let text = match content.as_ref().map(|i| &i.body) {
            Some(crate::inspect::Body::Text(t)) => t.clone(),
            Some(crate::inspect::Body::Geometry { ewkt, .. }) => ewkt.clone(),
            _ => String::new(),
        };
        self.inspector_text.update(cx, |s, cx| s.set_value(text, window, cx));
        self.inspector = content;
    }

    pub(crate) fn show_on_map(&mut self, whole_column: bool, cx: &mut Context<Self>) {
        let Some(s) = self.sel else { return };
        let column = s.cursor.col;
        let row = match self.row_ref(s.cursor.row) {
            RowRef::Orig(i) if !whole_column => Some(i),
            _ => None,
        };
        cx.emit(GridEvent::ShowOnMap { column, row });
    }

    pub(crate) fn cursor_value(&self) -> Option<&Value> {
        self.sel.map(|s| self.value_at(s.cursor.row, s.cursor.col))
    }
}

/// Header content width: name (12px UI font, medium) + type (11px) + padding
/// and the sort affordance. An estimate: measuring every header with the text
/// system would cost more than the half-pixel it buys.
pub(crate) fn header_px(name: &str, type_name: &str) -> f32 {
    name.chars().count() as f32 * 7.0 + type_name.chars().count() as f32 * 6.0 + 16.0 + 8.0 + 24.0
}
