//! Run the statement of a result on show again: "Load all" (the rows beyond the row limit) and
//! "Refresh" (the same rows again: after inserts the database could not read back, or when a rolled
//! back transaction left the result stale).
//!
//! A result stops at the row limit (1,000 by default) and the grid sorts and filters only what it
//! holds. Rather than let that mislead, the grid offers *Load all*, which runs the same statement
//! again with a much higher limit. It is an ordinary run as far as the rest of the app is concerned
//! (the connection is busy, the status bar counts the seconds, Cancel stops it), and when it ends the
//! grid keeps its sort, filter and column widths. The cap keeps a runaway `select *` from exhausting
//! memory; when it is reached the grid says so and asks for a narrower query.

use super::run::RunInfo;
use super::{BottomTab, Workspace};
use crate::connections::{ConnState, NoticeKind};
use crate::panels::messages::MessageKind;
use crate::runtime;
use gpui_kit::{Context, Window};
use std::sync::Arc;
use std::time::Instant;
use theta_core::{Error, ExecOptions};

use super::console::{fmt_ms, plural};

/// The most rows one "Load all" fetches.
pub const LOAD_ALL_CAP: usize = 500_000;

/// Which re-run this is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rerun {
    /// Fetch up to [`LOAD_ALL_CAP`] rows.
    LoadAll,
    /// Fetch again under the row limit of the settings.
    Refresh,
}

impl Workspace {
    /// The palette command and F5 in the grid: refresh the result on show.
    pub(super) fn refresh_active_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_tab {
            self.refresh_result(id, window, cx);
        }
    }

    /// The palette command: load the rest of the active console's result.
    pub(super) fn load_all_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab else { return };
        let partial = self.consoles.get(&id).is_some_and(|c| c.grid.read(cx).is_partial());
        if partial {
            self.load_all_rows(id, window, cx);
        } else {
            self.notify(NoticeKind::Info, "There are no more rows to load", window, cx);
        }
    }

    /// Load every row of the active result of console `id`, up to [`LOAD_ALL_CAP`].
    pub(super) fn load_all_rows(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.rerun_result(id, Rerun::LoadAll, window, cx);
    }

    /// Run the statement of the active result of console `id` again, keeping sort and filter.
    pub(super) fn refresh_result(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.rerun_result(id, Rerun::Refresh, window, cx);
    }

    fn rerun_result(&mut self, id: u64, what: Rerun, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.consoles.get(&id) else { return };
        let grid = c.grid.clone();
        let Some(submitted) = grid.read(cx).result().cloned() else { return };
        if what == Rerun::LoadAll && !submitted.truncated {
            return;
        }
        if grid.read(cx).has_pending_changes() {
            self.notify(
                NoticeKind::Info,
                "Submit or revert the pending changes first; running the query again would discard them",
                window,
                cx,
            );
            return;
        }
        let Some(item) = c.results.iter().find(|r| Arc::ptr_eq(&r.result, &submitted)) else { return };
        let (sql, mut meta) = (item.statement.clone(), item.meta.clone());
        let Some(conn_id) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone())
        else {
            return;
        };
        let kind =
            self.store.read(cx).config(&conn_id).map_or(theta_core::DatabaseKind::Postgres, |c| c.kind);
        // Only what the user ran as a query is run again on their behalf, never a write.
        if !matches!(theta_sql::classify(&sql, kind), theta_sql::StatementClass::Query) {
            self.notify(NoticeKind::Info, "Only a query can be run again from its result", window, cx);
            return;
        }
        if self.runs.contains_key(&conn_id) {
            self.notify(NoticeKind::Info, "A query is already running on this connection", window, cx);
            return;
        }
        let ConnState::Connected { connection, .. } = self.store.read(cx).state(&conn_id).clone() else {
            self.notify(NoticeKind::Info, "The connection is closed", window, cx);
            return;
        };
        self.runs.insert(
            conn_id.clone(),
            RunInfo { console: id, started: Instant::now(), cancelled: false, failed: false },
        );
        grid.update(cx, |g, cx| g.set_loading_all(what == Rerun::LoadAll, cx));
        if what == Rerun::Refresh {
            grid.update(cx, |g, cx| g.set_loading(true, cx));
        }
        self.sync_running(cx);
        self.ensure_ticker(cx);
        meta.stale = None;
        let limit = theta_ui::Settings::get(cx).row_limit;
        let opts = ExecOptions {
            row_limit: match what {
                Rerun::LoadAll => Some(LOAD_ALL_CAP),
                Rerun::Refresh => (limit > 0).then_some(limit),
            },
        };
        let started = Instant::now();
        cx.spawn_in(window, async move |this, cx| {
            let (c, q) = (connection.clone(), sql.clone());
            let res = runtime::spawn_result(async move { c.execute(&q, &opts).await }).await;
            this.update_in(cx, |this, window, cx| {
                let cancelled = this.runs.remove(&conn_id).is_some_and(|r| r.cancelled);
                this.sync_running(cx);
                grid.update(cx, |g, cx| {
                    g.set_loading_all(false, cx);
                    g.set_loading(false, cx);
                });
                let shown = submitted.rows.len();
                match res {
                    Ok(out) => {
                        let Some(rs) = out.result else { return };
                        let capped = rs.truncated;
                        let n = rs.rows.len() as u64;
                        let mut meta = meta.with_elapsed(out.elapsed);
                        if capped {
                            meta = meta.with_load_cap_reached();
                        }
                        let rs = Arc::new(rs);
                        // The console may have been given a newer result while this ran: only the
                        // result the request was made on is replaced.
                        let still = grid.read(cx).result().is_some_and(|r| Arc::ptr_eq(r, &submitted));
                        if let Some(item) = this
                            .consoles
                            .get_mut(&id)
                            .and_then(|c| c.results.iter_mut().find(|r| Arc::ptr_eq(&r.result, &submitted)))
                        {
                            item.result = rs.clone();
                            item.meta = meta.clone();
                            item.geo = None;
                        }
                        if still {
                            grid.update(cx, |g, cx| g.replace_result(rs.clone(), meta, cx));
                            if this.active_tab == Some(id) {
                                this.apply_console(id, window, cx);
                            }
                        }
                        let index = this
                            .consoles
                            .get(&id)
                            .and_then(|c| c.results.iter().position(|r| Arc::ptr_eq(&r.result, &rs)));
                        if let Some(i) = index {
                            this.detect_geometry(id, i, rs, window, cx);
                        }
                        let line = if what == Rerun::Refresh {
                            format!("Refreshed · {} · {}", plural(n, "row", "rows"), fmt_ms(out.elapsed))
                        } else if capped {
                            format!(
                                "Loaded the first {} (the most Load all fetches) · {}. More rows exist: narrow the query with WHERE or LIMIT.",
                                plural(n, "row", "rows"),
                                fmt_ms(out.elapsed)
                            )
                        } else {
                            format!("Loaded all {} · {}", plural(n, "row", "rows"), fmt_ms(out.elapsed))
                        };
                        this.log(&conn_id, &sql, MessageKind::Ok, vec![line.clone()], None, cx);
                        if what == Rerun::LoadAll {
                            this.notify(
                                if capped { NoticeKind::Info } else { NoticeKind::Success },
                                line,
                                window,
                                cx,
                            );
                        }
                    }
                    Err(e) if cancelled || matches!(e, Error::Cancelled) => {
                        let line = format!(
                            "Cancelled after {}; the {} already loaded are still shown",
                            fmt_ms(started.elapsed()),
                            plural(shown as u64, "row", "rows")
                        );
                        this.log(&conn_id, &sql, MessageKind::Info, vec![line.clone()], None, cx);
                        this.notify(NoticeKind::Info, line, window, cx);
                    }
                    Err(e) => {
                        this.log(
                            &conn_id,
                            &sql,
                            MessageKind::Error,
                            vec![format!("Could not run the query again: {e}")],
                            Some(&e),
                            cx,
                        );
                        this.set_bottom_tab(BottomTab::Messages, cx);
                    }
                }
                this.refresh_txn(conn_id.clone(), connection, cx);
            })
            .ok();
        })
        .detach();
    }
}
