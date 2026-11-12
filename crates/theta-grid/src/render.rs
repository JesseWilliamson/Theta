//! Element construction for the grid. Only what is visible is built: rows and
//! columns are windowed by the scroll offset (see `layout`), selection and
//! hover are overlay rectangles rather than per-cell state, and cell text comes
//! from a cache, so frame cost depends on the viewport, not the result size.

use crate::actions::{EditCancel, EditNext, EditPrev, FilterClose, NextCell, PrevCell};
use crate::editing::After;
use crate::format::{CellKind, format_count, format_duration};
use crate::grid::{CachedCell, HEADER_H, ROW_H, ResultGrid};
use crate::layout::{self, CELL_PAD};
use crate::pending::{CellState, RowRef, RowState};
use crate::selection::{Extent, Motion};
use crate::types::ExportFormatChoice;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    div, px,
};
use std::time::Instant;
use theta_core::TypeKind;
use theta_ui::contrast::ensure_contrast;
use theta_ui::{CHROME_TEXT_SIZE, Settings, data_surface, tool_button};

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub bg: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub line: Hsla,
    pub head_bg: Hsla,
    /// Toolbar and footer strip: chrome, one step lighter than the rows in themes that define panel colours.
    pub bar_bg: Hsla,
    pub hover: Hsla,
    pub sel_fill: Hsla,
    pub accent: Hsla,
    pub modified: Hsla,
    pub added: Hsla,
    pub deleted: Hsla,
    pub green: Hsla,
    pub red: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub border: Hsla,
}

pub(crate) fn palette(cx: &App) -> Palette {
    let t = cx.theme();
    // Text colours that a theme leaves too faint are pulled toward the
    // foreground until they reach WCAG AA (4.5:1) on the grid background.
    let data = data_surface(cx);
    let readable = |c: Hsla| ensure_contrast(c, data, t.foreground, 4.5);
    Palette {
        bg: data,
        fg: t.foreground,
        muted: readable(t.muted_foreground),
        line: t.border.opacity(0.55),
        head_bg: t.table_head,
        bar_bg: t.tab_bar,
        hover: t.foreground.opacity(0.045),
        sel_fill: t.primary.opacity(0.17),
        accent: t.primary,
        modified: t.warning.opacity(0.20),
        added: t.success.opacity(0.17),
        deleted: t.danger.opacity(0.17),
        green: readable(t.green),
        red: readable(t.red),
        warning: readable(t.warning),
        danger: readable(t.danger),
        border: t.border,
    }
}

impl Render for ResultGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        self.measure_font(window, cx);
        self.sync_inspector(window, cx);
        let p = palette(cx);
        let root = v_flex()
            .id("theta-result-grid")
            .relative()
            .size_full()
            .min_w_0()
            .bg(p.bg)
            .text_color(p.fg)
            .text_size(CHROME_TEXT_SIZE)
            .key_context("ResultGrid")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_action(cx.listener(|this, _: &EditNext, window, cx| {
                this.commit_edit(After::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &EditPrev, window, cx| {
                this.commit_edit(After::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextCell, _, cx| this.move_selection(Motion::Next, false, cx)))
            .on_action(cx.listener(|this, _: &PrevCell, _, cx| this.move_selection(Motion::Prev, false, cx)))
            .on_action(cx.listener(|this, _: &EditCancel, window, cx| this.cancel_edit(window, cx)))
            .on_action(cx.listener(|this, _: &FilterClose, window, cx| this.close_filter(window, cx)));
        if self.result.is_none() {
            return root.into_any_element();
        }
        let toolbar = self.render_toolbar(cx, p);
        let filter = self.filter_open.then(|| self.render_filter(cx, p));
        let grid =
            if self.record.is_some() { self.render_record(cx, p) } else { self.render_grid(window, cx, p) };
        let aggregates = self.render_aggregates(cx, p);
        let inspector = self.inspector_open.then(|| self.render_inspector(cx, p));
        let footer = self.render_footer(cx, p);
        let out = root
            .child(toolbar)
            .children(filter)
            .child(h_flex().flex_1().min_h_0().w_full().child(grid).children(inspector))
            .child(footer)
            .children(aggregates)
            .into_any_element();
        if self.stats.enabled {
            let dt = started.elapsed();
            let wall_start = *self.stats.first.get_or_insert(started);
            self.stats.frames += 1;
            self.stats.total += dt;
            self.stats.worst = self.stats.worst.max(dt);
            if self.stats.frames == 20 {
                // "period" is wall time between renders: the whole frame
                // (layout + software paint), of which `render` is the part this
                // crate controls.
                eprintln!(
                    "grid render: {} frames, build avg {:.2} ms worst {:.2} ms, frame period avg {:.1} ms, {} cells built",
                    self.stats.frames,
                    self.stats.total.as_secs_f64() * 1000.0 / self.stats.frames as f64,
                    self.stats.worst.as_secs_f64() * 1000.0,
                    wall_start.elapsed().as_secs_f64() * 1000.0 / self.stats.frames as f64,
                    self.stats.cells
                );
                self.stats.frames = 0;
                self.stats.total = Default::default();
                self.stats.worst = Default::default();
                self.stats.first = None;
            }
        }
        out
    }
}

impl ResultGrid {
    // ------------------------------------------------------------------ toolbar

    fn render_toolbar(&self, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let editable = self.is_editable();
        let pending = self.pending.as_ref().map_or(0, |pd| pd.count());
        let has_sel = self.sel.is_some();
        let grid = cx.entity();
        let range_selected = self.current_range().is_some_and(|r| !r.is_single());

        let left: AnyElement = if pending > 0 {
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_color(p.warning)
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{pending} pending change{}", if pending == 1 { "" } else { "s" })),
                )
                .child(
                    Button::new("grid-revert")
                        .xsmall()
                        .ghost()
                        .label("Revert")
                        .tooltip("Discard all pending changes")
                        .on_click(cx.listener(|this, _, _, cx| this.revert_changes(cx))),
                )
                .child(
                    Button::new("grid-submit")
                        .xsmall()
                        .primary()
                        .label("Submit")
                        .tooltip("Submit changes (Ctrl+Enter)")
                        .on_click(cx.listener(|this, _, _, cx| this.submit_changes(cx))),
                )
                .into_any_element()
        } else if let Some(pd) = &self.pending {
            h_flex()
                .gap_1p5()
                .items_center()
                .text_color(p.muted)
                .child(Icon::new(IconName::Pencil).size(px(12.)))
                .child(pd.target().qualified_name())
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let export = tool_button("grid-export", IconName::Download, "Export results").dropdown_menu({
            let grid = grid.clone();
            move |menu, window, cx| export_menu(menu, &grid, range_selected, window, cx)
        });

        h_flex()
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
            .child(left)
            .child(
                h_flex()
                    .gap_0p5()
                    .items_center()
                    .child(
                        tool_button("grid-add", IconName::Plus, "Add row")
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| this.add_row(window, cx))),
                    )
                    .child(
                        tool_button("grid-del", IconName::Minus, "Delete selected rows (Delete)")
                            .disabled(!editable || !has_sel)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_selected_rows(cx))),
                    )
                    .child(div().w(px(1.)).h(px(14.)).mx_1().bg(p.border.opacity(0.7)))
                    .child(
                        tool_button("grid-filter", IconName::ListFilter, "Filter rows (Ctrl+F)")
                            .selected(self.filter_open)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.filter_open {
                                    this.close_filter(window, cx);
                                } else {
                                    this.open_filter(window, cx);
                                }
                            })),
                    )
                    .child(export)
                    .child(
                        tool_button("grid-record", IconName::Rows3, "Record view (F4; r in Helix and Vim)")
                            .selected(self.record.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_record_view(cx))),
                    )
                    .child(
                        tool_button("grid-inspector", IconName::PanelRight, "Cell inspector (Space)")
                            .selected(self.inspector_open)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_inspector(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_filter(&self, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let total = self.result.as_ref().map_or(0, |r| r.rows.len());
        let shown = self.n_view();
        let count = if self.view_busy {
            "filtering…".to_string()
        } else {
            format!("{} of {} rows", format_count(shown), format_count(total))
        };
        h_flex()
            .key_context("GridFilter")
            .h(px(30.))
            .flex_shrink_0()
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(p.border.opacity(0.6))
            .child(
                div().flex_1().child(
                    Input::new(&self.filter_input)
                        .prefix(Icon::new(IconName::Search).size(px(13.)).text_color(p.muted))
                        .cleanable(true)
                        .h(px(24.)),
                ),
            )
            .child(div().text_color(p.muted).min_w(px(96.)).child(count))
            .child(
                tool_button("grid-filter-close", IconName::X, "Close filter (Esc)")
                    .on_click(cx.listener(|this, _, window, cx| this.close_filter(window, cx))),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------- footer

    fn render_footer(&self, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let rs = self.result.as_ref().expect("footer only with a result");
        let n = rs.rows.len();
        let mut left = h_flex().gap_1p5().items_center().overflow_hidden();
        if let Some(err) = &self.submit_error {
            let full = err.clone();
            left = left
                .child(Icon::new(IconName::TriangleAlert).size(px(13.)).text_color(p.danger))
                .child(
                    div()
                        .id("grid-submit-error")
                        .text_color(p.danger)
                        .truncate()
                        .max_w(px(900.))
                        .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                        .child(format!("Could not apply changes: {err}")),
                )
                .child(tool_button("grid-error-dismiss", IconName::X, "Dismiss").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.submit_error = None;
                        cx.notify();
                    },
                )));
        } else {
            let mut label = format!("{} row{}", format_count(n), if n == 1 { "" } else { "s" });
            if self.filter_query.is_some() {
                label = format!("{} of {label}", format_count(self.n_view()));
            }
            left = left.child(div().child(label));
            if let Some(reason) = self.meta.stale.clone() {
                left = left
                    .child(Icon::new(IconName::TriangleAlert).size(px(13.)).text_color(p.warning))
                    .child(div().text_color(p.warning).font_weight(FontWeight::MEDIUM).child(reason));
                if self.meta.refreshable && !self.loading {
                    left = left.child(
                        div()
                            .id("grid-refresh")
                            .px_1()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .text_color(p.accent)
                            .font_weight(FontWeight::MEDIUM)
                            .hover(|d| d.bg(p.hover))
                            .tooltip(|window, cx| Tooltip::new("Run the query again (F5)").build(window, cx))
                            .on_click(cx.listener(|this, _, _, cx| this.request_refresh(cx)))
                            .child("Refresh"),
                    );
                }
            }
            if rs.truncated {
                let partial = crate::view::partial_notice(
                    !self.sorts.is_empty(),
                    self.filter_query.is_some(),
                    n,
                    true,
                );
                let why = format!(
                    "The result was cut off at the row limit, so more rows exist on the server. \
                     Sorting and filtering only see the {} rows loaded. Load all re-runs the statement \
                     for the rest, or add a WHERE or LIMIT clause, or raise the row limit in Settings.",
                    format_count(n)
                );
                let why = if self.meta.load_cap_reached {
                    format!(
                        "Load all stopped at its cap of {} rows and more exist on the server. \
                         Narrow the query with a WHERE or LIMIT clause.",
                        format_count(n)
                    )
                } else {
                    why
                };
                let note = match &partial {
                    Some(text) => format!("· {text}"),
                    None => format!("· first {} shown, limit reached", format_count(n)),
                };
                left = left.child(
                    div()
                        .id("grid-truncated")
                        .text_color(p.warning)
                        .when(partial.is_some(), |d| d.font_weight(FontWeight::MEDIUM))
                        .child(note)
                        .tooltip(move |window, cx| Tooltip::new(why.clone()).build(window, cx)),
                );
                if self.loading_all {
                    left = left.child(div().text_color(p.muted).child("Loading all rows…"));
                } else if !self.meta.load_cap_reached {
                    left = left.child(
                        div()
                            .id("grid-load-all")
                            .px_1()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .text_color(p.accent)
                            .font_weight(FontWeight::MEDIUM)
                            .hover(|d| d.bg(p.hover))
                            .tooltip(|window, cx| {
                                Tooltip::new("Load every row of this result (Ctrl+Shift+L)").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.request_load_all(cx)))
                            .child("Load all"),
                    );
                }
            }
            if let Some(e) = self.meta.elapsed {
                left = left.child(div().text_color(p.muted).child(format!("· {}", format_duration(e))));
            }
            if let Some(stmt) = self.meta.statement.clone() {
                left = left.child(
                    div()
                        .id("grid-statement")
                        .text_color(p.muted)
                        .child(Icon::new(IconName::Info).size(px(12.)))
                        .tooltip(move |window, cx| Tooltip::new(stmt.clone()).build(window, cx)),
                );
            }
        }
        let lock = (self.pending.is_none()).then(|| {
            let reason =
                self.meta.read_only_reason.clone().unwrap_or_else(|| "This result is read-only.".to_string());
            div()
                .id("grid-readonly")
                .child(Icon::new(IconName::Lock).size(px(12.)).text_color(p.muted))
                .tooltip(move |window, cx| Tooltip::new(format!("Read-only: {reason}")).build(window, cx))
        });
        h_flex()
            .h(px(24.))
            .flex_shrink_0()
            .px_2()
            .gap_3()
            .items_center()
            .justify_between()
            .bg(p.bar_bg)
            .border_t_1()
            .border_color(p.border.opacity(0.6))
            .text_size(CHROME_TEXT_SIZE)
            .child(left)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_shrink_0()
                    .text_color(p.muted)
                    .when(self.modal_visual, |d| d.child(div().text_color(p.accent).child("VISUAL")))
                    .children(self.summary.clone().map(|s| {
                        div()
                            .id("grid-summary")
                            .px_1()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .hover(|d| d.bg(p.hover))
                            .when(self.aggregates_open(), |d| d.bg(p.hover))
                            .tooltip(|window, cx| {
                                Tooltip::new("Selection statistics (F3; a in Helix and Vim)")
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_aggregates(cx)))
                            .child(s)
                    }))
                    .children(lock),
            )
            .into_any_element()
    }

    // --------------------------------------------------------------------- grid

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>, p: Palette) -> AnyElement {
        let settings = Settings::get(cx);
        let mono: SharedString = settings.buffer_font_family.clone().into();
        let fsize = settings.buffer_font_size - 1.0;
        let nr = self.n_rows();
        self.gutter_w = layout::gutter_width(nr.max(1), self.char_w);
        let gw = self.gutter_w;

        let measured = self.body_bounds.get();
        self.scroll.update_metrics(measured, self.cols.total(), nr as f32 * ROW_H);
        let (bw, bh) = self.body_size();
        // The body is measured during paint, so after a resize this frame laid
        // out against the old size; ask for one more frame to converge.
        let (mw, mh) = (f32::from(measured.size.width), f32::from(measured.size.height));
        if (mw - bw).abs() > 0.5 || (mh - bh).abs() > 0.5 {
            cx.notify();
        }
        if let Some(old_w) = self.reveal_pending
            && (mw - old_w).abs() > 1.0
        {
            self.reveal_pending = None;
            if let Some(s) = self.sel {
                self.reveal(s.cursor);
            }
        }
        let (sx, sy) = self.scroll.xy();
        let rows = layout::visible_rows(sy, bh, ROW_H, nr, 1);
        let cols = self.cols.visible(sx, bw);
        let range = self.current_range();

        // --- body rows ---------------------------------------------------------
        let mut body_rows: Vec<AnyElement> = Vec::with_capacity(rows.len());
        let mut gutter_cells: Vec<AnyElement> = Vec::with_capacity(rows.len());
        let mut built = 0usize;
        for r in rows.clone() {
            let rr = self.row_ref(r);
            let (row_state, new_idx) = match (&self.pending, rr) {
                (Some(pd), rr) => (pd.row_state(rr), if let RowRef::New(i) = rr { Some(i) } else { None }),
                (None, _) => (RowState::Normal, None),
            };
            let hovered = self.hover_row == Some(r);
            let in_range_row = range.is_some_and(|rg| (rg.r0..=rg.r1).contains(&r));
            let y = r as f32 * ROW_H - sy;
            let mut cells: Vec<AnyElement> = Vec::with_capacity(cols.len());
            for c in cols.clone() {
                let cc = self.cell_text(r, c);
                let cell_state = self.pending.as_ref().map_or(CellState::Normal, |pd| pd.cell_state(rr, c));
                let unfilled = new_idx
                    .is_some_and(|i| self.pending.as_ref().is_some_and(|pd| !pd.new_cell_filled(i, c)));
                let tint = match (row_state, cell_state) {
                    (RowState::Deleted, _) => Some(p.deleted),
                    (RowState::New, _) => Some(p.added),
                    (_, CellState::Modified) => Some(p.modified),
                    _ if hovered => Some(p.hover),
                    _ => None,
                };
                let right = matches!(cc.kind, CellKind::Number);
                cells.push(
                    data_cell(
                        self.cols.width(c),
                        &cc,
                        right,
                        unfilled,
                        tint,
                        row_state == RowState::Deleted,
                        cell_state == CellState::Modified,
                        &p,
                    )
                    .into_any_element(),
                );
                built += 1;
            }
            body_rows.push(
                h_flex()
                    .font_family(mono.clone())
                    .text_size(px(fsize))
                    .absolute()
                    .top(px(y))
                    .left(px(self.cols.left(cols.start) - sx))
                    .h(px(ROW_H))
                    .children(cells)
                    .into_any_element(),
            );
            gutter_cells.push(
                gutter_cell(r, new_idx, row_state, in_range_row, hovered, y, gw, &p).into_any_element(),
            );
        }
        self.stats.cells = built;

        // --- selection overlays ------------------------------------------------
        let mut overlays: Vec<AnyElement> = Vec::new();
        if let (Some(rg), Some(sel)) = (range, self.sel) {
            let x0 = self.cols.left(rg.c0) - sx;
            let x1 = self.cols.left(rg.c1 + 1) - sx;
            let y0 = rg.r0 as f32 * ROW_H - sy;
            let y1 = (rg.r1 + 1) as f32 * ROW_H - sy;
            if !rg.is_single() {
                overlays.push(
                    div()
                        .absolute()
                        .left(px(x0))
                        .top(px(y0))
                        .w(px(x1 - x0))
                        .h(px(y1 - y0))
                        .bg(p.sel_fill)
                        .border_1()
                        .border_color(p.accent.opacity(0.45))
                        .into_any_element(),
                );
            }
            if sel.extent == Extent::Cells || rg.is_single() {
                let cx0 = self.cols.left(sel.cursor.col) - sx;
                let cw = self.cols.width(sel.cursor.col);
                let cy = sel.cursor.row as f32 * ROW_H - sy;
                overlays.push(
                    div()
                        .absolute()
                        .left(px(cx0))
                        .top(px(cy))
                        .w(px(cw))
                        .h(px(ROW_H))
                        .border_1()
                        .border_color(p.accent)
                        .into_any_element(),
                );
            }
        }

        // --- inline editor -------------------------------------------------------
        let mut editor_overlay: Vec<AnyElement> = Vec::new();
        if let Some(ed) = &self.editor
            && let Some(sel) = self.sel
        {
            let col = ed.col;
            let x = self.cols.left(col) - sx;
            let w = self.cols.width(col).max(180.0);
            let y = sel.cursor.row as f32 * ROW_H - sy;
            editor_overlay.push(
                div()
                    .key_context("GridCellEditor")
                    .font_family(mono.clone())
                    .text_size(px(fsize))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .bg(p.bg)
                    .border_1()
                    .border_color(if ed.error.is_some() { p.danger } else { p.accent })
                    .child(
                        Input::new(&ed.input)
                            .with_size(px(fsize / 0.875))
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false),
                    )
                    .into_any_element(),
            );
            if let Some(err) = &ed.error {
                // Below the cell, or above it when the cell is on the last visible row.
                let top = if y + ROW_H + 26.0 > bh { y - 24.0 } else { y + ROW_H + 2.0 };
                editor_overlay.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(top))
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .bg(p.danger)
                        .text_color(cx.theme().danger_foreground)
                        .text_size(px(11.))
                        .child(err.clone())
                        .into_any_element(),
                );
            }
        }

        // --- header ---------------------------------------------------------------
        let header_cells: Vec<AnyElement> = cols.clone().map(|c| self.header_cell(c, sx, &p, cx)).collect();

        let bounds_cell = self.body_bounds.clone();
        let loading = self.loading || self.view_busy;
        let empty = nr == 0;
        // A filter that hides every loaded row of a cut-off result says so: matches may exist beyond.
        let empty_text = if self.filter_query.is_some() && self.n_new() == 0 && self.is_partial() {
            "No match in the rows loaded"
        } else if self.filter_query.is_some() {
            "No rows match"
        } else {
            "No rows"
        };

        let body_clip = div()
            .id("grid-body")
            .absolute()
            .left(px(gw))
            .top(px(HEADER_H))
            .right_0()
            .bottom_0()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_body_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_body_mouse_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.hover_row.take().is_some() {
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(body_rows)
            .children(overlays)
            .children(editor_overlay)
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted)
                        .font_family(cx.theme().font_family.clone())
                        .child(empty_text),
                )
            })
            .child(Scrollbar::new(&self.scroll))
            .context_menu({
                let grid = cx.entity();
                move |menu, window, cx| body_menu(menu, &grid, window, cx)
            });

        let gutter = div()
            .id("grid-gutter")
            .absolute()
            .left_0()
            .top(px(HEADER_H))
            .bottom_0()
            .w(px(gw))
            .overflow_hidden()
            .font_family(mono.clone())
            .text_size(px(fsize))
            .bg(p.head_bg)
            .border_r_1()
            .border_color(p.border.opacity(0.8))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_gutter_mouse_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .children(gutter_cells);

        let corner = div()
            .id("grid-corner")
            .absolute()
            .left_0()
            .top_0()
            .w(px(gw))
            .h(px(HEADER_H))
            .bg(p.head_bg)
            .border_r_1()
            .border_b_1()
            .border_color(p.border.opacity(0.8))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.select_all(cx);
                }),
            );

        let header = div()
            .id("grid-header")
            .absolute()
            .left(px(gw))
            .top_0()
            .right_0()
            .h(px(HEADER_H))
            .overflow_hidden()
            .bg(p.head_bg)
            .border_b_1()
            .border_color(p.border.opacity(0.8))
            .on_scroll(cx.listener(Self::on_scroll))
            .children(header_cells);

        let _ = window;
        div()
            .id("grid-viewport")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .size_full()
                    .when(loading, |d| d.opacity(0.55))
                    .child(body_clip)
                    .child(gutter)
                    .child(header)
                    .child(corner),
            )
            .when(loading, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .child(Progress::new("grid-loading").loading(true)),
                )
            })
            .into_any_element()
    }

    fn header_cell(&self, c: usize, sx: f32, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rs = self.result.as_ref().expect("header only with a result");
        let col = &rs.columns[c];
        let w = self.cols.width(c);
        let sort = self.sorts.iter().position(|k| k.column == c);
        let descending = sort.is_some_and(|i| self.sorts[i].descending);
        let selected = self.current_range().is_some_and(|r| {
            (r.c0..=r.c1).contains(&c) && self.sel.is_some_and(|s| s.extent == Extent::Cols)
        });
        let grid = cx.entity();
        let sort_icon = match sort {
            Some(_) if descending => IconName::ArrowDown,
            Some(_) => IconName::ArrowUp,
            None => IconName::ChevronsUpDown,
        };
        let multi = self.sorts.len() > 1;

        div()
            .id(("grid-h", c))
            .absolute()
            .left(px(self.cols.left(c) - sx))
            .top_0()
            .w(px(w))
            .h(px(HEADER_H))
            .flex()
            .items_center()
            .pl(px(CELL_PAD))
            .border_r_1()
            .border_color(p.line)
            .when(selected, |d| d.bg(p.sel_fill))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev, window, cx| this.on_header_mouse_down(c, ev, window, cx)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .items_center()
                    .overflow_hidden()
                    .when(self.is_key_column(c), |d| {
                        d.child(Icon::new(IconName::Key).size(px(11.)).text_color(p.warning))
                    })
                    .when(self.is_reference_column(c), |d| {
                        d.child(Icon::new(IconName::Link).size(px(11.)).text_color(p.accent))
                    })
                    .child(
                        div()
                            .truncate()
                            .font_family(cx.theme().font_family.clone())
                            .text_size(CHROME_TEXT_SIZE)
                            .font_weight(FontWeight::MEDIUM)
                            .child(col.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(cx.theme().font_family.clone())
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .child(col.type_name.clone()),
                    ),
            )
            .child(
                div()
                    .id(("grid-sort", c))
                    .w(px(26.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_0p5()
                    .cursor_pointer()
                    .child(Icon::new(sort_icon).size(px(12.)).text_color(if sort.is_some() {
                        p.accent
                    } else {
                        p.muted.opacity(0.45)
                    }))
                    .when(multi && sort.is_some(), |d| {
                        d.child(
                            div()
                                .text_size(px(10.))
                                .text_color(p.accent)
                                .child((sort.unwrap_or(0) + 1).to_string()),
                        )
                    })
                    .tooltip(|window, cx| Tooltip::new("Sort (Shift: add secondary key)").build(window, cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &gpui_kit::MouseDownEvent, _, cx| {
                            this.toggle_sort(c, ev.modifiers.shift, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id(("grid-resize", c))
                    .absolute()
                    .right(px(-2.))
                    .top_0()
                    .w(px(6.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev, _, cx| {
                            this.on_resize_mouse_down(c, ev, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .context_menu(move |menu, _, _| header_menu(menu, &grid, c))
            .into_any_element()
    }
}

/// A frozen row-number cell.
#[allow(clippy::too_many_arguments)]
fn gutter_cell(
    r: usize,
    new_idx: Option<usize>,
    state: RowState,
    in_range: bool,
    hovered: bool,
    y: f32,
    gw: f32,
    p: &Palette,
) -> Div {
    let label = if new_idx.is_some() { "+".to_string() } else { (r + 1).to_string() };
    div()
        .absolute()
        .left_0()
        .top(px(y))
        .w(px(gw))
        .h(px(ROW_H))
        .flex()
        .items_center()
        .justify_end()
        .pr(px(8.))
        .text_color(if in_range { p.fg } else { p.muted })
        .border_b_1()
        .border_color(p.line)
        .when(hovered && !in_range, |d| d.bg(p.hover))
        .when(in_range, |d| d.bg(p.sel_fill))
        .when(state == RowState::Deleted, |d| d.bg(p.deleted).line_through())
        .when(state == RowState::New, |d| d.text_color(p.green))
        .child(label)
}

#[allow(clippy::too_many_arguments)]
fn data_cell(
    w: f32,
    cc: &CachedCell,
    right: bool,
    unfilled: bool,
    tint: Option<Hsla>,
    strike: bool,
    modified: bool,
    p: &Palette,
) -> Div {
    let text: AnyElement = if unfilled {
        div().italic().text_color(p.muted).child("DEFAULT").into_any_element()
    } else {
        match cc.kind {
            CellKind::Null => div().italic().text_color(p.muted).child("NULL").into_any_element(),
            CellKind::EmptyText => div().text_color(p.muted).child("''").into_any_element(),
            CellKind::Bool(b) => div()
                .truncate()
                .text_color(if b { p.green } else { p.red })
                .child(cc.text.clone())
                .into_any_element(),
            CellKind::Bytes => div().truncate().text_color(p.muted).child(cc.text.clone()).into_any_element(),
            CellKind::Geometry => h_flex()
                .gap(px(5.))
                .items_center()
                .min_w_0()
                .child(Icon::new(IconName::MapPin).size(px(11.)).text_color(p.accent).flex_shrink_0())
                .child(div().truncate().child(cc.text.clone()))
                .into_any_element(),
            _ => div().truncate().child(cc.text.clone()).into_any_element(),
        }
    };
    div()
        .relative()
        .w(px(w))
        .h(px(ROW_H))
        .flex_shrink_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .px(px(CELL_PAD))
        .border_r_1()
        .border_b_1()
        .border_color(p.line)
        .when(right, |d| d.justify_end())
        .when_some(tint, |d, t| d.bg(t))
        .when(strike, |d| d.line_through().text_color(p.muted))
        .when(modified, |d| d.child(div().absolute().top_0().right_0().w(px(6.)).h(px(6.)).bg(p.warning)))
        .child(text)
}

// ----------------------------------------------------------------------- menus

fn export_menu(
    menu: PopupMenu,
    grid: &gpui_kit::Entity<ResultGrid>,
    range_selected: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mut menu = menu;
    for format in [
        ExportFormatChoice::Csv,
        ExportFormatChoice::Tsv,
        ExportFormatChoice::Json,
        ExportFormatChoice::Markdown,
        ExportFormatChoice::SqlInserts,
    ] {
        let grid = grid.clone();
        menu = menu.submenu(format.label(), window, cx, move |m, _, _| {
            let item = |label: &'static str, selection_only: bool, to_clipboard: bool| {
                let grid = grid.clone();
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    grid.update(cx, |g, cx| g.request_export(format, selection_only, to_clipboard, cx));
                })
            };
            let mut m = m.item(item("To file…", false, false)).item(item("To clipboard", false, true));
            if range_selected {
                m = m.separator().item(item("Selection to file…", true, false)).item(item(
                    "Selection to clipboard",
                    true,
                    true,
                ));
            }
            m
        });
    }
    menu
}

fn body_menu(
    menu: PopupMenu,
    grid: &gpui_kit::Entity<ResultGrid>,
    _: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (editable_cell, editable, is_geom, is_ref, has_sel) = {
        let g = grid.read(cx);
        (
            g.can_edit_cursor(),
            g.is_editable(),
            g.sel.is_some_and(|s| g.column_kind(s.cursor.col) == TypeKind::Geometry),
            g.sel.is_some_and(|s| {
                g.is_reference_column(s.cursor.col) && !g.value_at(s.cursor.row, s.cursor.col).is_null()
            }),
            g.sel.is_some(),
        )
    };
    let item = |label: &'static str, enabled: bool, f: fn(&mut ResultGrid, &mut Context<ResultGrid>)| {
        let grid = grid.clone();
        PopupMenuItem::new(label).disabled(!enabled).on_click(move |_, _, cx| {
            grid.update(cx, f);
        })
    };
    let mut menu = menu
        .item(item("Copy", has_sel, |g, cx| g.copy_selection(false, cx)))
        .item(item("Copy with Headers", has_sel, |g, cx| g.copy_selection(true, cx)))
        .item(item("Copy as JSON", has_sel, |g, cx| {
            g.request_export(ExportFormatChoice::Json, true, true, cx)
        }))
        .item(item("Copy as INSERT", has_sel, |g, cx| {
            g.request_export(ExportFormatChoice::SqlInserts, true, true, cx)
        }))
        .item(item("Copy Column Name", has_sel, |g, cx| g.copy_column_name(cx)))
        .separator();
    if is_ref {
        menu = menu.item(item("Follow Reference", true, |g, cx| {
            if let Some(s) = g.sel {
                let value = g.value_at(s.cursor.row, s.cursor.col).clone();
                cx.emit(crate::GridEvent::FollowReference { column: s.cursor.col, value });
            }
        }));
    }
    if is_geom {
        menu = menu.item(item("Show on Map", true, |g, cx| g.show_on_map(false, cx))).item(item(
            "Show Column on Map",
            true,
            |g, cx| g.show_on_map(true, cx),
        ));
    }
    menu = menu.item(item("Inspect Value", has_sel, |g, cx| {
        if !g.inspector_open {
            g.toggle_inspector(cx);
        }
        g.inspector_dirty = true;
        cx.notify();
    }));
    if editable {
        menu = menu
            .separator()
            .item(PopupMenuItem::new("Edit Cell").disabled(!editable_cell).on_click({
                let grid = grid.clone();
                move |_, window, cx| {
                    grid.update(cx, |g, cx| g.start_edit(None, window, cx));
                }
            }))
            .item(item("Set NULL", editable_cell, |g, cx| g.set_null(cx)))
            .item(PopupMenuItem::new("Add Row").on_click({
                let grid = grid.clone();
                move |_, window, cx| {
                    grid.update(cx, |g, cx| g.add_row(window, cx));
                }
            }))
            .item(item("Delete Row(s)", has_sel, |g, cx| g.delete_selected_rows(cx)));
    }
    menu
}

fn header_menu(menu: PopupMenu, grid: &gpui_kit::Entity<ResultGrid>, col: usize) -> PopupMenu {
    let item = |label: &'static str, f: fn(&mut ResultGrid, usize, &mut Context<ResultGrid>)| {
        let grid = grid.clone();
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            grid.update(cx, |g, cx| f(g, col, cx));
        })
    };
    menu.item(item("Sort Ascending", |g, c, cx| g.set_sort(c, Some(false), cx)))
        .item(item("Sort Descending", |g, c, cx| g.set_sort(c, Some(true), cx)))
        .item(item("Clear Sort", |g, c, cx| g.set_sort(c, None, cx)))
        .separator()
        .item(item("Auto-fit Width", |g, c, cx| g.autofit_column(c, cx)))
        .item(item("Copy Column Name", |g, c, cx| {
            if let Some(rs) = &g.result {
                let name = rs.columns[c].name.clone();
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(name));
            }
        }))
}
