//! The Data Compare result tab: it runs the comparison (own sessions, in the background, with progress and
//! cancel), shows the counts and a grid of the differing rows, and opens the sync script in a console on the
//! target. It never changes data itself.
//!
//! The merge and the script are `theta_sql::data_diff`, the reading is `theta_import::compare` (tested live
//! against PostgreSQL, MariaDB and SQLite); this file is the shell around them.

use crate::connections::{ConnectionStore, NoticeKind};
use crate::runtime;
use crate::schema_compare::{CompareEvent, Endpoint};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollStrategy,
    StatefulInteractiveElement as _, Styled as _, Task, UniformListScrollHandle, Window, div, px,
    uniform_list,
};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::{ConnectionConfig, DatabaseKind, Family, Table, Value};
use theta_import::{CompareJob, CompareOutcome, CompareProgress, CompareReport, KeyOrder, compare_tables};
use theta_sql::data_diff::{DiffKind, DiffRow, Mapping, sync_script};

/// Differing rows kept for display; the counts are exact beyond it.
pub const CAP: usize = 10_000;
const PAGE_ROWS: usize = 2_000;
const COL_W: f32 = 150.0;

/// Both ends of a comparison, from the picker.
#[derive(Debug, Clone)]
pub struct Ends {
    pub source: Endpoint,
    pub source_table: Table,
    pub target: Endpoint,
    pub target_table: Table,
    /// The key the user chose; empty is the source's primary key.
    pub key: Vec<String>,
}

/// Why a connection cannot be read on a session of its own, which a comparison needs so it does not
/// interleave with a console.
pub fn side_session_blocker(config: &ConnectionConfig) -> Option<String> {
    match config.kind {
        DatabaseKind::DuckDb => {
            Some("A DuckDB database cannot be opened a second time, which a comparison needs".into())
        }
        DatabaseKind::Sqlite if config.path.trim().is_empty() || config.path.contains(":memory:") => Some(
            "An in-memory SQLite database cannot be opened a second time, which a comparison needs".into(),
        ),
        // The pages are read with `LIMIT` and a row-value keyset condition; only these dialects were written
        // and tested for it (CockroachDB, TimescaleDB, YugabyteDB, Redshift and TiDB share one of them).
        k if !matches!(k.family(), Family::Postgres | Family::MySql | Family::Sqlite) => Some(format!(
            "{} is not supported by Compare Table Data: it reads with LIMIT and keyset paging, written for PostgreSQL, MySQL and MariaDB and SQLite",
            k.label()
        )),
        _ => None,
    }
}

enum Phase {
    Running,
    Done(Box<CompareReport>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    OnlySource,
    OnlyTarget,
    Different,
}

impl Filter {
    fn keep(self, k: DiffKind) -> bool {
        match self {
            Filter::All => true,
            Filter::OnlySource => k == DiffKind::OnlyLeft,
            Filter::OnlyTarget => k == DiffKind::OnlyRight,
            Filter::Different => k == DiffKind::Different,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Filter::All => "all differences",
            Filter::OnlySource => "only in source",
            Filter::OnlyTarget => "only in target",
            Filter::Different => "different",
        }
    }
}

pub struct DataCompareView {
    store: Entity<ConnectionStore>,
    ends: Ends,
    mapping: Mapping,
    phase: Phase,
    progress: Arc<Mutex<CompareProgress>>,
    cancel: Arc<AtomicBool>,
    shown: Vec<usize>,
    filter: Filter,
    sel: usize,
    col_offset: usize,
    include_deletes: bool,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _task: Option<Task<()>>,
    _ticker: Option<Task<()>>,
}

impl EventEmitter<CompareEvent> for DataCompareView {}

impl Focusable for DataCompareView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn fmt_n(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn rows_word(n: u64) -> String {
    format!("{} row{}", fmt_n(n), if n == 1 { "" } else { "s" })
}

pub fn title_of(ends: &Ends) -> String {
    // Two tables of one name are told apart by their schemas.
    if ends.source_table.name.eq_ignore_ascii_case(&ends.target_table.name) {
        format!(
            "Data: {}.{} → {}.{}",
            ends.source.schema.name, ends.source_table.name, ends.target.schema.name, ends.target_table.name
        )
    } else {
        format!("Data: {} → {}", ends.source_table.name, ends.target_table.name)
    }
}

impl DataCompareView {
    /// `mapping` comes from `map_columns`, which the caller runs first because its error (no usable key, a key
    /// of two kinds) is a sentence for the user, not a tab.
    pub fn new(store: Entity<ConnectionStore>, ends: Ends, mapping: Mapping, cx: &mut Context<Self>) -> Self {
        let mut v = DataCompareView {
            store,
            ends,
            mapping,
            phase: Phase::Running,
            progress: Arc::new(Mutex::new(CompareProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            shown: Vec::new(),
            filter: Filter::All,
            sel: 0,
            col_offset: 0,
            include_deletes: false,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _task: None,
            _ticker: None,
        };
        v.start(cx);
        v
    }

    pub fn title(&self) -> String {
        title_of(&self.ends)
    }

    pub fn target_conn(&self) -> &str {
        &self.ends.target.conn_id
    }

    pub fn focus_handle_ref(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn report(&self) -> Option<&CompareReport> {
        match &self.phase {
            Phase::Done(r) => Some(r),
            _ => None,
        }
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running)
    }

    pub fn sync_text(&self) -> Option<String> {
        let r = self.report()?;
        let s = sync_script(
            &self.mapping,
            &r.rows,
            self.ends.target.kind,
            &self.ends.target.schema.name,
            &self.ends.target_table.name,
            self.include_deletes,
        );
        Some(s.render(
            &format!("{}.{}", self.ends.source.schema.name, self.ends.source_table.name),
            &format!("{}.{}", self.ends.target.schema.name, self.ends.target_table.name),
            r.capped,
        ))
    }

    /// Read both tables again (also the first time).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Running;
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.progress.lock().expect("progress lock") = CompareProgress::default();
        let (l, r) = {
            let s = self.store.read(cx);
            (
                s.open_side_connection(&self.ends.source.conn_id),
                s.open_side_connection(&self.ends.target.conn_id),
            )
        };
        let e = &self.ends;
        let job = CompareJob {
            mapping: self.mapping.clone(),
            left_kind: e.source.kind,
            left_schema: Some(e.source.schema.name.clone()),
            left_table: e.source_table.name.clone(),
            right_kind: e.target.kind,
            right_schema: Some(e.target.schema.name.clone()),
            right_table: e.target_table.name.clone(),
            page_rows: PAGE_ROWS,
            cap: CAP,
            allow_fallback: true,
        };
        let (cancel, progress) = (self.cancel.clone(), self.progress.clone());
        let work = runtime::spawn(async move {
            let (Some(l), Some(r)) = (l, r) else { return Err("A connection no longer exists".to_string()) };
            let left = l.await.map_err(|e| format!("Could not open a session on the source: {e}"))?;
            let right = r.await.map_err(|e| format!("Could not open a session on the target: {e}"))?;
            let report = compare_tables(left.as_ref(), right.as_ref(), &job, &cancel, &move |p| {
                if let Ok(mut g) = progress.lock() {
                    *g = *p;
                }
            })
            .await;
            let _ = left.close().await;
            let _ = right.close().await;
            Ok(report)
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| this.finished(result.map_err(|e| e.to_string()).and_then(|r| r), cx))
                .ok();
        }));
        // Redraw while it runs: the progress lives in a mutex the worker writes.
        self._ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.is_running()
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn finished(&mut self, result: Result<CompareReport, String>, cx: &mut Context<Self>) {
        self.phase = match result {
            Ok(r) => match r.outcome.clone() {
                CompareOutcome::Failed(why) => Phase::Failed(why),
                _ => Phase::Done(Box::new(r)),
            },
            Err(why) => Phase::Failed(why),
        };
        self.sel = 0;
        self.refilter();
        cx.notify();
    }

    fn refilter(&mut self) {
        self.shown = match &self.phase {
            Phase::Done(r) => (0..r.rows.len()).filter(|&i| self.filter.keep(r.rows[i].kind)).collect(),
            _ => Vec::new(),
        };
        self.sel = self.sel.min(self.shown.len().saturating_sub(1));
    }

    fn set_filter(&mut self, f: Filter, cx: &mut Context<Self>) {
        self.filter = f;
        self.refilter();
        cx.notify();
    }

    fn move_by(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        self.sel = (self.sel as isize + by).clamp(0, self.shown.len() as isize - 1) as usize;
        self.scroll.scroll_to_item(self.sel, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn open_script(&mut self, cx: &mut Context<Self>) {
        match (self.report(), self.sync_text()) {
            (Some(r), Some(text)) if r.counts.differences() > 0 => {
                cx.emit(CompareEvent::OpenScript { conn_id: self.ends.target.conn_id.clone(), sql: text })
            }
            (Some(_), _) => cx.emit(CompareEvent::Notice(
                NoticeKind::Info,
                "The rows are equal: there is nothing to sync".into(),
            )),
            _ => cx.emit(CompareEvent::Notice(NoticeKind::Info, "Wait for the comparison to finish".into())),
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        if m.control || m.platform || m.alt {
            return;
        }
        match ev.keystroke.key.as_str() {
            "escape" if self.is_running() => {
                self.cancel.store(true, Ordering::SeqCst);
                cx.notify();
            }
            "down" | "j" => self.move_by(1, cx),
            "up" | "k" => self.move_by(-1, cx),
            "pagedown" => self.move_by(10, cx),
            "pageup" => self.move_by(-10, cx),
            "home" | "g" if !m.shift => {
                self.sel = 0;
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            "end" | "g" => {
                self.sel = self.shown.len().saturating_sub(1);
                self.scroll.scroll_to_item(self.sel, ScrollStrategy::Bottom);
                cx.notify();
            }
            "right" | "l" => {
                self.col_offset = (self.col_offset + 1).min(self.mapping.columns.len().saturating_sub(1));
                cx.notify();
            }
            "left" | "h" => {
                self.col_offset = self.col_offset.saturating_sub(1);
                cx.notify();
            }
            "1" | "a" => self.set_filter(Filter::All, cx),
            "2" => self.set_filter(Filter::OnlySource, cx),
            "3" => self.set_filter(Filter::Different, cx),
            "4" => self.set_filter(Filter::OnlyTarget, cx),
            "d" => {
                self.include_deletes = !self.include_deletes;
                cx.notify();
            }
            "enter" | "o" => self.open_script(cx),
            "c" => match self.sync_text() {
                Some(t) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(t));
                    cx.emit(CompareEvent::Notice(NoticeKind::Success, "Copied the sync script".into()));
                }
                None => cx
                    .emit(CompareEvent::Notice(NoticeKind::Info, "Wait for the comparison to finish".into())),
            },
            "r" if !self.is_running() => cx.emit(CompareEvent::Reload),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The columns shown: the key first, then the others from `col_offset`.
    fn visible_columns(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.mapping.key.clone();
        v.extend((self.col_offset..self.mapping.columns.len()).filter(|i| !self.mapping.key.contains(i)));
        v
    }
}

fn cell(v: &Value) -> String {
    if v.is_null() { "NULL".to_string() } else { v.display().replace('\n', " ") }
}

impl Render for DataCompareView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus.is_focused(window);
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        let label = |e: &Endpoint, t: &Table| format!("{} · {}.{}", e.conn_name, e.schema.name, t.name);

        // Summary lines.
        let (summary, notes): (String, Vec<(String, bool)>) = match &self.phase {
            Phase::Running => {
                let p = *self.progress.lock().expect("progress lock");
                (
                    format!(
                        "Reading… {} from the source, {} from the target · {} so far",
                        rows_word(p.left_rows),
                        rows_word(p.right_rows),
                        match p.differences {
                            1 => "1 difference".to_string(),
                            n => format!("{} differences", fmt_n(n)),
                        }
                    ),
                    vec![("esc cancels".to_string(), false)],
                )
            }
            Phase::Failed(why) => (why.clone(), vec![]),
            Phase::Done(r) => {
                let c = r.counts;
                let mut s = format!(
                    "{} only in the source · {} only in the target · {} different · {} the same",
                    rows_word(c.only_left),
                    rows_word(c.only_right),
                    fmt_n(c.different),
                    fmt_n(c.same)
                );
                if r.outcome == CompareOutcome::Cancelled {
                    s = format!("Cancelled — so far: {s}");
                }
                let mut notes = Vec::new();
                if r.capped {
                    notes.push((
                        format!("Showing the first {} differences; the counts above are exact. The script covers those; run it and compare again for the rest.", fmt_n(CAP as u64)),
                        true,
                    ));
                }
                if r.order == KeyOrder::Binary {
                    notes.push(("The servers' own key orders differ, so both were read in binary key order (slower, and what makes the merge correct).".to_string(), false));
                }
                let ignored: Vec<String> = [
                    (!self.mapping.left_only.is_empty())
                        .then(|| format!("only in the source: {}", self.mapping.left_only.join(", "))),
                    (!self.mapping.right_only.is_empty())
                        .then(|| format!("only in the target: {}", self.mapping.right_only.join(", "))),
                    (!self.mapping.generated.is_empty())
                        .then(|| format!("generated: {}", self.mapping.generated.join(", "))),
                ]
                .into_iter()
                .flatten()
                .collect();
                if !ignored.is_empty() {
                    notes.push((format!("Columns not compared ({})", ignored.join("; ")), false));
                }
                (s, notes)
            }
        };
        let script_line = self.report().map(|r| {
            let s = sync_script(
                &self.mapping,
                &r.rows,
                self.ends.target.kind,
                &self.ends.target.schema.name,
                &self.ends.target_table.name,
                self.include_deletes,
            );
            let n = |k: DiffKind| s.statements.iter().filter(|x| x.kind == k).map(|x| x.rows).sum::<usize>();
            let (ins, upd, del) = (n(DiffKind::OnlyLeft), n(DiffKind::Different), n(DiffKind::OnlyRight));
            format!(
                "Sync script: {ins} insert{}, {upd} update{}, {del} delete{} · deletes are {} (d)",
                if ins == 1 { "" } else { "s" },
                if upd == 1 { "" } else { "s" },
                if del == 1 { "" } else { "s" },
                if self.include_deletes { "included" } else { "commented out" },
            )
        });
        let header = v_flex()
            .px_3()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label(&self.ends.source, &self.ends.source_table)),
                    )
                    .child(div().text_color(theme.muted_foreground).child("→ make"))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label(&self.ends.target, &self.ends.target_table)),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(format!("match it · key ({})", self.mapping.key_left().join(", "))),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .when(matches!(self.phase, Phase::Failed(_)), |d| d.text_color(theme.danger))
                    .child(summary),
            )
            .children(
                script_line.map(|l| div().text_size(px(11.)).text_color(theme.muted_foreground).child(l)),
            )
            .children(notes.into_iter().map(|(t, warn)| {
                div()
                    .text_size(px(11.))
                    .text_color(if warn { theme.warning } else { theme.muted_foreground })
                    .child(t)
            }));

        // The grid.
        let cols = self.visible_columns();
        let names: Vec<String> = cols.iter().map(|&i| self.mapping.columns[i].left.name.clone()).collect();
        let head =
            h_flex()
                .h(px(24.))
                .flex_none()
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(div().w(px(30.)).flex_none())
                .children(names.iter().enumerate().map(|(i, n)| {
                    div().w(px(COL_W)).flex_none().px_2().truncate().font_weight(FontWeight::MEDIUM).child(
                        if self.mapping.key.contains(&cols[i]) { format!("🔑 {n}") } else { n.clone() },
                    )
                }));
        let body: AnyElement = if self.shown.is_empty() {
            let text = match &self.phase {
                Phase::Running => "Comparing…".to_string(),
                Phase::Failed(_) => String::new(),
                Phase::Done(r) if r.counts.differences() == 0 => "The rows are equal".to_string(),
                Phase::Done(_) => format!("No rows match the filter ({})", self.filter.label()),
            };
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        } else {
            let cols2 = cols.clone();
            uniform_list(
                "data-compare-rows",
                self.shown.len(),
                cx.processor(move |this, range: Range<usize>, _w, cx| {
                    let theme = cx.theme().clone();
                    let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
                    let Phase::Done(rep) = &this.phase else { return vec![] };
                    range
                        .map(|pos| {
                            let r: &DiffRow = &rep.rows[this.shown[pos]];
                            let (mark, c) = match r.kind {
                                DiffKind::OnlyLeft => ("+", theme.green),
                                DiffKind::OnlyRight => ("−", theme.red),
                                DiffKind::Different => ("~", theme.yellow),
                            };
                            let line = |row: &Option<Vec<Value>>, changed: bool, tint: gpui_kit::Hsla| {
                                h_flex().h(px(18.)).flex_none().children(cols2.iter().map(|&ci| {
                                    let text = row.as_ref().map(|v| cell(&v[ci])).unwrap_or_default();
                                    let hot = changed && r.changed.contains(&ci);
                                    div()
                                        .w(px(COL_W))
                                        .flex_none()
                                        .px_2()
                                        .truncate()
                                        .font_family(mono.clone())
                                        .text_size(px(11.))
                                        .when(hot, |d| d.bg(tint.opacity(0.25)).font_weight(FontWeight::BOLD))
                                        .when(row.as_ref().is_some_and(|v| v[ci].is_null()), |d| {
                                            d.text_color(theme.muted_foreground)
                                        })
                                        .child(text)
                                }))
                            };
                            let (l1, l2) = match r.kind {
                                DiffKind::OnlyLeft => {
                                    (line(&r.left, false, theme.green), line(&None, false, theme.green))
                                }
                                DiffKind::OnlyRight => {
                                    (line(&r.right, false, theme.red), line(&None, false, theme.red))
                                }
                                DiffKind::Different => {
                                    (line(&r.left, true, theme.green), line(&r.right, true, theme.red))
                                }
                            };
                            h_flex()
                                .id(("data-row", pos))
                                .h(px(37.))
                                .w_full()
                                .flex_none()
                                .border_b_1()
                                .border_color(theme.border.opacity(0.5))
                                .cursor_pointer()
                                .when(pos == this.sel, |d| d.bg(theme.list_active))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    window.focus(&this.focus, cx);
                                    this.sel = pos;
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .w(px(30.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(c)
                                        .font_weight(FontWeight::BOLD)
                                        .child(mark),
                                )
                                .child(v_flex().child(l1).child(l2))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let _ = mono;

        let filters = h_flex()
            .gap_3()
            .px_3()
            .h(px(26.))
            .items_center()
            .text_size(px(11.))
            .border_b_1()
            .border_color(theme.border)
            .children(
                [
                    (Filter::All, "1 all"),
                    (Filter::OnlySource, "2 only in source"),
                    (Filter::Different, "3 different"),
                    (Filter::OnlyTarget, "4 only in target"),
                ]
                .map(|(f, t)| {
                    div()
                        .px_2()
                        .rounded_sm()
                        .when(self.filter == f, |d| d.bg(theme.list_active).font_weight(FontWeight::SEMIBOLD))
                        .text_color(if self.filter == f { theme.foreground } else { theme.muted_foreground })
                        .child(t)
                }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} shown", fmt_n(self.shown.len() as u64))),
            );

        let footer = div()
            .px_3()
            .py_1()
            .border_t_2()
            // The only focus mark the tab needs: without it nothing says the keys reach it.
            .border_color(if focused { theme.primary } else { theme.border })
            .text_size(px(11.))
            .text_color(theme.muted_foreground)
            .child("j k move · h l more columns · 1 2 3 4 filter · d deletes in the script on / off · enter or o open the sync script in a console · c copy it · r compare again · esc cancels a running comparison");

        v_flex()
            .size_full()
            .bg(theme.background)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .child(header)
            .child(filters)
            .child(v_flex().flex_1().min_h_0().overflow_hidden().child(head).child(body))
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators_and_rows_agree_in_number() {
        assert_eq!(fmt_n(0), "0");
        assert_eq!(fmt_n(999), "999");
        assert_eq!(fmt_n(1_000), "1,000");
        assert_eq!(fmt_n(10_000), "10,000");
        assert_eq!(fmt_n(1_234_567), "1,234,567");
        assert_eq!(rows_word(1), "1 row");
        assert_eq!(rows_word(0), "0 rows");
        assert_eq!(rows_word(2_000), "2,000 rows");
    }

    #[test]
    fn databases_that_cannot_be_opened_twice_are_refused_with_a_reason() {
        let mut c = ConnectionConfig::new("a", "a", DatabaseKind::Sqlite);
        c.path = ":memory:".into();
        assert!(side_session_blocker(&c).unwrap().contains("in-memory SQLite"));
        c.path = "/tmp/x.db".into();
        assert!(side_session_blocker(&c).is_none());
        assert!(
            side_session_blocker(&ConnectionConfig::new("a", "a", DatabaseKind::DuckDb))
                .unwrap()
                .contains("DuckDB")
        );
        assert!(side_session_blocker(&ConnectionConfig::new("a", "a", DatabaseKind::Postgres)).is_none());
        assert!(side_session_blocker(&ConnectionConfig::new("a", "a", DatabaseKind::MySql)).is_none());
        let why = side_session_blocker(&ConnectionConfig::new("a", "a", DatabaseKind::SqlServer)).unwrap();
        assert!(why.contains("not supported"), "{why}");
    }

    #[test]
    fn the_filters_keep_the_right_kinds() {
        assert!(Filter::All.keep(DiffKind::OnlyRight));
        assert!(Filter::OnlySource.keep(DiffKind::OnlyLeft) && !Filter::OnlySource.keep(DiffKind::Different));
        assert!(Filter::OnlyTarget.keep(DiffKind::OnlyRight) && !Filter::OnlyTarget.keep(DiffKind::OnlyLeft));
        assert!(Filter::Different.keep(DiffKind::Different) && !Filter::Different.keep(DiffKind::OnlyRight));
    }
}
