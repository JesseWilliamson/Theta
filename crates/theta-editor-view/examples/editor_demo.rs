//! Interactive demo and screenshot target for `EditorView`.
//!
//!     cargo run -p theta-editor-view --example editor_demo -- [--theme "Tokyo Night"]
//!         [--keymap helix|standard] [--lines 50000] [--preset cjk|long|empty]
//!         [--mark running|ok|failed] [--unfocused] [--scroll-to LINE] [--autoscroll]
//!
//! Every `EditorEvent::Effect` is printed to stderr. `THETA_EDITOR_PERF=1` prints frame costs.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, WindowBounds, WindowOptions, div, px, size,
};
use std::rc::Rc;
use std::sync::Arc;
use theta_core::{Catalog, DatabaseKind};
use theta_editor_view::{CompletionProvider, EditorEvent, EditorView, StatementStatus};
use theta_sql::{CompletionList, CompletionRequest};
use theta_ui::settings::{KeymapMode, Settings};

const SQL: &str = "-- Revenue per customer tier, last 30 days
WITH recent AS (
    SELECT o.id, o.customer_id, o.total, o.placed_at
    FROM orders o
    WHERE o.placed_at > now() - interval '30 days'
      AND o.status <> 'cancelled'
), ranked AS (
    SELECT r.customer_id,
           sum(r.total)                                   AS revenue,
           count(*)                                       AS orders,
           rank() OVER (ORDER BY sum(r.total) DESC)       AS rnk
    FROM recent r
    GROUP BY r.customer_id
)
SELECT c.name,
       c.tier,
       k.revenue,
       k.orders,
       round(k.revenue / nullif(k.orders, 0), 2) AS avg_order,
       k.rnk
FROM ranked k
JOIN customers c ON c.id = k.customer_id
LEFT JOIN stores s ON s.id = 7
WHERE k.rnk <= 10 /* top ten only */
ORDER BY k.rnk;

/* A deliberate mistake below: unknown column and a missing FROM target */
SELECT c.nme, count(*) AS n
FROM customers c
JOIN orders o ON o.customer_id = c.id
GROUP BY c.nme
HAVING count(*) > 3.5;

UPDATE products SET price = price * 1.1 WHERE category_id = 4;

DELETE FROM reviews;

SELECT 'it''s a string', 42, 0xFF, NULL, true, $1 FROM events WHERE kind = 'login';
";

const CJK: &str = "-- 日本語のコメント: 顧客テーブルから名前を取得
SELECT name AS 名前, '東京' AS city, '🙂 emoji 🚀' AS mood FROM customers WHERE name = '山田太郎';
SELECT 'e\u{301}' AS combining, '👨\u{200d}👩\u{200d}👧 family' AS zwj FROM stores;
SELECT 'ＡＢＣ fullwidth' FROM events;
";

struct CatalogProvider(Catalog);

impl CompletionProvider for CatalogProvider {
    fn complete(&self, text: &str, byte_offset: usize) -> CompletionList {
        theta_sql::complete(&CompletionRequest {
            sql: text,
            offset: byte_offset,
            kind: DatabaseKind::Postgres,
            catalog: Some(&self.0),
        })
    }
}

struct Demo {
    editor: Entity<EditorView>,
    _sub: Subscription,
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.editor.read(cx).status();
        let theme = cx.theme();
        let line = format!(
            "{}  {:<8} Ln {}, Col {}   {} sel / {} chars",
            status.mode_label(),
            status.pending_keys,
            status.line,
            status.column,
            status.selection_count,
            status.selected_chars
        );
        v_flex().size_full().child(div().flex_1().min_h_0().child(self.editor.clone())).child(
            h_flex()
                .h(px(24.))
                .px_3()
                .items_center()
                .justify_between()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.status_bar)
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(line)
                .child(status.message.unwrap_or_default()),
        )
    }
}

fn main() {
    let mut theme_arg: Option<String> = None;
    let mut keymap = KeymapMode::Helix;
    let mut lines: Option<usize> = None;
    let mut preset = String::new();
    let mut mark: Option<StatementStatus> = None;
    let mut unfocused = false;
    let mut scroll_to: Option<usize> = None;
    let mut autoscroll = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--theme" => theme_arg = args.next(),
            "--keymap" => {
                keymap = if args.next().as_deref() == Some("standard") {
                    KeymapMode::Standard
                } else {
                    KeymapMode::Helix
                }
            }
            "--lines" => lines = args.next().and_then(|v| v.parse().ok()),
            "--preset" => preset = args.next().unwrap_or_default(),
            "--mark" => {
                mark = match args.next().as_deref() {
                    Some("running") => Some(StatementStatus::Running),
                    Some("ok") => Some(StatementStatus::Ok),
                    Some("failed") => Some(StatementStatus::Failed),
                    _ => None,
                }
            }
            "--unfocused" => unfocused = true,
            "--autoscroll" => autoscroll = true,
            "--scroll-to" => scroll_to = args.next().and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    theta_ui::application().run(move |cx| {
        theta_ui::init(cx);
        if let Some(name) = theme_arg.clone() {
            theta_ui::themes::set_theme(&name, cx);
        }
        Settings::update(cx, |s| s.keymap = keymap);
        let (w, h) = theta_ui::window_size_from_env().unwrap_or((1100., 820.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let text: String = match (preset.as_str(), lines) {
            ("cjk", _) => CJK.to_string(),
            ("empty", _) => String::new(),
            ("long", _) => {
                let mut s = String::from("SELECT ");
                for i in 0..400 {
                    s.push_str(&format!("coalesce(c.col_{i}, 0) AS metric_{i}, "));
                }
                s.push_str("1 FROM customers c;\nSELECT 2;\n");
                s
            }
            (_, Some(n)) => (0..n)
                .map(|i| match i % 5 {
                    0 => format!("SELECT c.id, c.name, o.total FROM customers c JOIN orders o ON o.customer_id = c.id -- row {i}"),
                    1 => format!("    WHERE o.total > {} AND c.tier = 'gold'", i % 997),
                    2 => format!("    ORDER BY o.placed_at DESC LIMIT {};", 10 + i % 90),
                    3 => String::new(),
                    _ => format!("/* block {i} */ UPDATE products SET price = price + 0.{} WHERE id = {};", i % 9, i),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => SQL.to_string(),
        };
        let catalog = Arc::new(theta_sql::fixtures::shop());
        let (_, demo) = gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            |window, cx| {
                let editor = cx.new(|cx| EditorView::new(&text, window, cx));
                editor.update(cx, |e, _| {
                    e.set_completion_provider(Some(Rc::new(CatalogProvider((*catalog).clone()))));
                    let cat = catalog.clone();
                    e.set_diagnostics_provider(Some(Arc::new(move |sql: &str| {
                        theta_sql::lint(sql, DatabaseKind::Postgres, Some(&cat))
                    })));
                });
                let sub = cx.subscribe(&editor, |_, ev: &EditorEvent, _| {
                    if let EditorEvent::Effect(e) = ev {
                        eprintln!("effect: {e:?}");
                    }
                });
                if !unfocused {
                    window.focus(&editor.read(cx).focus_handle(cx), cx);
                }
                cx.new(|_| Demo { editor, _sub: sub })
            },
        )
        .expect("open window");
        let editor = demo.read(cx).editor.clone();
        if let Some(status) = mark {
            editor.update(cx, |e, cx| {
                let text = e.text();
                if let Some(span) = theta_sql::statement_at(&text, 0, DatabaseKind::Postgres) {
                    e.set_statement_status(Some((span.range, status)), cx);
                }
            });
        }
        if autoscroll {
            // 600 frames of steady scrolling through the buffer; pair with THETA_EDITOR_PERF=1.
            let editor = editor.clone();
            cx.spawn(async move |cx| {
                for i in 0..600usize {
                    cx.background_executor().timer(std::time::Duration::from_millis(16)).await;
                    editor.update(cx, |e, cx| e.scroll_to_line(10_000 + i * 3, cx));
                }
            })
            .detach();
        }
        if let Some(line) = scroll_to {
            editor.update(cx, |e, cx| {
                e.update_editor(cx, |ed| {
                    let at = ed.line_to_char(line);
                    ed.set_selections(vec![theta_editor::Selection::point(at)], 0);
                });
                e.scroll_to_line(line.saturating_sub(5), cx);
            });
        }
    });
}
