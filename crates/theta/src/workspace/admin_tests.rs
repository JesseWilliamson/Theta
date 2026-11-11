//! Entry points, capability gating, key handling and the password guarantee of the administration tabs.
//!
//! Gating uses synthetic connections (a real SQLite session labelled as another product) so it runs
//! everywhere. The tests that need a server use the local PostgreSQL (see AGENTS.md) and print a skip
//! message when it does not answer.

use super::*;
use crate::admin::{AdminEvent, AdminKind};
use crate::connections::ConnectionStore;
use crate::connections::secrets::SecretStore;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::{Catalog, ConnectionConfig, DatabaseKind, Feature};
use theta_sql::admin::roles as roles_logic;

const SENTINEL: &str = "Pw-Sentinel-9f3c1d";

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

struct Fx {
    ws: Entity<Workspace>,
    store: Entity<ConnectionStore>,
    data: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn boot(cx: &mut TestAppContext) -> (Fx, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.executor().allow_parking();
    cx.update(|cx| {
        theta_ui::init(cx);
        crate::keymap::bind(cx);
    });
    let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
    let store = cx.new(|_| store);
    let data = dir.path().to_path_buf();
    let d = data.clone();
    let st = store.clone();
    let (ws, vcx) = cx.add_window_view(move |window, cx| {
        let mut ws = Workspace::with_data_dir(st, d, window, cx);
        ws.quiet = true;
        ws
    });
    (Fx { ws, store, data, _dir: dir }, vcx)
}

/// A connected session of a real SQLite database presented as `kind`, to test what the application decides
/// from the product alone. Nothing it opens ever reaches a server of that product.
fn synthetic(f: &Fx, vcx: &mut VisualTestContext, kind: DatabaseKind) -> String {
    let mut real = ConnectionConfig::new("real", "real", DatabaseKind::Sqlite);
    real.path = ":memory:".into();
    let real_id = f.store.update(vcx, |s, cx| {
        let id = s.add_temporary(real, cx);
        s.connect(&id, cx);
        id
    });
    wait_for(vcx, "sqlite", |vcx| f.store.read_with(vcx, |s, _| s.state(&real_id).is_connected()));
    if kind == DatabaseKind::Sqlite {
        return real_id;
    }
    let fake = ConnectionConfig::new(format!("fake-{kind:?}"), "fake", kind);
    f.store.update(vcx, |s, cx| {
        let id = s.add_temporary(fake, cx);
        let conn = s.connection(&real_id).unwrap();
        s.set_catalog_synthetic(&id, Catalog::default(), conn, cx);
        id
    })
}

fn tab_count(f: &Fx, vcx: &mut VisualTestContext) -> usize {
    f.ws.read_with(vcx, |ws, _| ws.tabs.len())
}

fn open(f: &Fx, vcx: &mut VisualTestContext, conn: &str, kind: AdminKind) -> Option<u64> {
    f.ws.update_in(vcx, |ws, window, cx| ws.open_admin_tab(conn.to_string(), kind, window, cx))
}

#[gpui_kit::test]
fn embedded_databases_refuse_sessions_and_roles(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    let conn = synthetic(&f, vcx, DatabaseKind::Sqlite);
    let before = tab_count(&f, vcx);
    assert_eq!(open(&f, vcx, &conn, AdminKind::Sessions), None);
    assert_eq!(open(&f, vcx, &conn, AdminKind::Roles), None);
    assert_eq!(tab_count(&f, vcx), before, "no tab is opened for a product that cannot do it");
    // The reason is the descriptor's, and says what the product is.
    let why = DatabaseKind::Sqlite.unsupported(Feature::Sessions).unwrap();
    assert!(why.contains("embedded"), "{why}");
    // The palette and `:` command paths go through the same gate.
    f.ws.update_in(vcx, |ws, window, cx| {
        ws.active_connection = Some(conn.clone());
        ws.admin_host_command("sessions", "", window, cx);
        ws.admin_host_command("roles", "", window, cx);
    });
    assert_eq!(tab_count(&f, vcx), before);
    // DuckDB is embedded too.
    assert!(DatabaseKind::DuckDb.unsupported(Feature::Roles).is_some());
}

#[gpui_kit::test]
fn each_product_gets_exactly_the_tools_it_can_do(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    // (kind, sessions, roles) as decided by the capability table.
    for (kind, sessions, roles) in [
        (DatabaseKind::ClickHouse, true, false),
        (DatabaseKind::SqlServer, true, true),
        (DatabaseKind::CockroachDb, false, false),
        (DatabaseKind::Redshift, false, false),
        (DatabaseKind::TiDb, false, false),
    ] {
        let conn = synthetic(&f, vcx, kind);
        let before = tab_count(&f, vcx);
        let s = open(&f, vcx, &conn, AdminKind::Sessions);
        let r = open(&f, vcx, &conn, AdminKind::Roles);
        assert_eq!(s.is_some(), sessions, "{kind:?} sessions");
        assert_eq!(r.is_some(), roles, "{kind:?} roles");
        assert_eq!(tab_count(&f, vcx), before + s.iter().count() + r.iter().count(), "{kind:?}");
        // Opening it again switches to the same tab instead of adding one.
        if let Some(id) = s {
            assert_eq!(open(&f, vcx, &conn, AdminKind::Sessions), Some(id));
        }
    }
    // Dumping is off for products whose DDL generator is not verified, and says so.
    for kind in
        [DatabaseKind::SqlServer, DatabaseKind::ClickHouse, DatabaseKind::CockroachDb, DatabaseKind::Redshift]
    {
        assert!(kind.unsupported(Feature::Dump).is_some(), "{kind:?}");
    }
    for kind in [DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::Sqlite, DatabaseKind::DuckDb] {
        assert!(kind.unsupported(Feature::Dump).is_none(), "{kind:?}");
    }
    let conn = synthetic(&f, vcx, DatabaseKind::SqlServer);
    let refused =
        f.ws.update_in(vcx, |ws, window, cx| ws.refuse_unsupported(&conn, Feature::Dump, window, cx));
    assert!(refused);
}

#[gpui_kit::test]
fn sql_server_roles_are_read_only_and_say_why(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    let conn = synthetic(&f, vcx, DatabaseKind::SqlServer);
    let tab = open(&f, vcx, &conn, AdminKind::Roles).expect("listing is allowed");
    let view = f.ws.read_with(vcx, |ws, _| ws.roles_view(tab)).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let _sub = vcx.update(|_, cx| {
        cx.subscribe(&view, move |_, ev: &AdminEvent, _| sink.lock().unwrap().push(ev.clone()))
    });
    for cmd in [crate::admin::keys::Cmd::New, crate::admin::keys::Cmd::Drop] {
        view.update_in(vcx, |v, window, cx| v.command(cmd, window, cx));
    }
    let seen = events.lock().unwrap().clone();
    let notices: Vec<&String> =
        seen.iter().filter_map(|e| if let AdminEvent::Notice(_, m) = e { Some(m) } else { None }).collect();
    assert_eq!(notices.len(), 2, "{seen:?}");
    assert!(notices.iter().all(|m| m.contains("read-only") || m.contains("lists SQL Server")), "{notices:?}");
    // And nothing was sent: the generator itself refuses.
    let r = roles_logic::Role { name: "x".into(), ..Default::default() };
    assert!(roles_logic::drop_role(DatabaseKind::SqlServer, &r).is_err());
}

/// Every file under `dir`, as text.
fn files_text(dir: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(t) = std::fs::read(&p) {
                out.push((p, String::from_utf8_lossy(&t).into_owned()));
            }
        }
    }
    out
}

fn pg_connect(f: &Fx, vcx: &mut VisualTestContext) -> String {
    let mut c = ConnectionConfig::new("pg", "pg", DatabaseKind::Postgres);
    (c.host, c.port, c.user, c.database) =
        ("127.0.0.1".into(), 5432, "postgres".into(), "quarry_demo".into());
    c.password = Some("postgres".into());
    let id = f.store.update(vcx, |s, cx| {
        let id = s.add_temporary(c, cx);
        s.connect(&id, cx);
        id
    });
    wait_for(vcx, "postgres", |vcx| {
        f.store.read_with(vcx, |s, _| s.state(&id).is_connected() && s.catalog(&id).is_some())
    });
    id
}

/// The password goes to the server and nowhere else: not the Messages log, not the history, not any file the
/// application writes, not an error that echoes the statement.
#[gpui_kit::test]
fn a_password_never_reaches_messages_history_or_files(cx: &mut TestAppContext) {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let (f, vcx) = boot(cx);
    let conn = pg_connect(&f, vcx);
    let tab = open(&f, vcx, &conn, AdminKind::Roles).expect("opens");
    let view = f.ws.read_with(vcx, |ws, _| ws.roles_view(tab)).unwrap();
    wait_for(vcx, "the roles", |vcx| view.read_with(vcx, |v, _| !v.roles().is_empty()));

    let name = format!("a7b_mask_{}", std::process::id());
    let create = roles_logic::create_role(
        DatabaseKind::Postgres,
        &roles_logic::NewRole {
            name: name.clone(),
            login: true,
            inherit: true,
            password: Some(SENTINEL.into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(create.sql.contains(SENTINEL), "the statement that runs does carry it");
    assert!(!create.shown.contains(SENTINEL) && create.shown.contains("'***'"));
    view.update(vcx, |v, cx| v.execute(create.clone(), cx));
    wait_for(vcx, "the role to exist", |vcx| {
        view.read_with(vcx, |v, _| v.roles().iter().any(|r| r.name == name))
    });
    // Changing the password, and a failing statement, take the same path.
    let me = view.read_with(vcx, |v, _| v.roles().iter().find(|r| r.name == name).cloned()).unwrap();
    let change = roles_logic::set_password(DatabaseKind::Postgres, &me, SENTINEL).unwrap();
    view.update(vcx, |v, cx| v.execute(change, cx));
    let ghost = roles_logic::Role { name: format!("{name}_ghost"), ..Default::default() };
    let failing = roles_logic::set_password(DatabaseKind::Postgres, &ghost, SENTINEL).unwrap();
    view.update(vcx, |v, cx| v.execute(failing, cx));
    wait_for(vcx, "three log entries", |vcx| {
        f.ws.read_with(vcx, |ws, cx| ws.messages.read(cx).entries().len() >= 3)
    });

    let texts: Vec<String> =
        f.ws.read_with(vcx, |ws, cx| ws.messages.read(cx).entries().iter().map(|m| m.plain_text()).collect());
    assert!(texts.iter().any(|t| t.contains("ALTER ROLE") && t.contains("PASSWORD '***'")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("does not exist")), "the failure is logged: {texts:?}");
    for t in &texts {
        assert!(!t.contains(SENTINEL), "Messages leaked the password: {t}");
    }
    for (p, text) in files_text(&f.data) {
        assert!(!text.contains(SENTINEL), "{} contains the password", p.display());
    }
    // Clean up the scratch role.
    let drop = roles_logic::drop_role(DatabaseKind::Postgres, &me).unwrap();
    view.update(vcx, |v, cx| v.execute(drop, cx));
    wait_for(vcx, "the role to be dropped", |vcx| {
        view.read_with(vcx, |v, _| !v.roles().iter().any(|r| r.name == name))
    });
}

/// The keys of the tab, in the keymap that is selected, change what the view shows.
#[gpui_kit::test]
fn keys_drive_the_sessions_tab(cx: &mut TestAppContext) {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let (f, vcx) = boot(cx);
    let conn = pg_connect(&f, vcx);
    let tab = open(&f, vcx, &conn, AdminKind::Sessions).expect("opens");
    let view = f.ws.read_with(vcx, |ws, _| ws.sessions_view(tab)).unwrap();
    wait_for(vcx, "the sessions", |vcx| view.read_with(vcx, |v, _| v.rows().len() >= 2));
    view.update_in(vcx, |v, window, cx| v.focus_list(window, cx));
    vcx.run_until_parked();
    let first = view.read_with(vcx, |v, _| v.selected_session().map(|s| s.id.clone()));
    let press = |vcx: &mut VisualTestContext, k: &str| {
        vcx.update(|window, cx| window.press(k, cx));
        vcx.run_until_parked();
    };
    press(vcx, "down");
    let second = view.read_with(vcx, |v, _| v.selected_session().map(|s| s.id.clone()));
    assert_ne!(first, second, "down moves the selection in every keymap");
    press(vcx, "up");
    assert_eq!(view.read_with(vcx, |v, _| v.selected_session().map(|s| s.id.clone())), first);
    assert!(!view.read_with(vcx, |v, _| v.is_paused()));
    press(vcx, "p");
    assert!(view.read_with(vcx, |v, _| v.is_paused()), "p pauses");
    press(vcx, "p");
    assert!(!view.read_with(vcx, |v, _| v.is_paused()));
    // Run (ctrl-enter, F5) in the tab refreshes it. That path runs inside a workspace update, where the view
    // must not read the workspace (it once did, and panicked).
    let ok = f.ws.update(vcx, |ws, cx| ws.refresh_admin_tab(tab, cx));
    assert!(ok);
    wait_for(vcx, "the refresh", |vcx| !view.read_with(vcx, |v, _| v.is_loading()));
}
