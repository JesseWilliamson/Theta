//! Run the current statement or script on several connections at once ("multi-source run").
//!
//! Every chosen connection gets the same picked statements on its own session and its own terms:
//! its read-only setting refuses writes (that connection only), its transaction mode starts a transaction
//! lazily exactly as a console would (`begin_transaction_if_needed`), and a failure stops that connection's
//! script and nobody else's. The connections run concurrently on the Tokio runtime; each is marked busy
//! like a bulk operation so a console cannot interleave statements into the run.
//!
//! The answer lands in the console that asked, as result tabs: a Summary (one row per connection, from
//! `exec::multi::summary`) and one tab per connection that returned rows, titled with the connection.

use super::Workspace;
use super::console::ResultItem;
use super::run::{RunInfo, begin_transaction_if_needed, exec_statement};
use crate::connections::NoticeKind;
use crate::exec::multi::{ConnOutcome, Skipped, headline, status, summary, tab_title};
use crate::exec::pick::{self, Picked};
use crate::panels::messages::MessageKind;
use crate::runtime;
use gpui_kit::{Context, Window};
use std::sync::Arc;
use std::time::Instant;
use theta_core::{Connection, ExecOptions};
use theta_grid::GridMeta;
use theta_sql::classify;
use theta_ui::Settings;

/// What to run on each connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The selection, or the statement under the cursor.
    Statement,
    /// The whole console.
    Script,
}

const BULK_CONSOLE: u64 = u64::MAX;

impl Workspace {
    /// The statements `scope` means in the active console, split with the console's dialect.
    pub(super) fn multi_picks(&self, id: u64, scope: Scope, cx: &gpui_kit::App) -> Vec<Picked> {
        let Some((text, cursor, sel)) = self.editor_snapshot(id, cx) else { return Vec::new() };
        let kind = self.console_kind(id, cx);
        let picks: Vec<Picked> = match scope {
            Scope::Statement => match sel {
                Some((start, s)) if !s.trim().is_empty() => pick::selection(&text, start, &s, kind),
                _ => pick::at_cursor(&text, cursor, kind).into_iter().collect(),
            },
            Scope::Script => pick::all(&text, kind),
        };
        picks.into_iter().filter(|p| !p.sql.trim().is_empty()).collect()
    }

    /// For the dialog: each picked statement's first line and whether it writes, so it can warn in advance.
    pub fn multi_preview(&self, id: u64, scope: Scope, cx: &gpui_kit::App) -> Vec<(String, bool)> {
        self.multi_picks(id, scope, cx)
            .into_iter()
            .map(|p| {
                let writes =
                    matches!(p.class, theta_sql::StatementClass::Write | theta_sql::StatementClass::Ddl);
                (pick::first_line(&p.sql), writes)
            })
            .collect()
    }

    /// Open the dialog that picks the connections.
    pub(super) fn open_multi_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab.filter(|id| self.consoles.contains_key(id)) else {
            self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx);
            return;
        };
        if self.quiet {
            return;
        }
        let first = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone());
        // Computed here, before the dialog exists: a dialog opened from inside a workspace update must not
        // read the workspace (it is being updated), so everything it needs is handed over.
        let previews =
            [self.multi_preview(id, Scope::Statement, cx), self.multi_preview(id, Scope::Script, cx)];
        if let Err(why) = crate::dialogs::multi_run::open(
            cx.entity().downgrade(),
            self.store.clone(),
            id,
            first,
            previews,
            window,
            cx,
        ) {
            self.notify(NoticeKind::Info, why, window, cx);
        }
    }

    /// Run `scope` of console `id` on each of `conn_ids`. Nothing here stops at the first failure.
    pub fn run_on_several(
        &mut self,
        id: u64,
        conn_ids: Vec<String>,
        scope: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picks = self.multi_picks(id, scope, cx);
        if picks.is_empty() {
            self.notify(NoticeKind::Info, "There is no statement to run here", window, cx);
            return;
        }
        let limit = Settings::get(cx).row_limit;
        let opts = ExecOptions { row_limit: (limit > 0).then_some(limit) };
        struct Job {
            name: String,
            product: String,
            conn_id: String,
            // Boxed: a skipped outcome is a whole `ConnOutcome`, which clippy rightly finds large for an `Err`.
            work: Result<runtime::Spawned<ConnOutcome>, Box<ConnOutcome>>,
        }
        let mut jobs: Vec<Job> = Vec::new();
        for conn_id in conn_ids {
            let (name, kind, read_only) = match self.store.read(cx).config(&conn_id) {
                Some(c) => (c.name.clone(), c.kind, c.read_only),
                None => continue,
            };
            let product = kind.label().to_string();
            let total = picks.len();
            let skip = |why| Box::new(ConnOutcome::skipped(&name, &product, total, why));
            let work = (|| {
                let Some(connection) = self.store.read(cx).connection(&conn_id) else {
                    return Err(skip(Skipped::NotConnected));
                };
                if self.runs.contains_key(&conn_id) {
                    return Err(skip(Skipped::Busy));
                }
                // The classification depends on the dialect, so it is redone for this connection.
                if let Some(why) =
                    picks.iter().find_map(|p| pick::read_only_refusal(classify(&p.sql, kind), read_only))
                {
                    return Err(skip(Skipped::ReadOnly(why)));
                }
                let manual = self.manual_begin(&conn_id, cx);
                self.runs.insert(
                    conn_id.clone(),
                    RunInfo {
                        console: BULK_CONSOLE,
                        started: Instant::now(),
                        cancelled: false,
                        failed: false,
                    },
                );
                let (picks, opts, name, product) =
                    (picks.clone(), opts.clone(), name.clone(), product.clone());
                Ok(runtime::spawn(run_one(connection, manual, picks, opts, name, product)))
            })();
            jobs.push(Job { name: name.clone(), product: product.clone(), conn_id, work });
        }
        if jobs.is_empty() {
            return;
        }
        self.sync_running(cx);
        let sql_shown = picks.iter().map(|p| p.sql.clone()).collect::<Vec<_>>().join("\n");
        cx.spawn_in(window, async move |this, cx| {
            let mut outcomes = Vec::new();
            for job in jobs {
                let outcome = match job.work {
                    Err(skipped) => *skipped,
                    Ok(task) => task.await.unwrap_or_else(|e| {
                        let mut o = ConnOutcome::skipped(&job.name, &job.product, 0, Skipped::NotConnected);
                        o.skipped = None;
                        o.error = Some(e.to_string());
                        o
                    }),
                };
                outcomes.push((job.conn_id, outcome));
            }
            this.update_in(cx, |this, window, cx| this.finish_multi(id, outcomes, sql_shown, window, cx))
                .ok();
        })
        .detach();
    }

    fn finish_multi(
        &mut self,
        id: u64,
        outcomes: Vec<(String, ConnOutcome)>,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (conn_id, o) in &outcomes {
            if o.skipped.is_none() {
                self.end_bulk(conn_id, cx);
            }
            let kind = if o.failed() { MessageKind::Error } else { MessageKind::Ok };
            self.log(
                conn_id,
                &sql,
                kind,
                vec![format!("{}: {} ({} of {} statements)", o.name, status(o), o.ran, o.total)],
                None,
                cx,
            );
        }
        let all: Vec<ConnOutcome> = outcomes.into_iter().map(|(_, o)| o).collect();
        let mut items = vec![ResultItem {
            title: "Summary".into(),
            statement: sql.clone(),
            result: Arc::new(summary(&all)),
            meta: GridMeta::new(),
            geo: None,
            produced: Instant::now(),
        }];
        for o in &all {
            if let Some(rs) = &o.result {
                items.push(ResultItem {
                    title: tab_title(o),
                    statement: sql.clone(),
                    result: rs.clone(),
                    meta: GridMeta::new(),
                    geo: None,
                    produced: Instant::now(),
                });
            }
        }
        if let Some(c) = self.consoles.get_mut(&id) {
            c.results = items;
            c.active_result = 0;
            c.summary = None;
        }
        let head = headline(&all);
        let failed = all.iter().any(ConnOutcome::failed);
        self.notify(if failed { NoticeKind::Error } else { NoticeKind::Info }, head, window, cx);
        self.apply_console(id, window, cx);
        self.set_bottom_tab(super::BottomTab::Results, cx);
    }
}

/// One connection's share of the run, on the Tokio runtime.
async fn run_one(
    connection: Arc<dyn Connection>,
    manual: Option<super::run::ManualBegin>,
    picks: Vec<Picked>,
    opts: ExecOptions,
    name: String,
    product: String,
) -> ConnOutcome {
    let started = Instant::now();
    let mut o = ConnOutcome::skipped(&name, &product, picks.len(), Skipped::NotConnected);
    o.skipped = None;
    let mut affected = 0u64;
    let mut any_rows = false;
    for p in &picks {
        if let Err(e) = begin_transaction_if_needed(&connection, manual.as_ref(), p).await {
            o.error = Some(e.to_string());
            break;
        }
        match exec_statement(&connection, p, &opts).await {
            Ok(out) => {
                o.ran += 1;
                if let Some(rs) = out.result {
                    o.rows = Some(rs.rows.len() as u64);
                    o.truncated = rs.truncated;
                    o.result = Some(Arc::new(rs));
                    any_rows = true;
                } else if let Some(n) = out.rows_affected {
                    affected += n;
                }
            }
            Err(e) => {
                o.error = Some(e.to_string());
                break;
            }
        }
    }
    if !any_rows && affected > 0 {
        o.rows = Some(affected);
    }
    o.elapsed = started.elapsed();
    o
}
