//! Server sessions: what to ask each product, how to read the answer, and the statements that cancel a
//! query or end a session.
//!
//! Every product's list query returns the same aliased columns (`sid, usr, db, client, app, state, wait,
//! txn_age, query_age, query, blocked_by, is_self`), so one parser serves all of them and a new product only
//! has to write a query. Blocking is best-effort everywhere: PostgreSQL and SQL Server report it in the same
//! row; MySQL needs a second query against the InnoDB lock tables, which may be denied, so a failure there
//! must not hide the sessions (the caller passes the error text on as a note).

use super::{col, number, text, truthy};
use theta_core::{DatabaseKind, Feature, ResultSet};

/// A marker in ClickHouse's list query: its `system.processes` row for the query that lists them carries it,
/// which is how the app knows the row is its own (ClickHouse has no session id to compare).
pub const CLICKHOUSE_MARKER: &str = "/* theta-sessions */";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Session {
    /// Process id (PostgreSQL), connection id (MySQL), `session_id` (SQL Server) or `query_id` (ClickHouse).
    pub id: String,
    pub user: String,
    pub database: String,
    pub client: String,
    pub application: String,
    /// `active`, `idle`, `idle in transaction`, ...
    pub state: String,
    /// The wait event, lock wait or MySQL thread state.
    pub wait: String,
    /// Seconds since the session's transaction began; `None` outside a transaction.
    pub txn_age: Option<f64>,
    /// Seconds since the current (or last) statement began.
    pub query_age: Option<f64>,
    pub query: String,
    /// Ids of the sessions this one is waiting for.
    pub blocked_by: Vec<String>,
    /// The session Theta itself uses for this very listing.
    pub is_self: bool,
}

/// The queries for one connection. `blockers` is the optional second query.
#[derive(Debug, Clone, PartialEq)]
pub struct Queries {
    pub sessions: String,
    pub blockers: Option<String>,
    /// Asks the connection which id its own session has, to mark Theta's other sessions.
    pub own_id: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Stop the running statement and keep the session.
    Cancel,
    /// End the session; its open transaction is rolled back.
    Terminate,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Cancel => "Cancel Query",
            Action::Terminate => "Terminate Session",
        }
    }
}

fn is_mariadb(banner: &str) -> bool {
    banner.to_ascii_lowercase().contains("mariadb")
}

/// The queries to list sessions on `kind`; `Err` carries the reason a product cannot (`Features`).
/// `banner` is the server's version text, which tells MariaDB from MySQL where their lock tables differ.
pub fn queries(kind: DatabaseKind, banner: &str) -> Result<Queries, &'static str> {
    if let Some(why) = kind.unsupported(Feature::Sessions) {
        return Err(why);
    }
    use theta_core::Family::*;
    Ok(match kind.family() {
        Postgres => Queries {
            sessions: POSTGRES.trim().to_string(),
            blockers: None,
            own_id: Some("SELECT pg_backend_pid()::text"),
        },
        MySql => Queries {
            sessions: MYSQL.trim().to_string(),
            blockers: Some(
                if is_mariadb(banner) { MARIADB_BLOCKERS } else { MYSQL_BLOCKERS }.trim().to_string(),
            ),
            own_id: Some("SELECT CAST(CONNECTION_ID() AS CHAR)"),
        },
        SqlServer => Queries {
            sessions: MSSQL.trim().to_string(),
            blockers: None,
            own_id: Some("SELECT CAST(@@SPID AS varchar(20))"),
        },
        ClickHouse => Queries { sessions: CLICKHOUSE.trim().to_string(), blockers: None, own_id: None },
        Sqlite | DuckDb => return Err("an embedded database has no server sessions"),
    })
}

const POSTGRES: &str = r#"
SELECT pid::text AS sid,
       coalesce(usename, '') AS usr,
       coalesce(datname, '') AS db,
       CASE WHEN client_addr IS NULL THEN 'local' ELSE client_addr::text || ':' || client_port END AS client,
       coalesce(application_name, '') AS app,
       coalesce(state, '') AS state,
       CASE WHEN wait_event_type IS NULL THEN '' ELSE wait_event_type || ': ' || coalesce(wait_event, '') END AS wait,
       extract(epoch FROM now() - xact_start)::float8 AS txn_age,
       extract(epoch FROM now() - query_start)::float8 AS query_age,
       coalesce(query, '') AS query,
       array_to_string(pg_blocking_pids(pid), ',') AS blocked_by,
       (pid = pg_backend_pid()) AS is_self
FROM pg_stat_activity
WHERE backend_type = 'client backend'
ORDER BY pid
"#;

// `information_schema.PROCESSLIST` and `INNODB_TRX` exist in MySQL and MariaDB alike. A sleeping session
// with an open InnoDB transaction is what PostgreSQL calls "idle in transaction".
const MYSQL: &str = r#"
SELECT CAST(p.ID AS CHAR) AS sid,
       p.USER AS usr,
       COALESCE(p.DB, '') AS db,
       p.HOST AS client,
       '' AS app,
       CASE WHEN p.COMMAND = 'Sleep' AND t.trx_id IS NOT NULL THEN 'idle in transaction'
            WHEN p.COMMAND = 'Sleep' THEN 'idle' ELSE 'active' END AS state,
       COALESCE(p.STATE, '') AS wait,
       TIMESTAMPDIFF(SECOND, t.trx_started, NOW()) AS txn_age,
       p.TIME AS query_age,
       COALESCE(p.INFO, '') AS query,
       '' AS blocked_by,
       (p.ID = CONNECTION_ID()) AS is_self
FROM information_schema.PROCESSLIST p
LEFT JOIN information_schema.INNODB_TRX t ON t.trx_mysql_thread_id = p.ID
ORDER BY p.ID
"#;

// MariaDB (and MySQL 5.7) keep the waits in information_schema.INNODB_LOCK_WAITS ...
const MARIADB_BLOCKERS: &str = r#"
SELECT CAST(r.trx_mysql_thread_id AS CHAR) AS waiting, CAST(b.trx_mysql_thread_id AS CHAR) AS blocking
FROM information_schema.INNODB_LOCK_WAITS w
JOIN information_schema.INNODB_TRX r ON r.trx_id = w.requesting_trx_id
JOIN information_schema.INNODB_TRX b ON b.trx_id = w.blocking_trx_id
"#;

// ... MySQL 8 moved them to performance_schema.data_lock_waits.
const MYSQL_BLOCKERS: &str = r#"
SELECT CAST(r.trx_mysql_thread_id AS CHAR) AS waiting, CAST(b.trx_mysql_thread_id AS CHAR) AS blocking
FROM performance_schema.data_lock_waits w
JOIN information_schema.innodb_trx r ON r.trx_id = w.REQUESTING_ENGINE_TRANSACTION_ID
JOIN information_schema.innodb_trx b ON b.trx_id = w.BLOCKING_ENGINE_TRANSACTION_ID
"#;

const MSSQL: &str = r#"
SELECT CAST(s.session_id AS varchar(20)) AS sid,
       COALESCE(s.login_name, '') AS usr,
       COALESCE(DB_NAME(r.database_id), DB_NAME(s.database_id), '') AS db,
       COALESCE(c.client_net_address, '') AS client,
       COALESCE(s.program_name, '') AS app,
       CASE WHEN r.session_id IS NOT NULL THEN 'active'
            WHEN s.open_transaction_count > 0 THEN 'idle in transaction' ELSE 'idle' END AS state,
       COALESCE(r.wait_type, '') AS wait,
       DATEDIFF(SECOND, at.transaction_begin_time, GETDATE()) AS txn_age,
       DATEDIFF(SECOND, COALESCE(r.start_time, s.last_request_start_time), GETDATE()) AS query_age,
       COALESCE(t.text, '') AS query,
       CASE WHEN COALESCE(r.blocking_session_id, 0) > 0
            THEN CAST(r.blocking_session_id AS varchar(20)) ELSE '' END AS blocked_by,
       CASE WHEN s.session_id = @@SPID THEN 1 ELSE 0 END AS is_self
FROM sys.dm_exec_sessions s
LEFT JOIN sys.dm_exec_connections c ON c.session_id = s.session_id
LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id
LEFT JOIN sys.dm_tran_session_transactions st ON st.session_id = s.session_id
LEFT JOIN sys.dm_tran_active_transactions at ON at.transaction_id = st.transaction_id
OUTER APPLY sys.dm_exec_sql_text(COALESCE(r.sql_handle, c.most_recent_sql_handle)) t
WHERE s.is_user_process = 1
ORDER BY s.session_id
"#;

// ClickHouse has queries, not sessions. 18.x has `system.processes` with `elapsed` and `address`.
const CLICKHOUSE: &str = r#"
SELECT query_id AS sid, user AS usr, '' AS db, toString(address) AS client, '' AS app,
       'active' AS state, '' AS wait, elapsed AS query_age, query AS query,
       '' AS blocked_by, position(query, '/* theta-sessions */') > 0 AS is_self
FROM system.processes
ORDER BY elapsed DESC
"#;

/// Read the list query's rows. Missing columns read as empty, so a product may omit what it cannot know.
pub fn parse_sessions(rs: &ResultSet) -> Vec<Session> {
    let ix = |n: &str| col(rs, n);
    let (sid, usr, db, client, app, state, wait, txn, qage, query, blocked, own) = (
        ix("sid"),
        ix("usr"),
        ix("db"),
        ix("client"),
        ix("app"),
        ix("state"),
        ix("wait"),
        ix("txn_age"),
        ix("query_age"),
        ix("query"),
        ix("blocked_by"),
        ix("is_self"),
    );
    rs.rows
        .iter()
        .map(|row| {
            let t = |i: Option<usize>| i.and_then(|i| row.get(i)).map(text).unwrap_or_default();
            let n = |i: Option<usize>| i.and_then(|i| row.get(i)).and_then(number).filter(|v| *v >= 0.0);
            Session {
                id: t(sid),
                user: t(usr),
                database: t(db),
                client: t(client),
                application: t(app),
                state: t(state),
                wait: t(wait),
                txn_age: n(txn),
                query_age: n(qage),
                query: t(query),
                blocked_by: t(blocked)
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "0")
                    .map(String::from)
                    .collect(),
                is_self: own.and_then(|i| row.get(i)).is_some_and(truthy),
            }
        })
        .collect()
}

/// Merge the MySQL blockers query (`waiting`, `blocking`) into the sessions.
pub fn apply_blockers(sessions: &mut [Session], rs: &ResultSet) {
    let (Some(w), Some(b)) = (col(rs, "waiting"), col(rs, "blocking")) else { return };
    for row in &rs.rows {
        let (waiting, blocking) = (text(&row[w]), text(&row[b]));
        if let Some(s) = sessions.iter_mut().find(|s| s.id == waiting)
            && !blocking.is_empty()
            && !s.blocked_by.contains(&blocking)
        {
            s.blocked_by.push(blocking);
        }
    }
}

/// Flag Theta's own sessions: the one that ran the listing (`is_self`) and those whose ids the caller
/// found by asking the console connection (`own_ids`).
pub fn mark_own(sessions: &mut [Session], own_ids: &[String]) {
    for s in sessions {
        if own_ids.contains(&s.id) {
            s.is_self = true;
        }
    }
}

/// The statement that performs `action` on session `id`, or the reason it cannot be offered.
/// The id is validated here (digits for the numeric products, a plain token for ClickHouse) and never
/// concatenated otherwise, so a hostile row cannot inject SQL through a confirmation the user trusts.
pub fn kill_statement(kind: DatabaseKind, action: Action, id: &str) -> Result<String, String> {
    use theta_core::Family::*;
    if let Some(why) = kind.unsupported(Feature::Sessions) {
        return Err(why.to_string());
    }
    let numeric = || -> Result<u64, String> {
        id.parse::<u64>().map_err(|_| format!("“{id}” is not a session id"))
    };
    match (kind.family(), action) {
        (Postgres, Action::Cancel) => Ok(format!("SELECT pg_cancel_backend({})", numeric()?)),
        (Postgres, Action::Terminate) => Ok(format!("SELECT pg_terminate_backend({})", numeric()?)),
        (MySql, Action::Cancel) => Ok(format!("KILL QUERY {}", numeric()?)),
        (MySql, Action::Terminate) => Ok(format!("KILL {}", numeric()?)),
        // T-SQL has no statement that stops only the running statement: KILL ends the session.
        (SqlServer, Action::Cancel) => Err(
            "SQL Server has no statement that cancels only the running query; KILL ends the whole session (use Terminate Session)"
                .into(),
        ),
        (SqlServer, Action::Terminate) => Ok(format!("KILL {}", numeric()?)),
        (ClickHouse, Action::Cancel) => {
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("“{id}” is not a query id"));
            }
            Ok(format!("KILL QUERY WHERE query_id = '{id}'"))
        }
        (ClickHouse, Action::Terminate) => {
            Err("ClickHouse has queries, not sessions; only Cancel Query is available".into())
        }
        (Sqlite | DuckDb, _) => Err("an embedded database has no server sessions".into()),
    }
}

/// Why `action` cannot be offered on this product at all, `None` when it can (a particular session id may
/// still be refused by [`kill_statement`]).
pub fn action_unavailable(kind: DatabaseKind, action: Action) -> Option<String> {
    let sample = if kind.family() == theta_core::Family::ClickHouse { "x" } else { "1" };
    kill_statement(kind, action, sample).err()
}

/// What the confirmation says about the effect, per product and action.
pub fn effect_note(kind: DatabaseKind, action: Action) -> &'static str {
    use theta_core::Family::*;
    match (kind.family(), action) {
        (Postgres, Action::Cancel) => {
            "Sends SIGINT to the backend: the running statement stops and the session stays."
        }
        (Postgres, Action::Terminate) => {
            "Ends the backend. Its open transaction is rolled back and the client is disconnected."
        }
        (MySql, Action::Cancel) => "Stops the running statement; the connection and its transaction stay.",
        (MySql, Action::Terminate) => {
            "Closes the connection. Its open transaction is rolled back and the client is disconnected."
        }
        (SqlServer, _) => "Ends the session. Its open transaction is rolled back (this can take a while).",
        (ClickHouse, _) => "Asks the server to stop the query; it stops at the next block boundary.",
        _ => "",
    }
}

// ---------------------------------------------------------------------------- sorting and filtering

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionColumn {
    Id,
    User,
    Database,
    Client,
    Application,
    State,
    Wait,
    TxnAge,
    QueryAge,
    Query,
}

impl SessionColumn {
    pub const ALL: [SessionColumn; 10] = [
        SessionColumn::Id,
        SessionColumn::User,
        SessionColumn::Database,
        SessionColumn::Client,
        SessionColumn::Application,
        SessionColumn::State,
        SessionColumn::Wait,
        SessionColumn::TxnAge,
        SessionColumn::QueryAge,
        SessionColumn::Query,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SessionColumn::Id => "ID",
            SessionColumn::User => "User",
            SessionColumn::Database => "Database",
            SessionColumn::Client => "Client",
            SessionColumn::Application => "Application",
            SessionColumn::State => "State",
            SessionColumn::Wait => "Wait / lock",
            SessionColumn::TxnAge => "Txn age",
            SessionColumn::QueryAge => "Query age",
            SessionColumn::Query => "Query",
        }
    }
}

/// Sort in place. Ids sort numerically when they are numbers; ages sort by value with "none" last in either
/// direction (an idle session has no transaction age, and that should never lead a descending sort).
pub fn sort(rows: &mut [Session], by: SessionColumn, descending: bool) {
    use std::cmp::Ordering;
    let text_of = |s: &Session| -> String {
        match by {
            SessionColumn::User => s.user.to_lowercase(),
            SessionColumn::Database => s.database.to_lowercase(),
            SessionColumn::Client => s.client.to_lowercase(),
            SessionColumn::Application => s.application.to_lowercase(),
            SessionColumn::State => s.state.to_lowercase(),
            SessionColumn::Wait => s.wait.to_lowercase(),
            SessionColumn::Query => s.query.to_lowercase(),
            _ => String::new(),
        }
    };
    rows.sort_by(|a, b| {
        let ord = match by {
            SessionColumn::Id => match (a.id.parse::<u64>(), b.id.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                _ => a.id.cmp(&b.id),
            },
            SessionColumn::TxnAge | SessionColumn::QueryAge => {
                let pick = |s: &Session| if by == SessionColumn::TxnAge { s.txn_age } else { s.query_age };
                return match (pick(a), pick(b)) {
                    (Some(x), Some(y)) => {
                        let o = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
                        if descending { o.reverse() } else { o }
                    }
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                };
            }
            _ => text_of(a).cmp(&text_of(b)),
        };
        if descending { ord.reverse() } else { ord }
    });
}

/// Whitespace-separated terms; a session matches when every term occurs (case-insensitively) in some field.
pub fn matches_filter(s: &Session, filter: &str) -> bool {
    let hay = [s.id.as_str(), &s.user, &s.database, &s.client, &s.application, &s.state, &s.wait, &s.query]
        .iter()
        .map(|f| f.to_lowercase())
        .collect::<Vec<_>>();
    filter.split_whitespace().map(str::to_lowercase).all(|term| hay.iter().any(|h| h.contains(&term)))
}

/// Sessions that wait for another, and the ones that block others, for the table's highlighting.
pub fn is_blocked(s: &Session) -> bool {
    !s.blocked_by.is_empty()
}

pub fn blocks_others(all: &[Session], s: &Session) -> bool {
    all.iter().any(|o| o.blocked_by.contains(&s.id))
}

/// "1.2 s", "3 min 05 s", "2 h 10 min" for an age in seconds.
pub fn format_age(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 60.0 {
        format!("{:.0} s", s)
    } else if s < 3600.0 {
        format!("{} min {:02} s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else if s < 86400.0 {
        format!("{} h {:02} min", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64)
    } else {
        format!("{} d {:02} h", (s / 86400.0) as u64, ((s % 86400.0) / 3600.0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{Column, TypeKind, Value};

    fn rs(cols: &[&str], rows: Vec<Vec<Value>>) -> ResultSet {
        ResultSet {
            columns: cols
                .iter()
                .map(|n| Column {
                    name: n.to_string(),
                    type_name: "text".into(),
                    kind: TypeKind::Text,
                    origin: None,
                })
                .collect(),
            rows,
            truncated: false,
        }
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn products_without_sessions_say_why() {
        for k in [DatabaseKind::Sqlite, DatabaseKind::DuckDb] {
            assert!(queries(k, "").unwrap_err().contains("embedded"));
        }
        assert!(queries(DatabaseKind::CockroachDb, "").is_err());
        assert!(queries(DatabaseKind::Redshift, "").is_err());
        for k in [
            DatabaseKind::Postgres,
            DatabaseKind::TimescaleDb,
            DatabaseKind::MySql,
            DatabaseKind::SqlServer,
            DatabaseKind::ClickHouse,
        ] {
            assert!(queries(k, "").is_ok(), "{k:?}");
        }
    }

    #[test]
    fn every_query_returns_the_columns_the_parser_reads() {
        for k in
            [DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::SqlServer, DatabaseKind::ClickHouse]
        {
            let q = queries(k, "").unwrap().sessions;
            // ClickHouse has no transactions to age.
            for alias in [
                "AS sid",
                "AS usr",
                "AS db",
                "AS client",
                "AS app",
                "AS state",
                "AS wait",
                "AS query_age",
                "AS query",
                "AS blocked_by",
                "AS is_self",
            ] {
                assert!(q.contains(alias), "{k:?} lacks {alias}");
            }
            assert!(k == DatabaseKind::ClickHouse || q.contains("AS txn_age"), "{k:?} lacks AS txn_age");
        }
        assert!(queries(DatabaseKind::ClickHouse, "").unwrap().sessions.contains(CLICKHOUSE_MARKER));
    }

    #[test]
    fn mysql_picks_the_lock_table_by_server() {
        let maria = queries(DatabaseKind::MySql, "10.11.14-MariaDB-0ubuntu0.24.04.1").unwrap();
        assert!(maria.blockers.unwrap().contains("INNODB_LOCK_WAITS"));
        let my8 = queries(DatabaseKind::MySql, "8.0.36").unwrap();
        assert!(my8.blockers.unwrap().contains("performance_schema.data_lock_waits"));
    }

    #[test]
    fn parses_rows_and_blockers() {
        let r = rs(
            &[
                "sid",
                "usr",
                "db",
                "client",
                "app",
                "state",
                "wait",
                "txn_age",
                "query_age",
                "query",
                "blocked_by",
                "is_self",
            ],
            vec![
                vec![
                    t("11"),
                    t("ann"),
                    t("shop"),
                    t("10.0.0.5:5432"),
                    t("psql"),
                    t("active"),
                    t("Lock: transactionid"),
                    Value::Float(12.5),
                    Value::Float(3.0),
                    t("update t set a=1"),
                    t("22, 33"),
                    Value::Bool(false),
                ],
                vec![
                    t("22"),
                    t("bob"),
                    t("shop"),
                    t("local"),
                    t(""),
                    t("idle in transaction"),
                    t(""),
                    Value::Float(90.0),
                    Value::Null,
                    t("select 1"),
                    t(""),
                    Value::Bool(true),
                ],
            ],
        );
        let s = parse_sessions(&r);
        assert_eq!(s[0].blocked_by, vec!["22", "33"]);
        assert_eq!(s[0].txn_age, Some(12.5));
        assert!(s[1].is_self && s[1].query_age.is_none());
        assert!(is_blocked(&s[0]) && !is_blocked(&s[1]));
        assert!(blocks_others(&s, &s[1]) && !blocks_others(&s, &s[0]));
    }

    #[test]
    fn mysql_blockers_are_merged_once() {
        let mut s = vec![
            Session { id: "5".into(), ..Default::default() },
            Session { id: "6".into(), ..Default::default() },
        ];
        let b = rs(&["waiting", "blocking"], vec![vec![t("5"), t("6")], vec![t("5"), t("6")]]);
        apply_blockers(&mut s, &b);
        assert_eq!(s[0].blocked_by, vec!["6"]);
        assert!(s[1].blocked_by.is_empty());
    }

    #[test]
    fn kill_statements_per_product() {
        use Action::*;
        let k = |kind, a, id| kill_statement(kind, a, id);
        assert_eq!(k(DatabaseKind::Postgres, Cancel, "42").unwrap(), "SELECT pg_cancel_backend(42)");
        assert_eq!(k(DatabaseKind::Postgres, Terminate, "42").unwrap(), "SELECT pg_terminate_backend(42)");
        assert_eq!(k(DatabaseKind::MySql, Cancel, "7").unwrap(), "KILL QUERY 7");
        assert_eq!(k(DatabaseKind::MySql, Terminate, "7").unwrap(), "KILL 7");
        assert_eq!(k(DatabaseKind::SqlServer, Terminate, "55").unwrap(), "KILL 55");
        assert!(k(DatabaseKind::SqlServer, Cancel, "55").unwrap_err().contains("ends the whole session"));
        assert_eq!(
            k(DatabaseKind::ClickHouse, Cancel, "ab12-cd_3").unwrap(),
            "KILL QUERY WHERE query_id = 'ab12-cd_3'"
        );
        assert!(k(DatabaseKind::ClickHouse, Terminate, "x").is_err());
        assert!(k(DatabaseKind::Sqlite, Cancel, "1").is_err());
    }

    #[test]
    fn unavailable_actions_name_their_reason_without_an_id() {
        assert!(action_unavailable(DatabaseKind::ClickHouse, Action::Terminate).unwrap().contains("only Cancel"));
        assert!(action_unavailable(DatabaseKind::SqlServer, Action::Cancel).is_some());
        assert!(action_unavailable(DatabaseKind::Postgres, Action::Cancel).is_none());
        assert!(action_unavailable(DatabaseKind::MySql, Action::Terminate).is_none());
        assert!(action_unavailable(DatabaseKind::DuckDb, Action::Cancel).is_some());
    }

    #[test]
    fn kill_ids_cannot_inject_sql() {
        for bad in ["1; drop table t", "1 or 1=1", "", "-1", "1'"] {
            assert!(kill_statement(DatabaseKind::Postgres, Action::Terminate, bad).is_err(), "{bad}");
            assert!(kill_statement(DatabaseKind::MySql, Action::Cancel, bad).is_err(), "{bad}");
        }
        assert!(kill_statement(DatabaseKind::ClickHouse, Action::Cancel, "x' OR 1=1 --").is_err());
    }

    #[test]
    fn sorting_numbers_and_missing_ages() {
        let mut v = vec![
            Session { id: "10".into(), txn_age: None, ..Default::default() },
            Session { id: "9".into(), txn_age: Some(5.0), ..Default::default() },
            Session { id: "100".into(), txn_age: Some(50.0), ..Default::default() },
        ];
        sort(&mut v, SessionColumn::Id, false);
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["9", "10", "100"]);
        sort(&mut v, SessionColumn::TxnAge, true);
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["100", "9", "10"]);
        sort(&mut v, SessionColumn::TxnAge, false);
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["9", "100", "10"]);
    }

    #[test]
    fn filter_terms_are_anded_across_fields() {
        let s = Session {
            id: "7".into(),
            user: "Ann".into(),
            query: "select * from orders".into(),
            state: "active".into(),
            ..Default::default()
        };
        assert!(matches_filter(&s, ""));
        assert!(matches_filter(&s, "ann ORDERS"));
        assert!(matches_filter(&s, "active 7"));
        assert!(!matches_filter(&s, "ann customers"));
    }

    #[test]
    fn own_sessions_are_marked() {
        let mut v = vec![
            Session { id: "1".into(), ..Default::default() },
            Session { id: "2".into(), ..Default::default() },
        ];
        mark_own(&mut v, &["2".into()]);
        assert!(!v[0].is_self && v[1].is_self);
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(format_age(0.34), "0.3 s");
        assert_eq!(format_age(42.0), "42 s");
        assert_eq!(format_age(185.0), "3 min 05 s");
        assert_eq!(format_age(7500.0), "2 h 05 min");
        assert_eq!(format_age(100_000.0), "1 d 03 h");
    }
}
