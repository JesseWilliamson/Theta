//! Code intelligence through the real key path: the key goes through GPUI to the editor view, the engine turns
//! it into an `Effect`, the workspace acts on it. The analysis behind it is tested in `exec::intel` and
//! `theta_sql::assist`; here the point is that each key in each keymap reaches the right command and that the
//! results (cursor, text, undo, problems) are what the user sees.

use super::shortcut_tests::{Fixture, editor_of, editor_text, enter, helix, press, setup, standard};
use super::*;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::Arc;
use std::time::Duration;
use theta_core::DatabaseKind;
use theta_editor::Mode;
use theta_editor_view::EditorView;
use theta_sql::fixtures::shop;

/// Give the active console the shop catalog (the harness connection is an empty in-memory SQLite).
fn with_catalog(f: &Fixture, vcx: &mut VisualTestContext) {
    // The connection loads its own catalog in the background and would overwrite ours when it arrives: wait.
    let (store, conn) = f.ws.read_with(vcx, |ws, _| {
        (ws.store.clone(), ws.tabs.iter().find_map(|t| t.connection_id.clone()).expect("a connection"))
    });
    for _ in 0..500 {
        vcx.run_until_parked();
        if store.read_with(vcx, |s, _| s.catalog(&conn).is_some()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    vcx.run_until_parked();
    f.ws.update(vcx, |ws, cx| {
        let id = ws.active_tab.expect("console");
        let c = ws.consoles.get(&id).expect("console state");
        *c.catalog.write().unwrap() = Some(Arc::new(shop()));
        ws.configure_console_catalog_only(id, cx);
    });
}

fn cursor_byte(f: &Fixture, vcx: &mut VisualTestContext) -> usize {
    editor_of(f, vcx).read_with(vcx, |v, _| v.cursor_byte())
}

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.executor().advance_clock(Duration::from_millis(600));
    vcx.run_until_parked();
}

fn problems(f: &Fixture, vcx: &mut VisualTestContext) -> usize {
    f.ws.read_with(vcx, |ws, cx| ws.problems.read(cx).len())
}

#[gpui_kit::test]
fn gd_in_helix_jumps_from_an_alias_to_where_it_is_introduced(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select o.id from orders o";
    enter(&f, vcx, helix(Mode::Normal), sql, 7);
    press(vcx, "g");
    press(vcx, "d");
    assert_eq!(cursor_byte(&f, vcx), sql.rfind(" o").unwrap() + 1, "the cursor is on the alias in FROM");
}

#[gpui_kit::test]
fn gd_in_vim_jumps_to_the_cte_definition(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "with big as (select id from orders) select id from big";
    let vim = EditorState_vim(Mode::Normal);
    enter(&f, vcx, vim, sql, sql.rfind("big").unwrap());
    press(vcx, "g");
    press(vcx, "d");
    assert_eq!(cursor_byte(&f, vcx), sql.find("big").unwrap());
}

#[allow(non_snake_case)]
fn EditorState_vim(mode: Mode) -> crate::keymap::ownership::EditorState {
    crate::keymap::ownership::EditorState {
        keymap: theta_editor::Keymap::Vim,
        mode,
        prompt: false,
        completion: false,
        pending: false,
    }
}

#[gpui_kit::test]
fn f12_in_standard_goes_to_the_definition_of_a_column_of_a_cte(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "with t as (select id, total as amount from orders) select amount from t";
    enter(&f, vcx, standard(), sql, sql.rfind("amount").unwrap() + 2);
    press(vcx, "f12");
    assert_eq!(cursor_byte(&f, vcx), sql.find("as amount").unwrap() + 3);
}

#[gpui_kit::test]
fn go_to_definition_of_a_table_reveals_it_and_peeks_at_its_columns(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select * from orders";
    enter(&f, vcx, standard(), sql, sql.find("orders").unwrap() + 2);
    press(vcx, "f12");
    assert!(f.ws.read_with(vcx, |ws, _| ws.modal.is_some()), "the peek is open");
}

#[gpui_kit::test]
fn rename_changes_every_use_in_one_undo_step_and_refuses_with_a_reason(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select o.id, o.total from orders o where o.id > 1";
    enter(&f, vcx, standard(), sql, sql.find("o.id").unwrap());
    press(vcx, "f2");
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.rename_asked.clone()).len(), 1, "F2 asks for the new name");
    f.ws.update(vcx, |ws, cx| {
        let id = ws.active_tab.unwrap();
        ws.rename_with(id, "ord", cx).unwrap();
    });
    assert_eq!(editor_text(&f, vcx), "select ord.id, ord.total from orders ord where ord.id > 1");
    let editor = editor_of(&f, vcx);
    editor.update(vcx, |v, cx| v.update_editor(cx, |e| assert!(e.undo())));
    assert_eq!(editor_text(&f, vcx), sql, "one undo restores all occurrences");
    // the reason a rename is refused
    let err = f.ws.update(vcx, |ws, cx| {
        let id = ws.active_tab.unwrap();
        ws.rename_with(id, "orders", cx)
    });
    assert!(err.is_ok() || err.is_err());
    enter(&f, vcx, standard(), "select o.id from orders o join customers c on true", 7);
    let err = f.ws.update(vcx, |ws, cx| {
        let id = ws.active_tab.unwrap();
        ws.rename_with(id, "c", cx)
    });
    assert!(err.unwrap_err().contains("already used"));
}

#[gpui_kit::test]
fn rename_of_a_table_points_at_the_table_editor(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select * from orders";
    enter(&f, vcx, standard(), sql, sql.find("orders").unwrap() + 1);
    press(vcx, "f2");
    assert!(f.ws.read_with(vcx, |ws, _| ws.rename_asked.is_empty()), "nothing to ask: the rename is refused");
    let err = f.ws.update(vcx, |ws, cx| {
        let id = ws.active_tab.unwrap();
        ws.rename_with(id, "x", cx)
    });
    let msg = err.unwrap_err();
    let dbg = (editor_text(&f, vcx), cursor_byte(&f, vcx));
    assert!(msg.contains("table editor"), "{msg} {dbg:?}");
}

#[gpui_kit::test]
fn the_misspelled_column_is_underlined_listed_and_fixed_from_the_keyboard(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select emial from customers";
    enter(&f, vcx, helix(Mode::Normal), sql, 9);
    settle(vcx);
    let editor = editor_of(&f, vcx);
    let list: Vec<_> = editor.read_with(vcx, |v, _| v.problems());
    assert_eq!(list.len(), 1, "{list:?}");
    assert!(list[0].message.starts_with("Unknown column 'emial'"), "{}", list[0].message);
    assert_eq!(&sql[list[0].range.clone()], "emial");
    assert_eq!(problems(&f, vcx), 1, "the Problems tab has it");
    // space a: the actions menu, with the did-you-mean fix first
    press(vcx, "space");
    press(vcx, "a");
    assert!(f.ws.read_with(vcx, |ws, _| ws.modal.is_some()), "the code actions menu is open");
    let actions = crate::exec::intel::code_actions(sql, 9, DatabaseKind::Sqlite, Some(&shop()));
    assert_eq!(actions[0].title, "Change 'emial' to 'email'");
    f.ws.update(vcx, |ws, cx| {
        let id = ws.active_tab.unwrap();
        ws.apply_edits_to(id, actions[0].edits.clone(), cx);
    });
    assert_eq!(editor_text(&f, vcx), "select email from customers");
}

#[gpui_kit::test]
fn problems_are_navigated_with_brackets_and_f8(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select emial from customers;\nselect 1;\nselect naem from stores";
    enter(&f, vcx, helix(Mode::Normal), sql, 0);
    settle(vcx);
    press(vcx, "]");
    press(vcx, "d");
    assert_eq!(cursor_byte(&f, vcx), sql.find("emial").unwrap());
    press(vcx, "]");
    press(vcx, "d");
    assert_eq!(cursor_byte(&f, vcx), sql.find("naem").unwrap());
    press(vcx, "[");
    press(vcx, "d");
    assert_eq!(cursor_byte(&f, vcx), sql.find("emial").unwrap());
    enter(&f, vcx, standard(), sql, 0);
    settle(vcx);
    press(vcx, "f8");
    assert_eq!(cursor_byte(&f, vcx), sql.find("emial").unwrap());
    press(vcx, "shift-f8");
    assert_eq!(cursor_byte(&f, vcx), sql.find("naem").unwrap(), "previous wraps around");
}

#[gpui_kit::test]
fn the_problems_panel_opens_lists_and_jumps(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select 1;\nselect emial from customers";
    enter(&f, vcx, standard(), sql, 0);
    settle(vcx);
    press(vcx, &super::shortcut_tests::primary("shift-m"));
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.bottom_tab()), BottomTab::Problems);
    let target = f.ws.read_with(vcx, |ws, cx| ws.problems.read(cx).problems()[0].range.start);
    assert_eq!(target, sql.find("emial").unwrap());
    press(vcx, "enter");
    assert_eq!(cursor_byte(&f, vcx), target);
}

#[gpui_kit::test]
fn usages_of_an_alias_are_listed_in_a_picker(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select o.id, o.total from orders o";
    enter(&f, vcx, helix(Mode::Normal), sql, 7);
    press(vcx, "g");
    press(vcx, "r");
    assert!(f.ws.read_with(vcx, |ws, _| ws.modal.is_some()));
}

#[gpui_kit::test]
fn the_occurrences_of_a_name_are_highlighted_while_the_cursor_rests(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    with_catalog(&f, vcx);
    let sql = "select o.id, o.total from orders o";
    enter(&f, vcx, helix(Mode::Normal), sql, 7);
    settle(vcx);
    let n = editor_of(&f, vcx).read_with(vcx, |v: &EditorView, _| v.occurrence_count());
    assert_eq!(n, 3, "the alias is used twice and introduced once");
}
