//! The DDL engine against real servers: create a table from a draft, change it step by step with the
//! generated ALTER scripts, and read it back through introspection after every step.

mod common;

use common::*;
use theta_core::*;
use theta_sql::ddl::*;

struct Types {
    int: &'static str,
    text: &'static str,
    ts: &'static str,
    now: &'static str,
}

fn types(kind: DatabaseKind) -> Types {
    match kind {
        DatabaseKind::Postgres => Types { int: "integer", text: "text", ts: "timestamp", now: "now()" },
        DatabaseKind::MySql => {
            Types { int: "int", text: "varchar(100)", ts: "datetime", now: "CURRENT_TIMESTAMP" }
        }
        DatabaseKind::Sqlite => {
            Types { int: "INTEGER", text: "TEXT", ts: "TIMESTAMP", now: "CURRENT_TIMESTAMP" }
        }
    }
}

fn col(id: u32, name: &str, ty: &str, nullable: bool) -> ColumnDraft {
    ColumnDraft {
        id,
        name: name.into(),
        data_type: ty.into(),
        nullable,
        default: None,
        identity: None,
        generated: None,
        comment: None,
    }
}

async fn apply(c: &dyn Connection, stmts: &[Statement]) {
    for s in stmts {
        c.execute(&s.sql, &all()).await.unwrap_or_else(|e| panic!("`{}` failed: {e}", s.sql));
    }
}

async fn read(c: &dyn Connection, schema: &str, name: &str) -> Table {
    let cat = c.introspect().await.unwrap();
    cat.find_table(Some(schema), name).unwrap_or_else(|| panic!("{schema}.{name} missing")).1.clone()
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

async fn scenario(kind: DatabaseKind, c: &dyn Connection, schema: &str) {
    let ty = types(kind);
    let sch = (kind != DatabaseKind::Sqlite).then(|| schema.to_string());
    let q = |n: &str| match &sch {
        Some(s) => format!("{s}.{n}"),
        None => n.to_string(),
    };
    // A parent for the foreign key.
    apply(
        c,
        &[Statement {
            sql: format!("CREATE TABLE {} (code {} PRIMARY KEY, label {})", q("regions"), ty.int, ty.text),
            flags: vec![],
            explanation: String::new(),
        }],
    )
    .await;
    run(c, &format!("INSERT INTO {} VALUES (1, 'north'), (2, 'south')", q("regions"))).await;

    // 1. CREATE: composite primary key, unique, check, foreign key, index, default, identity-free.
    let d = TableDraft {
        schema: sch.clone(),
        name: "accounts".into(),
        columns: vec![
            col(1, "tenant", ty.int, false),
            col(2, "number", ty.int, false),
            col(3, "owner", ty.text, true),
            col(4, "region", ty.int, true),
            {
                let mut c = col(5, "opened", ty.ts, true);
                c.default = Some(ty.now.into());
                c
            },
        ],
        primary_key: Some(KeyDraft { id: 10, name: None, columns: vec![1, 2] }),
        uniques: vec![KeyDraft { id: 11, name: Some("accounts_owner_uq".into()), columns: vec![3] }],
        foreign_keys: vec![ForeignKeyDraft {
            id: 12,
            name: Some("accounts_region_fk".into()),
            columns: vec![4],
            ref_schema: sch.clone(),
            ref_table: "regions".into(),
            ref_columns: vec!["code".into()],
            on_delete: Some("SET NULL".into()),
            on_update: None,
            deferrable: false,
            initially_deferred: false,
        }],
        indexes: vec![IndexDraft {
            id: 13,
            name: "accounts_region_idx".into(),
            columns: vec![4],
            unique: false,
            method: None,
            predicate: None,
            raw: None,
        }],
        checks: vec![CheckDraft {
            id: 14,
            name: Some("accounts_number_pos".into()),
            expression: "number > 0".into(),
        }],
        ..Default::default()
    };
    assert!(validate(&d, kind).is_empty());
    apply(c, &create_table(&d, kind)).await;
    let t = read(c, schema, "accounts").await;
    assert_eq!(names(&t), ["tenant", "number", "owner", "region", "opened"], "{kind:?}");
    assert_eq!(t.primary_key, ["tenant", "number"]);
    assert!(!t.columns[0].nullable && t.columns[2].nullable);
    assert_eq!(t.unique_constraints.len(), 1, "{kind:?}: {:?}", t.unique_constraints);
    assert_eq!(t.unique_constraints[0].columns, ["owner"]);
    assert_eq!(t.checks.len(), 1);
    assert_eq!(t.foreign_keys.len(), 1);
    assert_eq!(t.foreign_keys[0].referenced_table, "regions");
    assert_eq!(t.foreign_keys[0].on_delete.as_deref(), Some("SET NULL"));
    assert!(t.indexes.iter().any(|i| i.name == "accounts_region_idx" && i.columns == ["region"]));
    assert!(t.columns[4].default.is_some());
    // The draft built back from the catalog needs no changes against itself, and its structure matches.
    let mut live = TableDraft::from_table(schema, &t);
    assert!(diff(&live, &live.clone(), kind).is_empty());
    assert_eq!(live.uniques.len(), 1);
    assert_eq!(live.foreign_keys.len(), 1);
    assert_eq!(live.indexes.len(), 1);
    run(
        c,
        &format!(
            "INSERT INTO {} (tenant, number, owner, region) VALUES (1, 1, 'ann', 1), (1, 2, NULL, 2)",
            q("accounts")
        ),
    )
    .await;

    // 2. add a column with a default
    let mut next = live.clone();
    let mut tier = col(next.next_id(), "tier", ty.int, false);
    tier.default = Some("1".into());
    next.columns.push(tier);
    let script = diff(&live, &next, kind);
    apply(c, &script).await;
    let t = read(c, schema, "accounts").await;
    assert_eq!(t.columns.last().unwrap().name, "tier");
    assert_eq!(
        scalar(c, &format!("SELECT sum(tier) FROM {}", q("accounts"))).await.display(),
        "2",
        "{kind:?}"
    );
    live = TableDraft::from_table(schema, &t);

    // 3. rename a column: the data and the unique constraint follow
    let mut next = live.clone();
    next.columns.iter_mut().find(|c| c.name == "owner").unwrap().name = "holder".into();
    let script = diff(&live, &next, kind);
    assert!(script.iter().all(|s| !s.is_destructive()), "{kind:?}: {:?}", script);
    apply(c, &script).await;
    let t = read(c, schema, "accounts").await;
    assert!(names(&t).contains(&"holder") && !names(&t).contains(&"owner"));
    assert_eq!(t.unique_constraints[0].columns, ["holder"], "{kind:?}");
    assert_eq!(scalar(c, &format!("SELECT count(holder) FROM {}", q("accounts"))).await.display(), "1");
    live = TableDraft::from_table(schema, &t);

    // 4. change a type: tier int -> bigint-ish text widening
    let mut next = live.clone();
    let wider = match kind {
        DatabaseKind::Postgres => "bigint",
        DatabaseKind::MySql => "bigint",
        DatabaseKind::Sqlite => "BIGINT",
    };
    next.columns.iter_mut().find(|c| c.name == "tier").unwrap().data_type = wider.into();
    let script = diff(&live, &next, kind);
    apply(c, &script).await;
    let t = read(c, schema, "accounts").await;
    assert!(
        t.columns.iter().find(|c| c.name == "tier").unwrap().data_type.to_lowercase().contains("bigint"),
        "{kind:?}"
    );
    assert_eq!(
        scalar(c, &format!("SELECT count(*) FROM {}", q("accounts"))).await.display(),
        "2",
        "rows survive {kind:?}"
    );
    assert_eq!(t.primary_key, ["tenant", "number"], "{kind:?}: the key survives a type change");
    live = TableDraft::from_table(schema, &t);

    // 5. drop a column
    let mut next = live.clone();
    next.columns.retain(|c| c.name != "tier");
    let script = diff(&live, &next, kind);
    assert!(script.iter().any(Statement::is_destructive));
    apply(c, &script).await;
    let t = read(c, schema, "accounts").await;
    assert!(!names(&t).contains(&"tier"));
    live = TableDraft::from_table(schema, &t);

    // 6. add a foreign key from a second column, and an index
    let mut next = live.clone();
    let id = next.next_id();
    next.columns.push(col(id, "backup_region", ty.int, true));
    let fk_id = next.next_id() + 1;
    next.foreign_keys.push(ForeignKeyDraft {
        id: fk_id,
        name: Some("accounts_backup_fk".into()),
        columns: vec![id],
        ref_schema: sch.clone(),
        ref_table: "regions".into(),
        ref_columns: vec!["code".into()],
        on_delete: None,
        on_update: None,
        deferrable: false,
        initially_deferred: false,
    });
    next.indexes.push(IndexDraft {
        id: fk_id + 1,
        name: "accounts_backup_idx".into(),
        columns: vec![id],
        unique: false,
        method: None,
        predicate: None,
        raw: None,
    });
    let script = diff(&live, &next, kind);
    apply(c, &script).await;
    let t = read(c, schema, "accounts").await;
    assert_eq!(t.foreign_keys.len(), 2, "{kind:?}: {:?}", t.foreign_keys);
    assert!(t.indexes.iter().any(|i| i.name == "accounts_backup_idx"));
    assert_eq!(scalar(c, &format!("SELECT count(*) FROM {}", q("accounts"))).await.display(), "2");
    live = TableDraft::from_table(schema, &t);

    // 7. a failing change: NOT NULL on a column that holds NULLs keeps the table as it was
    let mut next = live.clone();
    next.columns.iter_mut().find(|c| c.name == "holder").unwrap().nullable = false;
    let script = diff(&live, &next, kind);
    let mut failed = None;
    for s in &script {
        if let Err(e) = c.execute(&s.sql, &all()).await {
            failed = Some((s.sql.clone(), e));
            break;
        }
    }
    assert!(failed.is_some(), "{kind:?}: NOT NULL over NULLs must fail");
    // SQLite's rebuild is several statements; a failed copy leaves a half-built new table that a
    // transaction would have rolled back. The real flow wraps the script in one; do the same here.
    if kind == DatabaseKind::Sqlite {
        let _ = c.execute("ROLLBACK", &all()).await;
        let _ = c.execute("DROP TABLE IF EXISTS accounts__new", &all()).await;
        let _ = c.execute("PRAGMA foreign_keys = ON", &all()).await;
    }
    let t = read(c, schema, "accounts").await;
    assert!(t.columns.iter().find(|c| c.name == "holder").unwrap().nullable, "{kind:?}");

    // 8. rename the table, then drop it
    let mut next = live.clone();
    next.name = "ledgers".into();
    let script = diff(&live, &next, kind);
    apply(c, &script).await;
    let t = read(c, schema, "ledgers").await;
    assert_eq!(t.columns.len(), 6, "{kind:?}");
    assert_eq!(scalar(c, &format!("SELECT count(*) FROM {}", q("ledgers"))).await.display(), "2");
    run(c, &format!("DROP TABLE {}", q("ledgers"))).await;
    live = next;
    let _ = live;
}

#[tokio::test]
async fn postgres_round_trip() {
    let fx = PgFx::new().await;
    scenario(DatabaseKind::Postgres, fx.conn.as_ref(), &fx.schema).await;
    fx.done().await;
}

#[tokio::test]
async fn mysql_round_trip() {
    let fx = MyFx::new().await;
    scenario(DatabaseKind::MySql, fx.conn.as_ref(), &fx.db).await;
    fx.done().await;
}

#[tokio::test]
async fn sqlite_round_trip() {
    let c = theta_drivers::connect(&sqlite_config(":memory:")).await.unwrap();
    scenario(DatabaseKind::Sqlite, c.as_ref(), "main").await;
}
