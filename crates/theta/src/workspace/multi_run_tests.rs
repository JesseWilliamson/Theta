//! Multi-source run through the workspace: every connection answers on its own terms and a failure on one
//! neither stops nor hides the others.

use super::multi_run::Scope;
use super::shortcut_tests::{editor_of, setup, wait_for};
use super::*;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use theta_core::{ConnectionConfig, DatabaseKind, ExecOptions};

/// An in-memory SQLite connection, connected, with `setup_sql` run on it.
fn add(
    store: &Entity<ConnectionStore>,
    vcx: &mut VisualTestContext,
    name: &str,
    read_only: bool,
    setup_sql: &[&str],
) -> String {
    let mut cfg = ConnectionConfig::new(name, name, DatabaseKind::Sqlite);
    cfg.path = ":memory:".into();
    cfg.read_only = read_only;
    let id = store.update(vcx, |s, cx| {
        let id = s.add_temporary(cfg, cx);
        s.connect(&id, cx);
        id
    });
    wait_for(vcx, "the connection", |vcx| store.read_with(vcx, |s, _| s.state(&id).is_connected()));
    let conn = store.read_with(vcx, |s, _| s.connection(&id)).expect("connected");
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let sql: Vec<String> = setup_sql.iter().map(|s| s.to_string()).collect();
    // Dropping the handle detaches the task; the flag says it finished.
    drop(crate::runtime::spawn(async move {
        for s in sql {
            conn.execute(&s, &ExecOptions::default()).await.expect("seed");
        }
        flag.store(true, Ordering::SeqCst);
    }));
    wait_for(vcx, "the seed", |_| done.load(Ordering::SeqCst));
    id
}

fn summary_of(f: &super::shortcut_tests::Fixture, vcx: &mut VisualTestContext) -> Vec<Vec<String>> {
    f.ws.read_with(vcx, |ws, _| {
        let c = &ws.consoles[&ws.active_tab.unwrap()];
        assert_eq!(c.results[0].title, "Summary");
        c.results[0].result.rows.iter().map(|r| r.iter().map(|v| v.display()).collect()).collect()
    })
}

fn titles(f: &super::shortcut_tests::Fixture, vcx: &mut VisualTestContext) -> Vec<String> {
    f.ws.read_with(vcx, |ws, _| {
        ws.consoles[&ws.active_tab.unwrap()].results.iter().map(|r| r.title.clone()).collect()
    })
}

fn run(f: &super::shortcut_tests::Fixture, vcx: &mut VisualTestContext, sql: &str, conns: Vec<String>) {
    editor_of(f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text(sql)));
    let id = f.ws.read_with(vcx, |ws, _| ws.active_tab.unwrap());
    f.ws.update_in(vcx, |ws, window, cx| ws.run_on_several(id, conns, Scope::Script, window, cx));
    wait_for(vcx, "the summary", |vcx| {
        f.ws.read_with(vcx, |ws, _| ws.consoles[&id].results.first().is_some_and(|r| r.title == "Summary"))
    });
}

#[gpui_kit::test]
fn a_failure_on_one_connection_does_not_stop_or_hide_the_others(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    let alpha = add(
        &store,
        vcx,
        "alpha",
        false,
        &["create table t (n integer)", "insert into t values (1), (2), (3)"],
    );
    let beta = add(&store, vcx, "beta", false, &[]); // no table t
    let gamma = add(&store, vcx, "gamma", false, &["create table t (n integer)", "insert into t values (7)"]);
    run(&f, vcx, "select n from t order by n", vec![alpha, beta, gamma]);

    let s = summary_of(&f, vcx);
    // Connection, Product, Statements, Rows, Time, Status
    assert_eq!(
        (s[0][0].as_str(), s[0][2].as_str(), s[0][3].as_str(), s[0][5].as_str()),
        ("alpha", "1 of 1", "3", "ok")
    );
    assert_eq!((s[1][0].as_str(), s[1][2].as_str(), s[1][3].as_str()), ("beta", "0 of 1", ""));
    assert!(s[1][5].starts_with("error: ") && s[1][5].contains("no such table"), "{}", s[1][5]);
    assert_eq!(
        (s[2][0].as_str(), s[2][3].as_str(), s[2][5].as_str()),
        ("gamma", "1", "ok"),
        "gamma ran although beta failed"
    );
    // One tab per connection that returned rows, in the order chosen, after the summary.
    assert_eq!(titles(&f, vcx), ["Summary", "alpha · 3 rows", "gamma · 1 row"]);
}

#[gpui_kit::test]
fn a_read_only_connection_refuses_a_write_and_the_others_still_run_it(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    let writable = add(&store, vcx, "writable", false, &["create table t (n integer)"]);
    let locked = add(&store, vcx, "locked", true, &["create table t (n integer)"]);
    run(&f, vcx, "insert into t values (5)", vec![locked.clone(), writable.clone()]);

    let s = summary_of(&f, vcx);
    assert_eq!(s[0][0], "locked");
    assert!(s[0][5].starts_with("refused: This connection is read-only"), "{}", s[0][5]);
    assert_eq!(s[0][2], "0 of 1");
    assert!(s[0][4].is_empty(), "a refused connection has no time");
    assert_eq!(
        (s[1][0].as_str(), s[1][3].as_str(), s[1][5].as_str()),
        ("writable", "1", "ok"),
        "1 row affected"
    );

    // And the write really happened on the one that allows it, and not on the other: ask each.
    let mut check = |id: String| {
        let conn = store.read_with(vcx, |s, _| s.connection(&id)).unwrap();
        let out = Arc::new(std::sync::Mutex::new(None));
        let o = out.clone();
        drop(crate::runtime::spawn(async move {
            let r = conn.execute("select count(*) from t", &ExecOptions::default()).await.unwrap();
            *o.lock().unwrap() = Some(r.result.unwrap().rows[0][0].display());
        }));
        wait_for(vcx, "the count", |_| out.lock().unwrap().is_some());
        out.lock().unwrap().clone().unwrap()
    };
    assert_eq!(check(writable), "1");
    assert_eq!(check(locked), "0", "the read-only connection was never asked");
}

#[gpui_kit::test]
fn a_script_stops_at_its_first_error_on_that_connection_only(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    let with = add(&store, vcx, "with", false, &["create table t (n integer)"]);
    let without = add(&store, vcx, "without", false, &[]);
    run(&f, vcx, "insert into t values (1); select count(*) as c from t", vec![with, without]);
    let s = summary_of(&f, vcx);
    assert_eq!((s[0][2].as_str(), s[0][5].as_str()), ("2 of 2", "ok"));
    assert_eq!(s[1][2], "0 of 2", "the second statement did not run after the first failed");
    assert_eq!(titles(&f, vcx), ["Summary", "with · 1 row"]);
}

#[gpui_kit::test]
fn the_connections_are_free_again_afterwards(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    let a = add(&store, vcx, "a", false, &["create table t (n integer)"]);
    let b = add(&store, vcx, "b", false, &["create table t (n integer)"]);
    run(&f, vcx, "select 1", vec![a.clone(), b.clone()]);
    assert!(
        f.ws.read_with(vcx, |ws, _| !ws.runs.contains_key(&a) && !ws.runs.contains_key(&b)),
        "no connection is left marked busy"
    );
    run(&f, vcx, "select 2", vec![a, b]);
}
