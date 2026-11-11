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
    setup_with(cx, true, ":memory:")
}

/// `connect`: whether the SQLite connection is brought up before the console opens (a
/// restored console finds its connection disconnected).
fn setup_with<'a>(
    cx: &'a mut TestAppContext,
    connect: bool,
    path: &str,
) -> (Fixture, &'a mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.executor().allow_parking();
    cx.update(|cx| {
        quarry_ui::init(cx);
        crate::keymap::bind(cx);
    });
    let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
    let store = cx.new(|_| store);
    let mut config = ConnectionConfig::new("t1", "test", DatabaseKind::Sqlite);
    config.path = path.into();
    let id = store.update(cx, |s, cx| {
        let id = s.add_temporary(config, cx);
        if connect {
            s.connect(&id, cx);
        }
        id
    });
    let (ws, vcx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
        ws.quiet = true;
        ws
    });
    for _ in 0..500 {
        vcx.run_until_parked();
        if !connect || store.read_with(vcx, |s, _| s.state(&id).is_connected()) {
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
            crate::keymap::ownership::enter_mode(e, state.keymap, state.mode);
            if state.pending {
                press(e, if state.mode == Mode::Normal { "d" } else { "g" });
            }
            if state.prompt {
                match (state.keymap, state.mode) {
                    (Keymap::Standard, _) => press(e, "ctrl-f"),
                    (Keymap::Helix, Mode::Select) => press(e, "/"),
                    (Keymap::Vim, m) if m != Mode::Normal => press(e, "/"),
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

/// The platform's primary modifier plus `key`: Cmd on macOS, Ctrl elsewhere.
/// App-level shortcuts are bound to it (`mod-…` in the keymap), so tests that
/// exercise them must press what a user on that platform would press.
fn primary(key: &str) -> String {
    format!("{}-{key}", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" })
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
    EditorState { keymap: Keymap::Helix, mode, prompt: false, completion: false, pending: false }
}

fn standard() -> EditorState {
    EditorState { keymap: Keymap::Standard, mode: Mode::Standard, prompt: false, completion: false, pending: false }
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
fn completion_popup_owns_ctrl_n_and_ctrl_p_in_every_keymap(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    for state in [
        EditorState { completion: true, ..helix(Mode::Insert) },
        EditorState { completion: true, ..vim(Mode::Insert) },
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
    press(vcx, &primary("n"));
    assert_eq!(tab_count(&f, vcx), tabs + 1, "one ctrl-n opens exactly one console");
    // The new console is empty: closing it leaves nothing to reopen. Go back to the first.
    press(vcx, &primary("w"));
    assert_eq!(tab_count(&f, vcx), tabs, "one ctrl-w closes exactly one console");
    enter(&f, vcx, standard(), "select 42;", 4);
    press(vcx, &primary("w"));
    assert_eq!(tab_count(&f, vcx), tabs - 1);
    press(vcx, &primary("shift-t"));
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
        press(vcx, &primary("w"));
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
    press(vcx, &primary("shift-t"));
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

fn wait_for(vcx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    for _ in 0..800 {
        vcx.run_until_parked();
        if done(vcx) {
            return;
        }
        // The workspace polls the connection state on a timer, which a test clock only fires
        // when advanced.
        vcx.executor().advance_clock(std::time::Duration::from_millis(50));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn messages(f: &Fixture, vcx: &mut VisualTestContext) -> Vec<String> {
    f.ws.read_with(vcx, |ws, cx| ws.messages.read(cx).entries().iter().map(|m| m.plain_text()).collect())
}

/// A restored console finds its connection disconnected. Run must connect, wait and then run;
/// it used to connect and leave the user to press Run a second time.
#[gpui_kit::test]
fn run_on_a_disconnected_console_connects_waits_and_runs(cx: &mut TestAppContext) {
    let (f, vcx) = setup_with(cx, false, ":memory:");
    let id = f.ws.read_with(vcx, |ws, _| ws.tabs[0].connection_id.clone().unwrap());
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    assert!(!store.read_with(vcx, |s, _| s.state(&id).is_connected()), "starts disconnected");
    let editor = editor_of(&f, vcx);
    editor.update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 5 as five")));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    wait_for(vcx, "the result", |vcx| {
        f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some())
    });
    assert!(store.read_with(vcx, |s, _| s.state(&id).is_connected()));
}

/// Explain takes the same path (SQLite has no plan support, but it must get as far as asking).
#[gpui_kit::test]
fn explain_on_a_disconnected_console_connects_first(cx: &mut TestAppContext) {
    let (f, vcx) = setup_with(cx, false, ":memory:");
    let id = f.ws.read_with(vcx, |ws, _| ws.tabs[0].connection_id.clone().unwrap());
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    let editor = editor_of(&f, vcx);
    editor.update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 5 as five")));
    f.ws.update_in(vcx, |ws, window, cx| ws.explain_active(false, window, cx));
    wait_for(vcx, "the connection", |vcx| store.read_with(vcx, |s, _| s.state(&id).is_connected()));
    wait_for(vcx, "the explain outcome", |vcx| {
        f.ws.read_with(vcx, |ws, _| ws.consoles.values().any(|c| c.last_explain.is_some()))
    });
}

#[gpui_kit::test]
fn run_on_a_connection_that_cannot_connect_shows_the_error_in_messages(cx: &mut TestAppContext) {
    let (f, vcx) = setup_with(cx, false, "/nonexistent-dir/quarry/none.db");
    let editor = editor_of(&f, vcx);
    editor.update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 1")));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::Statement, window, cx));
    wait_for(vcx, "the error", |vcx| messages(&f, vcx).iter().any(|m| m.contains("Could not connect")));
    assert!(f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_none()));
}

/// Settings warnings (a hand-edited, invalid settings.json) exist before the window's `Root`
/// does. Showing them while the workspace is being built panicked at start-up.
#[gpui_kit::test]
fn start_up_warnings_do_not_panic_the_constructor(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        quarry_ui::init(cx);
        quarry_ui::Settings::warn(cx, "settings.json is not valid JSON; using defaults");
    });
    let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
    let store = cx.new(|_| store);
    let (ws, vcx) = cx.add_window_view(|window, cx| {
        Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx)
    });
    let held = ws.read_with(vcx, |ws, _| ws.startup_warnings.clone());
    assert_eq!(held, ["settings.json is not valid JSON; using defaults"]);
    ws.update_in(vcx, |ws, window, cx| {
        ws.quiet = true;
        ws.show_startup_warnings(window, cx);
    });
    assert!(ws.read_with(vcx, |ws, _| ws.startup_warnings.is_empty()), "shown once");
}

/// A statement that runs until it is cancelled (SQLite has no sleep).
const FOREVER: &str = "with recursive c(x) as (select 1 union all select x + 1 from c where x < 4000000000) select count(*) from c";

fn running(f: &Fixture, vcx: &mut VisualTestContext) -> bool {
    f.ws.read_with(vcx, |ws, _| !ws.runs.is_empty())
}

#[gpui_kit::test]
fn closing_a_tab_while_its_query_runs_cancels_it(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    editor_of(&f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text(FOREVER)));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    wait_for(vcx, "the query to start", |vcx| running(&f, vcx));
    let id = f.ws.read_with(vcx, |ws, _| ws.active_tab.unwrap());
    f.ws.update(vcx, |ws, cx| ws.close_tab(id, cx));
    // The run must end by itself (cancelled), leaving the connection usable for the next one.
    wait_for(vcx, "the cancelled run to finish", |vcx| !running(&f, vcx));
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    assert!(store.read_with(vcx, |s, _| s.first_connected().is_some()), "still connected");
}

#[gpui_kit::test]
fn rapid_run_and_cancel_leaves_a_working_console(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    editor_of(&f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text(FOREVER)));
    for _ in 0..15 {
        f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
        // A second Run while the first is going is refused, and Cancel right after is fine.
        f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
        f.ws.update_in(vcx, |ws, window, cx| ws.cancel_active(window, cx));
        f.ws.update_in(vcx, |ws, window, cx| ws.cancel_active(window, cx));
        wait_for(vcx, "the run to end", |vcx| !running(&f, vcx));
    }
    editor_of(&f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 3 as three")));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    wait_for(vcx, "the final result", |vcx| {
        f.ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some())
    });
}

/// Disconnect while a statement runs: the statement fails because of the disconnect, and that
/// must not be "recovered" by silently reconnecting against the user's wish.
#[gpui_kit::test]
fn disconnecting_while_a_query_runs_does_not_reconnect(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    editor_of(&f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text(FOREVER)));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    wait_for(vcx, "the query to start", |vcx| running(&f, vcx));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let store = f.ws.read_with(vcx, |ws, _| ws.store.clone());
    let id = f.ws.read_with(vcx, |ws, _| ws.tabs[0].connection_id.clone().unwrap());
    store.update(vcx, |s, cx| s.disconnect(&id, cx));
    f.ws.update_in(vcx, |ws, window, cx| ws.cancel_active(window, cx));
    wait_for(vcx, "the run to end", |vcx| !running(&f, vcx));
    for _ in 0..20 {
        vcx.run_until_parked();
        vcx.executor().advance_clock(std::time::Duration::from_millis(100));
    }
    assert!(
        matches!(
            store.read_with(vcx, |s, _| s.state(&id).clone()),
            crate::connections::ConnState::Disconnected
        ),
        "the connection stays disconnected"
    );
}

// ---- Vim keymap ----------------------------------------------------------------------------------------

fn vim(mode: Mode) -> EditorState {
    EditorState { keymap: Keymap::Vim, mode, prompt: false, completion: false, pending: false }
}

/// The first cell of the results grid as text.
fn first_cell(f: &Fixture, vcx: &mut VisualTestContext) -> Option<String> {
    f.ws.read_with(vcx, |ws, cx| {
        ws.results.read(cx).grid.read(cx).result().and_then(|r| r.rows.first()?.first().map(|v| format!("{v:?}")))
    })
}

fn keys(f: &Fixture, vcx: &mut VisualTestContext, keys: &[&str]) {
    editor_of(f, vcx).update_in(vcx, |v, _, cx| {
        v.update_editor(cx, |e| {
            for k in keys {
                e.handle_key(&quarry_editor::KeyInput::parse(k).unwrap());
            }
        })
    });
}

#[gpui_kit::test]
fn ctrl_enter_runs_in_every_vim_mode_and_runs_the_selection_in_visual(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    // (state, extra keys to select the first line, which cell the run must produce)
    let cases: [(EditorState, &[&str], &str); 6] = [
        (vim(Mode::Normal), &[], "22"),
        (vim(Mode::Insert), &[], "22"),
        (vim(Mode::Replace), &[], "22"),
        (vim(Mode::Visual), &["$"], "11"),
        (vim(Mode::VisualLine), &[], "11"),
        (vim(Mode::VisualBlock), &["$"], "11"),
    ];
    for (n, (state, extra, want)) in cases.into_iter().enumerate() {
        let text = "select 11 as v;\nselect 22 as v;\n";
        // Cursor on the second line unless the case selects the first line.
        let cursor = if want == "22" { 20 } else { 0 };
        enter(&f, vcx, state, text, cursor);
        keys(&f, vcx, extra);
        f.ws.update(vcx, |ws, cx| ws.results.update(cx, |r, cx| r.grid.update(cx, |g, cx| g.clear(cx))));
        let tabs = tab_count(&f, vcx);
        f.fired.borrow_mut().clear();
        press(vcx, &primary("enter"));
        assert_eq!(f.fired.borrow().last().cloned().flatten(), None, "the engine owns ctrl-enter ({})", state.describe());
        wait_for(vcx, "the result", |vcx| first_cell(&f, vcx).is_some());
        let cell = first_cell(&f, vcx).unwrap();
        assert!(cell.contains(want), "{}: expected {want}, got {cell} (case {n})", state.describe());
        assert_eq!(tab_count(&f, vcx), tabs);
    }
}

#[gpui_kit::test]
fn vim_ctrl_w_never_closes_the_console_and_ctrl_shift_chords_stay_the_applications(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    for mode in [Mode::Normal, Mode::Insert, Mode::Visual, Mode::VisualBlock] {
        enter(&f, vcx, vim(mode), "select foo bar;", 14);
        let tabs = tab_count(&f, vcx);
        // The literal chord: ctrl-w is the engine's (window prefix, delete word) on every platform.
        press(vcx, "ctrl-w");
        // Insert mode deletes the word; the other modes have nothing bound. Never a tab close.
        assert_eq!(tab_count(&f, vcx), tabs, "{mode:?}");
    }
    // ctrl-shift-f is Format Document, not ctrl-f (page down): the application's in every mode.
    for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
        enter(&f, vcx, vim(mode), "select 1", 0);
        f.fired.borrow_mut().clear();
        press(vcx, &primary("shift-f"));
        assert_eq!(
            f.fired.borrow().last().cloned().flatten().as_deref(),
            Some("quarry::FormatDocument"),
            "{mode:?}"
        );
    }
}

#[gpui_kit::test]
fn vim_ctrl_keys_are_the_editors_while_the_app_keeps_its_own(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let many: String = (0..200).map(|i| format!("select {i};\n")).collect();
    enter(&f, vcx, vim(Mode::Normal), &many, many.len() - 1);
    let explorer = f.ws.read_with(vcx, |ws, _| ws.state.explorer_open);
    let line = |vcx: &mut VisualTestContext| editor_of(&f, vcx).read_with(vcx, |v, _| v.status().line);
    let before = line(vcx);
    press(vcx, "ctrl-b");
    assert!(line(vcx) < before, "ctrl-b pages up in Vim");
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.state.explorer_open), explorer, "the explorer must not toggle");
    // ctrl-o is the jumplist, not "open file"; ctrl-r redoes
    press(vcx, "ctrl-o");
    assert!(f.ws.read_with(vcx, |ws, _| ws.modal.is_none()));
}

#[gpui_kit::test]
fn the_keymap_switches_live_helix_vim_standard(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let editor = editor_of(&f, vcx);
    let km = |vcx: &mut VisualTestContext| editor.read_with(vcx, |v, _| (v.editor().keymap(), v.editor().mode()));
    assert_eq!(km(vcx), (Keymap::Helix, Mode::Normal));
    for (mode, want) in [
        (quarry_ui::KeymapMode::Vim, (Keymap::Vim, Mode::Normal)),
        (quarry_ui::KeymapMode::Standard, (Keymap::Standard, Mode::Standard)),
        (quarry_ui::KeymapMode::Helix, (Keymap::Helix, Mode::Normal)),
    ] {
        f.ws.update_in(vcx, |ws, window, cx| ws.set_keymap(mode, window, cx));
        vcx.run_until_parked();
        assert_eq!(km(vcx), want);
    }
    // The pickers' quick actions go through the same path.
    f.ws.update_in(vcx, |_, window, cx| window.dispatch_action(Box::new(crate::actions::UseVimKeymap), cx));
    vcx.run_until_parked();
    assert_eq!(km(vcx), (Keymap::Vim, Mode::Normal));
}

#[gpui_kit::test]
fn vim_equals_formats_only_the_selected_lines(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let text = "select  1  from a;\nselect  2  from b;\nselect  3  from c;\n";
    enter(&f, vcx, vim(Mode::Normal), text, 0);
    for c in ["V", "j", "="] {
        // Typed like a user would: the effect the engine returns must reach the workspace.
        vcx.update(|window, cx| window.input(c, cx));
        vcx.run_until_parked();
    }
    let out = editor_text(&f, vcx);
    assert!(out.ends_with("select  3  from c;\n"), "the third line is untouched:\n{out}");
    assert_ne!(out, text, "the first two lines were formatted");
    assert!(!out.contains("select  1  from a;"), "{out}");
}
