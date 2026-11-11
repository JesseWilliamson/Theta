mod common;

use common::*;
use std::time::{Duration, Instant};
use theta_core::*;

#[tokio::test]
async fn type_round_trips() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TYPE mood AS ENUM ('sad','ok','happy')").await;
    run(c, "CREATE DOMAIN posint AS integer CHECK (VALUE > 0)").await;
    run(c, "CREATE TYPE pair AS (a integer, b text)").await;

    let out = run(
        c,
        "SELECT true AS b, 42::int2 AS i2, -7::int4 AS i4, 9223372036854775807::int8 AS i8, 123::oid AS o,
                0.1::float4 AS f4, 2.5::float8 AS f8, 12345.6789::numeric AS n, 0::numeric(10,2) AS n2,
                'NaN'::numeric AS nan, 'héllo'::text AS t, 'abc'::varchar(10) AS vc, 'ab'::bpchar(4) AS bp,
                'nm'::name AS nm, '\\xdeadbeef'::bytea AS by,
                '{\"a\": [1,  2]}'::json AS js, '{\"b\":   {\"c\": null}}'::jsonb AS jb,
                '123e4567-e89b-12d3-a456-426614174000'::uuid AS u, '2024-02-29'::date AS d,
                '13:14:15.5'::time AS tm, '13:14:15+02'::timetz AS tmz,
                '2024-02-29 13:14:15.123456'::timestamp AS ts, '2024-02-29 13:14:15+02'::timestamptz AS tstz,
                '1 year 2 mons 3 days 04:05:06'::interval AS iv, 'happy'::mood AS en, 5::posint AS dom,
                '10.0.0.1/24'::inet AS inet, '2001:db8::/32'::cidr AS cidr, '08:00:2b:01:02:03'::macaddr AS mac,
                B'101'::varbit AS vb, '[1,10)'::int4range AS rng, ROW(1,'x y')::pair AS comp,
                '-infinity'::timestamp AS inf, '0044-03-15 BC'::date AS bc",
    )
    .await;
    let rs = out.result.unwrap();
    let get = |name: &str| -> Value {
        let i = rs.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}"));
        rs.rows[0][i].clone()
    };
    assert_eq!(get("b"), Value::Bool(true));
    assert_eq!(get("i2"), Value::Int(42));
    assert_eq!(get("i4"), Value::Int(-7));
    assert_eq!(get("i8"), Value::Int(i64::MAX));
    assert_eq!(get("o"), Value::Int(123));
    assert_eq!(get("f4"), Value::Float(0.1));
    assert_eq!(get("f8"), Value::Float(2.5));
    assert_eq!(get("n"), Value::Decimal("12345.6789".into()));
    assert_eq!(get("n2"), Value::Decimal("0.00".into()));
    assert_eq!(get("nan"), Value::Decimal("NaN".into()));
    assert_eq!(get("t"), text("héllo"));
    assert_eq!(get("vc"), text("abc"));
    assert_eq!(get("bp"), text("ab  "));
    assert_eq!(get("nm"), text("nm"));
    assert_eq!(get("by"), Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    assert_eq!(get("js"), Value::Json("{\"a\":[1,2]}".into()));
    assert_eq!(get("jb"), Value::Json("{\"b\":{\"c\":null}}".into()));
    assert_eq!(get("u"), Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into()));
    assert_eq!(get("d"), Value::Date("2024-02-29".into()));
    assert_eq!(get("tm"), Value::Time("13:14:15.5".into()));
    assert_eq!(get("tmz"), Value::Time("13:14:15+02".into()));
    assert_eq!(get("ts"), Value::Timestamp("2024-02-29 13:14:15.123456".into()));
    assert_eq!(get("tstz"), Value::TimestampTz("2024-02-29 11:14:15+00".into()));
    assert_eq!(get("iv"), Value::Interval("1 year 2 mons 3 days 04:05:06".into()));
    assert_eq!(get("en"), Value::Unknown("happy".into()));
    assert_eq!(get("dom"), Value::Int(5));
    assert_eq!(get("inet"), Value::Unknown("10.0.0.1/24".into()));
    assert_eq!(get("cidr"), Value::Unknown("2001:db8::/32".into()));
    assert_eq!(get("mac"), Value::Unknown("08:00:2b:01:02:03".into()));
    assert_eq!(get("vb"), Value::Unknown("101".into()));
    assert_eq!(get("rng"), Value::Unknown("[1,10)".into()));
    assert_eq!(get("comp"), Value::Unknown("(1,\"x y\")".into()));
    assert_eq!(get("inf"), Value::Timestamp("-infinity".into()));
    assert_eq!(get("bc"), Value::Date("0044-03-15 BC".into()));

    // Declared kinds and display names.
    let kind = |name: &str| rs.columns.iter().find(|c| c.name == name).unwrap().kind;
    assert_eq!(kind("n"), TypeKind::Decimal);
    assert_eq!(kind("tstz"), TypeKind::Timestamp);
    assert_eq!(kind("js"), TypeKind::Json);
    assert_eq!(kind("dom"), TypeKind::Integer);
    let tn = |name: &str| rs.columns.iter().find(|c| c.name == name).unwrap().type_name.clone();
    assert_eq!(tn("vc"), "varchar(10)");
    assert_eq!(tn("n2"), "numeric(10,2)");
    assert_eq!(tn("tstz"), "timestamptz");
    fx.done().await;
}

#[tokio::test]
async fn nulls_and_arrays() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TYPE mood AS ENUM ('sad','ok')").await;
    let row = one(
        c,
        "SELECT NULL::int, NULL::text, NULL::jsonb, NULL::timestamptz, NULL::numeric, NULL::int[],
                ARRAY[1,2,3], ARRAY['a',NULL,'c'], ARRAY[[1,2],[3,4]], ARRAY[]::int4[],
                ARRAY['2024-01-01'::date], ARRAY[1.5::numeric, NULL], ARRAY['ok'::mood], ARRAY[true,false],
                ARRAY['{\"a\": 1}'::jsonb], ARRAY['2024-01-01 10:00+00'::timestamptz]",
    )
    .await;
    assert!(row[..6].iter().all(Value::is_null));
    assert_eq!(row[6], Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
    assert_eq!(row[7], Value::Array(vec![text("a"), Value::Null, text("c")]));
    assert_eq!(
        row[8],
        Value::Array(vec![
            Value::Array(vec![Value::Int(1), Value::Int(2)]),
            Value::Array(vec![Value::Int(3), Value::Int(4)])
        ])
    );
    assert_eq!(row[9], Value::Array(vec![]));
    assert_eq!(row[10], Value::Array(vec![Value::Date("2024-01-01".into())]));
    assert_eq!(row[11], Value::Array(vec![Value::Decimal("1.5".into()), Value::Null]));
    assert_eq!(row[12], Value::Array(vec![Value::Unknown("ok".into())]));
    assert_eq!(row[13], Value::Array(vec![Value::Bool(true), Value::Bool(false)]));
    assert_eq!(row[14], Value::Array(vec![Value::Json("{\"a\":1}".into())]));
    assert_eq!(row[15], Value::Array(vec![Value::TimestampTz("2024-01-01 10:00:00+00".into())]));
    let cols = run(c, "SELECT ARRAY[1,2]::int4[] AS a").await.result.unwrap().columns;
    assert_eq!((cols[0].kind, cols[0].type_name.as_str()), (TypeKind::Array, "int4[]"));
    fx.done().await;
}

#[tokio::test]
async fn postgis_geometry_is_ewkb() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    let out = run(
        c,
        "SELECT ST_SetSRID(ST_MakePoint(1, 2), 4326) AS g, ST_GeogFromText('POINT(3 4)') AS geo,
                ST_GeomFromText('LINESTRING(0 0, 1 1)') AS plain",
    )
    .await
    .result
    .unwrap();
    assert!(out.columns.iter().all(|c| c.kind == TypeKind::Geometry));
    assert_eq!(out.columns[0].type_name, "geometry");
    let Value::Geometry(g) = &out.rows[0][0] else { panic!("not geometry: {:?}", out.rows[0][0]) };
    // little-endian, type POINT|SRID flag, SRID 4326, then x and y
    assert_eq!(&g[..9], &[1, 1, 0, 0, 0x20, 0xE6, 0x10, 0, 0]);
    assert_eq!(&g[9..17], &1f64.to_le_bytes());
    assert_eq!(&g[17..25], &2f64.to_le_bytes());
    let Value::Geometry(geo) = &out.rows[0][1] else { panic!() };
    assert_eq!(geo[0], 1);
    let Value::Geometry(plain) = &out.rows[0][2] else { panic!() };
    assert_eq!(&plain[..5], &[1, 2, 0, 0, 0]);
    fx.done().await;
}

#[tokio::test]
async fn row_limit_and_truncation() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    let limited = |n| ExecOptions { row_limit: Some(n) };
    let out = c.execute("SELECT g FROM generate_series(1, 5000) g", &limited(100)).await.unwrap();
    let rs = out.result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (100, true));
    assert_eq!(rs.rows[99][0], Value::Int(100));
    let rs =
        c.execute("SELECT g FROM generate_series(1, 5000) g", &limited(5000)).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (5000, false));
    let rs = c.execute("SELECT g FROM generate_series(1, 5000) g", &all()).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (5000, false));
    let rs = c.execute("SELECT 1 WHERE false", &limited(10)).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (0, false));
    assert_eq!(rs.columns.len(), 1);

    // The server must stop producing rows once the limit is hit: this would not
    // finish in a lifetime if the whole result were fetched.
    let started = Instant::now();
    let rs = c
        .execute("SELECT g FROM (SELECT generate_series(1, 2000000000) AS g) s", &limited(10))
        .await
        .unwrap()
        .result
        .unwrap();
    assert!(rs.truncated && rs.rows.len() == 10);
    assert!(started.elapsed() < Duration::from_secs(5));

    // Limiting must not leave an implicit transaction behind...
    assert!(!c.in_transaction().await);
    // ...nor disturb a user's transaction.
    run(c, "BEGIN").await;
    let rs =
        c.execute("SELECT g FROM generate_series(1, 500) g", &limited(10)).await.unwrap().result.unwrap();
    assert!(rs.truncated);
    assert!(c.in_transaction().await);
    run(c, "CREATE TABLE keep (a int)").await;
    run(c, "ROLLBACK").await;
    assert!(!c.in_transaction().await);
    let missing = c.execute("SELECT * FROM keep", &all()).await;
    assert!(matches!(missing, Err(Error::Query { .. })), "rollback must have removed the table");
    fx.done().await;
}

#[tokio::test]
async fn rows_affected_and_returning() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    let ddl = run(c, "CREATE TABLE t (id serial PRIMARY KEY, v int)").await;
    assert_eq!((ddl.result, ddl.rows_affected), (None, None));
    let ins = run(c, "INSERT INTO t (v) VALUES (1),(2),(3)").await;
    assert_eq!((ins.result.is_none(), ins.rows_affected), (true, Some(3)));
    let upd = run(c, "UPDATE t SET v = v + 1 WHERE v >= 2").await;
    assert_eq!(upd.rows_affected, Some(2));
    let ret = c
        .execute("INSERT INTO t (v) VALUES (10),(11) RETURNING id, v", &ExecOptions { row_limit: Some(1) })
        .await
        .unwrap();
    assert_eq!(ret.rows_affected, Some(2));
    let rs = ret.result.unwrap();
    assert_eq!(rs.rows.len(), 1);
    assert!(rs.truncated);
    let del = run(c, "DELETE FROM t").await;
    assert_eq!(del.rows_affected, Some(5));
    let sel = run(c, "SELECT * FROM t").await;
    assert_eq!(sel.rows_affected, None);
    assert!(sel.elapsed > Duration::ZERO);
    fx.done().await;
}

#[tokio::test]
async fn notices_are_collected() {
    let fx = PgFx::new().await;
    let out =
        run(fx.conn.as_ref(), "DO $$ BEGIN RAISE NOTICE 'hello %', 42; RAISE WARNING 'careful'; END $$")
            .await;
    assert_eq!(out.notices, vec!["NOTICE: hello 42".to_string(), "WARNING: careful".to_string()]);
    let again = run(fx.conn.as_ref(), "SELECT 1").await;
    assert!(again.notices.is_empty());
    fx.done().await;
}

#[tokio::test]
async fn query_errors_carry_code_and_byte_position() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE t (name text)").await;
    // 'é' is two bytes, so the character position and byte offset differ.
    let sql = "SELECT 'é', nmae FROM t";
    let err = c.execute(sql, &all()).await.unwrap_err();
    let Error::Query { message, code, hint, position, .. } = err else { panic!("wrong variant") };
    assert_eq!(code.as_deref(), Some("42703"));
    assert!(message.contains("nmae"));
    assert!(hint.unwrap().contains("name"));
    assert_eq!(position.unwrap().offset, sql.find("nmae").unwrap());

    let err = c.execute("SELEC 1", &all()).await.unwrap_err();
    let Error::Query { code, position, .. } = err else { panic!() };
    assert_eq!(code.as_deref(), Some("42601"));
    assert_eq!(position.unwrap().offset, 0);

    // Exactly one statement per call.
    assert!(matches!(c.execute("SELECT 1; SELECT 2", &all()).await, Err(Error::Query { .. })));
    // The connection is still healthy.
    assert_eq!(scalar(c, "SELECT 1").await, Value::Int(1));
    fx.done().await;
}

#[tokio::test]
async fn connect_failures_map_to_variants() {
    let mut bad = pg_config();
    bad.password = Some("definitely-wrong".into());
    let e = theta_drivers::connect(&bad).await.err().expect("must fail");
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    let mut closed = pg_config();
    closed.port = 1;
    let e = theta_drivers::connect(&closed).await.err().expect("must fail");
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
    let mut nodb = pg_config();
    nodb.database = "no_such_database_xyz".into();
    let e = theta_drivers::connect(&nodb).await.err().expect("must fail");
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
}

#[tokio::test]
async fn ssl_modes() {
    for mode in [SslMode::Disable, SslMode::Prefer] {
        let mut cfg = pg_config();
        cfg.ssl_mode = mode;
        let conn = theta_drivers::connect(&cfg).await.unwrap_or_else(|e| panic!("{mode:?}: {e:?}"));
        assert_eq!(scalar(conn.as_ref(), "SELECT 1").await, Value::Int(1));
        conn.close().await.unwrap();
    }
    // Servers without TLS must be refused when TLS is mandatory; servers with it must work.
    let ssl_on = {
        let conn = theta_drivers::connect(&pg_config()).await.unwrap();
        scalar(conn.as_ref(), "SHOW ssl").await == text("on")
    };
    let mut cfg = pg_config();
    cfg.ssl_mode = SslMode::Require;
    let r = theta_drivers::connect(&cfg).await;
    assert_eq!(r.is_ok(), ssl_on, "Require must connect iff the server offers TLS: {:?}", r.err());
    // The test server's certificate is self-signed, so full verification must refuse it.
    let mut strict = pg_config();
    strict.ssl_mode = SslMode::VerifyFull;
    let r2 = theta_drivers::connect(&strict).await;
    if ssl_on {
        assert!(matches!(r2, Err(Error::Connect(_))), "VerifyFull must reject a self-signed certificate");
    } else {
        assert!(r2.is_err());
    }
    if let Ok(conn) = r {
        let enc = scalar(conn.as_ref(), "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()").await;
        assert_eq!(enc, Value::Bool(true));
    }
}

#[tokio::test]
async fn cancel_returns_promptly() {
    let fx = PgFx::new().await;
    let conn: std::sync::Arc<dyn Connection> = fx.conn.into();
    for sql in [
        "SELECT pg_sleep(30)",
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r",
    ] {
        let c2 = conn.clone();
        let started = Instant::now();
        let task = tokio::spawn(async move { c2.execute(sql, &ExecOptions::default()).await });
        tokio::time::sleep(Duration::from_millis(400)).await;
        conn.cancel().await.unwrap();
        let r =
            tokio::time::timeout(Duration::from_secs(5), task).await.expect("cancel took too long").unwrap();
        assert!(matches!(r, Err(Error::Cancelled)), "{sql}: {r:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        // Session is still usable.
        assert_eq!(scalar(conn.as_ref(), "SELECT 7").await, Value::Int(7));
    }
    // Cancelling an idle connection is fine and must not poison the next statement.
    conn.cancel().await.unwrap();
    assert_eq!(scalar(conn.as_ref(), "SELECT 8").await, Value::Int(8));
    // A statement_timeout is an ordinary query error, not a cancellation.
    run(conn.as_ref(), "SET statement_timeout = 100").await;
    let r = conn.execute("SELECT pg_sleep(5)", &all()).await;
    assert!(matches!(r, Err(Error::Query { .. })), "{r:?}");
    run(conn.as_ref(), "SET statement_timeout = 0").await;
    let _ = conn.execute(&format!("DROP SCHEMA {} CASCADE", fx.schema), &all()).await;
}

#[tokio::test]
async fn in_transaction_tracks_server_state() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    assert!(!c.in_transaction().await);
    run(c, "BEGIN").await;
    assert!(c.in_transaction().await);
    assert!(c.execute("SELECT 1/0", &all()).await.is_err());
    assert!(c.in_transaction().await, "an aborted transaction block is still open");
    run(c, "ROLLBACK").await;
    assert!(!c.in_transaction().await);
    run(c, "START TRANSACTION").await;
    run(c, "COMMIT").await;
    assert!(!c.in_transaction().await);
    fx.done().await;
}

#[tokio::test]
async fn statements_run_in_call_order() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    let opts = all();
    let (a, b, d) = tokio::join!(
        c.execute("SELECT pg_sleep(0.2), set_config('theta.x', 'one', false)", &opts),
        c.execute("SELECT set_config('theta.x', 'two', false)", &opts),
        c.execute("SELECT current_setting('theta.x')", &opts),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(d.unwrap().result.unwrap().rows[0][0], text("two"));
    fx.done().await;
}

#[tokio::test]
async fn result_columns_know_their_origin() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE people (id int PRIMARY KEY, name text)").await;
    run(c, "CREATE TABLE pets (id int PRIMARY KEY, owner int, name text)").await;
    run(c, "INSERT INTO people VALUES (1,'ann')").await;
    run(c, "INSERT INTO pets VALUES (1,1,'rex')").await;
    let cols = run(c, "SELECT p.id AS pid, p.name AS owner_name, t.name, p.id + 1 AS computed, 'x' AS lit FROM people p JOIN pets t ON t.owner = p.id")
        .await
        .result
        .unwrap()
        .columns;
    let origin = |i: usize| cols[i].origin.clone();
    let o = |table: &str, col: &str| {
        Some(ColumnOrigin { schema: Some(fx.schema.clone()), table: table.into(), column: col.into() })
    };
    assert_eq!(origin(0), o("people", "id"));
    assert_eq!(origin(1), o("people", "name"));
    assert_eq!(origin(2), o("pets", "name"));
    assert_eq!(origin(3), None);
    assert_eq!(origin(4), None);
    // Renamed columns are picked up (the cache is invalidated on DDL).
    run(c, "ALTER TABLE people RENAME COLUMN name TO full_name").await;
    let cols =
        run(c, "SELECT name FROM pets UNION ALL SELECT full_name FROM people").await.result.unwrap().columns;
    assert!(cols[0].origin.is_none(), "set operations have no single source column");
    let cols = run(c, "SELECT full_name FROM people").await.result.unwrap().columns;
    assert_eq!(cols[0].origin.as_ref().unwrap().column, "full_name");
    fx.done().await;
}

#[tokio::test]
async fn introspection_reads_the_whole_catalog() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    for sql in [
        "CREATE TABLE parent (a int, b int, label text NOT NULL DEFAULT 'x', PRIMARY KEY (a, b))",
        "COMMENT ON TABLE parent IS 'The parent table'",
        "COMMENT ON COLUMN parent.label IS 'A label'",
        "CREATE TABLE child (id serial PRIMARY KEY, pa int, pb int, note varchar(40), amount numeric(10,2), \
         FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE ON UPDATE SET NULL)",
        "CREATE UNIQUE INDEX child_note_idx ON child (lower(note))",
        "CREATE INDEX child_amount_idx ON child USING hash (amount)",
        "CREATE VIEW child_view AS SELECT id, note FROM child",
        "CREATE MATERIALIZED VIEW child_mv AS SELECT id FROM child",
        "CREATE SEQUENCE extra_seq",
        "CREATE FUNCTION add_one(x int) RETURNS int LANGUAGE sql AS 'SELECT x + 1'",
        "CREATE PROCEDURE do_nothing() LANGUAGE sql AS ''",
        "CREATE TABLE spatial (id int, g geometry(Polygon, 4326), anyg geometry, geo geography(Point, 4326))",
        "CREATE TABLE parted (id int, d date) PARTITION BY RANGE (d)",
        "INSERT INTO child (pa, pb) SELECT 1, 1 FROM generate_series(1, 3)",
    ] {
        if sql.starts_with("INSERT") {
            run(c, "INSERT INTO parent VALUES (1, 1)").await;
        }
        run(c, sql).await;
    }
    run(c, "ANALYZE child").await;
    let cat = c.introspect().await.unwrap();
    assert_eq!(cat.default_schema.as_deref(), Some(fx.schema.as_str()));
    assert!(cat.schema("pg_catalog").is_none() && cat.schema("information_schema").is_none());
    let s = cat.schema(&fx.schema).unwrap();
    let t = |n: &str| s.tables.iter().find(|t| t.name == n).unwrap_or_else(|| panic!("table {n}"));

    let parent = t("parent");
    assert_eq!(parent.kind, TableKind::Table);
    assert_eq!(parent.primary_key, vec!["a", "b"]);
    assert_eq!(parent.comment.as_deref(), Some("The parent table"));
    let label = parent.columns.iter().find(|c| c.name == "label").unwrap();
    assert_eq!((label.data_type.as_str(), label.nullable, label.kind), ("text", false, TypeKind::Text));
    assert_eq!(label.default.as_deref(), Some("'x'::text"));
    assert_eq!(label.comment.as_deref(), Some("A label"));
    assert_eq!(parent.row_estimate, None, "never analysed: reltuples is -1");

    let child = t("child");
    assert_eq!(child.primary_key, vec!["id"]);
    assert_eq!(child.row_estimate, Some(3));
    let fk = &child.foreign_keys[0];
    assert_eq!(fk.columns, vec!["pa", "pb"]);
    assert_eq!(fk.referenced_columns, vec!["a", "b"]);
    assert_eq!(
        (fk.referenced_table.as_str(), fk.referenced_schema.as_deref()),
        ("parent", Some(fx.schema.as_str()))
    );
    assert_eq!((fk.on_delete.as_deref(), fk.on_update.as_deref()), (Some("CASCADE"), Some("SET NULL")));
    let note = child.columns.iter().find(|c| c.name == "note").unwrap();
    assert_eq!(note.data_type, "character varying(40)");
    assert_eq!(child.columns.iter().find(|c| c.name == "amount").unwrap().kind, TypeKind::Decimal);
    let id = &child.columns[0];
    assert!(id.default.as_deref().unwrap().starts_with("nextval("));
    let pk = child.indexes.iter().find(|i| i.primary).unwrap();
    assert_eq!(
        (pk.columns.clone(), pk.unique, pk.method.as_deref()),
        (vec!["id".to_string()], true, Some("btree"))
    );
    let uq = child.indexes.iter().find(|i| i.name == "child_note_idx").unwrap();
    assert!(uq.unique && !uq.primary);
    assert_eq!(uq.columns, vec!["lower(note::text)"]);
    assert!(uq.definition.as_deref().unwrap().starts_with("CREATE UNIQUE INDEX child_note_idx"));
    let hash = child.indexes.iter().find(|i| i.name == "child_amount_idx").unwrap();
    assert_eq!(hash.method.as_deref(), Some("hash"));

    assert_eq!(t("child_view").kind, TableKind::View);
    assert_eq!(t("child_view").columns.len(), 2);
    assert_eq!(t("child_mv").kind, TableKind::MaterializedView);
    assert_eq!(t("parted").kind, TableKind::Table);
    assert!(
        s.sequences.contains(&"extra_seq".to_string()) && s.sequences.contains(&"child_id_seq".to_string())
    );
    let add = s.routines.iter().find(|r| r.name == "add_one").unwrap();
    assert_eq!(
        (add.kind, add.signature.as_str(), add.return_type.as_deref()),
        (RoutineKind::Function, "(x integer)", Some("integer"))
    );
    let proc_ = s.routines.iter().find(|r| r.name == "do_nothing").unwrap();
    assert_eq!(proc_.kind, RoutineKind::Procedure);
    assert!(!s.routines.iter().any(|r| r.name.starts_with("st_")), "extension routines are hidden");

    let sp = t("spatial");
    let g = sp.columns.iter().find(|c| c.name == "g").unwrap();
    assert_eq!(g.kind, TypeKind::Geometry);
    assert_eq!(
        g.geometry,
        Some(GeometryColumn { geometry_type: "POLYGON".into(), srid: Some(4326), geography: false })
    );
    let any = sp.columns.iter().find(|c| c.name == "anyg").unwrap();
    assert_eq!(
        any.geometry,
        Some(GeometryColumn { geometry_type: "GEOMETRY".into(), srid: None, geography: false })
    );
    let geo = sp.columns.iter().find(|c| c.name == "geo").unwrap();
    assert!(geo.geometry.as_ref().unwrap().geography);
    fx.done().await;
}

#[tokio::test]
async fn explain_and_analyze() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE t (id int PRIMARY KEY, v int)").await;
    run(c, "INSERT INTO t SELECT g, g % 10 FROM generate_series(1, 1000) g").await;
    run(c, "ANALYZE t").await;

    let plan = c.explain("SELECT v, count(*) FROM t GROUP BY v ORDER BY v", false).await.unwrap();
    assert!(!plan.analyzed && plan.execution_time_ms.is_none());
    assert!(plan.raw.contains("\"Plan\""));
    let mut seen_scan = false;
    plan.root.walk(&mut |n, _| {
        if n.relation.as_deref() == Some("t") {
            seen_scan = true;
            assert!(n.plan_rows.unwrap() > 0.0 && n.total_cost.unwrap() > 0.0 && n.actual_rows.is_none());
        }
    });
    assert!(seen_scan);

    let plan = c.explain("SELECT * FROM t WHERE id < 100", true).await.unwrap();
    assert!(plan.analyzed);
    assert!(plan.planning_time_ms.is_some() && plan.execution_time_ms.is_some());
    assert_eq!(plan.root.actual_rows, Some(99.0));
    assert!(plan.root.actual_total_ms.unwrap() >= 0.0);
    assert!(plan.root.shared_hit_blocks.is_some());

    // ANALYZE executes, but writes must vanish.
    let plan = c.explain("INSERT INTO t VALUES (5000, 1)", true).await.unwrap();
    assert_eq!(plan.root.node_type, "Insert");
    assert_eq!(scalar(c, "SELECT count(*) FROM t WHERE id = 5000").await, Value::Int(0));
    c.explain("DELETE FROM t", true).await.unwrap();
    assert_eq!(scalar(c, "SELECT count(*) FROM t").await, Value::Int(1000));
    assert!(!c.in_transaction().await);

    // Inside a user transaction: neither commit nor roll back the user's work.
    run(c, "BEGIN").await;
    run(c, "INSERT INTO t VALUES (6000, 1)").await;
    c.explain("INSERT INTO t VALUES (6001, 1)", true).await.unwrap();
    assert!(c.in_transaction().await);
    assert_eq!(scalar(c, "SELECT count(*) FROM t WHERE id IN (6000, 6001)").await, Value::Int(1));
    run(c, "ROLLBACK").await;
    assert_eq!(scalar(c, "SELECT count(*) FROM t WHERE id >= 6000").await, Value::Int(0));

    // Errors point into the user's SQL, not the EXPLAIN prefix.
    let sql = "SELECT nope FROM t";
    let Error::Query { position, .. } = c.explain(sql, false).await.unwrap_err() else { panic!() };
    assert_eq!(position.unwrap().offset, sql.find("nope").unwrap());
    // A failing ANALYZE must not leave the session in an aborted transaction.
    assert!(c.explain("SELECT 1/0", true).await.is_err());
    assert!(!c.in_transaction().await);
    fx.done().await;
}

#[tokio::test]
async fn demo_seed_builds_a_consistent_dataset() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    theta_drivers::demo::seed(c).await.unwrap();
    // Idempotent.
    theta_drivers::demo::seed(c).await.unwrap();
    let count = |t: &'static str| async move { scalar(c, &format!("SELECT count(*) FROM {t}")).await };
    assert_eq!(count("customers").await, Value::Int(200));
    assert_eq!(count("products").await, Value::Int(40));
    assert_eq!(count("orders").await, Value::Int(600));
    assert_eq!(count("stores").await, Value::Int(25));
    assert_eq!(count("delivery_zones").await, Value::Int(25));
    assert_eq!(count("routes").await, Value::Int(25));
    // Orders reference customers; totals match their items.
    assert_eq!(
        scalar(
            c,
            "SELECT count(*) FROM orders o LEFT JOIN customers c ON c.id = o.customer_id WHERE c.id IS NULL"
        )
        .await,
        Value::Int(0)
    );
    assert_eq!(
        scalar(c, "SELECT count(*) FROM orders o WHERE o.total <> order_total(o.id)").await,
        Value::Int(0)
    );
    assert_eq!(
        scalar(c, "SELECT count(*) FROM stores WHERE ST_SRID(location) = 4326 AND ST_IsValid(location)")
            .await,
        Value::Int(25)
    );
    assert_eq!(scalar(c, "SELECT count(*) FROM delivery_zones WHERE ST_IsValid(area)").await, Value::Int(25));
    assert_eq!(scalar(c, "SELECT count(*) FROM stores s JOIN delivery_zones z ON z.store_id = s.id WHERE ST_Contains(z.area, s.location)").await, Value::Int(25));
    let cat = c.introspect().await.unwrap();
    let s = cat.schema(&fx.schema).unwrap();
    let items = s.tables.iter().find(|t| t.name == "order_items").unwrap();
    assert_eq!(items.primary_key, vec!["order_id", "line_no"]);
    assert_eq!(items.foreign_keys.len(), 2);
    assert!(s.tables.iter().any(|t| t.name == "customer_order_summary" && t.kind == TableKind::View));
    assert!(s.tables.iter().find(|t| t.name == "orders").unwrap().comment.is_some());
    assert!(s.routines.iter().any(|r| r.name == "order_total"));
    // Seeding into this schema must not have touched anything in public.
    assert_eq!(
        scalar(
            c,
            "SELECT count(*) FROM pg_tables WHERE schemaname = 'public' AND tablename = 'customers_never'"
        )
        .await,
        Value::Int(0)
    );
    fx.done().await;
}

#[tokio::test]
async fn close_then_use_reports_closed() {
    let fx = PgFx::new().await;
    let schema = fx.schema.clone();
    let c = fx.conn;
    let _ = c.execute(&format!("DROP SCHEMA {schema} CASCADE"), &all()).await;
    c.close().await.unwrap();
    assert!(matches!(c.execute("SELECT 1", &all()).await, Err(Error::Closed)));
}

/// A server-terminated session must be classified as lost on the very first failing statement
/// (the host reconnects and re-runs from that signal), and every statement after it too.
#[tokio::test]
async fn terminating_the_own_backend_is_reported_as_connection_lost() {
    let conn = theta_drivers::connect(&pg_config()).await.expect("connect to PostgreSQL");
    let err = conn
        .execute("select pg_terminate_backend(pg_backend_pid())", &all())
        .await
        .expect_err("the backend terminated itself");
    assert!(err.is_connection_lost(), "first failure must be a session loss: {err:?}");
    let again = conn.execute("select 1", &all()).await.expect_err("the session is gone");
    assert!(again.is_connection_lost(), "later statements too: {again:?}");
    // A fresh connection works, and an ordinary SQL error is not a session loss.
    let fresh = theta_drivers::connect(&pg_config()).await.expect("reconnect");
    let syntax = fresh.execute("selec 1", &all()).await.expect_err("syntax error");
    assert!(!syntax.is_connection_lost(), "{syntax:?}");
    assert_eq!(scalar(fresh.as_ref(), "select 1").await, Value::Int(1));
    let _ = fresh.close().await;
}

/// Killed from the outside while idle (an admin, a failover): the next statement is the one
/// that finds out.
#[tokio::test]
async fn a_backend_killed_by_another_session_is_reported_as_connection_lost() {
    let victim = theta_drivers::connect(&pg_config()).await.expect("connect");
    let admin = theta_drivers::connect(&pg_config()).await.expect("connect");
    let pid = scalar(victim.as_ref(), "select pg_backend_pid()::int8").await;
    let Value::Int(pid) = pid else { panic!("pid is an integer") };
    assert_eq!(
        scalar(admin.as_ref(), &format!("select pg_terminate_backend({pid})")).await,
        Value::Bool(true)
    );
    // Give the server a moment to close the socket.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = victim.execute("select 1", &all()).await.expect_err("terminated");
    assert!(err.is_connection_lost(), "{err:?}");
    let _ = admin.close().await;
}
