//! The ER diagram tab inside a workspace: opening (and reopening), the `:diagram` argument, keyboard
//! selection and Enter opening the table's rows. The graph, the layout and the exporters are tested in
//! `theta-diagram`; this checks the wiring.

use super::*;
use crate::connections::ConnectionStore;
use crate::connections::secrets::SecretStore;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::Arc;
use std::time::Duration;
use theta_core::{Catalog, ColumnInfo, ConnectionConfig, DatabaseKind, ForeignKey, Schema, Table, TypeKind};

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

fn table(name: &str, cols: &[&str], fks: Vec<ForeignKey>) -> Table {
    Table {
        name: name.into(),
        columns: cols
            .iter()
            .map(|c| ColumnInfo {
                name: c.to_string(),
                data_type: "integer".into(),
                kind: TypeKind::Integer,
                ..Default::default()
            })
            .collect(),
        primary_key: vec!["id".into()],
        foreign_keys: fks,
        ..Default::default()
    }
}

fn catalog() -> Catalog {
    let fk = |col: &str, to: &str| ForeignKey {
        name: format!("{col}_fk"),
        columns: vec![col.into()],
        referenced_table: to.into(),
        referenced_columns: vec!["id".into()],
        ..Default::default()
    };
    Catalog {
        schemas: vec![Schema {
            name: "main".into(),
            tables: vec![
                table("customers", &["id"], vec![]),
                table("orders", &["id", "customer_id"], vec![fk("customer_id", "customers")]),
                table("items", &["id", "order_id"], vec![fk("order_id", "orders")]),
            ],
            ..Default::default()
        }],
        default_schema: Some("main".into()),
    }
}

struct Fx {
    ws: Entity<Workspace>,
    conn: String,
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
    let (data, st) = (dir.path().to_path_buf(), store.clone());
    let (ws, vcx) = cx.add_window_view(move |window, cx| {
        let mut ws = Workspace::with_data_dir(st, data, window, cx);
        ws.quiet = true;
        ws
    });
    let mut real = ConnectionConfig::new("real", "real", DatabaseKind::Sqlite);
    real.path = ":memory:".into();
    let real_id = store.update(vcx, |s, cx| {
        let id = s.add_temporary(real, cx);
        s.connect(&id, cx);
        id
    });
    wait_for(vcx, "sqlite", |vcx| store.read_with(vcx, |s, _| s.state(&real_id).is_connected()));
    let conn = store.update(vcx, |s, cx| {
        let c = s.connection(&real_id).unwrap();
        s.set_catalog_synthetic(&real_id, catalog(), c, cx);
        real_id.clone()
    });
    (Fx { ws, conn, _dir: dir }, vcx)
}

fn tabs(f: &Fx, vcx: &mut VisualTestContext) -> Vec<(u64, String)> {
    f.ws.read_with(vcx, |ws, _| ws.tabs.iter().map(|t| (t.id, t.title.to_string())).collect())
}

fn diagram(f: &Fx, vcx: &mut VisualTestContext, id: u64) -> Entity<crate::er::DiagramView> {
    f.ws.read_with(vcx, |ws, _| match ws.tool_tab(id) {
        Some(ToolTab::Diagram(v)) => v,
        None => panic!("tab {id} is not a diagram"),
    })
}

#[gpui_kit::test]
fn opening_a_diagram_twice_reuses_the_tab(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    let scope = theta_diagram::Scope::Schema("main".into());
    let a = f.ws.update_in(vcx, |ws, w, cx| ws.open_diagram(f.conn.clone(), scope.clone(), w, cx)).unwrap();
    let n = tabs(&f, vcx).len();
    let b = f.ws.update_in(vcx, |ws, w, cx| ws.open_diagram(f.conn.clone(), scope, w, cx)).unwrap();
    assert_eq!(a, b);
    assert_eq!(tabs(&f, vcx).len(), n);
    assert!(tabs(&f, vcx).iter().any(|(id, t)| *id == a && t == "Diagram: main"));
    let v = diagram(&f, vcx, a);
    v.read_with(vcx, |v, _| {
        assert_eq!(v.graph().nodes.len(), 3);
        assert_eq!(v.graph().edges.len(), 2);
    });
}

#[gpui_kit::test]
fn the_colon_command_takes_a_schema_or_a_table(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    f.ws.update_in(vcx, |ws, w, cx| {
        ws.active_connection = Some(f.conn.clone());
        ws.diagram_from_palette("orders", w, cx);
    });
    let tabs_now = tabs(&f, vcx);
    let (id, title) = tabs_now.last().unwrap().clone();
    assert_eq!(title, "Diagram: orders");
    let v = diagram(&f, vcx, id);
    v.read_with(vcx, |v, _| {
        let names: Vec<_> = v.graph().nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["customers", "items", "orders"], "the table and its neighbours");
        assert_eq!(v.selected_label().as_deref(), Some("orders"));
    });
    let before = tabs(&f, vcx).len();
    f.ws.update_in(vcx, |ws, w, cx| ws.diagram_from_palette("nonexistent", w, cx));
    assert_eq!(tabs(&f, vcx).len(), before, "an unknown name opens nothing");
    f.ws.update_in(vcx, |ws, w, cx| ws.diagram_from_palette("main", w, cx));
    assert_eq!(tabs(&f, vcx).last().unwrap().1, "Diagram: main");
}

#[gpui_kit::test]
fn tab_selects_and_enter_opens_the_table(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    let id =
        f.ws.update_in(vcx, |ws, w, cx| {
            ws.open_diagram(f.conn.clone(), theta_diagram::Scope::Schema("main".into()), w, cx)
        })
        .unwrap();
    vcx.run_until_parked();
    let v = diagram(&f, vcx, id);
    vcx.simulate_keystrokes("tab");
    // Tables are walked left to right: the referenced table first.
    assert_eq!(v.read_with(vcx, |v, _| v.selected_label()).as_deref(), Some("customers"));
    vcx.simulate_keystrokes("tab");
    assert_eq!(v.read_with(vcx, |v, _| v.selected_label()).as_deref(), Some("orders"));
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(v.read_with(vcx, |v, _| v.selected_label()).as_deref(), Some("customers"));
    let before = tabs(&f, vcx).len();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let after = tabs(&f, vcx);
    assert_eq!(after.len(), before + 1, "Enter opens the table's rows in a console");
    assert_eq!(after.last().unwrap().1, "customers");
}

#[gpui_kit::test]
fn the_jump_box_selects_by_fuzzy_name(cx: &mut TestAppContext) {
    let (f, vcx) = boot(cx);
    let id =
        f.ws.update_in(vcx, |ws, w, cx| {
            ws.open_diagram(f.conn.clone(), theta_diagram::Scope::Schema("main".into()), w, cx)
        })
        .unwrap();
    vcx.run_until_parked();
    let v = diagram(&f, vcx, id);
    vcx.simulate_keystrokes("/ i t m enter");
    assert_eq!(v.read_with(vcx, |v, _| v.selected_label()).as_deref(), Some("items"));
    vcx.simulate_keystrokes("escape");
    assert_eq!(v.read_with(vcx, |v, _| v.selected_label()), None, "escape clears the selection");
}
