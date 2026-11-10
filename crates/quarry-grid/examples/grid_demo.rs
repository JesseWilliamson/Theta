//! Interactive demo and screenshot target for `ResultGrid`.
//!
//!     cargo run -p quarry-grid --example grid_demo -- [--theme "Quarry Light"] [--editable]
//!         [--rows 100000 --cols 60] [--empty] [--fail] [--autoscroll]
//!
//! The default dataset is a hand-built "orders joined to customers" result that
//! contains every `Value` variant. `--rows/--cols` generate synthetic stress
//! data instead. `--autoscroll` runs a scripted scroll and prints frame timing.

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, WindowBounds, WindowOptions, div, px, size,
};
use quarry_core::{Column, ColumnOrigin, ResultSet, TypeKind, Value};
use quarry_grid::{EditTarget, GridEvent, GridMeta, ResultGrid};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Demo {
    grid: Entity<ResultGrid>,
    _sub: Subscription,
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.grid.clone())
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() as usize) % items.len()]
    }
}

fn col(name: &str, type_name: &str, kind: TypeKind, table: &str) -> Column {
    Column {
        name: name.into(),
        type_name: type_name.into(),
        kind,
        origin: Some(ColumnOrigin {
            schema: Some("public".into()),
            table: table.into(),
            column: name.into(),
        }),
    }
}

/// 200 orders joined to customers, every `Value` variant present.
fn orders_dataset() -> (ResultSet, EditTarget) {
    use TypeKind::*;
    let columns = vec![
        col("order_id", "bigint", Integer, "orders"),
        col("customer_id", "integer", Integer, "orders"),
        col("customer_name", "text", Text, "customers"),
        col("email", "varchar(120)", Text, "customers"),
        col("status", "order_status", Text, "orders"),
        col("total", "numeric(12,2)", Decimal, "orders"),
        col("discount", "double precision", Float, "orders"),
        col("paid", "boolean", Bool, "orders"),
        col("notes", "text", Text, "orders"),
        col("created_at", "timestamp", Timestamp, "orders"),
        col("shipped_at", "timestamptz", Timestamp, "orders"),
        col("ship_date", "date", Date, "orders"),
        col("window_start", "time", Time, "orders"),
        col("sla", "interval", Interval, "orders"),
        col("tags", "text[]", Array, "orders"),
        col("attrs", "jsonb", Json, "orders"),
        col("request_id", "uuid", Uuid, "orders"),
        col("signature", "bytea", Bytes, "orders"),
        col("location", "geometry(Point,4326)", Geometry, "customers"),
        col("units", "bigint unsigned", Integer, "orders"),
        col("client_ip", "inet", Other, "orders"),
    ];
    let first = [
        "Ada", "Grace", "Linus", "Margaret", "Dennis", "Barbara", "Ken", "Frances", "Alan", "Radia", "Tim",
        "Hedy",
    ];
    let last = [
        "Lovelace",
        "Hopper",
        "Torvalds",
        "Hamilton",
        "Ritchie",
        "Liskov",
        "Thompson",
        "Allen",
        "Turing",
        "Perlman",
        "Berners-Lee",
        "Lamarr",
    ];
    let statuses = ["pending", "paid", "shipped", "delivered", "cancelled", "refunded"];
    let notes = [
        "Leave at the front desk",
        "Gift wrap please.\nInclude a handwritten card with the message: \"Happy birthday!\"",
        "Fragile — handle with care. Customer called twice about the delivery window and asked for a signature on receipt.",
        "",
        "ASAP",
    ];
    let cities = [
        (151.2093, -33.8688),
        (-0.1276, 51.5072),
        (2.3522, 48.8566),
        (-74.006, 40.7128),
        (139.6917, 35.6895),
        (18.4241, -33.9249),
    ];
    let mut rng = Rng(42);
    let mut rows = Vec::new();
    for i in 0..200usize {
        let cid = 1000 + (rng.next() % 40) as i64;
        let (f, l) = (first[cid as usize % first.len()], last[(cid as usize / 3) % last.len()]);
        let cents = (rng.next() % 250_000) as i64;
        let day = 1 + (i % 28);
        let month = 1 + (i / 28) % 12;
        let (lon, lat) = *rng.pick(&cities);
        let jitter = |r: &mut Rng| (r.next() % 1000) as f64 / 10_000.0;
        let point = quarry_geo::parse_wkt(&format!(
            "SRID=4326;POINT({} {})",
            lon + jitter(&mut rng),
            lat + jitter(&mut rng)
        ))
        .expect("wkt");
        let geom = if i % 17 == 5 {
            Value::Null
        } else if i % 41 == 3 {
            let poly = quarry_geo::parse_wkt(&format!(
                "SRID=4326;POLYGON(({lon} {lat},{} {lat},{} {},{lon} {},{lon} {lat}))",
                lon + 0.05,
                lon + 0.05,
                lat + 0.05,
                lat + 0.05
            ))
            .expect("wkt");
            Value::Geometry(quarry_geo::to_ewkb(&poly))
        } else {
            Value::Geometry(quarry_geo::to_ewkb(&point))
        };
        rows.push(vec![
            Value::Int(10_000 + i as i64),
            Value::Int(cid),
            Value::Text(format!("{f} {l}")),
            if i % 13 == 0 { Value::Null } else { Value::Text(format!("{}.{}@example.com", f.to_lowercase(), l.to_lowercase())) },
            Value::Text(rng.pick(&statuses).to_string()),
            Value::Decimal(format!("{}.{:02}", cents / 100, cents % 100)),
            if i % 9 == 0 { Value::Null } else { Value::Float((rng.next() % 3000) as f64 / 100.0) },
            Value::Bool(i % 3 != 0),
            if i % 11 == 4 { Value::Null } else { Value::Text(rng.pick(&notes).to_string()) },
            Value::Timestamp(format!("2024-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}", i % 24, (i * 7) % 60, (i * 13) % 60, (i * 37) % 1000)),
            if i % 4 == 0 { Value::Null } else { Value::TimestampTz(format!("2024-{month:02}-{day:02} 09:30:00+02")) },
            Value::Date(format!("2024-{month:02}-{day:02}")),
            Value::Time(format!("{:02}:{:02}:00", 8 + i % 10, (i * 15) % 60)),
            Value::Interval(format!("{} days {:02}:00:00", i % 5, i % 12)),
            if i % 6 == 0 {
                Value::Array(vec![])
            } else {
                Value::Array(vec![Value::Text("rush".into()), Value::Text("gift".into()), if i % 5 == 0 { Value::Null } else { Value::Text("b2b".into()) }])
            },
            if i % 10 == 7 {
                Value::Null
            } else {
                Value::Json(format!(
                    r#"{{"channel":"{}","coupon":{},"items":[{{"sku":"SKU-{}","qty":{},"price":{}.5}},{{"sku":"SKU-{}","qty":1,"price":12.0}}],"gift":{},"address":{{"city":"{}","geo":null,"zip":"{:05}"}}}}"#,
                    if i % 2 == 0 { "web" } else { "store" },
                    if i % 4 == 0 { "\"SPRING24\"" } else { "null" },
                    100 + i,
                    1 + i % 4,
                    10 + i % 30,
                    500 + i,
                    i % 3 == 0,
                    ["Sydney", "London", "Paris", "New York"][i % 4],
                    (i * 777) % 100_000
                ))
            },
            Value::Uuid(format!("{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}", rng.next() as u32, rng.next() as u16, rng.next() % 0x1000, rng.next() % 0x1000, rng.next() * 7919)),
            if i % 8 == 1 { Value::Null } else { Value::Bytes((0..(8 + (i % 40))).map(|b| (b * 31 + i * 7) as u8).collect()) },
            geom,
            Value::UInt(u64::MAX - i as u64),
            Value::Unknown(format!("10.{}.{}.{}", i % 250, (i * 3) % 250, 1 + i % 200)),
        ]);
    }
    // editable: everything that belongs to `orders` except the key itself
    let editable: Vec<usize> = columns
        .iter()
        .enumerate()
        .filter(|(i, c)| *i != 0 && c.origin.as_ref().is_some_and(|o| o.table == "orders"))
        .map(|(i, _)| i)
        .collect();
    (
        ResultSet { columns, rows, truncated: false },
        EditTarget::new(Some("public".into()), "orders", vec![0], editable),
    )
}

/// Synthetic `rows x cols` data with a realistic mix of types.
fn stress_dataset(rows: usize, cols: usize) -> ResultSet {
    use TypeKind::*;
    let kinds = [Integer, Text, Float, Bool, Timestamp, Decimal, Text, Integer];
    let columns: Vec<Column> = (0..cols)
        .map(|c| {
            let k = kinds[c % kinds.len()];
            let tn = match k {
                Integer => "bigint",
                Text => "text",
                Float => "double precision",
                Bool => "boolean",
                Timestamp => "timestamp",
                Decimal => "numeric(12,2)",
                _ => "text",
            };
            Column { name: format!("col_{c:02}"), type_name: tn.into(), kind: k, origin: None }
        })
        .collect();
    let mut rng = Rng(7);
    let mut data = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            let x = rng.next();
            row.push(if x.is_multiple_of(23) {
                Value::Null
            } else {
                match kinds[c % kinds.len()] {
                    Integer => Value::Int((r * 31 + c) as i64 + (x % 1000) as i64),
                    Text => Value::Text(format!("row {r} / col {c} · {}", x % 100_000)),
                    Float => Value::Float((x % 1_000_000) as f64 / 100.0),
                    Bool => Value::Bool(x.is_multiple_of(2)),
                    Timestamp => {
                        Value::Timestamp(format!("2024-{:02}-{:02} 12:00:00", 1 + x % 12, 1 + x % 28))
                    }
                    Decimal => Value::Decimal(format!("{}.{:02}", x % 10_000, x % 100)),
                    _ => Value::Null,
                }
            });
        }
        data.push(row);
    }
    ResultSet { columns, rows: data, truncated: false }
}

/// utime + stime of this process in seconds (Linux only; 0 elsewhere).
fn cpu_seconds() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else { return 0.0 };
    // fields after the ")" that closes the command name; utime/stime are the 12th/13th
    let rest = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

fn main() {
    let (mut theme, mut editable, mut rows, mut cols, mut empty, mut fail, mut autoscroll) =
        (None, false, None, 60usize, false, false, false);
    let mut take: Option<usize> = None;
    let mut loading = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--theme" => theme = args.next(),
            "--editable" => editable = true,
            "--rows" => rows = args.next().and_then(|v| v.parse::<usize>().ok()),
            "--cols" => cols = args.next().and_then(|v| v.parse().ok()).unwrap_or(60),
            "--empty" => empty = true,
            "--take" => take = args.next().and_then(|v| v.parse().ok()),
            "--fail" => fail = true,
            "--loading" => loading = true,
            "--autoscroll" => autoscroll = true,
            _ => {}
        }
    }
    quarry_ui::application().run(move |cx| {
        quarry_ui::init(cx);
        if let Some(name) = theme.clone() {
            quarry_ui::themes::set_theme(&name, cx);
        }
        let (w, h) = quarry_ui::window_size_from_env().unwrap_or((1280., 800.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let (_, demo) = gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            move |window, cx| {
                let grid = cx.new(|cx| ResultGrid::new(window, cx));
                let (rs, meta) = match (rows, empty) {
                    (_, true) => {
                        let (mut rs, _) = orders_dataset();
                        rs.rows.clear();
                        (rs, GridMeta::new().with_elapsed(Duration::from_millis(3)).with_statement("select * from orders where false"))
                    }
                    (Some(n), _) => (
                        stress_dataset(n, cols),
                        GridMeta::new()
                            .with_elapsed(Duration::from_millis(1240))
                            .with_read_only_reason("Synthetic data")
                            .with_statement(format!("select * from synthetic -- {n} x {cols}")),
                    ),
                    _ => {
                        let (mut rs, target) = orders_dataset();
                        if let Some(n) = take {
                            rs.rows.truncate(n);
                        }
                        let mut meta = GridMeta::new()
                            .with_elapsed(Duration::from_millis(42))
                            .with_reference_columns(vec![1])
                            .with_statement("select o.*, c.name, c.email, c.location from orders o join customers c on c.id = o.customer_id");
                        meta = if editable { meta.with_editable(target) } else { meta.with_read_only_reason("Results from more than one table") };
                        (rs, meta)
                    }
                };
                let (n_rows, n_cols) = (rs.rows.len(), rs.columns.len());
                let started = Instant::now();
                grid.update(cx, |g, cx| {
                    g.set_result(Arc::new(rs), meta, window, cx);
                    g.set_loading(loading, cx);
                });
                eprintln!("set_result for {n_rows} x {n_cols}: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
                let focus = grid.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                cx.new(|cx| {
                let sub = cx.subscribe(&grid.clone(), move |this: &mut Demo, grid, ev: &GridEvent, cx| match ev {
                    GridEvent::SubmitChanges(changes) => {
                        eprintln!("submit: {} changes", changes.len());
                        for c in changes {
                            eprintln!("  {c:?}");
                        }
                        let grid = grid.clone();
                        cx.spawn(async move |_, cx| {
                            cx.background_executor().timer(Duration::from_millis(400)).await;
                            grid.update(cx, |g, cx| {
                                if fail {
                                    g.changes_failed("duplicate key value violates unique constraint \"orders_pkey\"".into(), cx);
                                } else {
                                    g.changes_applied(cx);
                                }
                            });
                        })
                        .detach();
                        let _ = this;
                    }
                    GridEvent::Copied { cells } => eprintln!("copied {cells} cells"),
                    GridEvent::Export(r) => eprintln!("export: {r:?}"),
                    GridEvent::ShowOnMap { column, row } => eprintln!("show on map: col {column} row {row:?}"),
                    GridEvent::FollowReference { column, value } => eprintln!("follow ref: col {column} = {value:?}"),
                    GridEvent::SelectionChanged => {}
                });
                Demo { grid, _sub: sub }
                })
            },
        )
        .expect("open window");

        if autoscroll {
            let grid = demo.read(cx).grid.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                // Scripted diagonal scroll: one step per frame for 300 frames.
                let start = Instant::now();
                let cpu0 = cpu_seconds();
                for step in 0..300 {
                    grid.update(cx, |g, cx| g.scroll_by(60.0, 1100.0, cx));
                    // yield to the frame loop
                    cx.background_executor().timer(Duration::from_millis(1)).await;
                    cx.update(|cx| cx.refresh_windows());
                    let _ = step;
                }
                eprintln!(
                    "autoscroll: 300 steps in {:.2} s wall, {:.2} s CPU",
                    start.elapsed().as_secs_f64(),
                    cpu_seconds() - cpu0
                );
            })
            .detach();
        }
    });
}
