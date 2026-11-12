//! Table-level DDL from the explorer and the palette: rename, truncate, drop and "duplicate structure".
//!
//! Every action builds its statements with the DDL engine (`theta_sql::ddl`), shows the **exact SQL** in a
//! confirmation dialog (with the engine's warnings and, for Drop, the foreign keys that point at the table),
//! and only then runs it. Read-only connections refuse; manual-commit sessions run the statements inside the
//! open transaction instead of committing behind the user's back; on MySQL the dialog says plainly that DDL
//! commits by itself.

use super::Workspace;
use crate::connections::NoticeKind;
use crate::dialogs::confirm::{self, Choice, Tone};
use crate::dialogs::name;
use crate::panels::messages::MessageKind;
use crate::runtime;
use gpui_kit::{Context, Window};
use std::rc::Rc;
use std::sync::Arc;
use theta_core::Family;
use theta_core::{Catalog, DatabaseKind, Table, TableKind};
use theta_sql::ddl::{self, Flag, Statement, TableDraft};
use theta_sql::keywords::qualified_name;

/// Everything an action needs to know about its table.
pub(super) struct Target {
    pub(super) kind: DatabaseKind,
    pub(super) schema: String,
    pub(super) table: Table,
    pub(super) catalog: Arc<Catalog>,
    pub(super) read_only: bool,
}

/// " (about 1,234 rows, the catalog's estimate)" for a table whose row count the catalog knows.
fn estimate_note(estimate: Option<u64>) -> String {
    match estimate {
        Some(n) => format!(" (about {}, the catalog's estimate)", theta_core::text::plural(n, "row", "rows")),
        None => String::new(),
    }
}

impl Workspace {
    pub(super) fn ddl_target(
        &self,
        conn_id: &str,
        schema: &str,
        table: &str,
        cx: &gpui_kit::App,
    ) -> Option<Target> {
        let store = self.store.read(cx);
        let config = store.config(conn_id)?;
        let catalog = store.catalog(conn_id)?;
        let (s, t) = catalog.find_table(Some(schema), table)?;
        Some(Target {
            kind: config.kind,
            schema: s.name.clone(),
            table: t.clone(),
            read_only: config.read_only,
            catalog,
        })
    }

    pub(super) fn refuse_read_only(&self, t: &Target, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if t.read_only {
            self.notify(
                NoticeKind::Info,
                "This connection is read-only, so its structure cannot be changed",
                window,
                cx,
            );
        }
        t.read_only
    }

    /// Lines that go above the SQL in the confirmation: what the engine warns about and where the
    /// transaction stands.
    pub(super) fn ddl_notes(
        &self,
        conn_id: &str,
        kind: DatabaseKind,
        stmts: &[Statement],
        cx: &gpui_kit::App,
    ) -> String {
        let mut lines = vec![ddl::summarize(stmts)];
        if kind.family() == Family::MySql {
            lines.push(
                "MySQL commits each DDL statement by itself: this cannot be rolled back, and it also commits any transaction that is open."
                    .into(),
            );
        } else if self.txn.contains(conn_id) || self.manual_begin(conn_id, cx).is_some() {
            lines.push("It runs inside your transaction: nothing is final until you commit.".into());
        } else if stmts.len() > 1 {
            lines.push("The statements run in one transaction: all of them or none.".into());
        }
        let destructive: Vec<&Statement> = stmts.iter().filter(|s| s.flags.contains(&Flag::Destructive)).collect();
        for s in &destructive {
            lines.push(format!("{}.", s.explanation.trim_end_matches('.')));
        }
        if !destructive.is_empty() {
            // Said once, and true for the way it will run: only a transaction you can still roll back keeps it
            // undoable (MySQL commits by itself, already said above).
            if kind.family() != Family::MySql {
                lines.push(
                    if self.txn.contains(conn_id) || self.manual_begin(conn_id, cx).is_some() {
                        "Until you commit, you can still undo this by rolling back."
                    } else {
                        "This cannot be undone once it has run."
                    }
                    .into(),
                );
            } else {
                lines.push("This cannot be undone once it has run.".into());
            }
        }
        lines.join("\n")
    }

    pub fn rename_table_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        if self.refuse_read_only(&t, window, cx) {
            return;
        }
        let existing: Vec<String> = t
            .catalog
            .schema(&t.schema)
            .map(|s| s.tables.iter().map(|x| x.name.clone()).collect())
            .unwrap_or_default();
        let ws = cx.entity().downgrade();
        let current = t.table.name.clone();
        let own = current.clone();
        let taken = existing.clone();
        name::open(
            name::Prompt {
                title: "Rename Table",
                label: "New name",
                initial: &current,
                submit: "Review SQL…",
            },
            Rc::new(move |text, _| {
                let n = text.trim();
                if n.is_empty() {
                    Err("A table needs a name".into())
                } else if taken.iter().any(|x| x.eq_ignore_ascii_case(n) && !x.eq_ignore_ascii_case(&own)) {
                    Err(format!("A table named {n} already exists"))
                } else {
                    Ok(())
                }
            }),
            Rc::new(move |new_name, window, cx| {
                let (conn_id, schema, table) = (conn_id.clone(), schema.clone(), table.clone());
                ws.update(cx, |this, cx| {
                    let Some(t) = this.ddl_target(&conn_id, &schema, &table, cx) else { return };
                    let original = TableDraft::from_table(&t.schema, &t.table);
                    let mut edited = original.clone();
                    edited.name = new_name.clone();
                    let stmts = ddl::diff(&original, &edited, t.kind);
                    this.confirm_ddl(
                        &conn_id,
                        format!("Rename {table} to {new_name}"),
                        "Rename Table",
                        "Rename",
                        Tone::Primary,
                        stmts,
                        window,
                        cx,
                    );
                })
                .ok();
            }),
            window,
            cx,
        );
    }

    pub fn truncate_table_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        if self.refuse_read_only(&t, window, cx) {
            return;
        }
        let name = qualified_name(
            (t.kind.family() != Family::Sqlite).then_some(t.schema.as_str()),
            &t.table.name,
            t.kind,
        );
        // SQLite has no TRUNCATE; DELETE without a WHERE is its (equally complete) spelling.
        let (sql, why) = match t.kind.family() {
            Family::Sqlite => (format!("DELETE FROM {name}"), "Delete all rows from"),
            _ => (format!("TRUNCATE TABLE {name}"), "Delete all rows from"),
        };
        let mut s = Statement {
            sql,
            flags: vec![Flag::Destructive],
            explanation: format!("{why} {}{}", t.table.name, estimate_note(t.table.row_estimate)),
        };
        if t.kind.family() == Family::MySql {
            s.flags.push(Flag::ImplicitCommit);
        }
        self.confirm_ddl(
            &conn_id,
            format!("Truncate {}", t.table.name),
            "Truncate Table",
            "Truncate",
            Tone::Danger,
            vec![s],
            window,
            cx,
        );
    }

    pub fn drop_table_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        if self.refuse_read_only(&t, window, cx) {
            return;
        }
        let name = qualified_name(
            (t.kind.family() != Family::Sqlite).then_some(t.schema.as_str()),
            &t.table.name,
            t.kind,
        );
        let what = match t.table.kind {
            TableKind::View => "VIEW",
            TableKind::MaterializedView => "MATERIALIZED VIEW",
            TableKind::ForeignTable => "FOREIGN TABLE",
            TableKind::Table => "TABLE",
        };
        let s = Statement {
            sql: format!("DROP {what} {name}"),
            flags: vec![Flag::Destructive],
            explanation: match t.table.kind {
                TableKind::View | TableKind::MaterializedView => {
                    format!("Drop the {} {}", what.to_lowercase(), t.table.name)
                }
                _ => format!(
                    "Drop the {} {} and all of its data{}",
                    what.to_lowercase(),
                    t.table.name,
                    estimate_note(t.table.row_estimate)
                ),
            },
        };
        // Foreign keys elsewhere that point here would block the drop (or, with CASCADE, be dropped).
        let dependents: Vec<String> = t
            .catalog
            .schemas
            .iter()
            .flat_map(|sc| sc.tables.iter().map(move |o| (sc, o)))
            .flat_map(|(sc, o)| {
                o.foreign_keys
                    .iter()
                    .filter(|f| {
                        f.referenced_table.eq_ignore_ascii_case(&t.table.name)
                            && f.referenced_schema
                                .as_deref()
                                .is_none_or(|rs| rs.eq_ignore_ascii_case(&t.schema))
                            && !(sc.name == t.schema && o.name == t.table.name)
                    })
                    .map(move |f| format!("{}.{} ({})", sc.name, o.name, f.name))
            })
            .collect();
        self.confirm_ddl_with(
            &conn_id,
            format!("Drop {}", t.table.name),
            "Drop Table",
            "Drop",
            Tone::Danger,
            vec![s],
            (!dependents.is_empty()).then(|| {
                format!(
                    "These foreign keys reference it and will make the drop fail until they are removed: {}.",
                    dependents.join(", ")
                )
            }),
            window,
            cx,
        );
    }

    /// "Duplicate Structure…": ask for a name, then open a console with the `CREATE TABLE` (no data).
    pub fn duplicate_structure_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        let existing: Vec<String> = t
            .catalog
            .schema(&t.schema)
            .map(|s| s.tables.iter().map(|x| x.name.clone()).collect())
            .unwrap_or_default();
        let initial = format!("{}_copy", t.table.name);
        let ws = cx.entity().downgrade();
        name::open(
            name::Prompt {
                title: "Duplicate Structure",
                label: "Name of the copy",
                initial: &initial,
                submit: "Open SQL",
            },
            Rc::new(move |text, _| {
                let n = text.trim();
                if n.is_empty() {
                    Err("A table needs a name".into())
                } else if existing.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                    Err(format!("A table named {n} already exists"))
                } else {
                    Ok(())
                }
            }),
            Rc::new(move |copy, window, cx| {
                let (conn_id, schema, table) = (conn_id.clone(), schema.clone(), table.clone());
                ws.update(cx, |this, cx| {
                    let Some(t) = this.ddl_target(&conn_id, &schema, &table, cx) else { return };
                    let mut draft = TableDraft::from_table(&t.schema, &t.table);
                    draft.name = copy.clone();
                    // Constraint and index names are schema-wide in Postgres and SQLite: reusing the
                    // originals would collide, so the copy lets the database name them.
                    draft.primary_key.iter_mut().for_each(|k| k.name = None);
                    draft.uniques.iter_mut().for_each(|k| k.name = None);
                    draft.foreign_keys.iter_mut().for_each(|k| k.name = None);
                    draft.checks.iter_mut().for_each(|k| k.name = None);
                    draft.indexes.iter_mut().for_each(|i| {
                        i.name = format!("{copy}_{}", i.name.trim_start_matches(&format!("{table}_")))
                    });
                    let sql = ddl::create_table(&draft, t.kind)
                        .into_iter()
                        .map(|s| format!("{};", s.sql))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    this.open_console(Some(conn_id.clone()), Some(sql), window, cx);
                })
                .ok();
            }),
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn confirm_ddl(
        &mut self,
        conn_id: &str,
        title: String,
        dialog_title: &'static str,
        verb: &'static str,
        tone: Tone,
        stmts: Vec<Statement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_ddl_with(conn_id, title, dialog_title, verb, tone, stmts, None, window, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn confirm_ddl_with(
        &mut self,
        conn_id: &str,
        title: String,
        dialog_title: &'static str,
        verb: &'static str,
        tone: Tone,
        stmts: Vec<Statement>,
        extra: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stmts.is_empty() {
            self.notify(NoticeKind::Info, "Nothing to change", window, cx);
            return;
        }
        let Some(kind) = self.store.read(cx).config(conn_id).map(|c| c.kind) else { return };
        let mut intro = self.ddl_notes(conn_id, kind, &stmts, cx);
        if let Some(e) = extra {
            intro.push('\n');
            intro.push_str(&e);
        }
        let ws = cx.entity().downgrade();
        let conn = conn_id.to_string();
        let sqls: Vec<String> = stmts.iter().map(|s| s.sql.clone()).collect();
        let run_stmts = stmts;
        confirm::open(
            window,
            cx,
            dialog_title,
            intro,
            sqls,
            vec![Choice::new(verb, tone, move |window, cx| {
                let (conn, title, stmts) = (conn.clone(), title.clone(), run_stmts.clone());
                ws.update(cx, |this, cx| this.run_ddl(&conn, &title, stmts, window, cx)).ok();
            })],
        );
    }

    /// Run `stmts` on `conn_id`. In a manual-commit session (or with a transaction already open) they join
    /// that transaction; otherwise PostgreSQL and SQLite run several statements as one transaction, and
    /// MySQL runs them one by one because it cannot do better.
    pub fn run_ddl(
        &mut self,
        conn_id: &str,
        title: &str,
        stmts: Vec<Statement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.store.read(cx).connection(conn_id) else { return };
        if !self.begin_bulk(conn_id, window, cx) {
            return;
        }
        let Some(kind) = self.store.read(cx).config(conn_id).map(|c| c.kind) else { return };
        let open = self.txn.contains(conn_id);
        let manual = self.manual_begin(conn_id, cx).map(|m| m.statements);
        let in_session = open || manual.is_some();
        if in_session && stmts.iter().any(|s| s.flags.contains(&Flag::OutsideTransaction)) {
            self.end_bulk(conn_id, cx);
            self.notify(
                NoticeKind::Error,
                "This change must run outside a transaction; commit or roll back first",
                window,
                cx,
            );
            return;
        }
        let mut script: Vec<String> = Vec::new();
        if in_session {
            if !open {
                script.extend(manual.unwrap_or_default());
            }
        } else if ddl::transactional_ddl(kind) && stmts.len() > 1 {
            script.push("BEGIN".into());
        }
        let wrapped = !in_session && script.first().is_some_and(|s| s == "BEGIN");
        script.extend(stmts.iter().map(|s| s.sql.clone()));
        if wrapped {
            script.push("COMMIT".into());
        }
        let (conn, title, shown) =
            (conn_id.to_string(), title.to_string(), stmts.iter().map(|s| s.sql.clone()).collect::<Vec<_>>());
        let task = runtime::spawn(async move {
            let opts = theta_core::ExecOptions { row_limit: Some(0) };
            for (i, sql) in script.iter().enumerate() {
                if let Err(e) = connection.execute(sql, &opts).await {
                    if wrapped {
                        let _ = connection.execute("ROLLBACK", &opts).await;
                    }
                    return Err((sql.clone(), e.to_string(), i));
                }
            }
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.end_bulk(&conn, cx);
                match result {
                    Ok(Ok(())) => {
                        this.log_bulk(&conn, &title, MessageKind::Ok, shown.clone(), cx);
                        this.notify(NoticeKind::Success, format!("{title}: done"), window, cx);
                        this.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
                    }
                    Ok(Err((sql, msg, _))) => {
                        let mut lines = vec![format!("Failed: {msg}"), format!("Statement: {sql}")];
                        lines.push("Nothing was changed by the failed statement.".into());
                        this.log_bulk(&conn, &title, MessageKind::Error, lines, cx);
                        this.notify(NoticeKind::Error, format!("{title} failed: {msg}"), window, cx);
                        this.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
                    }
                    Err(e) => this.notify(NoticeKind::Error, format!("{title} failed: {e}"), window, cx),
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::ConnectionStore;
    use crate::connections::secrets::SecretStore;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;
    use theta_core::{ConnectionConfig, ExecOptions, Value};

    fn wait_for(
        vcx: &mut VisualTestContext,
        what: &str,
        mut done: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        vcx.executor().allow_parking();
        for _ in 0..800 {
            vcx.run_until_parked();
            if done(vcx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn setup(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, Entity<ConnectionStore>, String, &mut VisualTestContext) {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        cx.executor().allow_parking();
        cx.update(|cx| {
            theta_ui::init(cx);
            crate::keymap::bind(cx);
        });
        let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
        let store = cx.new(|_| store);
        let mut config = ConnectionConfig::new("t", "t", DatabaseKind::Sqlite);
        config.path = ":memory:".into();
        let conn = store.update(cx, |s, cx| {
            let id = s.add_temporary(config, cx);
            s.connect(&id, cx);
            id
        });
        let (ws, vcx) = cx.add_window_view(|window, cx| {
            let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
            ws.quiet = true;
            ws
        });
        wait_for(vcx, "the connection", |vcx| store.read_with(vcx, |s, _| s.state(&conn).is_connected()));
        (ws, store, conn, vcx)
    }

    fn sql(
        store: &Entity<ConnectionStore>,
        conn: &str,
        vcx: &mut VisualTestContext,
        sql: &str,
    ) -> Result<Vec<Vec<Value>>, String> {
        let connection = store.read_with(vcx, |s, _| s.connection(conn)).unwrap();
        let slot = Arc::new(Mutex::new(None));
        let (w, q) = (slot.clone(), sql.to_string());
        runtime::runtime().spawn(async move {
            *w.lock().unwrap() = Some(connection.execute(&q, &ExecOptions { row_limit: None }).await);
        });
        wait_for(vcx, "the statement", |_| slot.lock().unwrap().is_some());
        let r = slot.lock().unwrap().take().unwrap();
        r.map(|o| o.result.map(|r| r.rows).unwrap_or_default()).map_err(|e| e.to_string())
    }

    fn stmt(sql: &str) -> Statement {
        Statement { sql: sql.into(), flags: vec![], explanation: String::new() }
    }

    #[gpui_kit::test]
    fn a_rename_runs_and_the_catalog_follows(cx: &mut TestAppContext) {
        let (ws, store, conn, vcx) = setup(cx);
        sql(&store, &conn, vcx, "CREATE TABLE old_name (id INTEGER PRIMARY KEY)").unwrap();
        let t = TableDraft { name: "old_name".into(), ..Default::default() };
        let mut renamed = t.clone();
        renamed.name = "new_name".into();
        let stmts = ddl::diff(&t, &renamed, DatabaseKind::Sqlite);
        assert_eq!(stmts[0].sql, "ALTER TABLE old_name RENAME TO new_name");
        ws.update_in(vcx, |ws, window, cx| ws.run_ddl(&conn, "Rename old_name", stmts, window, cx));
        wait_for(vcx, "the rename", |vcx| ws.read_with(vcx, |w, _| w.runs.is_empty()));
        assert!(sql(&store, &conn, vcx, "SELECT count(*) FROM new_name").is_ok());
        assert!(sql(&store, &conn, vcx, "SELECT count(*) FROM old_name").is_err());
    }

    #[gpui_kit::test]
    fn several_statements_are_all_or_nothing_where_ddl_is_transactional(cx: &mut TestAppContext) {
        let (ws, store, conn, vcx) = setup(cx);
        let stmts = vec![stmt("CREATE TABLE half_done (id INTEGER)"), stmt("CREATE TABLE this is not sql")];
        ws.update_in(vcx, |ws, window, cx| ws.run_ddl(&conn, "Two statements", stmts, window, cx));
        wait_for(vcx, "the failing script", |vcx| ws.read_with(vcx, |w, _| w.runs.is_empty()));
        assert!(
            sql(&store, &conn, vcx, "SELECT count(*) FROM half_done").is_err(),
            "the first statement must be rolled back"
        );
        assert!(!store.read_with(vcx, |s, _| s.connection(&conn)).is_none());
    }
}
