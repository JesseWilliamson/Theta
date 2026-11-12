//! Differential test of the SQL parser (`theta_sql::syntax`) against real engines.
//!
//! For every statement of the corpus (`crates/theta-sql/tests/corpus/*.sql`) the real engine is asked whether
//! the statement is syntactically valid, and the parser is run on it:
//!
//! * **(a) zero false errors.** If the engine accepts the statement, or rejects it for a *semantic* reason
//!   (unknown table, wrong type, ...: it parsed), the parser must report no syntax error. This is an assertion.
//! * **(b) catch rate.** Of the statements the engine rejects as *syntax* errors, how many the parser also
//!   flags. This is printed, never asserted: a miss is a missed hint, a false error is a bug.
//!
//! The same loop checks name resolution (`theta_sql::analysis`): no "unknown table / column" diagnostic on a
//! statement the engine accepted in full against the demo schema.
//!
//! How each engine is asked (nothing the corpus contains is committed):
//! PostgreSQL `PREPARE` for queries and DML (analysis without execution), `BEGIN` ... `ROLLBACK` around
//! everything else, error class 42601 = syntax. MariaDB `PREPARE` (never executes), error 1064 / 1149 = syntax.
//! SQLite `sqlite3_prepare` ("syntax error", "incomplete input", ...). DuckDB `prepare` ("Parser Error").
//! ClickHouse (18.16 only) runs read-only statements, error 62 = syntax. T-SQL has no server here, and the
//! ClickHouse and T-SQL corpora are *documentation examples* assumed valid.
//!
//! Each leg skips (printing why) when its server is unreachable, so `cargo test` stays green anywhere.

mod common;

use common::*;
use std::path::PathBuf;
use theta_core::*;
use theta_sql::syntax::parse;

#[derive(Debug, Clone, PartialEq)]
enum Verdict {
    /// The engine accepted the statement in full (prepared / planned / ran).
    Valid,
    /// The engine understood the syntax and rejected the meaning.
    Semantic(String),
    /// The engine's parser rejected the statement.
    Syntax(String),
    /// Not asked (would block, or has effects that cannot be undone).
    Skipped,
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../theta-sql/tests/corpus")
}

/// Statements of one corpus file: split on `-- ---` lines when the file has them, else one per line.
fn load(name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(corpus_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut out = Vec::new();
    if text.lines().any(|l| l.trim() == "-- ---") {
        let mut cur = String::new();
        for line in text.lines().chain(std::iter::once("-- ---")) {
            if line.trim() == "-- ---" {
                let s = cur.trim();
                if !s.is_empty() {
                    out.push(s.to_string());
                }
                cur.clear();
            } else if !(cur.is_empty() && line.trim_start().starts_with("--")) {
                cur.push_str(line);
                cur.push('\n');
            }
        }
    } else {
        for line in text.lines() {
            let l = line.trim();
            if !l.is_empty() && !l.starts_with("--") {
                out.push(l.to_string());
            }
        }
    }
    out
}

fn schema_statements(schema: &str) -> Vec<String> {
    let body: String =
        schema.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
    body.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn strip_semicolon(sql: &str) -> &str {
    sql.trim().trim_end_matches(';').trim()
}

fn first_word(sql: &str) -> String {
    sql.trim_start_matches(|c: char| c == '(' || c.is_whitespace())
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

#[derive(Default)]
struct Tally {
    /// Statements of typos.sql that the engine rejects as syntax errors and the parser let through.
    must_missed: Vec<String>,
    total: usize,
    valid: usize,
    semantic: usize,
    syntax: usize,
    skipped: usize,
    flagged: usize,
    false_errors: Vec<String>,
    false_resolution: Vec<String>,
    missed: Vec<String>,
    /// Statements the engine rejected for an unknown / ambiguous name, and how many of them the resolver flagged.
    name_errors: usize,
    name_flagged: usize,
    name_missed: Vec<String>,
}

/// Does the engine's complaint say "no such table / column" or "ambiguous column"?
fn is_name_error(msg: &str) -> bool {
    let l = msg.to_ascii_lowercase();
    (l.contains("does not exist") && (l.contains("relation") || l.contains("column") || l.contains("table")))
        || l.contains("unknown column")
        || l.contains("unknown table")
        || l.contains("doesn't exist")
        || l.contains("no such table")
        || l.contains("no such column")
        || l.contains("ambiguous")
        || l.contains("catalog error: table")
        || l.contains("binder error: referenced column")
        || l.contains("binder error: referenced table")
        || l.contains("table with name")
}

fn judge_all(
    name: &str,
    kind: DatabaseKind,
    catalog: Option<&Catalog>,
    items: Vec<(String, Verdict)>,
    assume_valid: bool,
    ddl_is_checked: bool,
) -> Tally {
    let mut t = Tally::default();
    let must_flag: std::collections::HashSet<String> = load("typos.sql").into_iter().collect();
    for (sql, verdict) in items {
        t.total += 1;
        let verdict = if assume_valid && verdict == Verdict::Skipped { Verdict::Valid } else { verdict };
        let parsed = parse(&sql, kind);
        let errs: Vec<String> = parsed.errors.iter().map(|e| e.message.clone()).collect();
        match &verdict {
            Verdict::Valid => t.valid += 1,
            Verdict::Semantic(_) => t.semantic += 1,
            Verdict::Syntax(_) => t.syntax += 1,
            Verdict::Skipped => t.skipped += 1,
        }
        match &verdict {
            Verdict::Valid | Verdict::Semantic(_) => {
                if !errs.is_empty() {
                    t.false_errors.push(format!("{sql}\n      -> {errs:?}\n      engine: {verdict:?}"));
                }
            }
            Verdict::Syntax(_) => {
                if errs.is_empty() {
                    // The editor shows `lint`, which is the parser's errors plus the checks that are not
                    // about syntax (a near-miss statement keyword such as `selct`), so that is what a
                    // typo must reach.
                    if must_flag.contains(&sql) && theta_sql::lint(&sql, kind, None).is_empty() {
                        t.must_missed.push(sql.clone());
                    }
                    t.missed.push(sql.clone());
                } else {
                    t.flagged += 1;
                }
            }
            Verdict::Skipped => {}
        }
        // MariaDB's PREPARE and DuckDB's prepare accept DDL without looking at the objects it names, so for
        // them "valid" says nothing about names in DDL.
        let resolvable = ddl_is_checked
            || matches!(
                first_word(&sql).as_str(),
                "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "VALUES" | "MERGE" | "TABLE" | "FROM"
            );
        if let (Verdict::Valid, Some(cat)) = (&verdict, catalog)
            && resolvable
        {
            let diags = theta_sql::analysis::name_diagnostics(&sql, kind, cat);
            if !diags.is_empty() {
                t.false_resolution.push(format!("{sql}\n      -> {diags:?}"));
            }
        }
        if let (Verdict::Semantic(msg), Some(cat)) = (&verdict, catalog)
            && is_name_error(msg)
            && theta_sql::split_statements(&sql, kind).len() == 1
            && errs.is_empty()
        {
            t.name_errors += 1;
            if theta_sql::analysis::name_diagnostics(&sql, kind, cat).is_empty() {
                t.name_missed.push(format!("{sql}\n      engine: {}", msg.lines().next().unwrap_or("")));
            } else {
                t.name_flagged += 1;
            }
        }
    }
    eprintln!(
        "\n=== {name}: {} statements | engine: {} valid, {} semantic errors, {} syntax errors, {} not asked",
        t.total, t.valid, t.semantic, t.syntax, t.skipped
    );
    eprintln!(
        "    (a) false syntax errors on accepted statements: {} of {}",
        t.false_errors.len(),
        t.valid + t.semantic
    );
    eprintln!(
        "    (a') false resolution diagnostics on fully valid statements: {} of {}",
        t.false_resolution.len(),
        t.valid
    );
    eprintln!(
        "    (c) engine-confirmed unknown / ambiguous names the resolver also flags: {} of {} ({:.0}%)",
        t.name_flagged,
        t.name_errors,
        if t.name_errors == 0 { 0.0 } else { 100.0 * t.name_flagged as f64 / t.name_errors as f64 }
    );
    eprintln!(
        "    (b) syntax errors the parser also flags: {} of {} ({:.0}%)",
        t.flagged,
        t.syntax,
        if t.syntax == 0 { 0.0 } else { 100.0 * t.flagged as f64 / t.syntax as f64 }
    );
    t
}

fn assert_clean(t: &Tally) {
    if std::env::var("THETA_DIFF_VERBOSE").is_ok() {
        for m in &t.missed {
            eprintln!("    missed: {m}");
        }
        for m in &t.name_missed {
            eprintln!("    name-missed: {m}");
        }
    }
    assert!(
        t.must_missed.is_empty(),
        "typos.sql: syntax errors that neither the parser nor the linter flag:\n  {}",
        t.must_missed.join("\n  ")
    );
    assert!(t.false_errors.is_empty(), "false syntax errors:\n  {}", t.false_errors.join("\n  "));
    assert!(
        t.false_resolution.is_empty(),
        "false resolution diagnostics:\n  {}",
        t.false_resolution.join("\n  ")
    );
}

fn corpus_for(files: &[&str], kind: DatabaseKind) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for f in files {
        for s in load(f) {
            // The engines judge one statement; scripts found in tests would be judged by their first part only.
            if theta_sql::split_statements(&s, kind).len() > 1 {
                continue;
            }
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- PostgreSQL

fn pg_syntax_code(code: &str) -> bool {
    // 42601 syntax_error, 42602 invalid_name, 42000 syntax_error_or_access_rule_violation (class default)
    matches!(code, "42601" | "42602" | "42000")
}

async fn pg_verdicts(conn: &dyn Connection, corpus: &[String]) -> Vec<(String, Verdict)> {
    let o = ExecOptions { row_limit: Some(5) };
    let _ = conn.execute("SET statement_timeout = '4s'", &o).await;
    let mut out = Vec::new();
    for sql in corpus {
        let s = strip_semicolon(sql);
        let fw = first_word(s);
        let low = s.to_ascii_lowercase();
        let verdict = if matches!(fw.as_str(), "COPY" | "LISTEN" | "UNLISTEN" | "NOTIFY")
            || low.contains("pg_advisory")
            || low.contains("create extension")
        {
            Verdict::Skipped
        } else {
            let preparable = matches!(
                fw.as_str(),
                "SELECT" | "WITH" | "VALUES" | "INSERT" | "UPDATE" | "DELETE" | "MERGE"
            ) || s.starts_with('(');
            let (res, cleanup) = if preparable {
                (conn.execute(&format!("PREPARE tdiff AS {s}"), &o).await, Some("DEALLOCATE tdiff"))
            } else if fw == "TABLE" {
                (conn.execute(&format!("EXPLAIN {s}"), &o).await, None)
            } else {
                let _ = conn.execute("BEGIN", &o).await;
                let r = conn.execute(s, &o).await;
                let _ = conn.execute("ROLLBACK", &o).await;
                (r, None)
            };
            if let Some(c) = cleanup {
                let _ = conn.execute(c, &o).await;
            }
            match res {
                Ok(_) => Verdict::Valid,
                Err(Error::Query { code: Some(c), message, .. }) if pg_syntax_code(&c) => {
                    Verdict::Syntax(message)
                }
                Err(Error::Query { message, .. }) => Verdict::Semantic(message),
                Err(e) => panic!("PostgreSQL connection problem on `{s}`: {e:?}"),
            }
        };
        out.push((sql.clone(), verdict));
    }
    out
}

#[tokio::test]
async fn postgres() {
    let Ok(conn) = theta_drivers::connect(&pg_config()).await else {
        eprintln!("skipping the PostgreSQL leg: server unreachable");
        return;
    };
    let catalog = conn.introspect().await.expect("introspect");
    let corpus = corpus_for(
        &["common.sql", "pg.sql", "typos.sql", "neighbours.sql", "broken.sql", "extracted.sql"],
        DatabaseKind::Postgres,
    );
    let v = pg_verdicts(conn.as_ref(), &corpus).await;
    let t = judge_all("PostgreSQL 16", DatabaseKind::Postgres, Some(&catalog), v, false, true);
    assert_clean(&t);
}

// ---------------------------------------------------------------- MariaDB

fn mysql_syntax_code(code: &str) -> bool {
    matches!(code, "1064" | "1149")
}

async fn mysql_verdicts(conn: &dyn Connection, corpus: &[String]) -> Vec<(String, Verdict)> {
    let o = ExecOptions { row_limit: Some(5) };
    let mut out = Vec::new();
    for sql in corpus {
        let s = strip_semicolon(sql);
        let low = s.to_ascii_lowercase();
        let verdict = if low.starts_with("prepare ")
            || low.starts_with("execute ")
            || low.starts_with("deallocate")
            || low.starts_with("drop prepare")
        {
            // PREPARE cannot be prepared; its own syntax is checked by running the text as is, harmlessly.
            Verdict::Skipped
        } else {
            let esc = s.replace('\\', "\\\\").replace('\'', "\\'");
            match conn.execute(&format!("PREPARE tdiff FROM '{esc}'"), &o).await {
                Ok(_) => {
                    let _ = conn.execute("DEALLOCATE PREPARE tdiff", &o).await;
                    Verdict::Valid
                }
                Err(Error::Query { code: Some(c), message, .. }) if mysql_syntax_code(&c) => {
                    Verdict::Syntax(message)
                }
                // 1295: the command parsed but is not preparable (USE, SHOW ..., LOCK ...).
                Err(Error::Query { code: Some(c), .. }) if c == "1295" => {
                    Verdict::Semantic("not preparable".into())
                }
                Err(Error::Query { message, .. }) => Verdict::Semantic(message),
                Err(e) => panic!("MariaDB connection problem on `{s}`: {e:?}"),
            }
        };
        out.push((sql.clone(), verdict));
    }
    out
}

#[tokio::test]
async fn mariadb() {
    let Ok(conn) = theta_drivers::connect(&mysql_config()).await else {
        eprintln!("skipping the MariaDB leg: server unreachable");
        return;
    };
    let catalog = conn.introspect().await.expect("introspect");
    let corpus = corpus_for(
        &["common.sql", "mysql.sql", "typos.sql", "neighbours.sql", "broken.sql", "extracted.sql"],
        DatabaseKind::MySql,
    );
    let v = mysql_verdicts(conn.as_ref(), &corpus).await;
    // `1295` statements are "semantic" for the syntax assertion but not for resolution (only Valid counts there).
    let t = judge_all("MariaDB 10.11", DatabaseKind::MySql, Some(&catalog), v, false, false);
    assert_clean(&t);
}

// ---------------------------------------------------------------- SQLite

fn sqlite_verdict(db: &rusqlite::Connection, sql: &str) -> Verdict {
    match db.prepare(sql) {
        Ok(_) => Verdict::Valid,
        Err(e) => {
            let msg = e.to_string();
            let l = msg.to_ascii_lowercase();
            if l.contains("unknown database") {
                // SQLite resolves the schema name while it is still parsing, so it can report this for a
                // statement that is also incomplete: no verdict.
                Verdict::Skipped
            } else if l.contains("syntax error")
                || l.contains("incomplete input")
                || l.contains("unrecognized token")
                || l.contains("unterminated")
                || l.contains("near \"")
                || l.contains("unrecognized statement")
            {
                Verdict::Syntax(msg)
            } else {
                Verdict::Semantic(msg)
            }
        }
    }
}

#[tokio::test]
async fn sqlite() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let schema = std::fs::read_to_string(corpus_dir().join("schema_sqlite.sql")).unwrap();
    db.execute_batch(&schema).unwrap();
    let mut cfg = sqlite_config(":memory:");
    cfg.path = ":memory:".into();
    let conn = theta_drivers::connect(&cfg).await.unwrap();
    for stmt in schema_statements(&schema) {
        run(conn.as_ref(), &stmt).await;
    }
    let catalog = conn.introspect().await.unwrap();
    let corpus = corpus_for(
        &["common.sql", "sqlite.sql", "typos.sql", "neighbours.sql", "broken.sql", "extracted.sql"],
        DatabaseKind::Sqlite,
    );
    let v: Vec<_> = corpus.iter().map(|s| (s.clone(), sqlite_verdict(&db, strip_semicolon(s)))).collect();
    let t = judge_all("SQLite (bundled)", DatabaseKind::Sqlite, Some(&catalog), v, false, true);
    assert_clean(&t);
}

// ---------------------------------------------------------------- DuckDB

#[cfg(feature = "duckdb")]
#[tokio::test]
async fn duckdb() {
    let db = duckdb::Connection::open_in_memory().unwrap();
    let schema = std::fs::read_to_string(corpus_dir().join("schema_duckdb.sql")).unwrap();
    db.execute_batch(&schema).unwrap();
    // DuckDB binds table functions and COPY while preparing: `copy t from stdin` reads standard input (and blocks
    // for ever where it stays open, as on a CI runner), `read_csv('x.csv')` opens files, and a missing extension
    // is fetched from the network. The verdict must depend on the SQL alone, so the oracle gets no outside
    // world; those statements then fail with a permission error, which counts as "not a syntax error".
    // It also folds constants while preparing: `select generate_series(1, 2000000000)` builds the two-billion
    // element list there and then, which exhausted a 16 GB CI runner. A small memory limit turns
    // that into an ordinary out-of-memory error.
    db.execute_batch(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         SET enable_external_access = false; SET memory_limit = '512MB';",
    )
    .unwrap();
    let mut cfg = ConnectionConfig::new("t-duck", "duck", DatabaseKind::DuckDb);
    cfg.path = ":memory:".into();
    let conn = theta_drivers::connect(&cfg).await.unwrap();
    for stmt in schema_statements(&schema) {
        run(conn.as_ref(), &stmt).await;
    }
    let catalog = conn.introspect().await.unwrap();
    let corpus = corpus_for(
        &["common.sql", "pg.sql", "duckdb.sql", "typos.sql", "neighbours.sql", "broken.sql", "extracted.sql"],
        DatabaseKind::DuckDb,
    );
    let v: Vec<_> = corpus
        .iter()
        .map(|s| {
            let sql = strip_semicolon(s);
            // Written straight to stderr (not through the test harness's capture) so that when the engine takes
            // the whole process down, the log still names the statement it was given.
            if std::env::var_os("THETA_DIFF_TRACE").is_some() {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "duckdb prepare: {sql:?}");
            }
            let verdict = match db.prepare(sql) {
                Ok(_) => Verdict::Valid,
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("Parser Error") { Verdict::Syntax(msg) } else { Verdict::Semantic(msg) }
                }
            };
            (s.clone(), verdict)
        })
        .collect();
    let t = judge_all("DuckDB 1.2", DatabaseKind::DuckDb, Some(&catalog), v, false, false);
    assert_clean(&t);
}

// ---------------------------------------------------------------- ClickHouse and T-SQL

#[tokio::test]
async fn clickhouse() {
    let corpus = load("clickhouse.sql");
    let mut cfg = ConnectionConfig::new("t-ch", "ch", DatabaseKind::ClickHouse);
    cfg.host = "127.0.0.1".into();
    cfg.user = "default".into();
    let conn = theta_drivers::connect(&cfg).await.ok();
    let o = ExecOptions { row_limit: Some(5) };
    let mut v = Vec::new();
    for sql in &corpus {
        let s = strip_semicolon(sql);
        let readonly =
            matches!(first_word(s).as_str(), "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXISTS");
        let verdict = match (&conn, readonly) {
            (Some(c), true) => match c.execute(s, &o).await {
                Ok(_) => Verdict::Valid,
                Err(Error::Query { code: Some(code), message, .. }) if code == "62" => {
                    Verdict::Syntax(message)
                }
                Err(Error::Query { message, .. }) => Verdict::Semantic(message),
                Err(_) => Verdict::Skipped,
            },
            _ => Verdict::Skipped,
        };
        v.push((sql.clone(), verdict));
    }
    if conn.is_none() {
        eprintln!(
            "ClickHouse server unreachable: the documentation corpus is only checked for parser errors"
        );
    }
    // The docs examples are assumed valid whatever the (very old) local server says about them.
    let v: Vec<_> = v.into_iter().map(|(s, _)| (s, Verdict::Skipped)).collect();
    let t = judge_all("ClickHouse (documentation examples)", DatabaseKind::ClickHouse, None, v, true, true);
    assert_clean(&t);
}

#[test]
fn tsql() {
    let v: Vec<_> = load("tsql.sql").into_iter().map(|s| (s, Verdict::Skipped)).collect();
    let t = judge_all("T-SQL (documentation examples)", DatabaseKind::SqlServer, None, v, true, true);
    assert_clean(&t);
}
