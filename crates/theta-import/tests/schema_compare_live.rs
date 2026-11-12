//! Schema comparison and the migration script, proved the only way that counts: build schema A (the target) and
//! schema B (the source) in a real database, generate the script that turns A into B, run it on copies of A,
//! introspect again and compare again. The second comparison must find nothing (or, with destructive statements
//! held back, exactly what was held back), and the rows the migration was not meant to touch must be there.
//!
//! PostgreSQL and MariaDB are the local test servers (AGENTS.md); a test prints why and passes when its server
//! does not answer or will not let it create a schema. SQLite uses files. Every schema, database and file has a
//! unique name and is removed at the end, even when an assertion fails. The seeded `quarry_demo` tables are
//! never touched.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use theta_core::*;
use theta_sql::schema_diff::{SchemaDiff, Side, compare, generate};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}_{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

async fn run(conn: &dyn Connection, sql: &str) -> Result<Vec<Vec<Value>>, Error> {
    conn.execute(sql, &ExecOptions { row_limit: None })
        .await
        .map(|r| r.result.map(|r| r.rows).unwrap_or_default())
}

async fn q(conn: &dyn Connection, sql: &str) -> Vec<Vec<Value>> {
    run(conn, sql).await.unwrap_or_else(|e| panic!("`{sql}` failed: {e}"))
}

async fn open(c: &ConnectionConfig) -> Arc<dyn Connection> {
    Arc::from(theta_drivers::connect(c).await.expect("connect"))
}

/// One schema of one connection.
#[derive(Clone)]
struct Db {
    conn: Arc<dyn Connection>,
    kind: DatabaseKind,
    /// The schema (PostgreSQL), database (MariaDB) or `main` (SQLite).
    schema: String,
    /// What goes before a table name in a statement of the test's own: `name.` or nothing.
    p: String,
}

impl Db {
    async fn catalog_schema(&self) -> Schema {
        let cat = self.conn.introspect().await.expect("introspect");
        cat.schema(&self.schema)
            .unwrap_or_else(|| panic!("schema {} not in the catalog", self.schema))
            .clone()
    }
}

/// Schema A: what exists. Schema B: what is wanted. Differences, by design:
///
/// * `customers`: `legacy` column dropped (destructive), `name` widened (not destructive), `nick` added;
/// * `orders`: default of `status` changed, `note` made NOT NULL, a foreign key to customers added, an index
///   added and `ix_orders_total` dropped (destructive);
/// * `old_stuff` dropped (destructive); `shipments` added, with a foreign key and a check.
fn ddl_a(kind: DatabaseKind, p: &str) -> Vec<String> {
    let (int, num) = types(kind);
    let on = |t: &str| if kind == DatabaseKind::Sqlite { t.to_string() } else { format!("{p}{t}") };
    vec![
        format!(
            "CREATE TABLE {p}customers (id {int} PRIMARY KEY, name varchar(40) NOT NULL, email varchar(80), legacy text, CONSTRAINT uq_customers_email UNIQUE (email))"
        ),
        format!("CREATE INDEX ix_customers_name ON {} (name)", on("customers")),
        format!(
            "CREATE TABLE {p}orders (id {int} PRIMARY KEY, customer_id {int} NOT NULL, status varchar(10) NOT NULL DEFAULT 'new', note varchar(100), total {num})"
        ),
        format!("CREATE INDEX ix_orders_total ON {} (total)", on("orders")),
        format!("CREATE TABLE {p}old_stuff (id {int} PRIMARY KEY, v text)"),
    ]
}

fn types(kind: DatabaseKind) -> (&'static str, &'static str) {
    match kind {
        DatabaseKind::Postgres => ("integer", "numeric(10,2)"),
        DatabaseKind::Sqlite => ("INTEGER", "NUMERIC(10,2)"),
        _ => ("int", "decimal(10,2)"),
    }
}

fn ddl_b(kind: DatabaseKind, p: &str) -> Vec<String> {
    let (int, num) = types(kind);
    let on = |t: &str| if kind == DatabaseKind::Sqlite { t.to_string() } else { format!("{p}{t}") };
    vec![
        format!(
            "CREATE TABLE {p}customers (id {int} PRIMARY KEY, name varchar(60) NOT NULL, email varchar(80), nick varchar(20), CONSTRAINT uq_customers_email UNIQUE (email))"
        ),
        format!("CREATE INDEX ix_customers_name ON {} (name)", on("customers")),
        format!(
            "CREATE TABLE {p}orders (id {int} PRIMARY KEY, customer_id {int} NOT NULL, status varchar(10) NOT NULL DEFAULT 'open', note varchar(100) NOT NULL, total {num}, CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES {p}customers (id) ON DELETE CASCADE)"
        ),
        format!("CREATE INDEX ix_orders_customer ON {} (customer_id)", on("orders")),
        format!(
            "CREATE TABLE {p}shipments (id {int} PRIMARY KEY, order_id {int} NOT NULL, qty {int} NOT NULL, CONSTRAINT ck_shipments_qty CHECK (qty > 0), CONSTRAINT fk_shipments_order FOREIGN KEY (order_id) REFERENCES {p}orders (id))"
        ),
    ]
}

async fn seed_rows(db: &Db) {
    let p = &db.p;
    q(db.conn.as_ref(), &format!("INSERT INTO {p}customers (id, name, email, legacy) VALUES (1, 'Ann', 'ann@x', 'old1'), (2, 'Bob', NULL, 'old2')")).await;
    q(db.conn.as_ref(), &format!("INSERT INTO {p}orders (id, customer_id, status, note, total) VALUES (1, 1, 'new', 'n1', 10.50), (2, 2, 'new', 'n2', 20)")).await;
    q(db.conn.as_ref(), &format!("INSERT INTO {p}old_stuff (id, v) VALUES (1, 'x')")).await;
}

/// `(object, element, name, status)` of every difference, the shape the assertions read.
fn shape(d: &SchemaDiff) -> Vec<String> {
    let mut v = Vec::new();
    for o in &d.objects {
        if o.changes.is_empty() {
            v.push(format!("{} {} {:?}", o.kind.label(), o.name, o.status));
        }
        for c in &o.changes {
            v.push(format!("{} {}: {} {} {:?}", o.kind.label(), o.name, c.element.label(), c.name, c.status));
        }
    }
    v.sort();
    v
}

async fn diff_of(source: &Db, target: &Db) -> SchemaDiff {
    let (s, t) = (source.catalog_schema().await, target.catalog_schema().await);
    compare(Side { kind: source.kind, schema: &s }, Side { kind: target.kind, schema: &t })
}

/// Run every live statement of the script on `target`, in order, the way a console would one by one.
async fn apply(
    source: &Db,
    target: &Db,
    include_destructive: bool,
) -> (SchemaDiff, theta_sql::schema_diff::Script) {
    let (s, t) = (source.catalog_schema().await, target.catalog_schema().await);
    let (ss, ts) = (Side { kind: source.kind, schema: &s }, Side { kind: target.kind, schema: &t });
    let diff = compare(ss, ts);
    let script = generate(ss, ts, &diff, include_destructive).expect("same product");
    let text = script.render(target.kind, &s.name, &t.name);
    for st in script.statements.iter().filter(|s| !s.commented) {
        if let Err(e) = run(target.conn.as_ref(), &st.sql).await {
            panic!("statement failed: {e}\n--- {}\n--- the whole script:\n{text}", st.sql);
        }
    }
    (diff, script)
}

async fn names_in(db: &Db, sql: &str) -> Vec<String> {
    let mut v: Vec<String> = q(db.conn.as_ref(), sql)
        .await
        .iter()
        .map(|r| match &r[0] {
            Value::Text(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    v.sort();
    v
}

async fn count(db: &Db, table: &str) -> i64 {
    match &q(db.conn.as_ref(), &format!("SELECT count(*) FROM {}{table}", db.p)).await[0][0] {
        Value::Int(n) => *n,
        other => panic!("count: {other:?}"),
    }
}

/// The whole scenario on four schemas of one product: a = target, b = source, c = copy of a (held-back
/// script, then the rest), d = another copy of a (everything in one go).
async fn scenario(a: Db, b: Db, c: Db, d: Db) {
    let kind = a.kind;
    for db in [&a, &c, &d] {
        for s in ddl_a(kind, &db.p) {
            q(db.conn.as_ref(), &s).await;
        }
    }
    for s in ddl_b(kind, &b.p) {
        q(b.conn.as_ref(), &s).await;
    }
    seed_rows(&c).await;
    seed_rows(&d).await;

    // 1. A against B: the differences are what the fixture says, and no more.
    let first = diff_of(&b, &a).await;
    let got = shape(&first);
    // SQLite's catalog has no constraint names, so a key is shown by what it connects.
    let fk_line = if kind == DatabaseKind::Sqlite {
        "table orders: foreign key (customer_id) -> customers OnlyInSource"
    } else {
        "table orders: foreign key fk_orders_customer OnlyInSource"
    };
    for expected in [
        "table customers: column legacy OnlyInTarget",
        "table customers: column name Different",
        "table customers: column nick OnlyInSource",
        "table old_stuff OnlyInTarget",
        "table orders: column note Different",
        "table orders: column status Different",
        fk_line,
        "table orders: index ix_orders_customer OnlyInSource",
        "table orders: index ix_orders_total OnlyInTarget",
        "table shipments OnlyInSource",
    ] {
        assert!(got.iter().any(|g| g == expected), "{kind:?}: missing `{expected}` in {got:#?}");
    }
    assert_eq!(got.len(), 10, "{kind:?}: nothing else differs: {got:#?}");
    assert!(diff_of(&a, &a).await.is_empty(), "a schema equals itself");

    // 2. The script with destructive statements held back.
    let (_, held) = apply(&b, &c, false).await;
    assert!(held.commented_count() >= 3, "{kind:?}: the drops are held back: {held:#?}");
    let text = held.render(kind, &b.schema, &c.schema);
    assert!(text.contains("-- destructive:"), "{text}");
    let remaining = shape(&diff_of(&b, &c).await);
    assert_eq!(
        remaining,
        [
            "table customers: column legacy OnlyInTarget",
            "table old_stuff OnlyInTarget",
            "table orders: index ix_orders_total OnlyInTarget",
        ],
        "{kind:?}: exactly what was held back remains\n{text}"
    );
    assert_eq!(count(&c, "old_stuff").await, 1, "the table that was not dropped keeps its row");
    let legacy = names_in(&c, &format!("SELECT legacy FROM {}customers ORDER BY id", c.p)).await;
    assert_eq!(legacy, ["old1", "old2"], "{kind:?}: held-back column keeps its data");

    // 3. The rest, now allowed.
    let (_, rest) = apply(&b, &c, true).await;
    assert_eq!(rest.commented_count(), 0);
    let after = diff_of(&b, &c).await;
    assert!(after.is_empty(), "{kind:?}: the second script converges: {:#?}", shape(&after));

    // 4. Everything in one go on the other copy.
    let (_, all) = apply(&b, &d, true).await;
    let text = all.render(kind, &b.schema, &d.schema);
    if std::env::var_os("THETA_SHOW_SCRIPT").is_some() {
        eprintln!("=== {kind:?} ===\n{text}");
    }
    let after = diff_of(&b, &d).await;
    assert!(after.is_empty(), "{kind:?}: one script converges: {:#?}\n{text}", shape(&after));
    for db in [&c, &d] {
        assert_eq!(count(db, "customers").await, 2, "{kind:?}: the customers survive the migration");
        assert_eq!(count(db, "orders").await, 2, "{kind:?}: the orders survive the migration");
        let names = names_in(db, &format!("SELECT name FROM {}customers ORDER BY id", db.p)).await;
        assert_eq!(names, ["Ann", "Bob"]);
        let status = names_in(db, &format!("SELECT status FROM {}orders ORDER BY id", db.p)).await;
        assert_eq!(status, ["new", "new"], "existing rows keep their value when only the default changed");
        assert!(run(db.conn.as_ref(), &format!("SELECT 1 FROM {}old_stuff", db.p)).await.is_err(), "dropped");
        // The new foreign key is enforced.
        let bad = run(
            db.conn.as_ref(),
            &format!("INSERT INTO {}orders (id, customer_id, status, note) VALUES (9, 99, 'x', 'n')", db.p),
        )
        .await;
        assert!(bad.is_err(), "{kind:?}: the added foreign key rejects an orphan");
    }
    // Nothing to do the second time.
    let again = generate(
        Side { kind, schema: &d.catalog_schema().await },
        Side { kind, schema: &d.catalog_schema().await },
        &diff_of(&d, &d).await,
        true,
    )
    .unwrap();
    assert!(again.is_empty());
}

async fn guarded(body: tokio::task::JoinHandle<()>, cleanup: impl std::future::Future<Output = ()>) {
    let r = body.await;
    cleanup.await;
    if let Err(e) = r {
        std::panic::resume_unwind(e.into_panic());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_round_trip() {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let mut cfg = ConnectionConfig::new("t", "pg", DatabaseKind::Postgres);
    (cfg.host, cfg.user, cfg.password, cfg.database) =
        ("127.0.0.1".into(), "postgres".into(), Some("postgres".into()), "quarry_demo".into());
    let conn = open(&cfg).await;
    let names: Vec<String> = ["a", "b", "c", "d"].iter().map(|n| unique(&format!("cmp_{n}"))).collect();
    let mk = |n: &String| Db {
        conn: conn.clone(),
        kind: DatabaseKind::Postgres,
        schema: n.clone(),
        p: format!("{n}."),
    };
    for n in &names {
        q(conn.as_ref(), &format!("CREATE SCHEMA {n}")).await;
    }
    let dbs: Vec<Db> = names.iter().map(mk).collect();
    let body = tokio::spawn(scenario(dbs[0].clone(), dbs[1].clone(), dbs[2].clone(), dbs[3].clone()));
    let c2 = conn.clone();
    guarded(body, async move {
        for n in names {
            let _ = run(c2.as_ref(), &format!("DROP SCHEMA IF EXISTS {n} CASCADE")).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mariadb_round_trip() {
    if !reachable(3306) {
        eprintln!("skipped: no MariaDB on 127.0.0.1:3306");
        return;
    }
    let mut cfg = ConnectionConfig::new("t", "my", DatabaseKind::MySql);
    (cfg.host, cfg.user, cfg.password, cfg.database) =
        ("127.0.0.1".into(), "quarry".into(), Some("quarry".into()), "quarry_demo".into());
    let conn = open(&cfg).await;
    let names: Vec<String> = ["a", "b", "c", "d"].iter().map(|n| unique(&format!("cmp_{n}"))).collect();
    for n in &names {
        if let Err(e) = run(conn.as_ref(), &format!("CREATE DATABASE {n}")).await {
            eprintln!("skipped: this MariaDB user may not create databases ({e})");
            for n in &names {
                let _ = run(conn.as_ref(), &format!("DROP DATABASE IF EXISTS {n}")).await;
            }
            return;
        }
    }
    let dbs: Vec<Db> = names
        .iter()
        .map(|n| Db { conn: conn.clone(), kind: DatabaseKind::MySql, schema: n.clone(), p: format!("{n}.") })
        .collect();
    let body = tokio::spawn(scenario(dbs[0].clone(), dbs[1].clone(), dbs[2].clone(), dbs[3].clone()));
    let c2 = conn.clone();
    guarded(body, async move {
        for n in names {
            let _ = run(c2.as_ref(), &format!("DROP DATABASE IF EXISTS {n}")).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_round_trip() {
    let dir = std::env::temp_dir().join(unique("theta_cmp"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut dbs = Vec::new();
    for n in ["a", "b", "c", "d"] {
        let mut c = ConnectionConfig::new("t", n, DatabaseKind::Sqlite);
        c.path = dir.join(format!("{n}.db")).to_string_lossy().into_owned();
        dbs.push(Db {
            conn: open(&c).await,
            kind: DatabaseKind::Sqlite,
            schema: "main".into(),
            p: String::new(),
        });
    }
    let body = tokio::spawn(scenario(dbs[0].clone(), dbs[1].clone(), dbs[2].clone(), dbs[3].clone()));
    let d2 = dir.clone();
    guarded(body, async move {
        drop(dbs);
        let _ = std::fs::remove_dir_all(d2);
    })
    .await;
}
