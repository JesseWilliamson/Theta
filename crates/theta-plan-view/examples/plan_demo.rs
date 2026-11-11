//! Plan panel demo with real captured plans.
//!
//!     cargo run -p theta-plan-view --example plan_demo -- --theme "Theta Dark" --view flame
//!
//! Flags: `--view tree|flame|raw`, `--estimate` (not executed), `--slow` (a real
//! plan with findings), `--mariadb`, `--sqlite`, `--big` (synthetic 300 nodes),
//! `--error`, `--empty`, `--loading`, `--select 0,0,1` (tree path), `--insight N`.
//! `THETA_WINDOW_SIZE=WxH` sets the window size.

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, WindowBounds, WindowOptions, div, px, size,
};
use std::sync::Arc;
use std::time::Duration;
use theta_core::{DatabaseKind, Plan, PlanNode};
use theta_plan_view::{PlanEvent, PlanInput, PlanPanel, ViewMode};
use theta_ui::{Settings, application, init, window_size_from_env};

const PG_SQL: &str = include_str!("../fixtures/pg_demo.sql");
const PG_JSON: &str = include_str!("../fixtures/pg_demo.json");
const SLOW_SQL: &str = include_str!("../fixtures/pg_slow.sql");
const SLOW_JSON: &str = include_str!("../fixtures/pg_slow.json");
const MARIA_SQL: &str = include_str!("../fixtures/mariadb_demo.sql");
const MARIA_JSON: &str = include_str!("../fixtures/mariadb_demo.json");
const SQLITE_SQL: &str = include_str!("../fixtures/sqlite_demo.sql");
const SQLITE_EQP: &str = include_str!("../fixtures/sqlite_demo.eqp");
/// Wall time of the SQLite statement measured when the plan was captured.
const SQLITE_MS: f64 = 2.56;
const SQLITE_ROWS: f64 = 12.0;

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Pg,
    Slow,
    Maria,
    Sqlite,
    Big,
}

struct Args {
    theme: Option<String>,
    view: ViewMode,
    source: Source,
    estimate: bool,
    error: bool,
    empty: bool,
    loading: bool,
    select: Option<Vec<usize>>,
    insight: Option<usize>,
}

fn parse_args() -> Args {
    let mut a = Args {
        theme: None,
        view: ViewMode::Tree,
        source: Source::Pg,
        estimate: false,
        error: false,
        empty: false,
        loading: false,
        select: None,
        insight: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--theme" => a.theme = it.next(),
            "--view" => a.view = it.next().and_then(|v| ViewMode::parse(&v)).unwrap_or(ViewMode::Tree),
            "--estimate" => a.estimate = true,
            "--slow" => a.source = Source::Slow,
            "--mariadb" => a.source = Source::Maria,
            "--sqlite" => a.source = Source::Sqlite,
            "--big" => a.source = Source::Big,
            "--error" => a.error = true,
            "--empty" => a.empty = true,
            "--loading" => a.loading = true,
            "--select" => {
                a.select = it.next().map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
            }
            "--insight" => a.insight = it.next().and_then(|s| s.parse().ok()),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    a
}

/// Strip timings and row counts the way a plain `EXPLAIN` would, so `--estimate`
/// shows what a non-executed plan looks like for any source.
fn without_measurements(mut plan: Plan) -> Plan {
    fn strip(n: &mut PlanNode) {
        n.actual_rows = None;
        n.actual_total_ms = None;
        n.loops = None;
        n.rows_removed_by_filter = None;
        n.shared_hit_blocks = None;
        n.shared_read_blocks = None;
        // Counters a plain EXPLAIN never reports (spills, memory, worker counts).
        n.extra.retain(|k, _| {
            !["Sort Method", "Sort Space", "Batches", "Memory", "Heap Fetches", "Workers", "Blocks"]
                .iter()
                .any(|w| k.contains(w))
        });
        n.children.iter_mut().for_each(strip);
    }
    strip(&mut plan.root);
    plan.analyzed = false;
    plan.planning_time_ms = None;
    plan.execution_time_ms = None;
    plan
}

fn build_input(source: Source, estimate: bool) -> PlanInput {
    let (plan, kind, sql) = match source {
        Source::Pg => (
            theta_drivers::parse_postgres_plan_json(PG_JSON, true).expect("pg fixture"),
            DatabaseKind::Postgres,
            PG_SQL.to_string(),
        ),
        Source::Slow => (
            theta_drivers::parse_postgres_plan_json(SLOW_JSON, true).expect("slow fixture"),
            DatabaseKind::Postgres,
            SLOW_SQL.to_string(),
        ),
        Source::Maria => (
            theta_drivers::parse_mysql_plan_json(MARIA_JSON, true).expect("mariadb fixture"),
            DatabaseKind::MySql,
            MARIA_SQL.to_string(),
        ),
        Source::Sqlite => {
            // Mirrors what the SQLite driver does for ANALYZE: it has no per-node
            // timings, so only the root carries the measured total.
            let mut plan = theta_drivers::parse_sqlite_plan(SQLITE_EQP).expect("sqlite fixture");
            plan.analyzed = true;
            plan.root.actual_total_ms = Some(SQLITE_MS);
            plan.root.actual_rows = Some(SQLITE_ROWS);
            plan.root.loops = Some(1.0);
            plan.execution_time_ms = Some(SQLITE_MS);
            (plan, DatabaseKind::Sqlite, SQLITE_SQL.to_string())
        }
        Source::Big => (big_plan(), DatabaseKind::Postgres, "-- synthetic 300-node plan".to_string()),
    };
    let plan = if estimate { without_measurements(plan) } else { plan };
    let (summary, insights) = theta_sql::analyze_plan(&plan, None, kind);
    PlanInput { plan: Arc::new(plan), summary, insights, kind, statement: sql }
}

/// A bushy synthetic plan of ~300 nodes with deterministic pseudo-random timings,
/// a few deliberately bad scans so the insights column has something to say.
fn big_plan() -> Plan {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64) / (1u64 << 31) as f64
        }
    }
    fn scan(rng: &mut Rng, i: usize) -> PlanNode {
        let rows = (rng.next() * 5000.0).round() + 1.0;
        let bad = i % 17 == 3;
        let mut n = PlanNode {
            node_type: if bad || rng.next() < 0.3 { "Seq Scan" } else { "Index Scan" }.into(),
            relation: Some(format!("t{i}")),
            alias: Some(format!("a{i}")),
            plan_rows: Some(if bad { rows / 40.0 } else { rows }),
            actual_rows: Some(rows),
            loops: Some(1.0),
            total_cost: Some(rows * 0.9 + 4.0),
            startup_cost: Some(0.0),
            actual_total_ms: Some(if bad { 8.0 + rng.next() * 30.0 } else { 0.05 + rng.next() * 1.5 }),
            ..Default::default()
        };
        if n.node_type == "Index Scan" {
            n.index = Some(format!("t{i}_pkey"));
            n.extra.insert("Index Cond".into(), format!("(id = a{i}.ref_id)"));
        } else {
            n.filter = Some(format!("(flag_{i} = 'x'::text)"));
            n.rows_removed_by_filter = Some(if bad { 120_000.0 } else { rows * 3.0 });
        }
        n
    }
    fn build(rng: &mut Rng, lo: usize, hi: usize) -> PlanNode {
        if hi - lo == 1 {
            return scan(rng, lo);
        }
        let mid = lo + (hi - lo) / 2;
        let (l, r) = (build(rng, lo, mid), build(rng, mid, hi));
        let hash = PlanNode {
            node_type: "Hash".into(),
            plan_rows: r.plan_rows,
            actual_rows: r.actual_rows,
            loops: Some(1.0),
            total_cost: r.total_cost.map(|c| c + 1.0),
            actual_total_ms: r.actual_total_ms.map(|t| t + 0.1),
            children: vec![r],
            ..Default::default()
        };
        let rows = l.actual_rows.unwrap_or(1.0).min(hash.actual_rows.unwrap_or(1.0));
        let time =
            l.actual_total_ms.unwrap_or(0.0) + hash.actual_total_ms.unwrap_or(0.0) + 0.2 + rows * 0.0004;
        let cost = l.total_cost.unwrap_or(0.0) + hash.total_cost.unwrap_or(0.0) + rows * 0.02;
        let mut j = PlanNode {
            node_type: "Hash Join".into(),
            detail: Some("Inner join".into()),
            plan_rows: Some(rows),
            actual_rows: Some(rows),
            loops: Some(1.0),
            total_cost: Some(cost),
            actual_total_ms: Some(time),
            children: vec![l, hash],
            ..Default::default()
        };
        j.extra.insert("Hash Cond".into(), format!("(a{lo}.ref_id = a{mid}.id)"));
        j
    }
    let mut rng = Rng(42);
    let joins = build(&mut rng, 0, 100);
    let mut agg = PlanNode {
        node_type: "Aggregate".into(),
        detail: Some("Hashed".into()),
        plan_rows: Some(200.0),
        actual_rows: Some(180.0),
        loops: Some(1.0),
        total_cost: Some(joins.total_cost.unwrap_or(0.0) + 50.0),
        actual_total_ms: Some(joins.actual_total_ms.unwrap_or(0.0) + 3.0),
        children: vec![joins],
        ..Default::default()
    };
    agg.extra.insert("Group Key".into(), "a0.region".into());
    let total = agg.actual_total_ms.unwrap_or(0.0);
    let root = PlanNode {
        node_type: "Limit".into(),
        plan_rows: Some(100.0),
        actual_rows: Some(100.0),
        loops: Some(1.0),
        total_cost: agg.total_cost.map(|c| c + 1.0),
        actual_total_ms: Some(total + 0.1),
        children: vec![agg],
        ..Default::default()
    };
    let raw = "[synthetic plan: no server output]".to_string();
    Plan { root, analyzed: true, planning_time_ms: Some(4.8), execution_time_ms: Some(total + 0.2), raw }
}

struct Demo {
    panel: Entity<PlanPanel>,
    source: Source,
    _sub: Subscription,
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        div().size_full().bg(cx.theme().background).child(self.panel.clone())
    }
}

fn main() {
    let args = parse_args();
    application().run(move |cx| {
        init(cx);
        if let Some(name) = args.theme.clone() {
            theta_ui::themes::set_theme(&name, cx);
        }
        let _ = Settings::get(cx);
        let (w, h) = window_size_from_env().unwrap_or((1500., 420.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let (source, estimate) = (args.source, args.estimate);
        gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            |window, cx| {
                let panel = cx.new(|cx| PlanPanel::new(window, cx));
                panel.read(cx).focus_handle(cx).focus(window, cx);
                panel.update(cx, |p, cx| {
                    p.set_empty_hint(vec!["space e".into(), "Ctrl+Shift+E".into()], cx);
                    p.set_view(args.view, cx);
                    if args.error {
                        p.set_error(
                            "ERROR: relation \"orderz\" does not exist\nLINE 1: EXPLAIN SELECT * FROM orderz\n                              ^".into(),
                            cx,
                        );
                    } else if !args.empty {
                        p.set_plan(build_input(source, estimate), window, cx);
                        if let Some(path) = &args.select {
                            p.select_path(path, cx);
                        }
                        if let Some(i) = args.insight {
                            p.select_insight(i, cx);
                        }
                    }
                    if args.loading {
                        p.set_loading(true, cx);
                    }
                });
                cx.new(|cx| {
                    let sub = cx.subscribe_in(&panel, window, move |this: &mut Demo, panel, ev: &PlanEvent, window, cx| {
                        match ev {
                            PlanEvent::Rerun { analyze } => {
                                eprintln!("rerun analyze={analyze}");
                                let (analyze, src) = (*analyze, this.source);
                                panel.update(cx, |p, cx| p.set_loading(true, cx));
                                let panel = panel.clone();
                                cx.spawn_in(window, async move |_, cx| {
                                    cx.background_executor().timer(Duration::from_millis(900)).await;
                                    cx.update(|window, cx| {
                                        panel.update(cx, |p, cx| p.set_plan(build_input(src, !analyze), window, cx))
                                    })
                                    .ok();
                                })
                                .detach();
                            }
                            PlanEvent::OpenSuggestion(sql) => eprintln!("open in console: {sql}"),
                            PlanEvent::CopyText(t) => eprintln!("copied {} bytes", t.len()),
                        }
                    });
                    Demo { panel, source, _sub: sub }
                })
            },
        )
        .expect("open window");
    });
}
