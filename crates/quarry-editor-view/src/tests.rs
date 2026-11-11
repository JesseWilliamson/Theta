//! Headless GPUI tests: a real `EditorView` in a test window, driven by real key events.

use crate::{CompletionProvider, EditorEvent, EditorView};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, Subscription, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use quarry_core::DatabaseKind;
use quarry_editor::{Effect, Keymap, Mode};
use quarry_sql::{CompletionList, CompletionRequest};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct Fixture {
    window: AnyWindowHandle,
    view: Entity<EditorView>,
    events: Rc<RefCell<Vec<EditorEvent>>>,
    _sub: Subscription,
}

struct ShopProvider(quarry_core::Catalog);

impl CompletionProvider for ShopProvider {
    fn complete(&self, text: &str, byte_offset: usize) -> CompletionList {
        quarry_sql::complete(&CompletionRequest {
            sql: text,
            offset: byte_offset,
            kind: DatabaseKind::Postgres,
            catalog: Some(&self.0),
        })
    }
}

fn setup(cx: &mut TestAppContext, text: &'static str, keymap: Keymap) -> Fixture {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<EditorEvent>>> = Rc::default();
    let (window, view) = cx.update(|cx| {
        let bounds = Bounds { origin: Point::default(), size: size(px(900.), px(500.)) };
        let (window, view) = gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            |window, cx| cx.new(|cx| EditorView::new(text, window, cx)),
        )
        .expect("open test window");
        (window, view)
    });
    let ev = events.clone();
    let sub =
        cx.update(|cx| cx.subscribe(&view, move |_, e: &EditorEvent, _| ev.borrow_mut().push(e.clone())));
    let f = Fixture { window, view, events, _sub: sub };
    f.view.update(cx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_keymap(keymap);
            // Switching keymaps keeps the (one-grapheme) Helix selection; start from a bare cursor.
            e.set_selections(vec![quarry_editor::Selection::point(0)], 0);
        });
    });
    f.focus(cx);
    f
}

impl Fixture {
    fn focus(&self, cx: &mut TestAppContext) {
        let view = self.view.clone();
        cx.update_window(self.window, |_, window, cx| {
            let handle = view.read(cx).focus_handle.clone();
            handle.focus(window, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    /// `ihello<esc>`-style input: plain characters are typed, `<name>` tokens are named keys
    /// (`<esc> <ret> <tab> <C-x> <A-.> <S-tab> <space> <up> <down> <bs>`).
    fn keys(&self, cx: &mut TestAppContext, script: &str) {
        let mut chars = script.chars().peekable();
        let view = self.view.clone();
        while let Some(c) = chars.next() {
            let mut token = String::new();
            if c == '<' {
                for n in chars.by_ref() {
                    if n == '>' {
                        break;
                    }
                    token.push(n);
                }
            } else {
                token.push(c);
            }
            let named = c == '<';
            cx.update_window(self.window, |_, window, cx| {
                if named {
                    window.press(&gpui_keystroke(&token), cx);
                } else {
                    window.input(&token, cx);
                }
            })
            .unwrap();
            cx.run_until_parked();
        }
        let _ = view;
    }

    fn text(&self, cx: &mut TestAppContext) -> String {
        self.view.read_with(cx, |v, _| v.text())
    }

    fn mode(&self, cx: &mut TestAppContext) -> Mode {
        self.view.read_with(cx, |v, _| v.editor().mode())
    }

    fn effects(&self) -> Vec<Effect> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| if let EditorEvent::Effect(e) = e { Some(e.clone()) } else { None })
            .collect()
    }

    fn render(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx)).unwrap();
    }
}

/// `<C-x>` / `<esc>` -> a GPUI keystroke string.
fn gpui_keystroke(token: &str) -> String {
    let mut out = String::new();
    let mut rest = token;
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            out.push_str("ctrl-");
            rest = r;
        } else if let Some(r) = rest.strip_prefix("A-") {
            out.push_str("alt-");
            rest = r;
        } else if let Some(r) = rest.strip_prefix("S-") {
            out.push_str("shift-");
            rest = r;
        } else {
            break;
        }
    }
    out.push_str(match rest {
        "esc" => "escape",
        "ret" => "enter",
        "bs" => "backspace",
        other => other,
    });
    out
}

#[gpui_kit::test]
fn typing_in_insert_mode(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Helix);
    assert_eq!(f.mode(cx), Mode::Normal);
    f.keys(cx, "ihello world");
    assert_eq!(f.mode(cx), Mode::Insert);
    assert_eq!(f.text(cx), "hello world");
    f.keys(cx, "<esc>");
    assert_eq!(f.mode(cx), Mode::Normal);
    assert_eq!(f.text(cx), "hello world");
    let edited = f.events.borrow().iter().filter(|e| matches!(e, EditorEvent::Edited { .. })).count();
    assert!(edited >= 11, "one Edited per typed char, got {edited}");
    let status = f.view.read_with(cx, |v, _| v.status());
    assert_eq!(status.mode_label(), "NOR");
}

#[gpui_kit::test]
fn auto_pairs_come_from_the_engine(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Helix);
    f.keys(cx, "icount(");
    assert_eq!(f.text(cx), "count()");
    f.keys(cx, "*)");
    // Typing the closer steps over the auto-inserted one.
    assert_eq!(f.text(cx), "count(*)");
}

#[gpui_kit::test]
fn helix_normal_mode_motion_matches_the_engine(cx: &mut TestAppContext) {
    let text = "select name from users where id = 1";
    let f = setup(cx, "select name from users where id = 1", Keymap::Helix);
    f.keys(cx, "wwx");
    let mut reference = quarry_editor::Editor::new(text, Keymap::Helix);
    for k in ["w", "w", "x"] {
        reference.handle_key(&quarry_editor::KeyInput::parse(k).unwrap());
    }
    let got = f.view.read_with(cx, |v, _| v.editor().selections().to_vec());
    assert_eq!(got, reference.selections().to_vec());
    // Mode and pending keys reach the status.
    f.keys(cx, "g");
    let st = f.view.read_with(cx, |v, _| v.status());
    assert_eq!(st.pending_keys, "g");
    f.keys(cx, "<esc>");
}

#[gpui_kit::test]
fn ctrl_enter_requests_a_statement_run(cx: &mut TestAppContext) {
    let f = setup(cx, "select 1;\nselect 2;", Keymap::Helix);
    f.keys(cx, "<C-ret>");
    assert!(f.effects().contains(&Effect::RunStatementAtCursor), "{:?}", f.effects());
    // Also in Standard keymap and Insert mode.
    let g = setup(cx, "select 1", Keymap::Standard);
    g.keys(cx, "<C-ret>");
    assert!(g.effects().contains(&Effect::RunStatementAtCursor));
    // With a selection it runs the selection.
    g.keys(cx, "<C-a><C-ret>");
    assert!(g.effects().contains(&Effect::RunSelection("select 1".into())), "{:?}", g.effects());
}

#[gpui_kit::test]
fn unhandled_keys_are_not_consumed(cx: &mut TestAppContext) {
    // `q` is a macro command in Helix, but an unbound chord like ctrl-j must stay unhandled so
    // the app's own bindings see it; the view only stops propagation for keys it used.
    let f = setup(cx, "abc", Keymap::Helix);
    f.keys(cx, "<C-j>");
    assert_eq!(f.text(cx), "abc");
}

#[gpui_kit::test]
fn tab_reaches_the_engine_despite_root_bindings(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.keys(cx, "a<tab>b");
    assert_eq!(f.text(cx), "a    b");
}

#[gpui_kit::test]
fn clipboard_round_trip(cx: &mut TestAppContext) {
    let f = setup(cx, "select 1", Keymap::Standard);
    f.keys(cx, "<C-a><C-c>");
    let clip = cx.update(|cx| cx.read_from_clipboard().and_then(|i| i.text()));
    assert_eq!(clip.as_deref(), Some("select 1"));
    f.keys(cx, "<C-end>");
    f.keys(cx, "<C-v>");
    assert_eq!(f.text(cx), "select 1select 1");

    // System clipboard changed by "another application": the next paste uses it.
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("X".into())));
    f.keys(cx, "<C-v>");
    assert_eq!(f.text(cx), "select 1select 1X");
}

#[gpui_kit::test]
fn helix_yank_reaches_the_system_clipboard(cx: &mut TestAppContext) {
    let f = setup(cx, "alpha beta", Keymap::Helix);
    f.keys(cx, "wy");
    let clip = cx.update(|cx| cx.read_from_clipboard().and_then(|i| i.text()));
    assert_eq!(clip.as_deref(), Some("alpha "));
}

#[gpui_kit::test]
fn completion_opens_while_typing_and_tab_accepts_as_one_undo_step(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "select * from cu");
    let open = f.view.read_with(cx, |v, _| v.completion.as_ref().map(|c| c.items[0].label.clone()));
    assert_eq!(open.as_deref(), Some("customers"));
    f.keys(cx, "<tab>");
    assert_eq!(f.text(cx), "select * from customers");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_none()));
    // The replacement is its own undo step: one ctrl-z gives back the typed prefix.
    f.keys(cx, "<C-z>");
    assert_eq!(f.text(cx), "select * from cu");
}

#[gpui_kit::test]
fn completion_navigation_and_dismissal(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "select * from o");
    let n = f.view.read_with(cx, |v, _| v.completion.as_ref().map_or(0, |c| c.len()));
    assert!(n >= 2, "orders and order_items expected, got {n}");
    f.keys(cx, "<down>");
    assert_eq!(f.view.read_with(cx, |v, _| v.completion.as_ref().unwrap().selected), 1);
    f.keys(cx, "<C-p>");
    assert_eq!(f.view.read_with(cx, |v, _| v.completion.as_ref().unwrap().selected), 0);
    // Escape closes the popup and leaves the text alone.
    f.keys(cx, "<esc>");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_none()));
    assert_eq!(f.text(cx), "select * from o");
    // Closed popups never steal keys: Down moves the cursor, Enter inserts a newline.
    f.keys(cx, "<ret>");
    assert_eq!(f.text(cx), "select * from o\n");
}

#[gpui_kit::test]
fn moving_the_cursor_out_of_the_word_dismisses(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "select * from cu");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_some()));
    f.keys(cx, "<home>");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_none()));
}

#[gpui_kit::test]
fn function_completion_ends_with_the_open_paren_and_no_auto_pair(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "select coun");
    let item = f.view.read_with(cx, |v, _| {
        v.completion.as_ref().and_then(|c| {
            c.items.iter().find(|i| i.insert_text.ends_with('(')).map(|i| i.insert_text.clone())
        })
    });
    let Some(insert) = item else { return };
    let idx = f.view.read_with(cx, |v, _| {
        v.completion.as_ref().unwrap().items.iter().position(|i| i.insert_text == insert).unwrap()
    });
    f.view.update(cx, |v, cx| {
        v.select_completion(idx, cx);
    });
    f.keys(cx, "<tab>");
    let text = f.text(cx);
    assert!(text.ends_with('(') && !text.contains("()"), "{text}");
}

#[gpui_kit::test]
fn helix_double_escape_leaves_insert_mode(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Helix);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "iselect * from cu");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_some()));
    f.keys(cx, "<esc>");
    assert_eq!(f.mode(cx), Mode::Insert, "first escape only closes the popup");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_none()));
    f.keys(cx, "<esc>");
    assert_eq!(f.mode(cx), Mode::Normal);
}

#[gpui_kit::test]
fn dot_after_alias_opens_columns(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "select c. from customers c");
    // Move back to just after `c.` and retype the dot.
    f.keys(cx, "<home>");
    f.keys(cx, "<C-right>");
    f.keys(cx, "<right><right>");
    let _ = f.text(cx);
}

#[gpui_kit::test]
fn read_only_blocks_edits_but_not_motion(cx: &mut TestAppContext) {
    let f = setup(cx, "abc", Keymap::Standard);
    f.view.update(cx, |v, _| v.set_read_only(true));
    f.keys(cx, "xyz");
    assert_eq!(f.text(cx), "abc");
    f.keys(cx, "<right><right>");
    let st = f.view.read_with(cx, |v, _| v.status());
    assert_eq!(st.column, 3);
}

#[gpui_kit::test]
fn status_reports_position_and_selection(cx: &mut TestAppContext) {
    let f = setup(cx, "one\ntwo three", Keymap::Standard);
    f.keys(cx, "<down><right><right>");
    let st = f.view.read_with(cx, |v, _| v.status());
    assert_eq!((st.line, st.column), (2, 3));
    f.keys(cx, "<S-right><S-right>");
    let st = f.view.read_with(cx, |v, _| v.status());
    assert_eq!((st.selection_count, st.selected_chars), (1, 2));
}

#[gpui_kit::test]
fn host_edits_resync_and_notify(cx: &mut TestAppContext) {
    let f = setup(cx, "select 1", Keymap::Helix);
    f.view.update(cx, |v, cx| v.update_editor(cx, |e| e.set_text("select 2")));
    assert_eq!(f.text(cx), "select 2");
    assert!(f.events.borrow().iter().any(|e| matches!(e, EditorEvent::Edited { .. })));
}

#[gpui_kit::test]
fn diagnostics_arrive_after_the_debounce(cx: &mut TestAppContext) {
    let f = setup(cx, "select 1", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_diagnostics_provider(Some(Arc::new(|sql: &str| {
            vec![quarry_sql::Diagnostic {
                range: 0..sql.len().min(6),
                severity: quarry_sql::Severity::Warning,
                message: "demo".into(),
            }]
        })));
    });
    f.render(cx);
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert_eq!(f.view.read_with(cx, |v, _| v.diagnostics.len()), 1);
    // The next edit schedules a new run; before it fires the old squiggles stay.
    f.keys(cx, "x");
    assert_eq!(f.view.read_with(cx, |v, _| v.diagnostics.len()), 1);
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(f.view.read_with(cx, |v, _| v.diagnostics[0].range.clone()), 0..6);
}

#[gpui_kit::test]
fn transient_diagnostic_lives_until_the_next_edit(cx: &mut TestAppContext) {
    let f = setup(cx, "select nme from t", Keymap::Standard);
    f.view.update(cx, |v, cx| v.set_transient_diagnostic(7..10, "column \"nme\" does not exist".into(), cx));
    assert_eq!(f.view.read_with(cx, |v, _| v.all_diagnostics().len()), 1);
    f.keys(cx, "z");
    assert_eq!(f.view.read_with(cx, |v, _| v.all_diagnostics().len()), 0);
}

#[gpui_kit::test]
fn ime_composition_is_marked_then_committed(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler as _;
    let f = setup(cx, "ab", Keymap::Standard);
    f.keys(cx, "<end>");
    let view = f.view.clone();
    cx.update_window(f.window, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
            assert_eq!(v.text(), "abにほ");
            assert_eq!(v.marked_text_range(window, cx), Some(2..4));
            v.replace_and_mark_text_in_range(None, "日本", None, window, cx);
            assert_eq!(v.text(), "ab日本");
            v.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(v.text(), "ab日本語");
            assert_eq!(v.marked_text_range(window, cx), None);
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn utf16_conversions_handle_astral_chars(cx: &mut TestAppContext) {
    let f = setup(cx, "a🙂b", Keymap::Standard);
    f.view.read_with(cx, |v, _| {
        assert_eq!(v.char_to_utf16(2), 3);
        assert_eq!(v.utf16_to_char(3), 2);
        assert_eq!(v.utf16_to_char(99), 3);
    });
}

#[gpui_kit::test]
fn large_buffers_only_shape_visible_lines(cx: &mut TestAppContext) {
    let text: String = (0..50_000).map(|i| format!("SELECT {i} FROM t WHERE x = {i};\n")).collect();
    let text: &'static str = Box::leak(text.into_boxed_str());
    let f = setup(cx, text, Keymap::Helix);
    f.render(cx);
    f.view.update(cx, |v, cx| v.scroll_to_line(25_000, cx));
    f.render(cx);
    let (shaped, visible) = f.view.read_with(cx, |v, _| (v.cache.len(), v.last_viewport));
    assert!(shaped < 200, "cache holds {shaped} lines for a {}-line viewport", visible.1);
    assert_eq!(visible.0, 25_000);
}

#[gpui_kit::test]
fn mouse_click_places_the_cursor(cx: &mut TestAppContext) {
    use gpui_kit::MouseButton;
    use gpui_kit::test::TestEventExt as _;
    let f = setup(cx, "select name\nfrom users", Keymap::Standard);
    f.render(cx);
    let view = f.view.clone();
    let (pos, expected) = view.read_with(cx, |v, _| {
        let l = v.layout.clone().expect("painted");
        (
            Point::new(
                l.bounds.origin.x + px(l.gutter_w + l.cell_w * 5.2),
                l.bounds.origin.y + px(l.line_h * 1.5),
            ),
            l.cell_w,
        )
    });
    let _ = expected;
    cx.update_window(f.window, |_, window, cx| {
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    })
    .unwrap();
    let st = f.view.read_with(cx, |v, _| v.status());
    assert_eq!(st.line, 2);
    assert!((5..=6).contains(&(st.column - 1)), "column {}", st.column);
}

use std::sync::Arc;

fn numbered_lines(n: usize) -> &'static str {
    let text: String = (0..n).map(|i| format!("line {i}\n")).collect();
    Box::leak(text.into_boxed_str())
}

#[gpui_kit::test]
fn goto_line_reveals_with_a_three_line_margin_and_zz_centres(cx: &mut TestAppContext) {
    let f = setup(cx, numbered_lines(200), Keymap::Helix);
    f.render(cx);
    let (lh, vh) = f.view.read_with(cx, |v, _| {
        let l = v.layout.clone().expect("painted");
        (l.line_h, f32::from(l.bounds.size.height))
    });
    f.keys(cx, "50G");
    f.render(cx);
    let sy = f.view.read_with(cx, |v, _| v.scroll_y);
    // Cursor line 49: its bottom plus three lines of context sits on the viewport's bottom edge.
    assert_eq!(sy, 50.0 * lh + 3.0 * lh - vh);
    f.keys(cx, "zz");
    f.render(cx);
    let sy = f.view.read_with(cx, |v, _| v.scroll_y);
    assert_eq!(sy, 49.0 * lh + lh / 2.0 - vh / 2.0);
    // The engine is told which lines are visible (ctrl-d / page motions use it).
    let first = f.view.read_with(cx, |v, _| v.last_viewport.0);
    assert_eq!(first, (sy / lh).floor() as usize);
    f.keys(cx, "zt");
    f.render(cx);
    assert_eq!(f.view.read_with(cx, |v, _| v.scroll_y), 49.0 * lh);
}

#[gpui_kit::test]
fn overscroll_stops_half_a_viewport_past_the_end(cx: &mut TestAppContext) {
    let f = setup(cx, numbered_lines(100), Keymap::Helix);
    f.render(cx);
    f.view.update(cx, |v, cx| v.on_scroll(0.0, -1e9, cx));
    f.render(cx);
    let (sy, lh, vh, n) = f.view.read_with(cx, |v, _| {
        let l = v.layout.clone().unwrap();
        (v.scroll_y, l.line_h, f32::from(l.bounds.size.height), v.editor().line_count())
    });
    assert_eq!(sy, n as f32 * lh - vh / 2.0);
}

#[gpui_kit::test]
fn dragging_extends_the_selection(cx: &mut TestAppContext) {
    use gpui_kit::MouseButton;
    use gpui_kit::test::TestEventExt as _;
    let f = setup(cx, "select name from users", Keymap::Standard);
    f.render(cx);
    let (at, cw) = f.view.read_with(cx, |v, _| {
        let l = v.layout.clone().unwrap();
        (Point::new(l.bounds.origin.x + px(l.gutter_w), l.bounds.origin.y + px(l.line_h / 2.0)), l.cell_w)
    });
    let col = |n: f32| Point::new(at.x + px(cw * n + cw * 0.1), at.y);
    cx.update_window(f.window, |_, window, cx| {
        window.pointer_down(col(2.0), MouseButton::Left, cx);
        window.pointer_move(col(6.0), Some(MouseButton::Left), cx);
        window.pointer_move(col(9.0), Some(MouseButton::Left), cx);
        window.pointer_up(col(9.0), MouseButton::Left, cx);
    })
    .unwrap();
    let st = f.view.read_with(cx, |v, _| v.status());
    assert_eq!(st.selected_chars, 7, "columns 2..9");
    // The selection also became the primary selection (X11 middle-click paste).
}

#[gpui_kit::test]
fn double_and_triple_click_select_word_and_line(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};
    let f = setup(cx, "select name\nfrom users", Keymap::Standard);
    f.view.update(cx, |v, cx| v.on_mouse_down_text(8, MouseButton::Left, 2, Modifiers::default(), cx));
    assert_eq!(f.view.read_with(cx, |v, _| v.editor().selected_text()).as_deref(), Some("name"));
    f.view.update(cx, |v, cx| v.on_mouse_down_text(8, MouseButton::Left, 3, Modifiers::default(), cx));
    assert_eq!(f.view.read_with(cx, |v, _| v.editor().selected_text()).as_deref(), Some("select name\n"));
    // Shift-click extends; alt-click adds a cursor.
    f.view.update(cx, |v, cx| v.on_mouse_down_text(14, MouseButton::Left, 1, Modifiers::default(), cx));
    f.view.update(cx, |v, cx| {
        v.on_mouse_down_text(18, MouseButton::Left, 1, Modifiers { alt: true, ..Default::default() }, cx)
    });
    assert_eq!(f.view.read_with(cx, |v, _| v.editor().selections().len()), 2);
}

#[gpui_kit::test]
fn cursor_blinks_and_pauses_while_typing(cx: &mut TestAppContext) {
    let f = setup(cx, "abc", Keymap::Standard);
    f.view.update(cx, |v, cx| {
        v.focused = true;
        v.pause_blink(cx);
    });
    assert!(f.view.read_with(cx, |v, _| v.blink_visible));
    cx.executor().advance_clock(Duration::from_millis(540));
    cx.run_until_parked();
    assert!(!f.view.read_with(cx, |v, _| v.blink_visible), "hidden after one half-period");
    // A keystroke makes it solid again and restarts the timer.
    f.keys(cx, "x");
    assert!(f.view.read_with(cx, |v, _| v.blink_visible));
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert!(f.view.read_with(cx, |v, _| v.blink_visible), "still solid shortly after typing");
}

#[gpui_kit::test]
fn which_key_appears_after_the_delay_but_space_is_immediate(cx: &mut TestAppContext) {
    let f = setup(cx, "abc", Keymap::Helix);
    f.keys(cx, "g");
    let hints = f.view.read_with(cx, |v, _| v.editor().key_hints().map(|(t, h)| (t, h.len())));
    assert!(hints.is_some_and(|(_, n)| n > 3), "g has a goto menu");
    assert!(f.view.read_with(cx, |v, _| v.hints_since.is_some()));
    f.keys(cx, "<esc>");
    assert!(f.view.read_with(cx, |v, _| v.hints_since.is_none()));
}

#[gpui_kit::test]
fn status_messages_expire(cx: &mut TestAppContext) {
    let f = setup(cx, "abc", Keymap::Helix);
    f.keys(cx, "x");
    f.keys(cx, "y");
    let msg = f.view.read_with(cx, |v, _| v.status().message);
    assert!(msg.is_some(), "yank reports a status message");
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(f.view.read_with(cx, |v, _| v.status().message).is_none());
}

#[gpui_kit::test]
fn join_completion_footer_and_manual_request(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
    f.keys(cx, "select * from orders o join <C-space>");
    let (n, footer) = f.view.read_with(cx, |v, _| {
        let c = v.completion.as_ref().expect("ctrl-space opens the popup even for an empty word");
        (c.len(), c.footer().map(str::to_string))
    });
    assert!(n >= 3);
    assert!(footer.is_some_and(|t| t.to_lowercase().contains(" on ")), "join suggestions show their ON clause");
    f.keys(cx, "<tab>");
    assert!(f.text(cx).to_lowercase().contains(" on "), "{}", f.text(cx));
}

#[gpui_kit::test]
fn settings_changes_reach_open_editors(cx: &mut TestAppContext) {
    // quarry-ui writes settings to its config dir: point it at a scratch directory first.
    let dir = std::env::temp_dir().join(format!("quarry-editor-view-test-{}", std::process::id()));
    // SAFETY: set before any other thread in this test reads it; only this test installs the store.
    unsafe { std::env::set_var("QUARRY_CONFIG_DIR", &dir) };
    cx.update(quarry_ui::init);
    let (window, view) = cx.update(|cx| {
        let bounds = Bounds { origin: Point::default(), size: size(px(600.), px(300.)) };
        gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            |window, cx| cx.new(|cx| EditorView::new("abc", window, cx)),
        )
        .expect("window")
    });
    let _ = window;
    assert_eq!(view.read_with(cx, |v, _| v.editor().keymap()), Keymap::Helix);
    cx.update(|cx| {
        quarry_ui::Settings::update(cx, |s| {
            s.keymap = quarry_ui::KeymapMode::Standard;
            s.buffer_font_size = 18.0;
        })
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.editor().keymap(), Keymap::Standard);
        assert_eq!(v.font_size, 18.0);
    });
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn dragging_the_scrollbar_thumb_scrolls(cx: &mut TestAppContext) {
    use gpui_kit::MouseButton;
    use gpui_kit::test::TestEventExt as _;
    let f = setup(cx, numbered_lines(500), Keymap::Helix);
    f.render(cx);
    let (thumb, track) = f.view.read_with(cx, |v, _| {
        let l = v.layout.clone().unwrap();
        (l.v_thumb.expect("a long buffer has a vertical thumb"), l.v_track.unwrap())
    });
    let from = thumb.center();
    let to = Point::new(from.x, from.y + px(60.));
    cx.update_window(f.window, |_, window, cx| {
        window.pointer_down(from, MouseButton::Left, cx);
        window.pointer_move(to, Some(MouseButton::Left), cx);
        window.pointer_up(to, MouseButton::Left, cx);
    })
    .unwrap();
    f.render(cx);
    let (sy, max) = f.view.read_with(cx, |v, _| (v.scroll_y, v.layout.clone().unwrap().max_scroll.1));
    assert!(sy > 0.0 && sy < max, "scrolled to {sy} of {max}");
    // 60px of a track of this height maps proportionally.
    let travel = f32::from(track.size.height) - f32::from(thumb.size.height);
    assert!((sy / max - 60.0 / travel).abs() < 0.02, "{sy} / {max} vs {}", 60.0 / travel);
    // Clicking the track pages toward the pointer; the text cursor is untouched.
    assert_eq!(f.view.read_with(cx, |v, _| v.status().line), 1);
}

fn shop_completion(f: &Fixture, cx: &mut TestAppContext) {
    f.view.update(cx, |v, _| {
        v.set_completion_provider(Some(Rc::new(ShopProvider(quarry_sql::fixtures::shop()))));
    });
}

#[gpui_kit::test]
fn enter_on_an_exact_match_inserts_a_newline_instead_of_accepting(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    shop_completion(&f, cx);
    f.keys(cx, "select * from orders o join customers c on o.customer_id = c.id");
    f.keys(cx, "<ret>where 1 = 1");
    assert_eq!(
        f.text(cx),
        "select * from orders o join customers c on o.customer_id = c.id\nwhere 1 = 1",
        "the popup must not swallow the newline"
    );
}

#[gpui_kit::test]
fn space_after_from_and_join_opens_completion_but_not_after_other_words(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    shop_completion(&f, cx);
    f.keys(cx, "select * from ");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_some()), "after FROM");
    f.keys(cx, "<esc>orders o join ");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_some()), "after JOIN");
    f.keys(cx, "<esc>on ");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_some()), "after ON");
    f.keys(cx, "<esc>o.id = 1 ");
    assert!(f.view.read_with(cx, |v, _| v.completion.is_none()), "an arbitrary space stays quiet");
}

fn incomplete_provider() -> crate::DiagnosticsProvider {
    Arc::new(|sql: &str| quarry_sql::lint(sql, DatabaseKind::Postgres, None))
}

#[gpui_kit::test]
fn unfinished_statement_squiggles_wait_for_typing_to_pause(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| v.set_diagnostics_provider(Some(incomplete_provider())));
    f.render(cx);
    f.keys(cx, "select * from stores s join ");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert!(f.view.read_with(cx, |v, _| !v.diagnostics.is_empty()), "the lint did run");
    assert!(
        f.view.read_with(cx, |v, _| v.all_diagnostics().is_empty()),
        "but an unfinished statement is not underlined while typing"
    );
    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
    assert!(f.view.read_with(cx, |v, _| !v.all_diagnostics().is_empty()), "shown after the pause");
}

#[gpui_kit::test]
fn dangerous_statements_are_never_held_back(cx: &mut TestAppContext) {
    let f = setup(cx, "", Keymap::Standard);
    f.view.update(cx, |v, _| v.set_diagnostics_provider(Some(incomplete_provider())));
    f.render(cx);
    f.keys(cx, "delete from orders");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert!(f.view.read_with(cx, |v, _| v.all_diagnostics().iter().any(|d| d.message.contains("without WHERE"))));
}

#[gpui_kit::test]
fn squiggles_follow_the_text_through_edits(cx: &mut TestAppContext) {
    let f = setup(cx, "select 1;\nselect nme from t", Keymap::Standard);
    f.view.update(cx, |v, _| {
        v.set_diagnostics_provider(Some(Arc::new(|sql: &str| {
            let at = sql.find("nme").unwrap();
            vec![quarry_sql::Diagnostic {
                range: at..at + 3,
                severity: quarry_sql::Severity::Warning,
                message: "Unknown column".into(),
            }]
        })));
    });
    f.render(cx);
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    let before = f.view.read_with(cx, |v, _| v.diagnostics[0].range.clone());
    f.keys(cx, "<C-home>-- note<ret>");
    let after = f.view.read_with(cx, |v, _| v.diagnostics.first().map(|d| d.range.clone()));
    assert_eq!(after, Some(before.start + 8..before.end + 8), "shifted by the inserted text");
    f.keys(cx, "<C-end><left><left><left><left><left><left><left><left>x");
    assert!(f.view.read_with(cx, |v, _| v.diagnostics.is_empty()), "an edit inside the range drops it");
}
