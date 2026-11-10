//! Seed the demo dataset into a database.
//!
//! ```text
//! cargo run -p quarry-drivers --example seed -- postgres [postgres://user:pass@host:port/db]
//! cargo run -p quarry-drivers --example seed -- mysql    [mysql://user:pass@host:port/db]
//! cargo run -p quarry-drivers --example seed -- sqlite   <file.db | :memory:>
//! ```
//!
//! Without a URL the local test services from AGENTS.md are used. Re-running is
//! safe: the demo tables are dropped and recreated.

use quarry_core::{ConnectionConfig, DatabaseKind, ExecOptions};

fn parse_url(url: &str, kind: DatabaseKind) -> Result<ConnectionConfig, String> {
    let rest = url.split_once("://").map(|(_, r)| r).ok_or("expected scheme://user:pass@host:port/db")?;
    let (auth, hostdb) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, pass) = auth.split_once(':').map(|(u, p)| (u, Some(p))).unwrap_or((auth, None));
    let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|e| format!("bad port: {e}"))?),
        None => (hostport, kind.default_port().unwrap_or(0)),
    };
    let mut c = ConnectionConfig::new("seed", "seed", kind);
    c.host = host.into();
    c.port = port;
    c.user = user.into();
    c.password = pass.map(str::to_string);
    c.database = db.into();
    Ok(c)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(kind_arg) = args.first() else {
        eprintln!("usage: seed postgres|mysql|sqlite [url | path]");
        std::process::exit(2);
    };
    let config = match kind_arg.as_str() {
        "postgres" => parse_url(
            args.get(1)
                .map(String::as_str)
                .unwrap_or("postgres://postgres:postgres@127.0.0.1:5432/quarry_demo"),
            DatabaseKind::Postgres,
        ),
        "mysql" => parse_url(
            args.get(1).map(String::as_str).unwrap_or("mysql://quarry:quarry@127.0.0.1:3306/quarry_demo"),
            DatabaseKind::MySql,
        ),
        "sqlite" => {
            let mut c = ConnectionConfig::new("seed", "seed", DatabaseKind::Sqlite);
            c.path = args.get(1).cloned().unwrap_or_else(|| "quarry_demo.sqlite".into());
            Ok(c)
        }
        other => Err(format!("unknown database kind `{other}` (postgres | mysql | sqlite)")),
    }
    .unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    let result = async {
        let conn = quarry_drivers::connect(&config).await?;
        quarry_drivers::demo::seed(conn.as_ref()).await?;
        let counts = conn
            .execute(
                "SELECT (SELECT count(*) FROM customers), (SELECT count(*) FROM products), \
                 (SELECT count(*) FROM orders), (SELECT count(*) FROM order_items)",
                &ExecOptions::default(),
            )
            .await?;
        if let Some(rs) = counts.result {
            let row: Vec<String> = rs.rows[0].iter().map(|v| v.display()).collect();
            println!(
                "seeded: {} customers, {} products, {} orders, {} order items",
                row[0], row[1], row[2], row[3]
            );
        }
        conn.close().await
    }
    .await;
    if let Err(e) = result {
        eprintln!("seeding failed: {e}");
        std::process::exit(1);
    }
}
