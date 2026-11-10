//! MySQL / MariaDB introspection through `information_schema`, in a fixed number
//! of queries (one per kind of metadata, never per table).

use mysql_async::prelude::*;
use mysql_async::{Conn, Row};
use quarry_core::*;
use std::collections::HashMap;

const SYSTEM: &str = "('mysql','information_schema','performance_schema','sys')";

async fn rows(conn: &mut Conn, sql: &str) -> Result<Vec<Row>> {
    conn.query::<Row, _>(sql).await.map_err(|e| Error::Query {
        message: e.to_string(),
        code: None,
        detail: None,
        hint: None,
        position: None,
    })
}

fn s(r: &Row, i: usize) -> Option<String> {
    r.get::<Option<String>, _>(i).flatten()
}

fn n(r: &Row, i: usize) -> Option<u64> {
    s(r, i).and_then(|v| v.parse().ok())
}

/// MariaDB shows string defaults quoted and a missing default as the text `NULL`;
/// MySQL 8 returns the raw value. Normalise to what a user would type.
fn normalise_default(raw: Option<String>, mariadb: bool, nullable: bool) -> Option<String> {
    let v = raw?;
    if mariadb && v == "NULL" && nullable {
        return None;
    }
    Some(v)
}

pub async fn introspect(conn: &mut Conn, mariadb: bool) -> Result<Catalog> {
    let schema_rows = rows(
        conn,
        &format!("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN {SYSTEM} ORDER BY 1"),
    )
    .await?;
    let mut schemas: Vec<Schema> =
        schema_rows.iter().map(|r| Schema { name: s(r, 0).unwrap_or_default(), ..Default::default() }).collect();
    let sidx: HashMap<String, usize> = schemas.iter().enumerate().map(|(i, x)| (x.name.clone(), i)).collect();
    let mut tidx: HashMap<(usize, String), usize> = HashMap::new();

    let tables = rows(
        conn,
        &format!(
            "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE, TABLE_ROWS, TABLE_COMMENT \
             FROM information_schema.TABLES WHERE TABLE_SCHEMA NOT IN {SYSTEM} ORDER BY TABLE_SCHEMA, TABLE_NAME"
        ),
    )
    .await?;
    for r in &tables {
        let Some(&si) = sidx.get(&s(r, 0).unwrap_or_default()) else { continue };
        let name = s(r, 1).unwrap_or_default();
        let ty = s(r, 2).unwrap_or_default();
        if ty == "SEQUENCE" {
            schemas[si].sequences.push(name);
            continue;
        }
        let kind = if ty.contains("VIEW") { TableKind::View } else { TableKind::Table };
        let comment = s(r, 4).filter(|c| !c.is_empty() && !(kind == TableKind::View && c == "VIEW"));
        schemas[si].tables.push(Table {
            name: name.clone(),
            kind,
            columns: vec![],
            primary_key: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            row_estimate: if kind == TableKind::View { None } else { n(r, 3) },
            comment,
        });
        tidx.insert((si, name), schemas[si].tables.len() - 1);
    }

    // MariaDB implements JSON as LONGTEXT plus a CHECK (json_valid(col)) named
    // after the column; detect those so the column reads as JSON.
    let mut json_cols: std::collections::HashSet<(String, String, String)> = Default::default();
    if mariadb {
        if let Ok(rs) = rows(
            conn,
            &format!(
                "SELECT CONSTRAINT_SCHEMA, TABLE_NAME, CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS \
                 WHERE CONSTRAINT_SCHEMA NOT IN {SYSTEM} AND CHECK_CLAUSE LIKE 'json\\_valid(%'"
            ),
        )
        .await
        {
            for r in rs {
                let clause = s(&r, 2).unwrap_or_default();
                if let Some(col) = clause.strip_prefix("json_valid(`").and_then(|c| c.strip_suffix("`)")) {
                    json_cols.insert((s(&r, 0).unwrap_or_default(), s(&r, 1).unwrap_or_default(), col.to_string()));
                }
            }
        }
    }
    // Spatial column details, best effort: the view differs between servers.
    let mut geo: HashMap<(String, String, String), (Option<i32>, Option<String>)> = HashMap::new();
    let geo_sql = if mariadb {
        "SELECT F_TABLE_SCHEMA, F_TABLE_NAME, F_GEOMETRY_COLUMN, SRID, GEOMETRY_TYPE FROM information_schema.GEOMETRY_COLUMNS"
    } else {
        "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, SRS_ID, GEOMETRY_TYPE_NAME FROM information_schema.ST_GEOMETRY_COLUMNS"
    };
    if let Ok(rs) = rows(conn, geo_sql).await {
        for r in rs {
            geo.insert(
                (s(&r, 0).unwrap_or_default(), s(&r, 1).unwrap_or_default(), s(&r, 2).unwrap_or_default()),
                (s(&r, 3).and_then(|v| v.parse().ok()), s(&r, 4)),
            );
        }
    }

    let cols = rows(
        conn,
        &format!(
            "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, \
                    COLUMN_COMMENT, EXTRA \
             FROM information_schema.COLUMNS WHERE TABLE_SCHEMA NOT IN {SYSTEM} \
             ORDER BY TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION"
        ),
    )
    .await?;
    for r in &cols {
        let schema = s(r, 0).unwrap_or_default();
        let table = s(r, 1).unwrap_or_default();
        let Some(&si) = sidx.get(&schema) else { continue };
        let Some(&ti) = tidx.get(&(si, table.clone())) else { continue };
        let name = s(r, 2).unwrap_or_default();
        let mut data_type = s(r, 3).unwrap_or_default();
        let base = s(r, 4).unwrap_or_default().to_ascii_lowercase();
        let nullable = s(r, 5).as_deref() == Some("YES");
        let extra = s(r, 8).unwrap_or_default();
        let mut default = normalise_default(s(r, 6), mariadb, nullable);
        if default.is_none() && extra.to_ascii_lowercase().contains("auto_increment") {
            default = Some("AUTO_INCREMENT".into());
        }
        let is_json = json_cols.contains(&(schema.clone(), table.clone(), name.clone()));
        if is_json {
            data_type = "json".into();
        }
        let mut kind = if is_json { TypeKind::Json } else { crate::util::kind_from_type_name(&base) };
        if base == "bit" {
            kind = TypeKind::Integer;
        }
        let geometry = if kind == TypeKind::Geometry {
            let g = geo.get(&(schema.clone(), table.clone(), name.clone()));
            let declared = base.to_ascii_uppercase();
            Some(GeometryColumn {
                geometry_type: match g.and_then(|g| g.1.clone()).filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("geometry")) {
                    Some(t) => t.to_ascii_uppercase(),
                    None => declared,
                },
                srid: g.and_then(|g| g.0).filter(|s| *s != 0),
                geography: false,
            })
        } else {
            None
        };
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name,
            data_type,
            kind,
            nullable,
            default,
            comment: s(r, 7).filter(|c| !c.is_empty()),
            geometry,
        });
    }

    // Indexes (and the primary key, which is the index named PRIMARY).
    let stats = rows(
        conn,
        &format!(
            "SELECT TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_TYPE \
             FROM information_schema.STATISTICS WHERE TABLE_SCHEMA NOT IN {SYSTEM} \
             ORDER BY TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX"
        ),
    )
    .await?;
    for r in &stats {
        let Some(&si) = sidx.get(&s(r, 0).unwrap_or_default()) else { continue };
        let Some(&ti) = tidx.get(&(si, s(r, 1).unwrap_or_default())) else { continue };
        let table = &mut schemas[si].tables[ti];
        let index_name = s(r, 2).unwrap_or_default();
        // Functional index parts have a NULL column name.
        let col = s(r, 3).unwrap_or_else(|| "<expression>".into());
        if index_name == "PRIMARY" {
            table.primary_key.push(col.clone());
        }
        if table.indexes.last().is_none_or(|i| i.name != index_name) {
            table.indexes.push(Index {
                name: index_name.clone(),
                columns: vec![],
                unique: s(r, 4).as_deref() == Some("0"),
                primary: index_name == "PRIMARY",
                method: s(r, 5).map(|m| m.to_ascii_lowercase()),
                definition: None,
            });
        }
        table.indexes.last_mut().unwrap().columns.push(col);
    }
    for sch in &mut schemas {
        for t in &mut sch.tables {
            let tname = t.name.clone();
            for i in &mut t.indexes {
                let q = |x: &str| DatabaseKind::MySql.quote_ident(x);
                let cols = i.columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
                i.definition = Some(if i.primary {
                    format!("ALTER TABLE {} ADD PRIMARY KEY ({cols})", q(&tname))
                } else {
                    format!(
                        "CREATE {}INDEX {} ON {} ({cols}){}",
                        if i.unique { "UNIQUE " } else { "" },
                        q(&i.name),
                        q(&tname),
                        i.method.as_deref().map(|m| format!(" USING {}", m.to_ascii_uppercase())).unwrap_or_default()
                    )
                });
            }
        }
    }

    let fks = rows(
        conn,
        &format!(
            "SELECT k.TABLE_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_SCHEMA, \
                    k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, r.UPDATE_RULE, r.DELETE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
               ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
              AND r.TABLE_NAME = k.TABLE_NAME \
             WHERE k.REFERENCED_TABLE_NAME IS NOT NULL AND k.TABLE_SCHEMA NOT IN {SYSTEM} \
             ORDER BY k.TABLE_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION"
        ),
    )
    .await?;
    for r in &fks {
        let Some(&si) = sidx.get(&s(r, 0).unwrap_or_default()) else { continue };
        let Some(&ti) = tidx.get(&(si, s(r, 1).unwrap_or_default())) else { continue };
        let cname = s(r, 2).unwrap_or_default();
        let table = &mut schemas[si].tables[ti];
        if table.foreign_keys.last().is_none_or(|f| f.name != cname) {
            table.foreign_keys.push(ForeignKey {
                name: cname,
                columns: vec![],
                referenced_schema: s(r, 4),
                referenced_table: s(r, 5).unwrap_or_default(),
                referenced_columns: vec![],
                on_update: action_rule(s(r, 7)),
                on_delete: action_rule(s(r, 8)),
            });
        }
        let fk = table.foreign_keys.last_mut().unwrap();
        fk.columns.push(s(r, 3).unwrap_or_default());
        fk.referenced_columns.push(s(r, 6).unwrap_or_default());
    }

    let routines = rows(
        conn,
        &format!(
            "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE, DTD_IDENTIFIER, SPECIFIC_NAME \
             FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA NOT IN {SYSTEM} ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME"
        ),
    )
    .await?;
    let params = rows(
        conn,
        &format!(
            "SELECT SPECIFIC_SCHEMA, SPECIFIC_NAME, PARAMETER_MODE, PARAMETER_NAME, DTD_IDENTIFIER \
             FROM information_schema.PARAMETERS WHERE SPECIFIC_SCHEMA NOT IN {SYSTEM} AND ORDINAL_POSITION > 0 \
             ORDER BY SPECIFIC_SCHEMA, SPECIFIC_NAME, ORDINAL_POSITION"
        ),
    )
    .await?;
    let mut sigs: HashMap<(String, String), Vec<String>> = HashMap::new();
    for r in &params {
        let mode = s(r, 2).map(|m| format!("{m} ")).unwrap_or_default();
        let name = s(r, 3).map(|m| format!("{m} ")).unwrap_or_default();
        sigs.entry((s(r, 0).unwrap_or_default(), s(r, 1).unwrap_or_default()))
            .or_default()
            .push(format!("{mode}{name}{}", s(r, 4).unwrap_or_default()));
    }
    for r in &routines {
        let schema = s(r, 0).unwrap_or_default();
        let Some(&si) = sidx.get(&schema) else { continue };
        let kind = if s(r, 2).as_deref() == Some("PROCEDURE") { RoutineKind::Procedure } else { RoutineKind::Function };
        let specific = s(r, 4).unwrap_or_default();
        schemas[si].routines.push(Routine {
            name: s(r, 1).unwrap_or_default(),
            kind,
            signature: format!("({})", sigs.get(&(schema, specific)).map(|v| v.join(", ")).unwrap_or_default()),
            return_type: if kind == RoutineKind::Function { s(r, 3) } else { None },
        });
    }

    let default_schema: Option<String> = conn.query_first("SELECT DATABASE()").await.ok().flatten().flatten();
    Ok(Catalog { schemas, default_schema })
}

fn action_rule(rule: Option<String>) -> Option<String> {
    // RESTRICT and NO ACTION behave the same in MySQL's InnoDB and are the default.
    rule.filter(|r| r != "NO ACTION" && r != "RESTRICT")
}
