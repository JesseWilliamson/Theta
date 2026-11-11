//! Who wins a keystroke: the application or the SQL editor?
//!
//! GPUI resolves key bindings before the focused view sees its key-down, so an application
//! binding on a key the editor also uses silently steals it (`ctrl-w` closing the tab while
//! the user deletes a word). `keymap::bind` guards every global binding with a context
//! predicate derived from the engine (see `keymap::ownership`); these tests press the real
//! keystrokes through GPUI's dispatch in a real workspace and check, for both keymaps and
//! every mode / prompt / popup state, that exactly one side handled each of them.

use super::*;
use crate::connections::secrets::SecretStore;
use crate::keymap::ownership::{EditorState, engine_key, engine_owns};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use quarry_core::{ConnectionConfig, DatabaseKind};
use quarry_editor::{Keymap, Mode};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct Fixture {
    ws: Entity<Workspace>,
    fired: Rc<RefCell<Vec<Option<String>>>>,
    _sub: Subscription,
    _dir: tempfile::TempDir,
}

fn setup(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.executor().allow_parking();
    cx.update(|cx| {
        quarry_ui::init(cx);
        crate::keymap::bind(cx);
    });
    let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
    let store = cx.new(|_| store);
    let mut config = ConnectionConfig::new("t1", "test", DatabaseKind::Sqlite);
    config.path = ":memory:".into();
    let id = store.update(cx, |s, cx| {
        let id = s.add_temporary(config, cx);
        s.connect(&id, cx);
        id
    });
    let (ws, vcx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
        ws.quiet = true;
        ws
    });
    for _ in 0..500 {
        vcx.run_until_parked();
        if store.read_with(vcx, |s, _| s.state(&id).is_connected()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // The action each keystroke resolved to (None: it fell through to the focused view).
    let fired: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
    let log = fired.clone();
    let sub = vcx.update(|_, cx| {
        cx.observe_keystrokes(move |ev, _, _| {
            log.borrow_mut().push(ev.action.as_ref().map(|a| a.name().to_string()));
        })
    });
    ws.update_in(vcx, |ws, window, cx| {
        ws.open_console(Some(id.clone()), Some(String::new()), window, cx);
    });
    (Fixture { ws, fired, _sub: sub, _dir: dir }, vcx)
}

fn editor_of(f: &Fixture, vcx: &mut VisualTestContext) -> Entity<quarry_editor_view::EditorView> {
    f.ws.read_with(vcx, |ws, _| ws.consoles[&ws.active_tab.expect("an active console")].editor.clone())
}

fn editor_text(f: &Fixture, vcx: &mut VisualTestContext) -> String {
    let e = editor_of(f, vcx);
    e.read_with(vcx, |e, _| e.text())
}

fn tab_count(f: &Fixture, vcx: &mut VisualTestContext) -> usize {
    f.ws.read_with(vcx, |ws, _| ws.tabs.len())
}

/// Put the focused editor into `state` with `text` and the cursor at `cursor` (chars).
fn enter(f: &Fixture, vcx: &mut VisualTestContext, state: EditorState, text: &str, cursor: usize) {
    f.ws.update_in(vcx, |ws, window, cx| {
        ws.close_modal(window, cx);
        if ws.active_tab.is_none() {
            ws.open_console(None, None, window, cx);
        }
        ws.focus_editor(window, cx);
    });
    let editor = editor_of(f, vcx);
    editor.update_in(vcx, |v, _, cx| {
        v.close_completion(cx);
        v.update_editor(cx, |e| {
            let esc = quarry_editor::KeyInput::parse("esc").unwrap();
            for _ in 0..3 {
                e.handle_key(&esc);
            }
            e.set_keymap(state.keymap);
            e.set_text(text);
            e.set_selections(vec![quarry_editor::Selection::point(cursor)], 0);
            let press = |e: &mut quarry_editor::Editor, k: &str| {
                e.handle_key(&quarry_editor::KeyInput::parse(k).unwrap());
            };
            match (state.keymap, state.mode) {
                (Keymap::Helix, Mode::Insert) => press(e, "i"),
                (Keymap::Helix, Mode::Select) => press(e, "v"),
                _ => {}
            }
            if state.prompt {
                match (state.keymap, state.mode) {
                    (Keymap::Standard, _) => press(e, "ctrl-f"),
                    (_, Mode::Select) => press(e, "/"),
                    _ => press(e, ":"),
                }
            }
        });
        if state.completion {
            v.request_completion(cx);
        }
    });
    vcx.update(|window, cx| window.refresh_and_render(cx));
    assert_eq!(
        editor.read_with(vcx, |v, _| (v.editor().keymap(), v.editor().mode(), v.editor().prompt().is_some())),
        (state.keymap, state.mode, state.prompt),
        "could not put the editor into {}",
        state.describe()
    );
    if state.completion {
        assert!(
            editor.read_with(vcx, |v, _| v.completion_open()),
            "{}: popup did not open",
            state.describe()
        );
    }
}

trait RefreshExt {
    fn refresh_and_render(&mut self, cx: &mut App);
}
impl RefreshExt for Window {
    fn refresh_and_render(&mut self, cx: &mut App) {
        self.render_frame(cx);
    }
}

fn press(vcx: &mut VisualTestContext, keystroke: &str) {
    vcx.update(|window, cx| window.press(keystroke, cx));
    vcx.run_until_parked();
}

/// Actions whose handler must not really run in a test (quit, native file dialogs): the
/// binding is only checked for being active in the editor's key context.
fn too_dangerous_to_press(action: &str) -> bool {
    matches!(
        action,
        "quarry::Quit" | "quarry::OpenSqlFile" | "quarry::OpenSettings" | "quarry::NewConnection"
    )
}

/// Text with a word at the cursor, so the completion popup has something to offer.
fn state_keys() -> (String, usize) {
    ("select 1;\ns".to_string(), 11)
}

/// For both keymaps and every editor state: each global application binding fires if and only
/// if the engine, the prompt or the popup does not use that keystroke.
#[gpui_kit::test]
fn every_global_binding_is_handled_by_exactly_one_side(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let bindings = crate::keymap::global_bindings();
    let os = quarry_ui::host_os();
    let mut checked = 0;
    let mut contested = 0;
    for state in EditorState::all() {
        if state.prompt && state.completion {
            continue; // the popup closes when a prompt opens
        }
        let (text, cursor) = state_keys();
        for b in &bindings {
            let strokes: Vec<String> = b.keystrokes().iter().map(|k| k.unparse(os)).collect();
            let action = b.action().name();
            let key = engine_key(&strokes[0]).unwrap_or_else(|| panic!("unparseable key {}", strokes[0]));
            let owned = engine_owns(state, &key);
            // `Explorer`-scoped bindings never apply while the editor is focused.
            let scoped_elsewhere = b.predicate().is_some_and(|p| p.to_string().starts_with("Explorer"));
            let expect_app = !owned && !scoped_elsewhere;
            contested += usize::from(owned);

            enter(&f, vcx, state, &text, cursor);
            if too_dangerous_to_press(action) {
                // Evaluated against the key contexts of the focused element, as GPUI does.
                let stack = vcx.update(|window, _| window.context_stack());
                let active = b.predicate().is_none_or(|p| p.depth_of(&stack).is_some());
                assert_eq!(active, expect_app, "{action} on {} in {}", strokes.join(" "), state.describe());
                checked += 1;
                continue;
            }
            f.fired.borrow_mut().clear();
            press(vcx, &strokes[0]);
            let first = f.fired.borrow().last().cloned().flatten();
            let chord = strokes.len() > 1;
            let who = format!("{} ({}) in {}", strokes.join(" "), action, state.describe());
            if chord {
                // The first stroke of a chord is "pending" exactly when the app binding is live.
                let pending = vcx.update(|window, _| window.has_pending_keystrokes());
                assert_eq!(pending, expect_app, "chord start: {who}");
                if pending {
                    press(vcx, &strokes[1]);
                    assert_eq!(
                        f.fired.borrow().last().cloned().flatten().as_deref(),
                        Some(action),
                        "chord end: {who}"
                    );
                }
            } else if expect_app {
                assert_eq!(
                    first.as_deref(),
                    Some(action),
                    "the application binding must fire: {who}; predicate {:?}; stack {:?}",
                    b.predicate().map(|p| p.to_string()),
                    vcx.update(|w, _| w.context_stack())
                );
            } else {
                assert_eq!(first, None, "the editor owns this key, the application must not fire: {who}");
            }
            checked += 1;
        }
    }
    assert!(checked > 300, "the table must really have been exercised ({checked})");
    assert!(contested > 40, "there must be many contested keystrokes ({contested})");
}

fn helix(mode: Mode) -> EditorState {
    EditorState { keymap: Keymap::Helix, mode, prompt: false, completion: false }
}

fn standard() -> EditorState {
    EditorState { keymap: Keymap::Standard, mode: Mode::Standard, prompt: false, completion: false }
}

#[gpui_kit::test]
fn helix_insert_ctrl_w_deletes_a_word_and_does_not_close_the_tab(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    enter(&f, vcx, helix(Mode::Insert), "select foo bar", 14);
    let tabs = tab_count(&f, vcx);
    press(vcx, "ctrl-w");
    assert_eq!(editor_text(&f, vcx), "select foo ");
    assert_eq!(tab_count(&f, vcx), tabs, "the console must survive ctrl-w");
}

#[gpui_kit::test]
fn helix_insert_ctrl_k_kills_to_line_end_without_waiting_for_a_chord(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    enter(&f, vcx, helix(Mode::Insert), "select foo bar", 6);
    press(vcx, "ctrl-k");
    assert_eq!(editor_text(&f, vcx), "select");
    assert!(!vcx.update(|window, _| window.has_pending_keystrokes()), "no pending ctrl-k chord");
}

#[gpui_kit::test]
fn helix_normal_ctrl_b_pages_up_and_ctrl_o_jumps_instead_of_app_actions(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let many: String = (0..200).map(|i| format!("select {i};\n")).collect();
    enter(&f, vcx, helix(Mode::Normal), &many, many.len() - 1);
    let explorer = f.ws.read_with(vcx, |ws, _| ws.state.explorer_open);
    let line_before = editor_of(&f, vcx).read_with(vcx, |v, _| v.status().line);
    press(vcx, "ctrl-b");
    let line_after = editor_of(&f, vcx).read_with(vcx, |v, _| v.status().line);
    assert!(line_after < line_before, "ctrl-b is page up in Helix ({line_before} -> {line_after})");
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.state.explorer_open), explorer, "explorer must not toggle");
    press(vcx, "ctrl-o");
    assert!(f.ws.read_with(vcx, |ws, _| ws.modal.is_none()));
}

#[gpui_kit::test]
fn completion_popup_owns_ctrl_n_and_ctrl_p_in_both_keymaps(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    for state in [
        EditorState { completion: true, ..helix(Mode::Insert) },
        EditorState { completion: true, ..standard() },
    ] {
        enter(&f, vcx, state, "s", 1);
        let tabs = tab_count(&f, vcx);
        let editor = editor_of(&f, vcx);
        let before = editor.read_with(vcx, |v, _| v.completion_selected());
        press(vcx, "ctrl-n");
        let after = editor.read_with(vcx, |v, _| v.completion_selected());
        assert_ne!(before, after, "ctrl-n moves the popup selection ({})", state.describe());
        press(vcx, "ctrl-p");
        assert_eq!(editor.read_with(vcx, |v, _| v.completion_selected()), before);
        assert_eq!(tab_count(&f, vcx), tabs, "no console may open ({})", state.describe());
        assert!(f.ws.read_with(vcx, |ws, _| ws.modal.is_none()), "no picker may open");
    }
}

#[gpui_kit::test]
fn standard_keymap_has_exactly_one_path_for_new_close_and_reopen(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    enter(&f, vcx, standard(), "select 42;", 4);
    let tabs = tab_count(&f, vcx);
    press(vcx, "ctrl-n");
    assert_eq!(tab_count(&f, vcx), tabs + 1, "one ctrl-n opens exactly one console");
    // The new console is empty: closing it leaves nothing to reopen. Go back to the first.
    press(vcx, "ctrl-w");
    assert_eq!(tab_count(&f, vcx), tabs, "one ctrl-w closes exactly one console");
    enter(&f, vcx, standard(), "select 42;", 4);
    press(vcx, "ctrl-w");
    assert_eq!(tab_count(&f, vcx), tabs - 1);
    press(vcx, "ctrl-shift-t");
    assert_eq!(tab_count(&f, vcx), tabs, "ctrl-shift-t reopens the closed console");
    assert_eq!(editor_text(&f, vcx), "select 42;");
    let cursor = editor_of(&f, vcx).read_with(vcx, |v, _| v.status().column);
    assert_eq!(cursor, 5, "the cursor comes back too");
}

#[gpui_kit::test]
fn f5_runs_all_in_the_editor_and_refreshes_in_the_explorer(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    for state in [helix(Mode::Normal), standard()] {
        enter(&f, vcx, state, "select 7 as seven", 0);
        f.fired.borrow_mut().clear();
        press(vcx, "f5");
        assert_eq!(f.fired.borrow().last().cloned().flatten(), None, "F5 reaches the engine");
        for _ in 0..500 {
            vcx.run_until_parked();
            if f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some()),
            "F5 must run the statement ({})",
            state.describe()
        );
    }
    // With the explorer focused F5 is Refresh Schema.
    f.ws.update_in(vcx, |ws, window, cx| ws.explorer.update(cx, |e, cx| e.focus(window, cx)));
    vcx.update(|window, cx| window.render_frame(cx));
    f.fired.borrow_mut().clear();
    press(vcx, "f5");
    assert_eq!(f.fired.borrow().last().cloned().flatten().as_deref(), Some("quarry::RefreshSchema"));
    // And the global chord works from the editor.
    enter(&f, vcx, helix(Mode::Insert), "x", 1);
    f.fired.borrow_mut().clear();
    press(vcx, &format!("{}-alt-r", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" }));
    assert_eq!(f.fired.borrow().last().cloned().flatten().as_deref(), Some("quarry::RefreshSchema"));
}

#[gpui_kit::test]
fn closing_a_console_keeps_it_on_a_persisted_reopen_stack(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let conn = f.ws.read_with(vcx, |ws, _| ws.tabs[0].connection_id.clone());
    assert!(conn.is_some());
    let closed_dir = crate::exec::consoles::closed_dir(f._dir.path());
    // Three consoles with distinct text, closed in order; the reopen order is the reverse.
    for (i, text) in ["select 1;", "select 2;", "select 3;"].iter().enumerate() {
        enter(&f, vcx, standard(), text, i + 1);
        press(vcx, "ctrl-w");
    }
    let on_disk = crate::exec::consoles::list_closed(&closed_dir);
    assert_eq!(
        on_disk.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        ["select 3;", "select 2;", "select 1;"]
    );
    assert!(on_disk.iter().all(|c| c.connection_id == conn), "the connection is kept");
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.tabs.len()), 0, "all three closed");

    // Via the `:reopen` command of the Helix keymap as well as the shortcut.
    enter(&f, vcx, helix(Mode::Normal), "", 0);
    let editor = editor_of(&f, vcx);
    editor.update_in(vcx, |v, window, cx| {
        v.update_editor(cx, |e| {
            for k in ":reopen".chars() {
                e.handle_key(&quarry_editor::KeyInput::ch(k));
            }
        });
        let _ = window;
    });
    press(vcx, "enter");
    assert_eq!(editor_text(&f, vcx), "select 3;");
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.active_console().and_then(|t| t.connection_id.clone())), conn);
    enter(&f, vcx, standard(), "select 3;", 3);
    press(vcx, "ctrl-shift-t");
    assert_eq!(editor_text(&f, vcx), "select 2;");
    assert_eq!(crate::exec::consoles::list_closed(&closed_dir).len(), 1);
}

#[derive(Clone, Copy, Debug)]
enum View {
    Grid,
    Plan,
    Map,
    Explorer,
    Picker,
}

/// A console with a finished query, so the results grid exists.
fn ensure_result(f: &Fixture, vcx: &mut VisualTestContext) {
    let has = |vcx: &mut VisualTestContext| {
        f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some())
    };
    if has(vcx) {
        return;
    }
    enter(f, vcx, standard(), "select 1 as one, 'x' as two", 0);
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    for _ in 0..500 {
        vcx.run_until_parked();
        if has(vcx) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the query never produced a result");
}

fn focus_view(f: &Fixture, vcx: &mut VisualTestContext, view: View) {
    ensure_result(f, vcx);
    f.ws.update_in(vcx, |ws, window, cx| {
        ws.close_modal(window, cx);
        if !ws.state.explorer_open {
            ws.toggle_explorer(cx);
        }
        match view {
            View::Grid => ws.focus_results(window, cx),
            View::Plan => {
                ws.set_bottom_tab(BottomTab::Plan, cx);
                let h = ws.plan.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            }
            View::Map => {
                ws.set_bottom_tab(BottomTab::Map, cx);
                let h = ws.map.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            }
            View::Explorer => ws.explorer.update(cx, |e, cx| e.focus(window, cx)),
            View::Picker => ws.open_table_picker(window, cx),
        }
    });
    vcx.update(|window, cx| window.render_frame(cx));
}

/// The grid, map, plan panel, explorer and pickers each own some keys while they have focus.
/// For every global application binding: if the focused view owns the keystroke the view gets
/// it (the application must stay out), otherwise the application's binding fires.
#[gpui_kit::test]
fn focused_views_win_their_own_keys_and_leave_the_rest_to_the_application(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let os = quarry_ui::host_os();
    let scopes = crate::keymap::scopes();
    let mut owned_seen = Vec::new();
    for view in [View::Grid, View::Plan, View::Map, View::Explorer, View::Picker] {
        let context = match view {
            View::Grid => Some("ResultGrid"),
            View::Explorer => Some("Explorer"),
            View::Picker => Some("Picker"),
            View::Plan | View::Map => None,
        };
        for b in crate::keymap::global_bindings() {
            let strokes: Vec<String> = b.keystrokes().iter().map(|k| k.unparse(os)).collect();
            let action = b.action().name();
            if too_dangerous_to_press(action) || strokes.len() > 1 {
                continue;
            }
            let view_owns =
                context.is_some_and(|c| scopes.iter().any(|s| s.context == c && s.owns(&strokes[0])));
            let scoped_elsewhere = b.predicate().is_some_and(|p| p.to_string().starts_with("Explorer"));
            // F5 is the explorer's own refresh binding.
            let expect_app = !view_owns && (!scoped_elsewhere || matches!(view, View::Explorer));
            focus_view(&f, vcx, view);
            let want = match view {
                View::Grid => "ResultGrid",
                View::Plan => "PlanPanel",
                View::Map => "QuarryMap",
                View::Explorer => "Explorer",
                View::Picker => "Picker",
            };
            let stack = vcx.update(|w, _| w.context_stack());
            assert!(stack.iter().any(|c| c.contains(want)), "{view:?} must be focused, stack {stack:?}");
            f.fired.borrow_mut().clear();
            press(vcx, &strokes[0]);
            let first = f.fired.borrow().last().cloned().flatten();
            let who = format!("{} ({action}) with the {view:?} focused", strokes[0]);
            if view_owns {
                owned_seen.push(format!("{view:?}:{}", strokes[0]));
                assert!(
                    first.as_deref() != Some(action),
                    "the focused view owns this key, the application must not fire: {who}"
                );
            } else if expect_app {
                assert_eq!(
                    first.as_deref(),
                    Some(action),
                    "the application binding must fire: {who}; stack {:?}; predicate {:?}",
                    vcx.update(|w, _| w.context_stack()),
                    b.predicate().map(|p| p.to_string())
                );
            }
        }
    }
    // The conflicts this test is about: ctrl-0 (grid), ctrl-n / ctrl-p (explorer and pickers).
    let ctrl_or_cmd = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    assert!(owned_seen.contains(&format!("Grid:{ctrl_or_cmd}-0")), "{owned_seen:?}");
    assert!(owned_seen.contains(&format!("Grid:{ctrl_or_cmd}-enter")), "{owned_seen:?}");
    if !cfg!(target_os = "macos") {
        for v in ["Explorer", "Picker"] {
            assert!(owned_seen.contains(&format!("{v}:ctrl-n")), "{owned_seen:?}");
            assert!(owned_seen.contains(&format!("{v}:ctrl-p")), "{owned_seen:?}");
        }
    }
}
