//! SQLite driver.
//!
//! `rusqlite` is synchronous, so every connection owns one dedicated OS thread
//! that runs jobs sent over a channel. That gives us, for free, the contract's
//! "statements run one at a time, in call order" guarantee, and keeps blocking
//! SQLite calls off the Tokio workers. `cancel` never goes through the queue: it
//! uses SQLite's thread-safe interrupt handle.

use crate::util::{kind_from_type_name, leading_keyword, reports_rows_affected};
use async_trait::async_trait;
use quarry_core::*;
use rusqlite::types::ValueRef;
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Instant;
use tokio::sync::oneshot;

type Job = Box<dyn FnOnce(&mut rusqlite::Connection) + Send>;

pub struct SqliteDriver;

#[async_trait]
impl Driver for SqliteDriver {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::Sqlite
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        if config.path.trim().is_empty() {
            return Err(Error::Connect(
                "no SQLite database path given (use `:memory:` for a scratch database)".into(),
            ));
        }
        let path = config.path.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("quarry-sqlite".into())
            .spawn(move || {
                let opened = if path == ":memory:" {
                    rusqlite::Connection::open_in_memory()
                } else {
                    rusqlite::Connection::open(&path)
                };
                let mut conn = match opened {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(Error::Connect(e.to_string())));
                        return;
                    }
                };
                // Wait a little for other writers instead of failing instantly.
                let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                // Foreign keys are off by default in SQLite, which surprises
                // users who declared them; a database IDE should show real behaviour.
                let _ = conn.execute_batch("PRAGMA foreign_keys = ON");
                // Touch the file so that a non-database file fails here, at connect time.
                if let Err(e) = conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
                    let _ = ready_tx.send(Err(Error::Connect(e.to_string())));
                    return;
                }
                let _ = ready_tx.send(Ok(conn.get_interrupt_handle()));
                while let Ok(job) = job_rx.recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| Error::Connect(e.to_string()))?;
        let interrupt = ready_rx.await.map_err(|_| Error::Connect("SQLite thread died".into()))??;
        Ok(Box::new(SqliteConnection { jobs: std::sync::Mutex::new(Some(job_tx)), interrupt }))
    }
}

pub struct SqliteConnection {
    jobs: std::sync::Mutex<Option<mpsc::Sender<Job>>>,
    interrupt: rusqlite::InterruptHandle,
}

impl SqliteConnection {
    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> T + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        let sender = self.jobs.lock().unwrap().clone().ok_or(Error::Closed)?;
        sender
            .send(Box::new(move |c| {
                let _ = tx.send(f(c));
            }))
            .map_err(|_| Error::Closed)?;
        rx.await.map_err(|_| Error::Closed)
    }
}

#[async_trait]
impl Connection for SqliteConnection {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::Sqlite
    }

    async fn server_info(&self) -> Result<ServerInfo> {
        self.call(|c| {
            let version: String =
                c.query_row("SELECT sqlite_version()", [], |r| r.get(0)).unwrap_or_default();
            let file = c.path().map(|p| p.to_string()).filter(|p| !p.is_empty());
            ServerInfo {
                version: format!("SQLite {version}"),
                current_database: Some(file.unwrap_or_else(|| ":memory:".into())),
                current_user: None,
            }
        })
        .await
    }

    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        let sql = sql.to_string();
        let limit = options.row_limit;
        self.call(move |c| run_statement(c, &sql, limit)).await?
    }

    async fn cancel(&self) -> Result<()> {
        self.interrupt.interrupt();
        Ok(())
    }

    async fn introspect(&self) -> Result<Catalog> {
        self.call(introspect).await?
    }

    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        let sql = sql.to_string();
        self.call(move |c| explain(c, &sql, analyze)).await?
    }

    async fn in_transaction(&self) -> bool {
        self.call(|c| !c.is_autocommit()).await.unwrap_or(false)
    }

    async fn close(&self) -> Result<()> {
        // Dropping the sender ends the worker loop; the rusqlite connection is
        // closed when the thread drops it.
        self.jobs.lock().unwrap().take();
        Ok(())
    }
}

fn map_err(e: rusqlite::Error) -> Error {
    use rusqlite::ErrorCode;
    match &e {
        rusqlite::Error::SqliteFailure(f, msg) => {
            if f.code == ErrorCode::OperationInterrupted {
                return Error::Cancelled;
            }
            Error::Query {
                message: msg.clone().unwrap_or_else(|| e.to_string()),
                code: Some(format!("{:?}", f.code)),
                detail: Some(format!("extended result code {}", f.extended_code)),
                hint: None,
                position: None,
            }
        }
        rusqlite::Error::SqlInputError { error, msg, offset, .. } => {
            if error.code == ErrorCode::OperationInterrupted {
                return Error::Cancelled;
            }
            Error::Query {
                message: msg.clone(),
                code: Some(format!("{:?}", error.code)),
                detail: None,
                hint: None,
                // SQLite reports a byte offset already; negative means "unknown".
                position: usize::try_from(*offset).ok().map(|offset| ErrorPosition { offset }),
            }
        }
        rusqlite::Error::MultipleStatement => {
            Error::query("only one SQL statement can be executed at a time")
        }
        _ => Error::query(e.to_string()),
    }
}

fn type_kind_for_decl(decl: &str) -> TypeKind {
    // SQLite's own affinity rules would classify nearly everything as NUMERIC or
    // TEXT; users care about what they wrote in the DDL, so use the name.
    let d = decl.to_ascii_lowercase();
    if d.is_empty() {
        return TypeKind::Other;
    }
    let k = kind_from_type_name(&d);
    if k != TypeKind::Other {
        return k;
    }
    if d.contains("int") {
        TypeKind::Integer
    } else if d.contains("char") || d.contains("clob") || d.contains("text") {
        TypeKind::Text
    } else if d.contains("blob") {
        TypeKind::Bytes
    } else if d.contains("real") || d.contains("floa") || d.contains("doub") {
        TypeKind::Float
    } else {
        TypeKind::Other
    }
}

fn cell(v: ValueRef<'_>, kind: TypeKind) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => {
            if kind == TypeKind::Bool && (i == 0 || i == 1) {
                Value::Bool(i == 1)
            } else {
                Value::Int(i)
            }
        }
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            match kind {
                TypeKind::Json => Value::Json(s),
                TypeKind::Date => Value::Date(s),
                TypeKind::Time => Value::Time(s),
                TypeKind::Timestamp => Value::Timestamp(s),
                TypeKind::Decimal => Value::Decimal(s),
                _ => Value::Text(s),
            }
        }
        ValueRef::Blob(b) => Value::Bytes(b.to_vec()),
    }
}

fn kind_of_value(v: &Value) -> TypeKind {
    match v {
        Value::Int(_) | Value::UInt(_) => TypeKind::Integer,
        Value::Float(_) => TypeKind::Float,
        Value::Text(_) => TypeKind::Text,
        Value::Bytes(_) => TypeKind::Bytes,
        _ => TypeKind::Other,
    }
}

fn run_statement(
    conn: &mut rusqlite::Connection,
    sql: &str,
    limit: Option<usize>,
) -> Result<StatementOutcome> {
    let started = Instant::now();
    let mut stmt = conn.prepare(sql).map_err(map_err)?;
    let ncols = stmt.column_count();
    if ncols == 0 {
        let n = stmt.execute([]).map_err(map_err)?;
        drop(stmt);
        return Ok(StatementOutcome {
            result: None,
            rows_affected: reports_rows_affected(sql).then_some(n as u64),
            notices: Vec::new(),
            elapsed: started.elapsed(),
        });
    }
    let mut columns: Vec<Column> = stmt
        .columns_with_metadata()
        .map(|m| {
            let decl = String::new();
            let origin = match (m.table_name(), m.origin_name()) {
                (Some(t), Some(c)) => Some(ColumnOrigin {
                    schema: m.database_name().map(str::to_string),
                    table: t.to_string(),
                    column: c.to_string(),
                }),
                _ => None,
            };
            Column { name: m.name().to_string(), type_name: decl, kind: TypeKind::Other, origin }
        })
        .collect();
    for (i, c) in stmt.columns().enumerate() {
        let decl = c.decl_type().unwrap_or("").to_string();
        columns[i].kind = type_kind_for_decl(&decl);
        columns[i].type_name = decl;
    }
    let kinds: Vec<TypeKind> = columns.iter().map(|c| c.kind).collect();
    let mut rows: Vec<Vec<Value>> = Vec::new();
    let mut truncated = false;
    {
        let mut q = stmt.query([]).map_err(map_err)?;
        while let Some(row) = q.next().map_err(map_err)? {
            if limit.is_some_and(|l| rows.len() >= l) {
                truncated = true;
                break;
            }
            let mut out = Vec::with_capacity(ncols);
            for (i, k) in kinds.iter().enumerate() {
                out.push(cell(row.get_ref(i).map_err(map_err)?, *k));
            }
            rows.push(out);
        }
    }
    drop(stmt);
    // Expression columns have no declared type: fall back to the first non-NULL value.
    for (i, col) in columns.iter_mut().enumerate() {
        if col.kind == TypeKind::Other
            && col.type_name.is_empty()
            && let Some(v) = rows.iter().map(|r| &r[i]).find(|v| !v.is_null())
        {
            col.kind = kind_of_value(v);
        }
    }
    let rows_affected = reports_rows_affected(sql).then(|| conn.changes());
    Ok(StatementOutcome {
        result: Some(ResultSet { columns, rows, truncated }),
        rows_affected,
        notices: Vec::new(),
        elapsed: started.elapsed(),
    })
}

// ---------------------------------------------------------------------------
// Introspection
// ---------------------------------------------------------------------------

fn introspect(conn: &mut rusqlite::Connection) -> Result<Catalog> {
    let dbs: Vec<String> = {
        let mut stmt = conn.prepare("SELECT name FROM pragma_database_list ORDER BY seq").map_err(map_err)?;
        let names = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(map_err)?;
        names.collect::<std::result::Result<_, _>>().map_err(map_err)?
    };
    let mut schemas = Vec::new();
    for db in dbs.iter().filter(|d| d.as_str() != "temp") {
        schemas.push(introspect_schema(conn, db)?);
    }
    Ok(Catalog { schemas, default_schema: Some("main".into()) })
}

fn q(name: &str) -> String {
    DatabaseKind::Sqlite.quote_ident(name)
}

fn introspect_schema(conn: &rusqlite::Connection, db: &str) -> Result<Schema> {
    struct Obj {
        kind: TableKind,
        name: String,
    }
    let objs: Vec<Obj> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT type, name FROM {}.sqlite_master WHERE type IN ('table','view') \
                 AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
                q(db)
            ))
            .map_err(map_err)?;
        let rows = stmt
            .query_map([], |r| {
                let t: String = r.get(0)?;
                Ok(Obj {
                    kind: if t == "view" { TableKind::View } else { TableKind::Table },
                    name: r.get(1)?,
                })
            })
            .map_err(map_err)?;
        rows.collect::<std::result::Result<_, _>>().map_err(map_err)?
    };
    // Index definitions, keyed by index name (auto-indexes have NULL sql).
    let mut index_sql: BTreeMap<String, String> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT name, sql FROM {}.sqlite_master WHERE type = 'index' AND sql IS NOT NULL",
                q(db)
            ))
            .map_err(map_err)?;
        let rows =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(map_err)?;
        for r in rows {
            let (n, s) = r.map_err(map_err)?;
            index_sql.insert(n, s);
        }
    }
    let mut estimates: BTreeMap<String, u64> = BTreeMap::new();
    // sqlite_stat1 only exists after ANALYZE. Every row (table or index) starts
    // with the table's row count, so the first one seen per table is enough.
    if let Ok(mut stmt) = conn.prepare(&format!("SELECT tbl, stat FROM {}.sqlite_stat1", q(db)))
        && let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
    {
        for (t, s) in rows.flatten() {
            if let Some(n) = s.split_whitespace().next().and_then(|n| n.parse().ok()) {
                estimates.entry(t).or_insert(n);
            }
        }
    }
    let mut tables = Vec::new();
    for o in objs {
        let mut columns = Vec::new();
        let mut pk: Vec<(i64, String)> = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_xinfo(?1, ?2) WHERE hidden IN (0, 2, 3) ORDER BY cid")
                .map_err(map_err)?;
            let rows = stmt
                .query_map([&o.name, db], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })
                .map_err(map_err)?;
            for r in rows {
                let (name, ty, notnull, dflt, pkpos) = r.map_err(map_err)?;
                if pkpos > 0 {
                    pk.push((pkpos, name.clone()));
                }
                columns.push(ColumnInfo {
                    kind: type_kind_for_decl(&ty),
                    data_type: ty,
                    nullable: notnull == 0 && pkpos == 0,
                    default: dflt,
                    comment: None,
                    geometry: None,
                    name,
                });
            }
        }
        pk.sort();
        let mut foreign_keys: Vec<ForeignKey> = Vec::new();
        if o.kind == TableKind::Table {
            let mut stmt = conn
                .prepare("SELECT id, seq, \"table\", \"from\", \"to\", on_update, on_delete FROM pragma_foreign_key_list(?1, ?2) ORDER BY id, seq")
                .map_err(map_err)?;
            let rows = stmt
                .query_map([&o.name, db], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                })
                .map_err(map_err)?;
            let mut last_id = None;
            for r in rows {
                let (id, table, from, to, on_update, on_delete) = r.map_err(map_err)?;
                if last_id != Some(id) {
                    last_id = Some(id);
                    foreign_keys.push(ForeignKey {
                        name: format!("fk_{}_{}", o.name, id),
                        columns: vec![],
                        referenced_schema: Some(db.to_string()),
                        referenced_table: table,
                        referenced_columns: vec![],
                        on_delete: Some(on_delete).filter(|s| s != "NO ACTION"),
                        on_update: Some(on_update).filter(|s| s != "NO ACTION"),
                    });
                }
                let fk = foreign_keys.last_mut().unwrap();
                fk.columns.push(from);
                // `to` is NULL when the FK targets the parent's primary key implicitly.
                if let Some(to) = to {
                    fk.referenced_columns.push(to);
                }
            }
        }
        let mut indexes = Vec::new();
        if o.kind == TableKind::Table {
            let list: Vec<(String, bool, String)> = {
                let mut stmt = conn
                    .prepare("SELECT name, \"unique\", origin FROM pragma_index_list(?1, ?2) ORDER BY seq")
                    .map_err(map_err)?;
                let rows = stmt
                    .query_map([&o.name, db], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)))
                    .map_err(map_err)?;
                rows.collect::<std::result::Result<_, _>>().map_err(map_err)?
            };
            for (name, unique, origin) in list {
                let mut stmt = conn
                    .prepare("SELECT name FROM pragma_index_info(?1, ?2) ORDER BY seqno")
                    .map_err(map_err)?;
                let cols: Vec<Option<String>> = stmt
                    .query_map([&name, db], |r| r.get(0))
                    .map_err(map_err)?
                    .collect::<std::result::Result<_, _>>()
                    .map_err(map_err)?;
                indexes.push(Index {
                    definition: index_sql.get(&name).cloned(),
                    name,
                    // NULL name = an expression column.
                    columns: cols.into_iter().map(|c| c.unwrap_or_else(|| "<expression>".into())).collect(),
                    unique,
                    primary: origin == "pk",
                    method: Some("btree".into()),
                });
            }
        }
        tables.push(Table {
            row_estimate: estimates.get(&o.name).copied(),
            name: o.name,
            kind: o.kind,
            columns,
            primary_key: pk.into_iter().map(|(_, n)| n).collect(),
            foreign_keys,
            indexes,
            comment: None,
        });
    }
    // sqlite_sequence only exists once an AUTOINCREMENT table was created.
    let sequences = conn
        .prepare(&format!("SELECT name FROM {}.sqlite_sequence ORDER BY name", q(db)))
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    Ok(Schema { name: db.to_string(), tables, routines: vec![], sequences })
}

// ---------------------------------------------------------------------------
// EXPLAIN
// ---------------------------------------------------------------------------

fn explain(conn: &mut rusqlite::Connection, sql: &str, analyze: bool) -> Result<Plan> {
    let rows: Vec<(i64, i64, String)> = {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).map_err(map_err)?;
        let it = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(3)?)))
            .map_err(map_err)?;
        it.collect::<std::result::Result<_, _>>().map_err(map_err)?
    };
    let raw =
        rows.iter().map(|(id, parent, d)| format!("{id}\t{parent}\t{d}")).collect::<Vec<_>>().join("\n");
    let mut root = plan_from_eqp(&rows);
    let mut execution_time_ms = None;
    if analyze {
        // SQLite has no per-node timing; run the statement to completion inside a
        // transaction we roll back so writes leave no trace, and report the total.
        let in_txn = !conn.is_autocommit();
        let begin = if in_txn { "SAVEPOINT quarry_explain" } else { "BEGIN" };
        let end = if in_txn { "ROLLBACK TO quarry_explain; RELEASE quarry_explain" } else { "ROLLBACK" };
        conn.execute_batch(begin).map_err(map_err)?;
        let started = Instant::now();
        let outcome = (|| -> Result<u64> {
            let mut stmt = conn.prepare(sql).map_err(map_err)?;
            if stmt.column_count() == 0 {
                return stmt.execute([]).map(|n| n as u64).map_err(map_err);
            }
            let mut q = stmt.query([]).map_err(map_err)?;
            let mut n = 0u64;
            while q.next().map_err(map_err)?.is_some() {
                n += 1;
            }
            Ok(n)
        })();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let _ = conn.execute_batch(end);
        let n = outcome?;
        root.actual_rows = Some(n as f64);
        root.actual_total_ms = Some(ms);
        root.loops = Some(1.0);
        execution_time_ms = Some(ms);
    }
    Ok(Plan { root, analyzed: analyze, planning_time_ms: None, execution_time_ms, raw })
}

pub(crate) fn plan_from_eqp(rows: &[(i64, i64, String)]) -> PlanNode {
    // `parent` is the id of the enclosing row; 0 means top level. Rows arrive in
    // an order where parents precede their children.
    fn build(rows: &[(i64, i64, String)], parent: i64) -> Vec<PlanNode> {
        rows.iter()
            .filter(|(_, p, _)| *p == parent)
            .map(|(id, _, detail)| {
                let mut node = node_from_detail(detail);
                node.children = build(rows, *id);
                node
            })
            .collect()
    }
    let mut top = build(rows, 0);
    if top.len() == 1 {
        top.pop().unwrap()
    } else {
        PlanNode { node_type: "QUERY PLAN".into(), children: top, ..Default::default() }
    }
}

fn node_from_detail(detail: &str) -> PlanNode {
    let mut node = PlanNode { detail: Some(detail.to_string()), ..Default::default() };
    let word = leading_keyword(detail);
    if word == "SCAN" || word == "SEARCH" {
        node.node_type = word;
        let rest = detail[node.node_type.len()..].trim_start();
        let rest = rest.strip_prefix("TABLE ").unwrap_or(rest);
        let (head, using) = match rest.find(" USING ") {
            Some(i) => (&rest[..i], Some(&rest[i + 7..])),
            None => (rest, None),
        };
        let mut parts = head.split_whitespace();
        node.relation = parts.next().map(str::to_string);
        if parts.next().is_some_and(|w| w.eq_ignore_ascii_case("AS")) {
            node.alias = parts.next().map(str::to_string);
        }
        if let Some(u) = using {
            let u = u.trim_start_matches("COVERING ");
            if let Some(i) = u.strip_prefix("INDEX ") {
                let (name, cond) = split_cond(i);
                node.index = Some(name.to_string());
                if let Some(c) = cond {
                    node.extra.insert("Index Cond".into(), c.to_string());
                }
            } else if u.starts_with("INTEGER PRIMARY KEY") {
                node.index = Some("rowid".into());
                if let Some((_, c)) = u.split_once('(') {
                    node.extra.insert("Index Cond".into(), format!("({c}"));
                }
            }
            if detail.contains("COVERING INDEX") {
                node.extra.insert("Covering".into(), "true".into());
            }
        }
    } else {
        node.node_type = detail.to_string();
    }
    node
}

fn split_cond(s: &str) -> (&str, Option<&str>) {
    match s.find(" (") {
        Some(i) => (&s[..i], Some(s[i + 1..].trim())),
        None => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eqp_tree_is_built_from_parent_ids() {
        let rows = vec![
            (3, 0, "SEARCH o USING INDEX idx_o_c (customer_id=?)".to_string()),
            (7, 0, "SCAN c AS cu".to_string()),
            (9, 7, "USE TEMP B-TREE FOR ORDER BY".to_string()),
        ];
        let plan = plan_from_eqp(&rows);
        assert_eq!(plan.node_type, "QUERY PLAN");
        assert_eq!(plan.children.len(), 2);
        let s = &plan.children[0];
        assert_eq!(
            (s.node_type.as_str(), s.relation.as_deref(), s.index.as_deref()),
            ("SEARCH", Some("o"), Some("idx_o_c"))
        );
        assert_eq!(s.extra["Index Cond"], "(customer_id=?)");
        let c = &plan.children[1];
        assert_eq!(c.alias.as_deref(), Some("cu"));
        assert_eq!(c.children[0].node_type, "USE TEMP B-TREE FOR ORDER BY");
    }

    #[test]
    fn single_top_level_row_becomes_root() {
        let plan = plan_from_eqp(&[(2, 0, "SCAN TABLE t".to_string())]);
        assert_eq!(plan.node_type, "SCAN");
        assert_eq!(plan.relation.as_deref(), Some("t"));
    }
}
