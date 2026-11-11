//! Painting the table editor: header, section strip, the editable grid, the column popup and the DDL preview.

use super::highlight::{self, Class};
use super::keys::Cmd;
use super::model::{Cell, CellKind, Field, Row, RowState, Section, Spec};
use super::{EditCancel, EditNext, EditPrev, Next, Run, TabNextCell, TabPrevCell, TableEditor};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, MoveDown, MoveUp};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, DragMoveEvent, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Window, deferred, div, px, uniform_list,
};
use std::ops::Range;
use theta_core::DatabaseKind;
use theta_sql::ddl::{Flag, Severity};
use theta_ui::{CHROME_TEXT_SIZE, ROW_HEIGHT, Settings, data_surface};

/// Below this window width the preview moves under the grid instead of beside it.
const NARROW: f32 = 1180.0;
const GUTTER: f32 = 34.0;

#[derive(Clone, Copy)]
struct PreviewDrag;

impl Render for PreviewDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_0()
    }
}

/// Size a cell: checkbox-width columns keep their width, the others share what is left in proportion.
fn sized(d: gpui_kit::Div, width: f32) -> gpui_kit::Div {
    if width == 0.0 {
        d.flex_basis(px(120.)).flex_grow(2.0).flex_shrink(1.0).min_w(px(110.))
    } else if width <= 70.0 {
        d.w(px(width)).flex_none()
    } else {
        d.flex_basis(px(width)).flex_grow(1.0).flex_shrink(1.0).min_w(px(width * 0.62))
    }
}

fn is_mono(field: Field) -> bool {
    matches!(
        field,
        Field::Type
            | Field::Default
            | Field::CheckExpr
            | Field::IdxPredicate
            | Field::FkRefTable
            | Field::OptValue
    )
}

impl Render for TableEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = data_surface(cx);
        let narrow = f32::from(window.viewport_size().width) < NARROW;
        let header = self.render_header(narrow, cx);
        let tabs = self.render_tabs(cx);
        let grid = self.render_grid(window, cx);
        let preview = self.preview_open.then(|| self.render_preview(narrow, window, cx));
        let theme = cx.theme().clone();
        let body = if narrow {
            v_flex().flex_1().min_h_0().child(grid).when_some(preview, |d, p| d.child(p))
        } else {
            h_flex().flex_1().min_h_0().items_stretch().child(grid).when_some(preview, |d, p| {
                d.child(
                    div().relative().flex_none().w(px(1.)).h_full().bg(theme.border).child(
                        div()
                            .id("te-resize")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(-3.))
                            .w(px(7.))
                            .cursor_col_resize()
                            .on_drag(PreviewDrag, |d, _, _, cx| cx.new(|_| *d))
                            .hover(move |s| s.bg(theme.drag_border.opacity(0.35))),
                    ),
                )
                .child(p)
            })
        };
        v_flex()
            .id("table-editor")
            .size_full()
            .bg(surface)
            .key_context("TableEditor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &TabNextCell, w, cx| this.command(Cmd::Next, w, cx)))
            .on_action(cx.listener(|this, _: &TabPrevCell, w, cx| this.command(Cmd::Prev, w, cx)))
            .on_action(cx.listener(|this, _: &EditNext, w, cx| this.commit_edit(Next::Forward, w, cx)))
            .on_action(cx.listener(|this, _: &EditPrev, w, cx| this.commit_edit(Next::Back, w, cx)))
            .on_action(cx.listener(|this, _: &EditCancel, w, cx| {
                this.cancel_edit(w, cx);
                // Escape in the name field gives the keyboard back to the grid too.
                w.focus(&this.focus, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.editing.as_ref().is_some_and(|e| !e.suggestions.is_empty()) {
                    this.suggest_step(-1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.editing.as_ref().is_some_and(|e| !e.suggestions.is_empty()) {
                    this.suggest_step(1, cx);
                    cx.stop_propagation();
                }
            }))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<PreviewDrag>, window, cx| {
                let total: f32 = window.viewport_size().width.into();
                let x: f32 = e.event.position.x.into();
                this.preview_width = (total - x).clamp(280.0, 760.0);
                cx.notify();
            }))
            .child(header)
            .child(tabs)
            .child(body)
    }
}

impl TableEditor {
    // ------------------------------------------------------------ header

    fn render_header(&mut self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let a = self.model.analysis();
        let dirty = self.model.dirty();
        let destructive = a.statements.iter().any(|s| s.is_destructive());
        let summary_color = if !dirty {
            theme.muted_foreground
        } else if destructive {
            theme.danger
        } else {
            theme.foreground
        };
        let summary = a.summary.clone();
        let problem = self.execute_problem();
        let schema = self.model.draft().schema.clone();
        let kind = self.model.kind();
        let chip = |text: String| {
            div()
                .px_1p5()
                .h(px(18.))
                .flex()
                .items_center()
                .rounded(px(3.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(text)
        };
        h_flex()
            .flex_none()
            .h(px(42.))
            .px_3()
            .gap_2()
            .items_center()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(IconName::Table2).size(px(14.)).text_color(theme.muted_foreground))
            .when_some(schema.filter(|s| !s.is_empty() && kind != DatabaseKind::Sqlite), |d, s| {
                d.child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(format!("{s}.")))
            })
            .child(
                div()
                    .key_context("TableEditorName")
                    .flex_none()
                    .w(px(if narrow { 170. } else { 210. }))
                    .child(Input::new(&self.name_input).small().disabled(self.read_only)),
            )
            // The title bar already names the connection; at narrow widths the room goes to the buttons.
            .when(!narrow, |d| d.child(chip(self.conn_name.clone())))
            .child(chip(kind.label().to_string()))
            .when(self.read_only, |d| d.child(chip("read-only".into())))
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .min_w_0()
                    .flex_shrink_1()
                    .truncate()
                    .text_size(CHROME_TEXT_SIZE)
                    .text_color(summary_color)
                    .child(summary),
            )
            .child(
                Button::new("te-revert")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::Undo2))
                    .when(!narrow, |b| b.label("Revert"))
                    .tooltip("Discard every change (alt-r)")
                    .disabled(!dirty || self.is_running())
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::Revert, w, cx))),
            )
            .child(
                Button::new("te-copy")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::Copy))
                    .when(!narrow, |b| b.label("Copy SQL"))
                    .tooltip("Copy the script (alt-c)")
                    .disabled(!dirty)
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::CopySql, w, cx))),
            )
            .child(
                Button::new("te-console")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::SquareTerminal))
                    .when(!narrow, |b| b.label("Open in Console"))
                    .tooltip("Open the script in a console (alt-o)")
                    .disabled(!dirty)
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::OpenInConsole, w, cx))),
            )
            .child(
                Button::new("te-execute")
                    .primary()
                    .small()
                    .icon(Icon::new(IconName::Play))
                    .label(if self.is_running() { "Running…" } else { "Execute" })
                    .tooltip(problem.unwrap_or_else(|| "Review and run the script (ctrl-enter)".into()))
                    .disabled(self.execute_problem().is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.request_execute(cx))),
            )
            .into_any_element()
    }

    // ---------------------------------------------------------- sections

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .flex_none()
            .h(px(30.))
            .px_2()
            .gap_1()
            .items_end()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(Section::ALL.into_iter().map(|s| {
                let active = s == self.section;
                let count = self.model.count(s);
                let worst = self.model.analysis().worst(s);
                h_flex()
                    .id(("te-section", s as usize))
                    .h_full()
                    .px_2p5()
                    .gap_1p5()
                    .items_center()
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if active { theme.ring } else { gpui_kit::transparent_black() })
                    .text_size(px(12.5))
                    .text_color(if active { theme.foreground } else { theme.muted_foreground })
                    .when(!active, |d| d.hover(|s| s.text_color(theme.foreground)))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.switch_section(s, w, cx);
                        w.focus(&this.focus, cx);
                    }))
                    .child(s.label())
                    .when(count > 0, |d| {
                        d.child(
                            div()
                                .px_1()
                                .min_w(px(16.))
                                .h(px(15.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(8.))
                                .bg(theme.muted)
                                .text_size(px(10.))
                                .text_color(theme.muted_foreground)
                                .child(count.to_string()),
                        )
                    })
                    .when_some(worst, |d, sev| {
                        d.child(div().size(px(7.)).rounded_full().bg(if sev == Severity::Error {
                            theme.danger
                        } else {
                            theme.warning
                        }))
                    })
            }))
            .into_any_element()
    }

    // -------------------------------------------------------------- grid

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let spec = self.model.spec(self.section);
        let count = spec.rows.len();
        let header = h_flex()
            .flex_none()
            .h(px(24.))
            .items_center()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_none().w(px(GUTTER)))
            .children(spec.cols.iter().map(|c| {
                sized(div(), c.width)
                    .px_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(c.title)
            }));
        let suggestions = (self.section == Section::ForeignKeys).then(|| self.render_suggestions(cx));
        let body = if count == 0 {
            self.render_empty(cx)
        } else {
            uniform_list(
                "te-rows",
                count,
                cx.processor(|this, range: Range<usize>, window, cx| {
                    let spec = this.model.spec(this.section);
                    range.map(|i| this.render_row(i, &spec, window, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let popup = self.popup.is_some().then(|| self.render_popup(&spec, window, cx));
        v_flex()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(body)
            .when_some(suggestions.flatten(), |d, s| d.child(s))
            .child(self.render_footer(cx))
            .when_some(popup, |d, p| d.child(p))
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let modal = Settings::get(cx).keymap.is_modal();
        let key = if modal { "o" } else { "Insert" };
        let what = match self.section {
            Section::Columns => "No columns yet",
            Section::Keys => "No keys",
            Section::ForeignKeys => "No foreign keys",
            Section::Indexes => "No indexes",
            Section::Checks => "No check constraints",
            Section::Options => "Nothing to set here",
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_1()
            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(what))
            .when(self.section != Section::Options, |d| {
                d.child(
                    div()
                        .text_size(CHROME_TEXT_SIZE)
                        .text_color(theme.muted_foreground)
                        .child(format!("Press {key} to add one.")),
                )
            })
            .into_any_element()
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let modal = Settings::get(cx).keymap.is_modal();
        let hint = if self.popup.is_some() {
            "Space choose · Alt+Up/Down reorder · Enter done"
        } else if self.editing.is_some() {
            "Enter apply · Tab next cell · Esc cancel"
        } else if modal {
            "i edit · o add · dd delete · yy copy · u undo · [ ] section · Alt+Up/Down move"
        } else {
            "Enter edit · Space toggle · Insert add · Delete remove · Ctrl+Z undo · Alt+Left/Right section"
        };
        let (error, text) = match &self.status {
            Some((e, t)) => (*e, t.clone()),
            None => (false, hint.to_string()),
        };
        h_flex()
            .flex_none()
            .h(px(24.))
            .px_3()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .text_size(px(11.))
            .text_color(if error { theme.danger } else { theme.muted_foreground })
            .child(div().truncate().child(text))
            .into_any_element()
    }

    fn render_suggestions(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.open_suggestions();
        if open.is_empty() || self.model.is_new() {
            return None;
        }
        let theme = cx.theme().clone();
        let more = open.len().saturating_sub(4);
        Some(
            v_flex()
                .flex_none()
                .border_t_1()
                .border_color(theme.border)
                .py_1()
                .child(
                    h_flex()
                        .px_3()
                        .h(px(22.))
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.))
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::Unlink).size(px(12.)))
                        .child("Suggested from column names (Alt+A to choose)"),
                )
                .children(open.into_iter().take(4).enumerate().map(|(i, v)| {
                    let label =
                        format!("{} \u{2192} {}.{}", v.column, v.referenced_table, v.referenced_column);
                    let detail = format!("{:.0}% · {}", v.confidence * 100.0, v.reason);
                    let vv = v.clone();
                    h_flex()
                        .px_3()
                        .h(px(24.))
                        .gap_2()
                        .items_center()
                        .child(div().w(px(260.)).truncate().text_size(px(12.)).child(label))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        )
                        .child(
                            Button::new(("te-add-suggestion", i))
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::Plus))
                                .label("Add")
                                .disabled(self.read_only)
                                .on_click(cx.listener(move |this, _, w, cx| this.add_suggested(&vv, w, cx))),
                        )
                }))
                .when(more > 0, |d| {
                    d.child(
                        div()
                            .px_3()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(format!("and {more} more")),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_row(
        &mut self,
        ri: usize,
        spec: &Spec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let (cur_r, cur_c) = self.clamped_cursor(spec);
        let row = &spec.rows[ri];
        let on_cursor_row = ri == cur_r;
        let reorderable = self.section == Section::Columns && self.model.is_new();
        let stripe = match row.state {
            RowState::Added => Some(theme.success),
            RowState::Modified => Some(theme.warning),
            RowState::Unchanged => None,
        };
        let gutter = div()
            .relative()
            .flex_none()
            .w(px(GUTTER))
            .h_full()
            .flex()
            .items_center()
            .justify_end()
            .pr_1p5()
            .text_size(px(10.5))
            .text_color(if on_cursor_row { theme.foreground } else { theme.muted_foreground })
            .when_some(stripe, |d, c| d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(c)))
            .child(if row.struck {
                Icon::new(IconName::Trash).size(px(12.)).text_color(theme.danger).into_any_element()
            } else if reorderable && on_cursor_row {
                Icon::new(IconName::GripVertical).size(px(12.)).into_any_element()
            } else {
                div().child((ri + 1).to_string()).into_any_element()
            });
        h_flex()
            .id(("te-row", ri))
            .h(ROW_HEIGHT)
            .w_full()
            .flex_none()
            .items_center()
            .when(on_cursor_row, |d| d.bg(theme.list_hover))
            .when(row.struck, |d| d.bg(theme.danger.opacity(0.07)))
            .child(gutter)
            .children(spec.cols.iter().enumerate().map(|(ci, col)| {
                self.render_cell(ri, ci, col.width, row, ci == cur_c && on_cursor_row, window, cx)
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_cell(
        &mut self,
        ri: usize,
        ci: usize,
        width: f32,
        row: &Row,
        active: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let cell: &Cell = &row.cells[ci];
        let mono = Settings::get(cx).buffer_font_family.clone();
        let text_color = if row.struck {
            theme.muted_foreground
        } else if cell.muted {
            theme.muted_foreground.opacity(0.7)
        } else {
            theme.foreground
        };
        let editing_here = self.editing.as_ref().is_some_and(|e| e.row == row.id && e.field == cell.field);
        let base = sized(div(), width)
            .id(("te-cell", ri * 64 + ci))
            .relative()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .when(!editing_here, |d| d.overflow_hidden())
            .text_size(px(12.5))
            .when(is_mono(cell.field), |d| d.font_family(mono.clone()).text_size(px(12.)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.click_cell(ri, ci, ev.click_count, window, cx)
                }),
            );
        if editing_here {
            return self.render_editing_cell(base, cx);
        }
        let content: AnyElement = match cell.kind {
            CellKind::Toggle => {
                let disabled = row.struck;
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if cell.on && !disabled {
                                theme.primary
                            } else {
                                theme.muted_foreground.opacity(0.6)
                            })
                            .when(cell.on && !disabled, |d| d.bg(theme.primary))
                            .when(cell.on, |d| {
                                d.child(Icon::new(IconName::Check).size(px(11.)).text_color(if disabled {
                                    theme.muted_foreground
                                } else {
                                    theme.primary_foreground
                                }))
                            }),
                    )
                    .when(!cell.text.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.muted_foreground)
                                .child(cell.text.clone()),
                        )
                    })
                    .into_any_element()
            }
            CellKind::Columns => {
                if cell.chips.is_empty() {
                    div()
                        .text_color(theme.muted_foreground.opacity(0.7))
                        .child("(choose columns)")
                        .into_any_element()
                } else {
                    h_flex()
                        .gap_1()
                        .overflow_hidden()
                        .children(cell.chips.iter().enumerate().map(|(n, c)| {
                            div()
                                .flex_none()
                                .px_1p5()
                                .h(px(17.))
                                .flex()
                                .items_center()
                                .rounded(px(3.))
                                .bg(theme.muted)
                                .text_size(px(11.5))
                                .text_color(if cell.kind == CellKind::Static {
                                    theme.muted_foreground
                                } else {
                                    theme.foreground
                                })
                                .child(if cell.chips.len() > 1 && cell.kind == CellKind::Columns {
                                    format!("{} {c}", n + 1)
                                } else {
                                    c.clone()
                                })
                        }))
                        .into_any_element()
                }
            }
            CellKind::Cycle => h_flex()
                .gap_1()
                .items_center()
                .child(div().truncate().text_color(text_color).child(cell.text.clone()))
                .when(active && !cell.text.is_empty(), |d| {
                    d.child(
                        Icon::new(IconName::ChevronsUpDown).size(px(11.)).text_color(theme.muted_foreground),
                    )
                })
                .into_any_element(),
            CellKind::RefTable => h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .truncate()
                        .text_color(if cell.text.is_empty() {
                            theme.muted_foreground.opacity(0.7)
                        } else {
                            text_color
                        })
                        .child(if cell.text.is_empty() {
                            "(choose table)".to_string()
                        } else {
                            cell.text.clone()
                        }),
                )
                .into_any_element(),
            _ => div()
                .truncate()
                .text_color(text_color)
                .when(row.struck, |d| d.line_through())
                .child(cell.text.clone())
                .into_any_element(),
        };
        base.child(content)
            .when(active, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .border_1()
                        .border_color(theme.ring),
                )
            })
            .into_any_element()
    }

    fn render_editing_cell(
        &mut self,
        base: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(e) = &self.editing else { return base.into_any_element() };
        let suggestions: Vec<(usize, &'static str)> = e.suggestions.iter().copied().enumerate().collect();
        let highlighted = e.highlighted;
        let input = e.input.clone();
        base.px_0()
            .child(
                div()
                    .key_context("TableEditorCell")
                    .size_full()
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(theme.ring)
                    .bg(theme.background)
                    .child(
                        Input::new(&input).small().appearance(false).bordered(false).focus_bordered(false),
                    ),
            )
            .when(!suggestions.is_empty(), |d| {
                d.child(deferred(
                    gpui_kit::anchored()
                        .anchor(gpui_kit::Anchor::TopLeft)
                        .offset(gpui_kit::point(px(0.), px(25.)))
                        .child(
                            v_flex()
                                .id("te-type-suggestions")
                                .occlude()
                                .w(px(190.))
                                .p_1()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.popover)
                                .shadow_md()
                                .children(suggestions.into_iter().map(|(i, s)| {
                                    div()
                                        .h(px(22.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(3.))
                                        .text_size(px(12.))
                                        .text_color(theme.foreground)
                                        .when(highlighted == Some(i), |d| d.bg(theme.list_active))
                                        .child(s)
                                })),
                        ),
                ))
            })
            .into_any_element()
    }

    fn click_cell(
        &mut self,
        ri: usize,
        ci: usize,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = self.model.spec(self.section);
        let Some(row) = spec.rows.get(ri) else { return };
        let kind = row.cells.get(ci).map(|c| c.kind);
        // A click elsewhere ends an edit in progress (the input's blur commits it).
        if self.editing.is_some() && self.cursors.get(&self.section) != Some(&(ri, ci)) {
            self.cancel_or_commit(window, cx);
        }
        self.cursors.insert(self.section, (ri, ci));
        window.focus(&self.focus, cx);
        let direct = matches!(kind, Some(CellKind::Toggle | CellKind::Cycle));
        if clicks >= 2 || direct {
            self.activate(window, cx);
        }
        cx.notify();
    }

    fn cancel_or_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(Next::Stay, window, cx);
        if self.editing.is_some() {
            self.cancel_edit(window, cx);
        }
    }

    // ------------------------------------------------------------- popup

    fn render_popup(&mut self, spec: &Spec, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(p) = &self.popup else { return div().into_any_element() };
        let (row, _) = self.clamped_cursor(spec);
        let scroll_y: f32 = self.scroll.0.borrow().base_handle.offset().y.into();
        let viewport_h: f32 = window.viewport_size().height.into();
        let item_h = 22.0;
        let height = (p.items.len() as f32 * item_h + 40.0).min(260.0);
        let y =
            (24.0 + row as f32 * 24.0 + scroll_y + 26.0).clamp(26.0, (viewport_h - 300.0 - height).max(40.0));
        let title = match p.field {
            Field::FkRefColumns => "Referenced columns",
            _ => "Columns",
        };
        div()
            .absolute()
            .left(px(GUTTER + 24.0))
            .top(px(y))
            .child(
                v_flex()
                    .id("te-popup")
                    .occlude()
                    .w(px(300.))
                    .max_h(px(260.))
                    .p_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_md()
                    .child(
                        div()
                            .px_2()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .text_size(px(10.5))
                            .text_color(theme.muted_foreground)
                            .child(format!("{title} · Space chooses, Alt+Up/Down orders, Enter closes")),
                    )
                    .child(v_flex().id("te-popup-items").overflow_y_scroll().children(
                        p.items.iter().enumerate().map(|(i, (key, label))| {
                            let order = p.chosen.iter().position(|c| c == key);
                            h_flex()
                                .id(("te-popup-item", i))
                                .h(px(item_h))
                                .px_2()
                                .gap_2()
                                .items_center()
                                .rounded(px(3.))
                                .cursor_pointer()
                                .when(i == p.cursor, |d| d.bg(theme.list_active))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(p) = &mut this.popup {
                                        p.cursor = i;
                                    }
                                    this.popup_toggle(cx);
                                }))
                                .child(
                                    div()
                                        .size(px(14.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(if order.is_some() {
                                            theme.primary
                                        } else {
                                            theme.muted_foreground.opacity(0.6)
                                        })
                                        .when(order.is_some(), |d| d.bg(theme.primary))
                                        .when_some(order, |d, n| {
                                            d.child(
                                                div()
                                                    .text_size(px(9.5))
                                                    .text_color(theme.primary_foreground)
                                                    .child((n + 1).to_string()),
                                            )
                                        }),
                                )
                                .child(div().truncate().text_size(px(12.)).child(label.clone()))
                        }),
                    )),
            )
            .into_any_element()
    }

    // ----------------------------------------------------------- preview

    fn render_preview(&mut self, narrow: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let surface = data_surface(cx);
        let a = self.model.analysis().clone();
        let mono = Settings::get(cx).buffer_font_family.clone();
        let colors = theta_ui::SyntaxColors::get(cx);
        let kind = self.model.kind();
        let failed = match &self.run {
            Run::Failed { statement, message, note } => Some((*statement, message.clone(), note.clone())),
            _ => None,
        };
        let running = self.is_running();
        let progress = self.progress;
        let dirty = self.model.dirty();
        let height = (f32::from(window.viewport_size().height) * 0.36).clamp(180.0, 340.0);

        let issues = a.issues.clone();
        let issue_rows = issues.into_iter().enumerate().map(|(i, issue)| {
            let error = issue.severity == Severity::Error;
            h_flex()
                .id(("te-issue", i))
                .px_3()
                .py_1()
                .gap_2()
                .items_start()
                .child(
                    Icon::new(if error { IconName::CircleAlert } else { IconName::TriangleAlert })
                        .size(px(13.))
                        .mt(px(2.))
                        .text_color(if error { theme.danger } else { theme.warning }),
                )
                .child(div().flex_1().min_w_0().text_size(px(12.)).child(issue.message.clone()))
                .when_some(issue.fix.clone(), |d, fix| {
                    d.child(
                        Button::new(("te-fix", i))
                            .small()
                            .label("Fix")
                            .on_click(cx.listener(move |this, _, _, cx| this.apply_fix(&fix, cx))),
                    )
                })
        });

        let blocks: Vec<AnyElement> = a
            .statements
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let fail = failed.as_ref().filter(|(n, _, _)| *n == Some(i));
                let not_run = failed.as_ref().is_some_and(|(n, _, _)| n.is_some_and(|n| i > n));
                let badges: Vec<AnyElement> = s
                    .flags
                    .iter()
                    .map(|f| {
                        let (text, color) = match f {
                            Flag::Destructive => ("destructive".to_string(), theme.danger),
                            Flag::TableRewrite => ("table rewrite".to_string(), theme.warning),
                            Flag::ImplicitCommit => ("implicit commit".to_string(), theme.warning),
                            Flag::Lock(t) => (format!("lock: {t}"), theme.warning),
                            Flag::OutsideTransaction => ("outside transaction".to_string(), theme.info),
                            Flag::Rebuild => ("rebuild".to_string(), theme.muted_foreground),
                        };
                        div()
                            .px_1p5()
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(color.opacity(0.6))
                            .bg(color.opacity(0.12))
                            .text_size(px(10.))
                            .text_color(color)
                            .child(text)
                            .into_any_element()
                    })
                    .collect();
                let sql = format!("{};", s.sql);
                let highlights: Vec<(Range<usize>, HighlightStyle)> = highlight::spans(&sql, kind)
                    .into_iter()
                    .map(|(r, c)| {
                        let color = match c {
                            Class::Keyword => colors.keyword,
                            Class::Type => colors.type_,
                            Class::String => colors.string,
                            Class::Number => colors.number,
                            Class::Comment => colors.comment,
                            Class::Identifier => colors.identifier,
                        };
                        (r, HighlightStyle { color: Some(color), ..Default::default() })
                    })
                    .collect();
                v_flex()
                    .id(("te-stmt", i))
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(fail.is_some(), |d| {
                        d.bg(theme.danger.opacity(0.10)).border_l_2().border_color(theme.danger)
                    })
                    .when(not_run, |d| d.opacity(0.55))
                    .when(running && progress.is_some_and(|(done, _)| done == i), |d| {
                        d.bg(theme.info.opacity(0.10)).border_l_2().border_color(theme.info)
                    })
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{}", i + 1)),
                            )
                            .children(badges)
                            .child(div().flex_1())
                            .child(
                                Button::new(("te-copy-stmt", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Copy))
                                    .tooltip("Copy this statement")
                                    .on_click(cx.listener(move |this, _, _, cx| this.copy_statement(i, cx))),
                            ),
                    )
                    .when(!s.explanation.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(colors.comment)
                                .child(format!("-- {}", s.explanation)),
                        )
                    })
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .text_color(colors.identifier)
                            .child(StyledText::new(SharedString::from(sql)).with_highlights(highlights)),
                    )
                    .when_some(fail, |d, (_, msg, _)| {
                        d.child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .text_color(theme.danger)
                                .child(format!("Failed: {msg}")),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        let empty_text = if dirty {
            "Nothing to run"
        } else if self.model.is_new() {
            "Name the table and add columns to see its CREATE TABLE script."
        } else {
            "No changes. Edit a column, key or index and the SQL appears here."
        };
        let panel = v_flex()
            .flex_none()
            .bg(surface)
            .when(narrow, |d| d.h(px(height)).w_full().border_t_1().border_color(theme.border))
            .when(!narrow, |d| d.w(px(self.preview_width)).h_full())
            .child(
                h_flex()
                    .flex_none()
                    .h(px(28.))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.tab_bar)
                    .child(Icon::new(IconName::FileCode).size(px(13.)).text_color(theme.muted_foreground))
                    .child(
                        div()
                            .text_size(CHROME_TEXT_SIZE)
                            .font_weight(FontWeight::MEDIUM)
                            .child("DDL Preview"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(if dirty { a.summary.clone() } else { String::new() }),
                    )
                    .child(
                        Button::new("te-hide-preview")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::X))
                            .tooltip("Hide the preview (alt-p)")
                            .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::TogglePreview, w, cx))),
                    ),
            )
            .when(!a.issues.is_empty(), |d| {
                d.child(
                    v_flex()
                        .flex_none()
                        .max_h(px(120.))
                        .id("te-issues")
                        .overflow_y_scroll()
                        .py_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .children(issue_rows),
                )
            })
            .when(running, |d| d.child(banner(&theme, false, "Running the script…".to_string())))
            .when_some(failed.as_ref().filter(|(_, _, n)| !n.is_empty()), |d, (_, _, note)| {
                d.child(banner(&theme, true, note.clone()))
            })
            .child(
                div()
                    .id("te-preview-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(blocks.is_empty(), |d| {
                        d.child(
                            div()
                                .p_4()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(empty_text),
                        )
                    })
                    .children(blocks),
            );
        panel.into_any_element()
    }

    pub(crate) fn popup_toggle(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.popup else { return };
        let key = p.items[p.cursor].0.clone();
        if let Some(at) = p.chosen.iter().position(|c| *c == key) {
            p.chosen.remove(at);
        } else {
            p.chosen.push(key);
        }
        self.push_list(cx);
    }
}

fn banner(theme: &gpui_kit::component::Theme, error: bool, text: String) -> AnyElement {
    let color = if error { theme.danger } else { theme.info };
    div()
        .flex_none()
        .px_3()
        .py_1p5()
        .border_b_1()
        .border_color(theme.border)
        .bg(color.opacity(0.10))
        .text_size(px(12.))
        .text_color(color)
        .child(text)
        .into_any_element()
}
