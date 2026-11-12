//! Search Across Tables: find a text in every table of a connection, a schema or a hand-picked list.
//!
//! The queries come from `theta_sql::search` (one statement per table, every identifier and the text
//! itself escaped by the SQL generator) and are run by `theta_import::search::run_search`, which owns the
//! bounded concurrency, the per-table timeout and cancellation. This view only collects the options,
//! shows hits as they stream in, and opens a table filtered to the rows that matched.
//!
//! **Sessions.** Server products (PostgreSQL, MySQL and their relatives) get up to four extra sessions of
//! their own (`ConnectionStore::open_side_connection`), so a console keeps working and the search does not
//! share its transaction. A product whose database cannot be opened a second time (an in-memory SQLite, or
//! DuckDB, whose file is locked by the first handle) searches through the console's own session instead; the
//! connection is then marked busy for the duration, exactly as an import does.

use crate::connections::ConnectionStore;
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use std::collections::BTreeSet;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::{Catalog, Connection, DatabaseKind, Family, Feature, TableKind};
use theta_import::search::{CancelToken, SearchOutcome, SearchReport, TableResult, run_search};
use theta_sql::search::{ColumnClass, Scope, SearchOptions, Skipped, filter_statement, plan};

const SCOPES: [&str; 3] = ["Whole connection", "One schema", "Chosen tables"];
/// Sessions opened for one search. Four keeps a slow table from holding up the others without taking
/// a noticeable share of a server's connection limit.
const POOL: usize = 4;

type Opening = Pin<Box<dyn Future<Output = Result<Arc<dyn Connection>, theta_core::Error>> + Send>>;

struct Item {
    schema: String,
    name: String,
    checked: bool,
}

/// What the results list shows, in order. Built from the results by [`build_rows`].
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A table with matches (index into the results).
    Table(usize),
    /// One matching column of that table (result index, column index in `Counts::columns`).
    Column(usize, usize),
    Header(&'static str),
    /// A table that was not searched, timed out, was cancelled or failed.
    Problem {
        name: String,
        text: String,
        error: bool,
    },
}

impl Row {
    fn selectable(&self) -> bool {
        matches!(self, Row::Table(_) | Row::Column(..))
    }
}

fn qualified(schema: &str, table: &str) -> String {
    if schema.is_empty() { table.to_string() } else { format!("{schema}.{table}") }
}

/// The rows of the results list: tables with matches in the order they arrived, each followed by its
/// matching columns; then everything that has no answer, with the reason.
pub fn build_rows(results: &[TableResult], skipped: &[Skipped], limit: Duration) -> Vec<Row> {
    let mut rows = Vec::new();
    for (i, r) in results.iter().enumerate() {
        if let SearchOutcome::Matched(c) = &r.outcome {
            rows.push(Row::Table(i));
            rows.extend((0..c.columns.len()).map(|j| Row::Column(i, j)));
        }
    }
    let mut problems = Vec::new();
    for r in results {
        let name = qualified(&r.schema, &r.table);
        match &r.outcome {
            SearchOutcome::TimedOut => problems.push(Row::Problem {
                name,
                text: format!("timed out after {} s; not searched to the end", limit.as_secs()),
                error: true,
            }),
            SearchOutcome::Failed(e) => {
                problems.push(Row::Problem { name, text: format!("failed: {e}"), error: true })
            }
            SearchOutcome::Cancelled => {
                problems.push(Row::Problem { name, text: "cancelled".into(), error: true })
            }
            _ => {}
        }
    }
    for s in skipped {
        problems.push(Row::Problem {
            name: qualified(&s.schema, &s.table),
            text: format!("skipped: {}", s.reason),
            error: false,
        });
    }
    if !problems.is_empty() {
        rows.push(Row::Header("Not searched, timed out or failed"));
        rows.extend(problems);
    }
    rows
}

/// The one line that sums a finished search up.
pub fn summary(r: &SearchReport, skipped: usize) -> String {
    let mut s = format!(
        "{} searched · {} with matches · {} matching · {}",
        r.tables - r.not_run,
        r.matched_tables,
        theta_core::text::plural(r.matched_rows, "row", "rows"),
        theta_core::text::duration(r.elapsed)
    );
    for (n, what) in [(r.timed_out, "timed out"), (r.failed, "failed"), (skipped, "skipped")] {
        if n > 0 {
            s.push_str(&format!(" · {n} {what}"));
        }
    }
    if r.cancelled {
        s.push_str(&format!(" · cancelled, {} not started", r.not_run));
    }
    s
}

#[derive(Default)]
struct Shared {
    results: Vec<TableResult>,
}

pub struct SearchView {
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    kind: DatabaseKind,
    catalog: Arc<Catalog>,
    text: Entity<InputState>,
    row_limit: Entity<InputState>,
    timeout: Entity<InputState>,
    scope: Entity<SelectState<Vec<String>>>,
    schema: Entity<SelectState<Vec<String>>>,
    items: Vec<Item>,
    case_sensitive: bool,
    whole_value: bool,
    classes: BTreeSet<ColumnClass>,
    running: bool,
    cancel: CancelToken,
    shared: Arc<Mutex<Shared>>,
    /// The options that produced `results`, so a hit opens with the same predicates.
    searched: Option<(SearchOptions, Duration)>,
    results: Vec<TableResult>,
    skipped: Vec<Skipped>,
    report: Option<SearchReport>,
    rows: Vec<Row>,
    sel: Option<usize>,
    scroll: UniformListScrollHandle,
    tables_scroll: UniformListScrollHandle,
    status: Option<(bool, String)>,
    _subs: Vec<Subscription>,
}

impl Drop for SearchView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// `preselect` is the explorer entry the dialog was opened from: a schema, or a schema and table.
pub fn open(
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    preselect: Option<(String, Option<String>)>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(view) = SearchView::build(ws, store, conn_id, preselect, window, cx) else { return };
    let (for_cancel, focus) = (view.clone(), view.clone());
    window.open_dialog(cx, move |dialog, _, cx| {
        let for_cancel = for_cancel.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Search Across Tables")
            .w(px(720.))
            .overlay_closable(false)
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| for_cancel.update(cx, |v, cx| v.escape(cx)))
            .child(view.clone())
    });
    // The dialog takes focus when it opens; the text box must have it after that.
    window.defer(cx, move |window, cx| {
        let input = focus.read(cx).text.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
}

impl SearchView {
    fn build(
        ws: WeakEntity<Workspace>,
        store: Entity<ConnectionStore>,
        conn_id: String,
        preselect: Option<(String, Option<String>)>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<Self>> {
        let (config, catalog) = {
            let s = store.read(cx);
            (s.config(&conn_id).cloned()?, s.catalog(&conn_id)?)
        };
        let kind = config.kind;
        let mut items = Vec::new();
        let mut schemas = Vec::new();
        for s in &catalog.schemas {
            let mut any = false;
            for t in s.tables.iter().filter(|t| t.kind == TableKind::Table) {
                any = true;
                items.push(Item {
                    schema: s.name.clone(),
                    name: t.name.clone(),
                    checked: matches!(&preselect, Some((ps, Some(pt))) if ps == &s.name && pt == &t.name),
                });
            }
            if any {
                schemas.push(s.name.clone());
            }
        }
        let scope_index = match &preselect {
            Some((_, Some(_))) => 2,
            Some((_, None)) => 1,
            None => 0,
        };
        let schema_index = preselect
            .as_ref()
            .and_then(|(s, _)| schemas.iter().position(|n| n == s))
            .or_else(|| catalog.default_schema.as_ref().and_then(|d| schemas.iter().position(|n| n == d)))
            .unwrap_or(0);
        Some(cx.new(|cx| {
            let text = cx.new(|cx| InputState::new(window, cx).placeholder("Text to find"));
            let row_limit = cx.new(|cx| InputState::new(window, cx).default_value("100000"));
            let timeout = cx.new(|cx| InputState::new(window, cx).default_value("20"));
            let scope = cx.new(|cx| {
                SelectState::new(
                    SCOPES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    Some(IndexPath::new(scope_index)),
                    window,
                    cx,
                )
            });
            let schema =
                cx.new(|cx| SelectState::new(schemas, Some(IndexPath::new(schema_index)), window, cx));
            let subs = vec![
                cx.subscribe_in(&scope, window, |_: &mut Self, _, _: &SelectEvent<Vec<String>>, _, cx| {
                    cx.notify()
                }),
                cx.subscribe_in(&text, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.enter(window, cx);
                    }
                }),
            ];
            SearchView {
                ws,
                store,
                conn_id,
                kind,
                catalog,
                text,
                row_limit,
                timeout,
                scope,
                schema,
                items,
                case_sensitive: false,
                whole_value: false,
                classes: ColumnClass::ALL.into_iter().collect(),
                running: false,
                cancel: CancelToken::new(),
                shared: Arc::default(),
                searched: None,
                results: Vec::new(),
                skipped: Vec::new(),
                report: None,
                rows: Vec::new(),
                sel: None,
                scroll: UniformListScrollHandle::new(),
                tables_scroll: UniformListScrollHandle::new(),
                status: None,
                _subs: subs,
            }
        }))
    }

    fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.running {
            self.cancel.cancel();
            self.status = Some((false, "Cancelling…".into()));
            cx.notify();
            return false;
        }
        true
    }

    fn scope_choice(&self, cx: &App) -> Scope {
        match self.scope.read(cx).selected_value().map(String::as_str) {
            Some("One schema") => {
                Scope::Schema(self.schema.read(cx).selected_value().cloned().unwrap_or_default())
            }
            Some("Chosen tables") => Scope::Tables(
                self.items.iter().filter(|i| i.checked).map(|i| (i.schema.clone(), i.name.clone())).collect(),
            ),
            _ => Scope::Connection,
        }
    }

    fn options(&self, cx: &App) -> Result<(SearchOptions, Duration), String> {
        let mut o = SearchOptions::new(self.text.read(cx).value().to_string());
        o.case_sensitive = self.case_sensitive;
        o.whole_value = self.whole_value;
        o.classes = self.classes.clone();
        o.row_limit = self
            .row_limit
            .read(cx)
            .value()
            .trim()
            .replace(['_', ','], "")
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("The row limit per table must be a positive number.")?;
        let secs = self
            .timeout
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("The time limit per table must be a whole number of seconds.")?;
        Ok((o, Duration::from_secs(secs)))
    }

    /// Enter in the text box: search, or open the highlighted hit when the results are for this text.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fresh =
            self.searched.as_ref().is_some_and(|(o, _)| o.text == self.text.read(cx).value().as_ref());
        if !self.running && fresh && self.sel.is_some() {
            self.open_hit(window, cx);
        } else {
            self.start(window, cx);
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let (opts, limit) = match self.options(cx) {
            Ok(o) => o,
            Err(e) => return self.fail(e, cx),
        };
        let scope = self.scope_choice(cx);
        if matches!(&scope, Scope::Tables(t) if t.is_empty()) {
            return self.fail("Tick at least one table, or search the whole connection.".into(), cx);
        }
        let planned = match plan(&self.catalog, self.kind, &scope, &opts) {
            Ok(p) => p,
            Err(e) => return self.fail(e, cx),
        };
        if planned.jobs.is_empty() {
            self.skipped = planned.skipped;
            self.results.clear();
            self.rows = build_rows(&[], &self.skipped, limit);
            return self
                .fail("There is no table with a column of the chosen types in that scope.".into(), cx);
        }

        // Where the statements run: sessions of their own for a server, the console's session otherwise.
        let own_sessions = matches!(self.kind.family(), Family::Postgres | Family::MySql);
        let (openings, main): (Vec<Opening>, Option<Arc<dyn Connection>>) = if own_sessions {
            let n = POOL.min(planned.jobs.len());
            let opens = (0..n)
                .filter_map(|_| self.store.read(cx).open_side_connection(&self.conn_id))
                .map(|f| Box::pin(f) as Opening)
                .collect();
            (opens, None)
        } else {
            let Some(c) = self.store.read(cx).connection(&self.conn_id) else {
                return self.fail("The connection is not open.".into(), cx);
            };
            let Some(ws) = self.ws.upgrade() else { return };
            if !ws.update(cx, |w, cx| w.begin_bulk(&self.conn_id, window, cx)) {
                return;
            }
            (Vec::new(), Some(c))
        };

        self.running = true;
        self.cancel = CancelToken::new();
        self.shared = Arc::default();
        self.results.clear();
        self.skipped = planned.skipped.clone();
        self.report = None;
        self.sel = None;
        self.rows = build_rows(&[], &self.skipped, limit);
        self.searched = Some((opts.clone(), limit));
        self.status = Some((true, format!("Searching {}…", theta_core::text::plural(planned.jobs.len() as u64, "table", "tables"))));
        cx.notify();

        let (shared, cancel, row_limit) = (self.shared.clone(), self.cancel.clone(), opts.row_limit);
        let jobs = planned.jobs;
        let task = runtime::spawn(async move {
            let mut pool: Vec<Arc<dyn Connection>> = Vec::new();
            let mut opened: Vec<Arc<dyn Connection>> = Vec::new();
            let mut first_error = None;
            for f in openings {
                match f.await {
                    Ok(c) => {
                        pool.push(c.clone());
                        opened.push(c);
                    }
                    Err(e) => first_error = first_error.or(Some(e.to_string())),
                }
            }
            pool.extend(main);
            if pool.is_empty() {
                return Err(first_error.unwrap_or_else(|| "no session could be opened".into()));
            }
            let sink = shared.clone();
            let report = run_search(
                pool,
                jobs,
                row_limit,
                limit,
                cancel,
                Arc::new(move |r| sink.lock().unwrap().results.push(r)),
            )
            .await;
            for c in opened {
                let _ = c.close().await;
            }
            Ok(report)
        });
        self.tick(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            this.update_in(cx, |this, _, cx| this.finish(outcome, cx)).ok();
        })
        .detach();
    }

    fn fail(&mut self, text: String, cx: &mut Context<Self>) {
        self.status = Some((false, text));
        cx.notify();
    }

    /// Pull what the workers have reported so far into the view, ten times a second while running.
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let alive = this.update(cx, |this, cx| {
                    this.pull(cx);
                    this.running
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn pull(&mut self, cx: &mut Context<Self>) {
        let n = self.shared.lock().unwrap().results.len();
        if n == self.results.len() {
            return;
        }
        self.results = self.shared.lock().unwrap().results.clone();
        let limit = self.searched.as_ref().map_or(Duration::ZERO, |(_, l)| *l);
        self.rows = build_rows(&self.results, &self.skipped, limit);
        if self.sel.is_none() {
            self.sel = self.rows.iter().position(Row::selectable);
        }
        cx.notify();
    }

    fn finish(&mut self, outcome: Result<SearchReport, String>, cx: &mut Context<Self>) {
        self.running = false;
        let conn = self.conn_id.clone();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.end_bulk(&conn, cx));
        }
        self.pull(cx);
        let (ok, text) = match outcome {
            Ok(r) => {
                let text = summary(&r, self.skipped.len());
                self.report = Some(r);
                (true, text)
            }
            Err(e) => (false, format!("The search could not start: {e}")),
        };
        if let (Some((o, _)), true) = (&self.searched, ok) {
            self.log(&o.text.clone(), &text, cx);
        }
        self.status = Some((ok, text));
        cx.notify();
    }

    fn log(&self, needle: &str, text: &str, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let mut lines = vec![text.to_string()];
        for r in &self.results {
            match &r.outcome {
                SearchOutcome::Matched(c) => lines.push(format!(
                    "{}: {} ({})",
                    qualified(&r.schema, &r.table),
                    theta_core::text::plural(c.matched_rows, "row", "rows"),
                    c.columns.iter().map(|(n, k)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")
                )),
                SearchOutcome::TimedOut => {
                    lines.push(format!("{}: timed out", qualified(&r.schema, &r.table)))
                }
                SearchOutcome::Failed(e) => {
                    lines.push(format!("{}: failed: {e}", qualified(&r.schema, &r.table)))
                }
                _ => {}
            }
        }
        let title = format!("SEARCH FOR “{}”", needle.chars().take(60).collect::<String>());
        let conn = self.conn_id.clone();
        ws.update(cx, |w, cx| w.log_bulk(&conn, &title, MessageKind::Ok, lines, cx));
    }

    fn move_sel(&mut self, delta: isize, cx: &mut Context<Self>) {
        let hits: Vec<usize> =
            self.rows.iter().enumerate().filter(|(_, r)| r.selectable()).map(|(i, _)| i).collect();
        if hits.is_empty() {
            return;
        }
        let at = self.sel.and_then(|s| hits.iter().position(|h| *h == s)).unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, hits.len() as isize - 1) as usize;
        self.sel = Some(hits[next]);
        self.scroll.scroll_to_item(hits[next], gpui_kit::ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Open the table of the highlighted row, filtered to the rows that matched.
    fn open_hit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((opts, _)) = self.searched.clone() else { return };
        let Some(row) = self.sel.and_then(|s| self.rows.get(s)).cloned() else { return };
        let (r, columns): (&TableResult, Vec<String>) = match row {
            Row::Table(i) => match &self.results[i].outcome {
                SearchOutcome::Matched(c) => {
                    (&self.results[i], c.columns.iter().map(|(n, _)| n.clone()).collect())
                }
                _ => return,
            },
            Row::Column(i, j) => match &self.results[i].outcome {
                SearchOutcome::Matched(c) => (&self.results[i], vec![c.columns[j].0.clone()]),
                _ => return,
            },
            _ => return,
        };
        let sql = filter_statement(self.kind, &r.schema, &r.table, &columns, &opts);
        let (conn, table) = (self.conn_id.clone(), r.table.clone());
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.open_search_hit(conn, table, sql, window, cx));
        }
        window.close_dialog(cx);
    }

    fn render_row(&self, i: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selected = self.sel == Some(i);
        let base = h_flex()
            .id(("search-row", i))
            .w_full()
            .h(px(24.))
            .px_2()
            .gap_2()
            .items_center()
            .text_size(px(12.))
            .when(selected, |d| d.bg(theme.list_active))
            .hover(|s| s.bg(theme.list_hover));
        match &self.rows[i] {
            Row::Table(r) => {
                let res = &self.results[*r];
                let SearchOutcome::Matched(c) = &res.outcome else { return base.into_any_element() };
                let mut notes = Vec::new();
                if res.partial {
                    notes.push("first rows only".to_string());
                }
                if res.columns_cut > 0 {
                    notes.push(format!("{} left out", theta_core::text::plural(res.columns_cut as u64, "column", "columns")));
                }
                base.cursor_pointer()
                    .on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                        this.sel = Some(i);
                        if ev.click_count() >= 2 {
                            this.open_hit(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().font_weight(FontWeight::MEDIUM).child(qualified(&res.schema, &res.table)))
                    .child(div().text_color(theme.muted_foreground).child(format!(
                        "{} matching {}",
                        c.matched_rows,
                        if c.matched_rows == 1 { "row" } else { "rows" }
                    )))
                    .when(!notes.is_empty(), |d| {
                        d.child(div().text_color(theme.warning).child(notes.join(" · ")))
                    })
                    .into_any_element()
            }
            Row::Column(r, j) => {
                let SearchOutcome::Matched(c) = &self.results[*r].outcome else {
                    return base.into_any_element();
                };
                let (name, n) = &c.columns[*j];
                base.pl_6()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                        this.sel = Some(i);
                        if ev.click_count() >= 2 {
                            this.open_hit(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().font_family("monospace").child(name.clone()))
                    .child(div().text_color(theme.muted_foreground).child(if *n == 1 {
                        "1 row".to_string()
                    } else {
                        format!("{n} rows")
                    }))
                    .into_any_element()
            }
            Row::Header(t) => h_flex()
                .h(px(24.))
                .px_2()
                .items_end()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(*t)
                .into_any_element(),
            Row::Problem { name, text, error } => h_flex()
                .h(px(24.))
                .px_2()
                .gap_2()
                .items_center()
                .text_size(px(12.))
                .child(div().flex_shrink_0().child(name.clone()))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if *error { theme.danger } else { theme.muted_foreground })
                        .child(text.clone()),
                )
                .into_any_element(),
        }
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let caption = |t: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let scope = self.scope.read(cx).selected_value().cloned().unwrap_or_default();
        let chosen = self.items.iter().filter(|i| i.checked).count();
        let unsupported = self.kind.unsupported(Feature::TableSearch);
        v_flex()
            .gap_3()
            .on_key_down(cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "down" => this.move_sel(1, cx),
                    "up" => this.move_sel(-1, cx),
                    "enter"
                        if !this.running
                            && this.sel.is_some()
                            && (ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform) =>
                    {
                        this.open_hit(window, cx)
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(caption("Find"))
                            .child(Input::new(&self.text)),
                    )
                    .child(
                        Button::new("search-go")
                            .primary()
                            .label(if self.running { "Searching…" } else { "Search" })
                            .disabled(self.running || unsupported.is_some())
                            .on_click(cx.listener(|this, _, w, cx| this.start(w, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex().gap_1().w(px(190.)).child(caption("Where")).child(Select::new(&self.scope)),
                    )
                    .when(scope == "One schema", |d| {
                        d.child(
                            v_flex()
                                .gap_1()
                                .w(px(190.))
                                .child(caption("Schema"))
                                .child(Select::new(&self.schema)),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(110.))
                            .child(caption("Rows per table"))
                            .child(Input::new(&self.row_limit)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(90.))
                            .child(caption("Seconds"))
                            .child(Input::new(&self.timeout)),
                    ),
            )
            .when(scope == "Chosen tables", |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(caption("Tables"))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{chosen} of {} chosen", self.items.len())),
                                )
                                .child(div().flex_1())
                                .child(Button::new("search-none").label("None").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.items.iter_mut().for_each(|i| i.checked = false);
                                        cx.notify();
                                    },
                                ))),
                        )
                        .child(
                            div().h(px(120.)).border_1().border_color(theme.border).rounded_md().child(
                                uniform_list(
                                    "search-tables",
                                    self.items.len(),
                                    cx.processor(|this, range: Range<usize>, _w, cx| {
                                        let theme = cx.theme().clone();
                                        range
                                            .map(|i| {
                                                let it = &this.items[i];
                                                h_flex()
                                                    .id(("search-table", i))
                                                    .h(px(24.))
                                                    .px_2()
                                                    .gap_2()
                                                    .items_center()
                                                    .cursor_pointer()
                                                    .text_size(px(12.))
                                                    .hover(|s| s.bg(theme.list_hover))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.items[i].checked = !this.items[i].checked;
                                                        cx.notify();
                                                    }))
                                                    .child(div().w(px(14.)).child(if it.checked {
                                                        "☑"
                                                    } else {
                                                        "☐"
                                                    }))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .truncate()
                                                            .child(qualified(&it.schema, &it.name)),
                                                    )
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .track_scroll(&self.tables_scroll)
                                .size_full(),
                            ),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .child(
                        Switch::new("search-case")
                            .label("Case sensitive")
                            .checked(self.case_sensitive)
                            .on_change(cx.listener(|this, v, _, cx| {
                                this.case_sensitive = *v;
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("search-whole").label("Whole value").checked(self.whole_value).on_change(
                            cx.listener(|this, v, _, cx| {
                                this.whole_value = *v;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(h_flex().gap_2().items_center().flex_wrap().child(caption("Columns")).children(
                ColumnClass::ALL.map(|c| {
                    Switch::new(("search-class", c as usize))
                        .label(c.label())
                        .checked(self.classes.contains(&c))
                        .on_change(cx.listener(move |this, on, _, cx| {
                            if *on {
                                this.classes.insert(c);
                            } else {
                                this.classes.remove(&c);
                            }
                            cx.notify();
                        }))
                }),
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex().gap_2().child(caption("Results")).child(
                            div().text_size(px(11.)).text_color(theme.muted_foreground).child(
                                "up / down choose, enter opens the table filtered to the matching rows",
                            ),
                        ),
                    )
                    .child(div().h(px(230.)).border_1().border_color(theme.border).rounded_md().child(
                        if self.rows.is_empty() {
                            div()
                                .p_3()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(if self.running {
                                    "Waiting for the first table…".to_string()
                                } else if let Some(r) = &self.report {
                                    format!("Nothing found in {}.", theta_core::text::plural((r.tables - r.not_run) as u64, "table", "tables"))
                                } else {
                                    "Matches are listed here as they are found.".to_string()
                                })
                                .into_any_element()
                        } else {
                            uniform_list(
                                "search-results",
                                self.rows.len(),
                                cx.processor(|this, range: Range<usize>, _w, cx| {
                                    range.map(|i| this.render_row(i, cx)).collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(&self.scroll)
                            .size_full()
                            .into_any_element()
                        },
                    )),
            )
            .child(v_flex().gap_1().min_h(px(20.)).when_some(self.status.clone(), |d, (ok, t)| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(if ok { theme.foreground } else { theme.danger })
                        .child(t),
                )
            }))
            .when_some(unsupported, |d, why| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.warning)
                        .child(format!("Search is off for this product: {why}")),
                )
            })
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("search-close")
                        .label(if self.running { "Cancel Search" } else { "Close" })
                        .on_click(cx.listener(|this, _, w, cx| {
                            if this.running {
                                this.escape(cx);
                            } else {
                                w.close_dialog(cx);
                            }
                        })),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_sql::search::Counts;

    fn result(table: &str, outcome: SearchOutcome) -> TableResult {
        TableResult {
            schema: "public".into(),
            table: table.into(),
            outcome,
            partial: false,
            columns_cut: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn matched(rows: u64, cols: &[(&str, u64)]) -> SearchOutcome {
        SearchOutcome::Matched(Counts {
            scanned: 10,
            matched_rows: rows,
            columns: cols.iter().map(|(c, n)| (c.to_string(), *n)).collect(),
        })
    }

    #[test]
    fn hits_come_first_in_arrival_order_then_everything_that_has_no_answer() {
        let results = vec![
            result("a", SearchOutcome::NoMatch { scanned: 3 }),
            result("b", matched(3, &[("x", 2), ("y", 1)])),
            result("c", SearchOutcome::TimedOut),
            result("d", matched(1, &[("z", 1)])),
            result("e", SearchOutcome::Failed("permission denied".into())),
        ];
        let skipped = vec![Skipped {
            schema: "public".into(),
            table: "blobs".into(),
            reason: "no column of the chosen types".into(),
        }];
        let rows = build_rows(&results, &skipped, Duration::from_secs(20));
        let shape: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Table(i) => format!("table {i}"),
                Row::Column(i, j) => format!("column {i}.{j}"),
                Row::Header(_) => "header".into(),
                Row::Problem { name, text, .. } => format!("{name}: {text}"),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "table 1",
                "column 1.0",
                "column 1.1",
                "table 3",
                "column 3.0",
                "header",
                "public.c: timed out after 20 s; not searched to the end",
                "public.e: failed: permission denied",
                "public.blobs: skipped: no column of the chosen types",
            ]
        );
    }

    #[test]
    fn a_search_with_no_trouble_has_no_problem_section() {
        let rows = build_rows(&[result("a", matched(1, &[("x", 1)]))], &[], Duration::from_secs(5));
        assert!(rows.iter().all(|r| !matches!(r, Row::Header(_) | Row::Problem { .. })));
        assert!(build_rows(&[], &[], Duration::ZERO).is_empty());
    }

    #[test]
    fn the_summary_names_every_kind_of_gap() {
        let r = SearchReport {
            tables: 10,
            matched_tables: 2,
            matched_rows: 41,
            timed_out: 1,
            failed: 1,
            not_run: 3,
            cancelled: true,
            elapsed: Duration::from_millis(2100),
        };
        assert_eq!(
            summary(&r, 2),
            "7 searched · 2 with matches · 41 rows matching · 2.10 s · 1 timed out · 1 failed · 2 skipped · cancelled, 3 not started"
        );
        let clean = SearchReport { tables: 4, elapsed: Duration::from_millis(300), ..Default::default() };
        assert_eq!(summary(&clean, 0), "4 searched · 0 with matches · 0 rows matching · 300 ms");
    }

    #[test]
    fn only_hits_are_selectable() {
        assert!(Row::Table(0).selectable() && Row::Column(0, 0).selectable());
        assert!(!Row::Header("x").selectable());
        assert!(!Row::Problem { name: String::new(), text: String::new(), error: false }.selectable());
    }
}
