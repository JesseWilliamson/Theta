//! A console (and `--run`) that executes DDL must leave the explorer showing the result.
//!
//! The same code path serves every kind, so the embedded kinds run everywhere and PostgreSQL / MariaDB run
//! when the local test servers (see AGENTS.md) answer; otherwise those two print a skip message.

use super::run::RunReq;
use super::*;
use crate::connections::ConnectionStore;
use crate::connections::secrets::SecretStore;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::Arc;
use std::time::Duration;
use theta_core::{ConnectionConfig, DatabaseKind};

fn wait_for(vcx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    vcx.executor().allow_parking();
    for _ in 0..1000 {
        vcx.run_until_parked();
        if done(vcx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

fn has_table(store: &Entity<ConnectionStore>, conn: &str, vcx: &mut VisualTestContext, table: &str) -> bool {
    store.read_with(vcx, |s, _| {
        s.catalog(conn).is_some_and(|c| {
            c.schemas.iter().any(|sc| sc.tables.iter().any(|t| t.name.eq_ignore_ascii_case(table)))
        })
    })
}

/// Connect, run `script` in a new console the way `--run` does, and report whether the catalog the explorer
/// reads has `table` afterwards, with no manual refresh.
fn run_and_check(
    cx: &mut TestAppContext,
    config: ConnectionConfig,
    script: &str,
    table: &str,
    cleanup: &str,
) {
    let data = tempfile::tempdir().unwrap();
    cx.executor().allow_parking();
    cx.update(|cx| {
        theta_ui::init(cx);
        crate::keymap::bind(cx);
    });
    let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
    let store = cx.new(|_| store);
    let conn = store.update(cx, |s, cx| {
        let id = s.add_temporary(config, cx);
        s.connect(&id, cx);
        id
    });
    let (ws, vcx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::with_data_dir(store.clone(), data.path().to_path_buf(), window, cx);
        ws.quiet = true;
        ws
    });
    wait_for(vcx, "the connection", |vcx| {
        store.read_with(vcx, |s, _| s.state(&conn).is_connected() && s.catalog(&conn).is_some())
    });
    assert!(!has_table(&store, &conn, vcx, table), "{table} must not exist before the run");
    let id = ws.update_in(vcx, |ws, window, cx| {
        let id = ws.open_console(Some(conn.clone()), Some(script.to_string()), window, cx);
        ws.run_request(id, RunReq::All, window, cx);
        id
    });
    let _ = id;
    wait_for(vcx, "the explorer's catalog to show the new table", |vcx| has_table(&store, &conn, vcx, table));
    // Clean up through the same session.
    ws.update_in(vcx, |ws, window, cx| {
        let id = ws.open_console(Some(conn.clone()), Some(cleanup.to_string()), window, cx);
        ws.run_request(id, RunReq::All, window, cx);
    });
    wait_for(vcx, "the cleanup", |vcx| !has_table(&store, &conn, vcx, table));
}

#[gpui_kit::test]
fn sqlite_console_ddl_refreshes_the_catalog(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = ConnectionConfig::new("t", "t", DatabaseKind::Sqlite);
    c.path = dir.path().join("t.db").to_string_lossy().into_owned();
    run_and_check(cx, c, "create table ddl_refresh_t(a int)", "ddl_refresh_t", "drop table ddl_refresh_t");
}

#[cfg(feature = "duckdb")]
#[gpui_kit::test]
fn duckdb_console_ddl_refreshes_the_catalog(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = ConnectionConfig::new("t", "t", DatabaseKind::DuckDb);
    c.path = dir.path().join("t.duckdb").to_string_lossy().into_owned();
    run_and_check(cx, c, "create table ddl_refresh_t(a int)", "ddl_refresh_t", "drop table ddl_refresh_t");
}

#[gpui_kit::test]
fn postgres_console_ddl_refreshes_the_catalog(cx: &mut TestAppContext) {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let mut c = ConnectionConfig::new("t", "t", DatabaseKind::Postgres);
    (c.host, c.port, c.user, c.database) =
        ("127.0.0.1".into(), 5432, "postgres".into(), "quarry_demo".into());
    c.password = Some("postgres".into());
    // The statement is split into a script, so a second DDL also proves one refresh covers the run.
    run_and_check(
        cx,
        c,
        "drop table if exists public.ddl_refresh_t; create table public.ddl_refresh_t(a int)",
        "ddl_refresh_t",
        "drop table public.ddl_refresh_t",
    );
}

#[gpui_kit::test]
fn mysql_console_ddl_refreshes_the_catalog(cx: &mut TestAppContext) {
    if !reachable(3306) {
        eprintln!("skipped: no MariaDB on 127.0.0.1:3306");
        return;
    }
    let mut c = ConnectionConfig::new("t", "t", DatabaseKind::MySql);
    (c.host, c.port, c.user, c.database) = ("127.0.0.1".into(), 3306, "quarry".into(), "quarry_demo".into());
    c.password = Some("quarry".into());
    run_and_check(
        cx,
        c,
        "drop table if exists ddl_refresh_t; create table ddl_refresh_t(a int)",
        "ddl_refresh_t",
        "drop table ddl_refresh_t",
    );
}
