//! Results, plans, edits and status belong to the console that asked for them.
//!
//! The bottom panels (grid, plan, map, status bar) are shared widgets that show the active
//! console. A statement, an EXPLAIN or a grid submit that is still in flight when the user
//! switches tabs must be stored with the console it came from and must not touch what the
//! other console is showing. A `Gated` connection holds a statement on the Tokio thread until
//! the test lets it go, so "finishes while another tab is active" is deterministic instead of
//! a race against a slow query.

use super::console::PlanState;
use super::run::RunReq;
use super::*;
use crate::connections::ConnectionStore;
use crate::connections::secrets::SecretStore;
use crate::panels::messages::MessageKind;
use async_trait::async_trait;
use gpui_kit::{TestAppContext, VisualTestContext};
use quarry_core::{
    Catalog, Connection, ConnectionConfig, DatabaseKind, Error, ExecOptions, Plan, ServerInfo,
    StatementOutcome, Value,
};
use std::sync::Arc;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// A latch: statements carrying the marker (or every statement when `hold_all`) wait while it
/// is closed.
struct Gate {
    closed: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn closed() -> Arc<Self> {
        Arc::new(Self { closed: Mutex::new(true), changed: Condvar::new() })
    }

    fn set(&self, closed: bool) {
        *self.closed.lock().unwrap() = closed;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut closed = self.closed.lock().unwrap();
        while *closed {
            closed = self.changed.wait(closed).unwrap();
        }
    }
}

const MARK: &str = "/*gate*/";

struct Gated {
    inner: Arc<dyn Connection>,
    gate: Arc<Gate>,
    /// Hold every statement, not just the marked ones (for grid submits, whose SQL is generated).
    hold_all: Arc<std::sync::atomic::AtomicBool>,
}

impl Gated {
    fn hold(&self, sql: &str) {
        if sql.contains(MARK) || self.hold_all.load(std::sync::atomic::Ordering::SeqCst) {
            // Blocks this Tokio worker, which is the point: the statement is "in flight".
            self.gate.wait();
        }
    }
}

#[async_trait]
impl Connection for Gated {
    fn kind(&self) -> DatabaseKind {
        self.inner.kind()
    }
    async fn server_info(&self) -> Result<ServerInfo, Error> {
        self.inner.server_info().await
    }
    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome, Error> {
        self.hold(sql);
        self.inner.execute(sql, options).await
    }
    async fn cancel(&self) -> Result<(), Error> {
        self.inner.cancel().await
    }
    async fn introspect(&self) -> Result<Catalog, Error> {
        self.inner.introspect().await
    }
    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan, Error> {
        self.hold(sql);
        self.inner.explain(sql, analyze).await
    }
    async fn in_transaction(&self) -> bool {
        self.inner.in_transaction().await
    }
    async fn close(&self) -> Result<(), Error> {
        self.inner.close().await
    }
}

struct Fixture {
    ws: Entity<Workspace>,
    store: Entity<ConnectionStore>,
    gate: Arc<Gate>,
    hold_all: Arc<std::sync::atomic::AtomicBool>,
    /// Connection of console A (gated) and of console B (plain).
    conn_a: String,
    conn_b: String,
    _dir: tempfile::TempDir,
}

fn wait_for(vcx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    vcx.executor().allow_parking();
    for _ in 0..800 {
        vcx.run_until_parked();
        if done(vcx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// Two in-memory SQLite connections; the first is wrapped in a `Gated` whose latch starts closed.
fn setup(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.executor().allow_parking();
    cx.update(|cx| {
        quarry_ui::init(cx);
        crate::keymap::bind(cx);
    });
    let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
    let store = cx.new(|_| store);
    let ids: Vec<String> = ["a", "b"]
        .iter()
        .map(|n| {
            let mut config = ConnectionConfig::new(*n, *n, DatabaseKind::Sqlite);
            config.path = ":memory:".into();
            store.update(cx, |s, cx| {
                let id = s.add_temporary(config, cx);
                s.connect(&id, cx);
                id
            })
        })
        .collect();
    let (ws, vcx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
        ws.quiet = true;
        ws
    });
    wait_for(vcx, "the connections", |vcx| {
        store.read_with(vcx, |s, _| ids.iter().all(|i| s.state(i).is_connected()))
    });
    let gate = Gate::closed();
    let hold_all = Arc::new(std::sync::atomic::AtomicBool::new(false));
    store.update(vcx, |s, cx| {
        let inner = s.connection(&ids[0]).expect("connected");
        let gated: Arc<dyn Connection> =
            Arc::new(Gated { inner, gate: gate.clone(), hold_all: hold_all.clone() });
        s.set_catalog_synthetic(&ids[0], Catalog::default(), gated, cx);
    });
    let fx = Fixture { ws, store, gate, hold_all, conn_a: ids[0].clone(), conn_b: ids[1].clone(), _dir: dir };
    (fx, vcx)
}

fn open(f: &Fixture, vcx: &mut VisualTestContext, conn: &str, sql: &str) -> u64 {
    f.ws.update_in(vcx, |ws, window, cx| {
        ws.open_console(Some(conn.to_string()), Some(sql.into()), window, cx)
    })
}

fn run(f: &Fixture, vcx: &mut VisualTestContext, tab: u64) {
    f.ws.update_in(vcx, |ws, window, cx| ws.run_request(tab, RunReq::All, window, cx));
}

fn activate(f: &Fixture, vcx: &mut VisualTestContext, tab: u64) {
    f.ws.update_in(vcx, |ws, window, cx| ws.activate_tab(tab, window, cx));
}

/// First cell of the first row of the grid on show, as text.
fn shown(f: &Fixture, vcx: &mut VisualTestContext) -> Option<String> {
    f.ws.read_with(vcx, |ws, cx| {
        let grid = ws.results.read(cx).grid.read(cx);
        match grid.result()?.rows.first()?.first()? {
            Value::Text(t) => Some(t.clone()),
            v => Some(format!("{v:?}")),
        }
    })
}

fn run_finished(f: &Fixture, vcx: &mut VisualTestContext, tab: u64) -> bool {
    f.ws.read_with(vcx, |ws, _| {
        ws.runs.is_empty() && ws.consoles[&tab].summary.is_some()
            || ws.runs.is_empty() && !ws.consoles[&tab].results.is_empty()
    })
}

#[gpui_kit::test]
fn a_result_finishing_in_a_background_console_does_not_touch_the_visible_one(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    // B finishes first and is what the user is looking at.
    let b = open(&f, vcx, &f.conn_b.clone(), "select 'from b' as who");
    run(&f, vcx, b);
    wait_for(vcx, "B's result", |vcx| run_finished(&f, vcx, b));
    assert_eq!(shown(&f, vcx).as_deref(), Some("from b"));

    // A starts a statement that is held on the server, then the user moves to B.
    let a = open(&f, vcx, &f.conn_a.clone(), &format!("select 'from a' as who {MARK}"));
    run(&f, vcx, a);
    wait_for(vcx, "A's statement to start", |vcx| f.ws.read_with(vcx, |ws, _| !ws.runs.is_empty()));
    // While A runs, A shows its own running state...
    f.ws.read_with(vcx, |ws, cx| {
        assert!(ws.running_elapsed(cx).is_some(), "the active console is the one running");
        assert!(ws.results.read(cx).is_running());
    });
    activate(&f, vcx, b);
    // ...and B shows B's result, no running state.
    assert_eq!(shown(&f, vcx).as_deref(), Some("from b"));
    f.ws.read_with(vcx, |ws, cx| {
        assert!(ws.running_elapsed(cx).is_none(), "B is idle");
        assert!(!ws.results.read(cx).is_running());
    });
    let (info_before, tab_before) =
        f.ws.read_with(vcx, |ws, _| (ws.status.result_info.clone(), ws.state.bottom_tab));
    f.ws.update_in(vcx, |ws, _, cx| ws.set_bottom_tab(BottomTab::Map, cx));

    // Let A finish while B is on screen.
    f.gate.set(false);
    wait_for(vcx, "A to finish", |vcx| run_finished(&f, vcx, a));
    assert_eq!(shown(&f, vcx).as_deref(), Some("from b"), "B's grid is unchanged");
    f.ws.read_with(vcx, |ws, cx| {
        assert_eq!(ws.status.result_info, info_before, "status line belongs to B");
        assert_eq!(
            ws.state.bottom_tab,
            BottomTab::Map,
            "A finishing must not switch B's tab (was {tab_before:?})"
        );
        assert_eq!(ws.consoles[&a].results.len(), 1, "A's result is stored with A");
        assert_eq!(ws.consoles[&b].results.len(), 1);
        assert!(ws.results.read(cx).grid == ws.consoles[&b].grid);
    });

    // Switching back shows A's result.
    activate(&f, vcx, a);
    assert_eq!(shown(&f, vcx).as_deref(), Some("from a"));
    f.ws.read_with(vcx, |ws, cx| assert!(ws.results.read(cx).grid == ws.consoles[&a].grid));
    activate(&f, vcx, b);
    assert_eq!(shown(&f, vcx).as_deref(), Some("from b"));
}

#[gpui_kit::test]
fn switching_to_a_running_console_shows_its_running_state(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let a = open(&f, vcx, &f.conn_a.clone(), &format!("select 1 {MARK}"));
    let b = open(&f, vcx, &f.conn_b.clone(), "select 'b'");
    run(&f, vcx, a);
    wait_for(vcx, "A to start", |vcx| f.ws.read_with(vcx, |ws, _| !ws.runs.is_empty()));
    activate(&f, vcx, b);
    activate(&f, vcx, a);
    f.ws.read_with(vcx, |ws, cx| {
        assert!(ws.results.read(cx).is_running(), "A still runs: the panel says so, not 'Run a query'");
        assert!(ws.running_elapsed(cx).is_some());
        assert!(ws.results.read(cx).grid.read(cx).result().is_none());
    });
    f.gate.set(false);
    wait_for(vcx, "A to finish", |vcx| run_finished(&f, vcx, a));
    f.ws.read_with(vcx, |ws, cx| {
        assert!(!ws.results.read(cx).is_running());
        assert!(ws.results.read(cx).grid.read(cx).result().is_some());
    });
}

#[gpui_kit::test]
fn map_features_follow_the_console(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let b = open(&f, vcx, &f.conn_b.clone(), "select 'plain' as who");
    run(&f, vcx, b);
    wait_for(vcx, "B", |vcx| run_finished(&f, vcx, b));
    let a = open(&f, vcx, &f.conn_a.clone(), &format!("select 'POINT(1 2)' as geom {MARK}"));
    run(&f, vcx, a);
    wait_for(vcx, "A to start", |vcx| f.ws.read_with(vcx, |ws, _| !ws.runs.is_empty()));
    activate(&f, vcx, b);
    f.gate.set(false);
    wait_for(vcx, "A to finish", |vcx| run_finished(&f, vcx, a));
    // Geometry is detected on a worker; give it time to land (in A, not in B's map).
    wait_for(vcx, "A's geometry", |vcx| {
        f.ws.read_with(vcx, |ws, _| ws.consoles[&a].results[0].geo.is_some())
    });
    f.ws.read_with(vcx, |ws, _| assert_eq!(ws.map_badge, None, "B has no geometry; A's must not leak in"));
    activate(&f, vcx, a);
    f.ws.read_with(vcx, |ws, _| assert_eq!(ws.map_badge, Some(1)));
    activate(&f, vcx, b);
    f.ws.read_with(vcx, |ws, _| assert_eq!(ws.map_badge, None));
}

#[gpui_kit::test]
fn a_plan_finishing_in_a_background_console_is_stored_with_its_console(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let a = open(&f, vcx, &f.conn_a.clone(), &format!("select 1 as a {MARK}"));
    let b = open(&f, vcx, &f.conn_b.clone(), "select 2 as b");
    // A explains (held), the user moves to B and explains there.
    activate(&f, vcx, a);
    f.ws.update_in(vcx, |ws, window, cx| ws.explain_console(a, false, window, cx));
    activate(&f, vcx, b);
    f.ws.update_in(vcx, |ws, window, cx| ws.explain_console(b, false, window, cx));
    wait_for(vcx, "B's plan", |vcx| {
        f.ws.read_with(vcx, |ws, cx| {
            ws.plan.read(cx).shown_statement().is_some_and(|s| s.contains("select 2"))
        })
    });
    f.ws.read_with(vcx, |ws, _| {
        assert!(matches!(ws.consoles[&a].plan, PlanState::Loading), "A is still waiting for its plan");
    });

    f.gate.set(false);
    wait_for(vcx, "A's plan", |vcx| {
        f.ws.read_with(vcx, |ws, _| matches!(ws.consoles[&a].plan, PlanState::Ready(_)))
    });
    f.ws.read_with(vcx, |ws, cx| {
        let shown = ws.plan.read(cx).shown_statement().unwrap_or_default().to_string();
        assert!(shown.contains("select 2"), "B's plan stays on screen, got {shown:?}");
        assert_eq!(ws.plan_owner, Some(b));
    });
    activate(&f, vcx, a);
    f.ws.read_with(vcx, |ws, cx| {
        assert!(ws.plan.read(cx).shown_statement().is_some_and(|s| s.contains("select 1")));
        assert_eq!(ws.plan_owner, Some(a));
    });
}

/// SQLite session with a table to edit; returns the console id of `select id, v from t` on A.
fn editable_console(f: &Fixture, vcx: &mut VisualTestContext) -> u64 {
    let a = open(
        f,
        vcx,
        &f.conn_a.clone(),
        "create table t (id integer primary key, v text); insert into t values (1, 'one'), (2, 'two')",
    );
    run(f, vcx, a);
    wait_for(vcx, "the setup statements", |vcx| run_finished(f, vcx, a));
    // The editable result needs the table in the catalog.
    let conn = f.conn_a.clone();
    f.store.update(vcx, |s, cx| s.refresh_catalog(&conn, cx));
    wait_for(vcx, "the catalog", |vcx| {
        f.store.read_with(vcx, |s, _| {
            s.catalog(&conn).is_some_and(|c| c.schemas.iter().any(|s| s.tables.iter().any(|t| t.name == "t")))
        })
    });
    f.ws.update_in(vcx, |ws, window, cx| {
        ws.consoles[&a]
            .editor
            .update(cx, |e, cx| e.update_editor(cx, |ed| ed.set_text("select id, v from t")));
        ws.run_request(a, RunReq::All, window, cx)
    });
    wait_for(vcx, "the select", |vcx| {
        f.ws.read_with(vcx, |ws, _| ws.runs.is_empty() && ws.consoles[&a].results.len() == 1)
    });
    f.ws.read_with(vcx, |ws, _| {
        assert!(ws.consoles[&a].results[0].meta.editable.is_some(), "the result must be editable");
    });
    a
}

fn stage_edit(f: &Fixture, vcx: &mut VisualTestContext, tab: u64, text: &str) {
    let grid = f.ws.read_with(vcx, |ws, _| ws.consoles[&tab].grid.clone());
    // Column `v` of the first row.
    grid.update(vcx, |g, cx| g.stage_cell_text(0, 1, text, cx)).unwrap();
    assert!(grid.read_with(vcx, |g, _| g.has_pending_changes()));
}

#[gpui_kit::test]
fn an_edit_applied_while_another_console_is_active_lands_in_its_own_grid(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let a = editable_console(&f, vcx);
    let b = open(&f, vcx, &f.conn_b.clone(), "select 'from b' as who");
    run(&f, vcx, b);
    wait_for(vcx, "B", |vcx| run_finished(&f, vcx, b));
    activate(&f, vcx, a);
    stage_edit(&f, vcx, a, "ONE");

    // Submit; the server holds the statements, and the user switches to B meanwhile.
    f.hold_all.store(true, std::sync::atomic::Ordering::SeqCst);
    let grid_a = f.ws.read_with(vcx, |ws, _| ws.consoles[&a].grid.clone());
    grid_a.update(vcx, |g, cx| g.submit_changes(cx));
    activate(&f, vcx, b);
    assert_eq!(shown(&f, vcx).as_deref(), Some("from b"));
    f.gate.set(false);
    wait_for(vcx, "the apply to finish", |vcx| !grid_a.read_with(vcx, |g, _| g.has_pending_changes()));

    // B's grid is untouched; A's grid has the edit folded in and no pending changes.
    assert_eq!(shown(&f, vcx).as_deref(), Some("from b"));
    let a_rows = grid_a.read_with(vcx, |g, _| g.result().map(|r| r.rows.clone()).unwrap());
    assert_eq!(a_rows[0][1], Value::Text("ONE".into()));
    f.ws.read_with(vcx, |ws, cx| {
        assert!(ws.results.read(cx).grid == ws.consoles[&b].grid);
        // The stored result is the folded one, so re-activating A does not reload stale rows.
        assert_eq!(ws.consoles[&a].results[0].result.rows[0][1], Value::Text("ONE".into()));
        assert_eq!(ws.messages.read(cx).entries().last().map(|m| m.kind), Some(MessageKind::Ok));
    });
    activate(&f, vcx, a);
    let a_rows =
        f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().unwrap().rows.clone());
    assert_eq!(a_rows[0][1], Value::Text("ONE".into()));
}

#[gpui_kit::test]
fn a_failed_apply_keeps_the_edit_and_reports_in_the_footer_and_messages(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let a = editable_console(&f, vcx);
    stage_edit(&f, vcx, a, "ONE");
    // The table disappears behind the grid's back (another session dropped it).
    let drop_tab = open(&f, vcx, &f.conn_a.clone(), "drop table t");
    run(&f, vcx, drop_tab);
    wait_for(vcx, "the drop", |vcx| run_finished(&f, vcx, drop_tab));
    activate(&f, vcx, a);

    let grid = f.ws.read_with(vcx, |ws, _| ws.consoles[&a].grid.clone());
    grid.update(vcx, |g, cx| g.submit_changes(cx));
    wait_for(vcx, "the error", |vcx| grid.read_with(vcx, |g, _| g.submit_error().is_some()));
    let err = grid.read_with(vcx, |g, _| g.submit_error().map(str::to_string)).unwrap();
    assert!(err.contains("no such table"), "server error in the footer: {err}");
    assert!(grid.read_with(vcx, |g, _| g.has_pending_changes()), "the edit stays staged for a retry");
    f.ws.read_with(vcx, |ws, cx| {
        let last = ws.messages.read(cx).entries().last().cloned().unwrap();
        assert_eq!(last.kind, MessageKind::Error);
        assert!(last.lines[0].starts_with("Changes rolled back"), "{:?}", last.lines);
        assert!(!ws.txn.contains(&f.conn_a), "the transaction was rolled back");
    });
}
