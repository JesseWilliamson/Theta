//! Table editor tabs: opening them, answering their events, and running their script.
//!
//! The editor (`crate::table_editor`) never touches a database; everything that needs the connection, the
//! transaction mode or a dialog is here, built from the same pieces as the explorer's table actions
//! (`ddl_actions.rs`): the exact-SQL confirmation, the Messages log, the catalog refresh, and the
//! "refuse on read-only, join the open transaction in manual mode" rules.

use super::Workspace;
use crate::connections::NoticeKind;
use crate::dialogs::confirm::{self, Choice, Item, Tone};
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::table_editor::model::DraftModel;
use crate::table_editor::{TableEditor, TableEditorEvent};
use gpui_kit::assets::IconName;
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, WeakEntity, Window};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use theta_core::Family;
use theta_core::{DatabaseKind, TableKind};
use theta_sql::VirtualForeignKey;
use theta_sql::ddl::{self, Flag, Session, Statement};
use theta_ui::picker::{PickerDelegate, PickerItem};

/// What a table editor tab is opened for.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorTarget {
    /// A table that does not exist yet, in `schema` (the connection's default when `None`).
    New { schema: Option<String> },
    /// An existing table; `column` puts the cursor on that column.
    Modify { schema: String, table: String, column: Option<String> },
}

/// How a script ended, as the Tokio task reports it.
enum Outcome {
    Done,
    Failed { statement: Option<usize>, message: String, completed: usize, rolled_back: bool },
}

impl Workspace {
    pub fn table_editor(&self, tab: u64) -> Option<Entity<TableEditor>> {
        self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.table.clone())
    }

    /// The tab (if any) that is editing `schema.table` on `conn_id`.
    fn find_editor_tab(&self, conn_id: &str, schema: &str, table: &str, cx: &App) -> Option<u64> {
        self.tabs.iter().find_map(|t| {
            let e = t.table.as_ref()?;
            let e = e.read(cx);
            let o = e.model().original()?;
            (e.conn_id() == conn_id
                && o.name.eq_ignore_ascii_case(table)
                && o.schema.as_deref().is_none_or(|s| s.eq_ignore_ascii_case(schema) || schema.is_empty()))
            .then_some(t.id)
        })
    }

    /// Whether `feature` is missing for the connection's product; if so, tells the user why and returns
    /// `true`. The reason text comes from the kind's descriptor (`theta_core::KindInfo`), so the menu
    /// entries, the palette and the command line all say the same thing.
    pub fn refuse_unsupported(
        &self,
        conn_id: &str,
        feature: theta_core::Feature,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(kind) = self.store.read(cx).config(conn_id).map(|c| c.kind) else { return false };
        let Some(reason) = kind.unsupported(feature) else { return false };
        self.notify(NoticeKind::Info, format!("{}: {reason}", kind.label()), window, cx);
        true
    }

    /// Open the editor for a new table or an existing one. `None` (after telling the user why) when the
    /// connection is not ready or the table cannot be edited.
    pub fn open_table_editor(
        &mut self,
        conn_id: String,
        target: EditorTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let feature = match target {
            EditorTarget::New { .. } => theta_core::Feature::CreateTable,
            EditorTarget::Modify { .. } => theta_core::Feature::ModifyTable,
        };
        if self.refuse_unsupported(&conn_id, feature, window, cx) {
            return None;
        }
        let (config, catalog) = {
            let store = self.store.read(cx);
            (store.config(&conn_id).cloned(), store.catalog(&conn_id))
        };
        let (Some(config), Some(catalog)) = (config, catalog) else {
            self.notify(
                NoticeKind::Info,
                "Connect to the database first; its tables load after connecting",
                window,
                cx,
            );
            return None;
        };
        let model = match &target {
            EditorTarget::New { schema } => {
                let schema = schema
                    .clone()
                    .or_else(|| catalog.default_schema.clone())
                    .or_else(|| catalog.schemas.first().map(|s| s.name.clone()));
                DraftModel::new_table(config.kind, schema, Some(catalog.clone()))
            }
            EditorTarget::Modify { schema, table, column } => {
                if let Some(id) = self.find_editor_tab(&conn_id, schema, table, cx) {
                    self.activate_tab(id, window, cx);
                    if let (Some(col), Some(e)) = (column, self.table_editor(id)) {
                        e.update(cx, |e, cx| e.focus_column(col, window, cx));
                    }
                    return Some(id);
                }
                let Some((s, t)) = catalog.find_table(Some(schema), table) else {
                    self.notify(
                        NoticeKind::Error,
                        format!("There is no table {table} in {schema}"),
                        window,
                        cx,
                    );
                    return None;
                };
                if t.kind != TableKind::Table {
                    self.notify(
                        NoticeKind::Info,
                        format!("{table} is not a table; only tables can be modified here"),
                        window,
                        cx,
                    );
                    return None;
                }
                DraftModel::modify(config.kind, &s.name, t, Some(catalog.clone()))
            }
        };
        let new = model.is_new();
        let name = self.store.read(cx).display_name(&conn_id);
        let title = crate::table_editor::title_for(&model);
        let editor = {
            let (conn, read_only) = (conn_id.clone(), config.read_only);
            cx.new(|cx| TableEditor::new(model, conn, name, read_only, window, cx))
        };
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(super::ConsoleTab {
            id,
            title: title.into(),
            connection_id: Some(conn_id.clone()),
            modified: false,
            initial_sql: None,
            table: Some(editor.clone()),
            admin: None,
            tool: None,
        });
        let sub = cx.subscribe_in(&editor, window, move |this, _, ev: &TableEditorEvent, window, cx| {
            this.on_table_event(id, ev.clone(), window, cx)
        });
        self.table_subs.insert(id, sub);
        self.console_views.insert(id, editor.clone().into());
        self.active_connection = Some(conn_id.clone());
        self.activate_tab(id, window, cx);
        if let EditorTarget::Modify { column: Some(col), .. } = &target {
            editor.update(cx, |e, cx| e.focus_column(col, window, cx));
        }
        if new {
            editor.update(cx, |e, cx| e.focus_name(window, cx));
        }
        if let EditorTarget::Modify { table, .. } = &target
            && config.kind.family() == Family::Sqlite
        {
            self.load_sqlite_triggers(&conn_id, table, editor, window, cx);
        }
        self.save_all_consoles(cx);
        Some(id)
    }

    /// A SQLite table rebuild drops the table's triggers; read them so the engine can recreate them.
    fn load_sqlite_triggers(
        &mut self,
        conn_id: &str,
        table: &str,
        editor: Entity<TableEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.store.read(cx).connection(conn_id) else { return };
        let query = format!(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND tbl_name = '{}' AND sql IS NOT NULL ORDER BY name",
            table.replace('\'', "''")
        );
        let task = runtime::spawn(async move {
            connection.execute(&query, &theta_core::ExecOptions { row_limit: Some(1000) }).await
        });
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(out)) = task.await else { return };
            let triggers: Vec<String> = out
                .result
                .map(|r| {
                    r.rows.into_iter().filter_map(|row| row.into_iter().next()).map(|v| v.display()).collect()
                })
                .unwrap_or_default();
            if !triggers.is_empty() {
                editor.update(cx, |e, cx| e.set_triggers(triggers, cx));
            }
        })
        .detach();
    }

    fn on_table_event(
        &mut self,
        tab: u64,
        ev: TableEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.table_editor(tab) else { return };
        match ev {
            TableEditorEvent::StateChanged => {
                let (title, dirty) = {
                    let e = editor.read(cx);
                    (e.title(), e.dirty())
                };
                self.set_tab_title(tab, title, cx);
                self.set_tab_modified(tab, dirty, cx);
            }
            TableEditorEvent::Execute => self.confirm_table_execute(tab, window, cx),
            TableEditorEvent::Reloaded { schema, table } => {
                let conn = editor.read(cx).conn_id().to_string();
                self.explorer.update(cx, |ex, cx| ex.reveal_table(&conn, &schema, &table, cx));
            }
            TableEditorEvent::OpenInConsole(sql) => {
                let conn = editor.read(cx).conn_id().to_string();
                self.open_console(Some(conn), Some(sql), window, cx);
            }
            TableEditorEvent::PickTable { fk } => {
                let (conn, kind) = {
                    let e = editor.read(cx);
                    (e.conn_id().to_string(), e.kind())
                };
                let Some(catalog) = self.store.read(cx).catalog(&conn) else { return };
                let delegate = RefTablePicker { editor: editor.downgrade(), catalog, fk, kind };
                self.open_picker(delegate, window, cx);
            }
            TableEditorEvent::PickSuggestion => {
                let items = editor.read(cx).open_suggestions();
                self.open_picker(SuggestionPicker { editor: editor.downgrade(), items }, window, cx);
            }
        }
    }

    /// Hand a refreshed catalog to the editors of that connection.
    pub(super) fn sync_table_editors(&mut self, conn_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let catalog = self.store.read(cx).catalog(conn_id);
        let editors: Vec<Entity<TableEditor>> = self
            .tabs
            .iter()
            .filter_map(|t| t.table.clone())
            .filter(|e| e.read(cx).conn_id() == conn_id)
            .collect();
        for e in editors {
            e.update(cx, |e, cx| e.set_catalog(catalog.clone(), window, cx));
        }
    }

    // ------------------------------------------------------------ execute

    fn confirm_table_execute(&mut self, tab: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.table_editor(tab) else { return };
        // Headless test windows have no `Root` to host the dialog: record that it would have been shown.
        if self.quiet {
            #[cfg(test)]
            self.execute_asked.push(tab);
            return;
        }
        let (conn, kind, stmts, new, name) = {
            let e = editor.read(cx);
            (
                e.conn_id().to_string(),
                e.kind(),
                e.statements().to_vec(),
                e.model().is_new(),
                e.model().draft().name.clone(),
            )
        };
        if stmts.is_empty() {
            return;
        }
        let mut intro = self.ddl_notes(&conn, kind, &stmts, cx);
        if kind == DatabaseKind::Sqlite && stmts.iter().any(|s| s.flags.contains(&Flag::Rebuild)) {
            intro.push_str(
                "\nSQLite rebuilds the table for this change: every row is copied into a new table.",
            );
        }
        let items: Vec<Item> = stmts.iter().map(item_of).collect();
        let danger = stmts.iter().any(Statement::is_destructive);
        let (title, verb) = if new { ("Create Table", "Create") } else { ("Apply Changes", "Execute") };
        let ws = cx.entity().downgrade();
        let run_stmts = stmts;
        let label = if new { format!("Create table {name}") } else { format!("Modify table {name}") };
        confirm::open_items(
            window,
            cx,
            title,
            intro,
            items,
            vec![Choice::new(verb, if danger { Tone::Danger } else { Tone::Primary }, move |window, cx| {
                let (stmts, label) = (run_stmts.clone(), label.clone());
                ws.update(cx, |this, cx| this.run_table_script(tab, label, stmts, window, cx)).ok();
            })],
        );
    }

    /// Run `stmts` for the editor in `tab`. The plan (`ddl::plan_script`) decides what is wrapped in a
    /// transaction; this reports each outcome back to the editor so a failure is marked on its statement.
    fn run_table_script(
        &mut self,
        tab: u64,
        title: String,
        stmts: Vec<Statement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.table_editor(tab) else { return };
        let conn_id = editor.read(cx).conn_id().to_string();
        let Some(connection) = self.store.read(cx).connection(&conn_id) else {
            self.notify(NoticeKind::Error, "The connection is closed; reconnect and try again", window, cx);
            return;
        };
        let Some(kind) = self.store.read(cx).config(&conn_id).map(|c| c.kind) else { return };
        if self.store.read(cx).config(&conn_id).is_some_and(|c| c.read_only) {
            self.notify(
                NoticeKind::Info,
                "This connection is read-only, so its structure cannot be changed",
                window,
                cx,
            );
            return;
        }
        let open = self.txn.contains(&conn_id);
        let manual = self.manual_begin(&conn_id, cx).map(|m| m.statements);
        let in_session = open || manual.is_some();
        let session = if in_session {
            Session::Joined { begin: if open { vec![] } else { manual.unwrap_or_default() } }
        } else {
            Session::Own
        };
        let script = match ddl::plan_script(&stmts, kind, &session) {
            Ok(s) => s,
            Err(message) => {
                editor.update(cx, |e, cx| e.set_failed(None, message.clone(), String::new(), cx));
                self.notify(NoticeKind::Error, message, window, cx);
                return;
            }
        };
        if !self.begin_bulk(&conn_id, window, cx) {
            return;
        }
        let total = stmts.len();
        editor.update(cx, |e, cx| e.set_running(total, cx));
        // The task reports how far it got; a poller shows it in the preview while the script runs.
        let done = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        {
            let (done, finished, editor) = (done.clone(), finished.clone(), editor.downgrade());
            cx.spawn(async move |_, cx| {
                while !finished.load(Ordering::Relaxed) {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let n = done.load(Ordering::Relaxed);
                    if editor.update(cx, |e, cx| e.set_progress(n, total, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let shown: Vec<String> = stmts.iter().map(|s| s.sql.clone()).collect();
        let own = script.own_transaction;
        let (task_done, task_finished) = (done.clone(), finished.clone());
        let task = runtime::spawn(async move {
            let opts = theta_core::ExecOptions { row_limit: Some(10) };
            let mut failure: Option<(Option<usize>, String)> = None;
            let mut completed = 0usize;
            let mut rolled_back = false;
            for step in &script.steps {
                if failure.is_some() && !step.always {
                    continue;
                }
                let result = connection.execute(&step.sql, &opts).await;
                let problem = match result {
                    Ok(out) if step.fails_on_rows => {
                        out.result.as_ref().filter(|r| !r.rows.is_empty()).map(|r| {
                            format!("{} foreign key violation(s) found after the rebuild", r.rows.len())
                        })
                    }
                    Ok(_) => None,
                    Err(e) => Some(e.to_string()),
                };
                match problem {
                    None if failure.is_none() && step.statement.is_some() => {
                        completed += 1;
                        task_done.store(step.statement.map_or(0, |i| i + 1), Ordering::Relaxed);
                    }
                    None => {}
                    Some(message) if failure.is_none() => {
                        failure = Some((step.statement, message));
                        if own {
                            let _ = connection.execute("ROLLBACK", &opts).await;
                            rolled_back = true;
                        }
                    }
                    // A step that must run after a failure (restoring a pragma) failed too: the first
                    // problem is the one that matters.
                    Some(_) => {}
                }
            }
            task_finished.store(true, Ordering::Relaxed);
            match failure {
                None => Outcome::Done,
                Some((statement, message)) => Outcome::Failed { statement, message, completed, rolled_back },
            }
        });
        let conn = conn_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            this.update_in(cx, |this, window, cx| {
                this.end_bulk(&conn, cx);
                match outcome {
                    Ok(Outcome::Done) => {
                        this.log_bulk(&conn, &title, MessageKind::Ok, shown.clone(), cx);
                        this.notify(NoticeKind::Success, format!("{title}: done"), window, cx);
                        editor.update(cx, |e, cx| e.applied(in_session, cx));
                        this.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
                    }
                    Ok(Outcome::Failed { statement, message, completed, rolled_back }) => {
                        let note =
                            failure_note(kind, in_session, completed, rolled_back, statement.is_some());
                        let mut lines = vec![format!("Failed: {message}")];
                        if let Some(s) = statement.and_then(|i| shown.get(i)) {
                            lines.push(format!("Statement: {s}"));
                        }
                        lines.push(note.clone());
                        this.log_bulk(&conn, &title, MessageKind::Error, lines, cx);
                        // The editor shows the failure where the user is looking; a toast only when it is
                        // not on screen (the user moved to another tab while the statements ran).
                        if this.active_tab != Some(tab) {
                            this.notify(NoticeKind::Error, format!("{title} failed: {message}"), window, cx);
                        }
                        editor.update(cx, |e, cx| e.set_failed(statement, message, note, cx));
                        if completed > 0 && !rolled_back {
                            // Something did change (MySQL commits each statement): show it in the explorer.
                            this.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
                        }
                    }
                    Err(e) => {
                        editor.update(cx, |e2, cx| e2.set_failed(None, e.to_string(), String::new(), cx));
                        this.record_notice(MessageKind::Error, &title, &e.to_string(), cx);
                        if this.active_tab != Some(tab) {
                            this.notify(NoticeKind::Error, format!("{title} failed: {e}"), window, cx);
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------- closing

    /// `true` when the tab holds unapplied edits (the caller then asks before closing).
    pub(super) fn table_tab_dirty(&self, tab: u64, cx: &App) -> bool {
        self.table_editor(tab).is_some_and(|e| e.read(cx).dirty())
    }

    /// Ask "Discard / Cancel" for a table tab with unapplied changes; closes it on Discard.
    pub(super) fn confirm_discard_table_tab(
        &mut self,
        tab: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quiet {
            #[cfg(test)]
            self.discard_asked.push(tab);
            return;
        }
        let title = self.tabs.iter().find(|t| t.id == tab).map(|t| t.title.to_string()).unwrap_or_default();
        let ws = cx.entity().downgrade();
        confirm::open(
            window,
            cx,
            "Discard changes?",
            format!("{title} has changes that were not applied. Closing the tab discards them."),
            vec![],
            vec![Choice::new("Discard", Tone::Danger, move |window, cx| {
                ws.update(cx, |this, cx| this.close_tab_now(tab, window, cx)).ok();
            })],
        );
    }

    /// Dirty table tabs, for the quit prompt.
    pub(super) fn dirty_table_tabs(&self, cx: &App) -> Vec<String> {
        self.tabs
            .iter()
            .filter(|t| t.table.as_ref().is_some_and(|e| e.read(cx).dirty()))
            .map(|t| t.title.to_string())
            .collect()
    }

    pub(super) fn is_table_tab(&self, tab: u64) -> bool {
        self.tabs.iter().any(|t| t.id == tab && t.table.is_some())
    }
}

fn item_of(s: &Statement) -> Item {
    let mut badges = Vec::new();
    for f in &s.flags {
        badges.push(match f {
            Flag::Destructive => ("destructive".to_string(), true),
            Flag::TableRewrite => ("table rewrite".to_string(), false),
            Flag::ImplicitCommit => ("implicit commit".to_string(), false),
            Flag::Lock(t) => (format!("lock: {t}"), false),
            Flag::OutsideTransaction => ("outside transaction".to_string(), false),
            Flag::Rebuild => ("rebuild".to_string(), false),
        });
    }
    Item { sql: s.sql.clone(), badges, note: (!s.explanation.is_empty()).then(|| s.explanation.clone()) }
}

/// What the user needs to know about the state of the database after a failed script.
fn failure_note(
    kind: DatabaseKind,
    in_session: bool,
    completed: usize,
    rolled_back: bool,
    at_statement: bool,
) -> String {
    if rolled_back {
        return "Everything was rolled back: the database is as it was. Your edits are kept.".into();
    }
    if in_session {
        return "The statements ran inside your open transaction. Roll it back to undo them; on PostgreSQL the failed statement leaves it aborted until you do. Your edits are kept.".into();
    }
    if kind.family() == Family::MySql && completed > 0 {
        return format!(
            "MySQL commits each DDL statement: the {completed} statement{} before the failed one {} already applied and cannot be rolled back. Your edits are kept; close and reopen the table to continue from its new state.",
            if completed == 1 { "" } else { "s" },
            if completed == 1 { "is" } else { "are" }
        );
    }
    if at_statement {
        "The failed statement changed nothing. Your edits are kept.".into()
    } else {
        "Nothing was changed. Your edits are kept.".into()
    }
}

/// Chooses the table a foreign key points at.
struct RefTablePicker {
    editor: WeakEntity<TableEditor>,
    catalog: std::sync::Arc<theta_core::Catalog>,
    fk: u32,
    kind: DatabaseKind,
}

impl PickerDelegate for RefTablePicker {
    fn placeholder(&self) -> SharedString {
        "Referenced table…".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.catalog
            .schemas
            .iter()
            .flat_map(|s| {
                s.tables.iter().filter(|t| t.kind == TableKind::Table).map(move |t| {
                    PickerItem::new(format!("{}\u{1}{}", s.name, t.name), format!("{}.{}", s.name, t.name))
                        .icon(IconName::Table2)
                        .detail(if t.primary_key.is_empty() {
                            "no primary key".to_string()
                        } else {
                            format!("key: {}", t.primary_key.join(", "))
                        })
                })
            })
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some((schema, table)) = item.id.split_once('\u{1}') else { return };
        let (schema, table, fk, editor) =
            (schema.to_string(), table.to_string(), self.fk, self.editor.clone());
        let _ = self.kind;
        window.defer(cx, move |window, cx| {
            editor
                .update(cx, |e, cx| {
                    e.choose_fk_target(fk, Some(schema), &table, cx);
                    window.focus(&e.focus_handle_ref(), cx);
                })
                .ok();
        });
    }
}

/// Chooses one of the foreign keys the catalog suggests.
struct SuggestionPicker {
    editor: WeakEntity<TableEditor>,
    items: Vec<VirtualForeignKey>,
}

impl PickerDelegate for SuggestionPicker {
    fn placeholder(&self) -> SharedString {
        "Add a suggested foreign key…".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, v)| {
                PickerItem::new(
                    i.to_string(),
                    format!("{} \u{2192} {}.{}", v.column, v.referenced_table, v.referenced_column),
                )
                .icon(IconName::Unlink)
                .detail(format!("{:.0}%", v.confidence * 100.0))
            })
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some(v) = item.id.parse::<usize>().ok().and_then(|i| self.items.get(i).cloned()) else { return };
        let editor = self.editor.clone();
        window.defer(cx, move |window, cx| {
            editor
                .update(cx, |e, cx| {
                    e.add_suggested(&v, window, cx);
                    window.focus(&e.focus_handle_ref(), cx);
                })
                .ok();
        });
    }
}

/// What "Drop …" on an explorer leaf removes.
pub enum DropWhat {
    Index(String),
    ForeignKey(String),
}

impl Workspace {
    /// The statements that turn `table` into what `edit` leaves of it, from the same model the editor
    /// uses, so a drop that has consequences (a column that is part of a key or an index) shows all of them.
    fn script_for(
        &self,
        t: &super::ddl_actions::Target,
        edit: impl FnOnce(&mut DraftModel),
    ) -> Vec<Statement> {
        let mut m = DraftModel::modify(t.kind, &t.schema, &t.table, Some(t.catalog.clone()));
        edit(&mut m);
        m.analysis().statements.clone()
    }

    pub fn rename_column_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        column: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::table_editor::model::{CellValue, Field, RowId};
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        if self.refuse_read_only(&t, window, cx) {
            return;
        }
        let siblings: Vec<String> = t.table.columns.iter().map(|c| c.name.clone()).collect();
        let (own, taken) = (column.clone(), siblings);
        let ws = cx.entity().downgrade();
        let initial = column.clone();
        crate::dialogs::name::open(
            crate::dialogs::name::Prompt {
                title: "Rename Column",
                label: "New name",
                initial: &initial,
                submit: "Review SQL…",
            },
            std::rc::Rc::new(move |text, _| {
                let n = text.trim();
                if n.is_empty() {
                    Err("A column needs a name".into())
                } else if taken.iter().any(|x| x.eq_ignore_ascii_case(n) && !x.eq_ignore_ascii_case(&own)) {
                    Err(format!("A column named {n} already exists"))
                } else {
                    Ok(())
                }
            }),
            std::rc::Rc::new(move |new_name, window, cx| {
                let (conn_id, schema, table, column) =
                    (conn_id.clone(), schema.clone(), table.clone(), column.clone());
                ws.update(cx, |this, cx| {
                    let Some(t) = this.ddl_target(&conn_id, &schema, &table, cx) else { return };
                    let stmts = this.script_for(&t, |m| {
                        if let Some(id) = m.draft().columns.iter().find(|c| c.name == column).map(|c| c.id) {
                            let _ =
                                m.set_cell(RowId::Column(id), Field::Name, CellValue::Text(new_name.clone()));
                        }
                    });
                    this.confirm_ddl(
                        &conn_id,
                        format!("Rename column {column} to {new_name}"),
                        "Rename Column",
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

    pub fn drop_column_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        column: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::table_editor::model::RowId;
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        if self.refuse_read_only(&t, window, cx) {
            return;
        }
        if t.table.columns.len() == 1 {
            self.notify(
                NoticeKind::Info,
                "A table needs at least one column; drop the table instead",
                window,
                cx,
            );
            return;
        }
        let stmts = self.script_for(&t, |m| {
            if let Some(id) = m.draft().columns.iter().find(|c| c.name == column).map(|c| c.id) {
                let _ = m.delete_row(RowId::Column(id));
            }
        });
        self.confirm_ddl(
            &conn_id,
            format!("Drop column {column}"),
            "Drop Column",
            "Drop",
            Tone::Danger,
            stmts,
            window,
            cx,
        );
    }

    pub fn drop_constraint_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        what: DropWhat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::table_editor::model::RowId;
        let Some(t) = self.ddl_target(&conn_id, &schema, &table, cx) else { return };
        if self.refuse_read_only(&t, window, cx) {
            return;
        }
        let label = match &what {
            DropWhat::Index(n) => format!("Drop index {n}"),
            DropWhat::ForeignKey(n) => format!("Drop foreign key {n}"),
        };
        let mut found = false;
        let stmts = self.script_for(&t, |m| {
            let row = match &what {
                DropWhat::Index(n) => {
                    m.draft().indexes.iter().find(|i| i.name == *n).map(|i| RowId::Index(i.id))
                }
                DropWhat::ForeignKey(n) => m
                    .draft()
                    .foreign_keys
                    .iter()
                    .find(|f| f.name.as_deref() == Some(n.as_str()))
                    .map(|f| RowId::Fk(f.id)),
            };
            if let Some(row) = row {
                found = m.delete_row(row).is_ok();
            }
        });
        if !found {
            self.notify(
                NoticeKind::Info,
                "That entry is part of a key or constraint; change it in Modify Table",
                window,
                cx,
            );
            return;
        }
        let dialog = if matches!(what, DropWhat::Index(_)) { "Drop Index" } else { "Drop Foreign Key" };
        self.confirm_ddl(&conn_id, label, dialog, "Drop", Tone::Danger, stmts, window, cx);
    }

    /// "New Index…" / "New Foreign Key…": the editor on that table with a fresh row, its columns popup open.
    pub fn new_constraint_row(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        section: crate::table_editor::model::Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) =
            self.open_table_editor(conn_id, EditorTarget::Modify { schema, table, column: None }, window, cx)
        else {
            return;
        };
        if let Some(e) = self.table_editor(id) {
            e.update(cx, |e, cx| e.begin_new_row(section, window, cx));
        }
    }

    pub fn add_suggested_foreign_key(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        v: VirtualForeignKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) =
            self.open_table_editor(conn_id, EditorTarget::Modify { schema, table, column: None }, window, cx)
        else {
            return;
        };
        if let Some(e) = self.table_editor(id) {
            e.update(cx, |e, cx| e.add_suggested(&v, window, cx));
        }
    }

    /// `:table new` / `:table modify [NAME]` in the editor's command line.
    pub(super) fn table_host_command(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (verb, name) =
            arg.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((arg, ""));
        let Some(conn) = self.bulk_connection(cx) else {
            self.notify(NoticeKind::Info, "Connect to a database first", window, cx);
            return;
        };
        match verb {
            "new" => {
                let schema = (!name.is_empty()).then(|| name.to_string());
                self.open_table_editor(conn, EditorTarget::New { schema }, window, cx);
            }
            "copy" if name.is_empty() => self.pick_table_for(crate::palette::TablePurpose::Copy, window, cx),
            "copy" => {
                let found = self.store.read(cx).catalog(&conn).and_then(|c| {
                    let (s, t) = match name.split_once('.') {
                        Some((s, t)) => (Some(s), t),
                        None => (None, name),
                    };
                    c.find_table(s, t).map(|(s, t)| (s.name.clone(), t.name.clone()))
                });
                match found {
                    Some((schema, table)) => self.open_copy_dialog(conn, schema, table, window, cx),
                    None => self.notify(NoticeKind::Error, format!("No table named {name}"), window, cx),
                }
            }
            "modify" | "edit" if name.is_empty() => {
                self.pick_table_for(crate::palette::TablePurpose::Modify, window, cx)
            }
            "modify" | "edit" => {
                let found = self.store.read(cx).catalog(&conn).and_then(|c| {
                    let (s, t) = match name.split_once('.') {
                        Some((s, t)) => (Some(s), t),
                        None => (None, name),
                    };
                    c.find_table(s, t).map(|(s, t)| (s.name.clone(), t.name.clone()))
                });
                match found {
                    Some((schema, table)) => {
                        self.open_table_editor(
                            conn,
                            EditorTarget::Modify { schema, table, column: None },
                            window,
                            cx,
                        );
                    }
                    None => self.notify(NoticeKind::Error, format!("No table named {name}"), window, cx),
                }
            }
            _ => self.notify(
                NoticeKind::Info,
                "Usage: :table new [SCHEMA], :table modify [SCHEMA.]TABLE or :table copy [SCHEMA.]TABLE",
                window,
                cx,
            ),
        }
    }

    /// `--modify` / `--new-table`: open the editor as soon as the first connection and its catalog are ready.
    pub fn table_editor_at_startup(&mut self, target: StartupTable) {
        self.startup_table = Some(target);
    }

    pub(super) fn maybe_open_startup_table(
        &mut self,
        conn_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.startup_table.is_none() || self.store.read(cx).catalog(conn_id).is_none() {
            return;
        }
        let Some(target) = self.startup_table.take() else { return };
        let catalog = self.store.read(cx).catalog(conn_id);
        let target = match target {
            StartupTable::New(schema) => EditorTarget::New { schema },
            StartupTable::Modify(name) => {
                let (s, t) = match name.split_once('.') {
                    Some((s, t)) => (Some(s), t),
                    None => (None, name.as_str()),
                };
                match catalog.as_ref().and_then(|c| c.find_table(s, t)) {
                    Some((schema, table)) => EditorTarget::Modify {
                        schema: schema.name.clone(),
                        table: table.name.clone(),
                        column: None,
                    },
                    None => {
                        eprintln!("theta: --modify {name}: no such table");
                        let message = format!("--modify: there is no table named {name} in this connection.");
                        self.record_notice(MessageKind::Warning, "Start-up", &message, cx);
                        self.notify(NoticeKind::Warning, message, window, cx);
                        return;
                    }
                }
            }
        };
        self.open_table_editor(conn_id.to_string(), target, window, cx);
    }
}

/// The table editor a command-line flag asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum StartupTable {
    New(Option<String>),
    Modify(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::ConnectionStore;
    use crate::connections::secrets::SecretStore;
    use crate::table_editor::Run;
    use crate::table_editor::model::{CellValue, Field, RowId, Section};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};

    /// What a user on this platform presses for an application shortcut (`cmd` on macOS).
    fn primary(key: &str) -> String {
        format!("{}-{key}", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" })
    }
    use std::time::Duration;
    use theta_core::{ConnectionConfig, ExecOptions, Value};

    struct Fixture {
        ws: Entity<Workspace>,
        store: Entity<ConnectionStore>,
        conn: String,
        _dir: tempfile::TempDir,
    }

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

    fn setup(cx: &mut TestAppContext, read_only: bool) -> (Fixture, &mut VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.executor().allow_parking();
        cx.update(|cx| {
            theta_ui::init(cx);
            crate::keymap::bind(cx);
        });
        let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
        let store = cx.new(|_| store);
        let mut config = ConnectionConfig::new("t", "test", DatabaseKind::Sqlite);
        // A file, not `:memory:`: the catalog loader and the script runner may use different connections.
        config.path = dir.path().join("t.db").to_string_lossy().into_owned();
        config.read_only = read_only;
        let conn = store.update(cx, |s, cx| {
            let id = s.add_temporary(config, cx);
            s.connect(&id, cx);
            id
        });
        let data = tempfile::tempdir().unwrap();
        let (ws, vcx) = cx.add_window_view(|window, cx| {
            let mut ws = Workspace::with_data_dir(store.clone(), data.path().to_path_buf(), window, cx);
            ws.quiet = true;
            ws
        });
        wait_for(vcx, "the connection", |vcx| {
            store.read_with(vcx, |s, _| s.state(&conn).is_connected() && s.catalog(&conn).is_some())
        });
        let _ = Box::leak(Box::new(data));
        (Fixture { ws, store, conn, _dir: dir }, vcx)
    }

    fn sql(f: &Fixture, vcx: &mut VisualTestContext, text: &str) -> Result<Vec<Vec<Value>>, String> {
        let connection = f.store.read_with(vcx, |s, _| s.connection(&f.conn)).unwrap();
        let slot = Arc::new(Mutex::new(None));
        let (w, q) = (slot.clone(), text.to_string());
        runtime::runtime().spawn(async move {
            *w.lock().unwrap() = Some(connection.execute(&q, &ExecOptions { row_limit: None }).await);
        });
        wait_for(vcx, "the statement", |_| slot.lock().unwrap().is_some());
        let r = slot.lock().unwrap().take().unwrap();
        r.map(|o| o.result.map(|r| r.rows).unwrap_or_default()).map_err(|e| e.to_string())
    }

    /// Reload the catalog and wait for it (tables made behind the store's back).
    fn refresh(f: &Fixture, vcx: &mut VisualTestContext) {
        f.store.update(vcx, |s, cx| s.refresh_catalog(&f.conn, cx));
        for _ in 0..60 {
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn open_new(f: &Fixture, vcx: &mut VisualTestContext) -> (u64, Entity<TableEditor>) {
        let id =
            f.ws.update_in(vcx, |ws, window, cx| {
                ws.open_table_editor(f.conn.clone(), EditorTarget::New { schema: None }, window, cx)
            })
            .expect("the editor opens");
        (id, f.ws.read_with(vcx, |ws, _| ws.table_editor(id)).unwrap())
    }

    fn open_modify(f: &Fixture, vcx: &mut VisualTestContext, table: &str) -> (u64, Entity<TableEditor>) {
        let id =
            f.ws.update_in(vcx, |ws, window, cx| {
                ws.open_table_editor(
                    f.conn.clone(),
                    EditorTarget::Modify { schema: "main".into(), table: table.into(), column: None },
                    window,
                    cx,
                )
            })
            .expect("the editor opens");
        (id, f.ws.read_with(vcx, |ws, _| ws.table_editor(id)).unwrap())
    }

    fn script(editor: &Entity<TableEditor>, vcx: &mut VisualTestContext) -> Vec<String> {
        editor.read_with(vcx, |e, _| e.statements().iter().map(|s| s.sql.clone()).collect())
    }

    fn press(vcx: &mut VisualTestContext, keys: &[&str]) {
        for k in keys {
            vcx.update(|window, cx| window.press(k, cx));
            vcx.run_until_parked();
        }
    }

    fn focus(editor: &Entity<TableEditor>, vcx: &mut VisualTestContext) {
        editor.update_in(vcx, |e, window, cx| window.focus(&e.focus_handle_ref(), cx));
        vcx.run_until_parked();
    }

    /// Run what the editor previews, the way the confirmation dialog's button does.
    fn execute(f: &Fixture, vcx: &mut VisualTestContext, tab: u64, editor: &Entity<TableEditor>) {
        let stmts = editor.read_with(vcx, |e, _| e.statements().to_vec());
        f.ws.update_in(vcx, |ws, window, cx| ws.run_table_script(tab, "test".into(), stmts, window, cx));
        wait_for(vcx, "the script", |vcx| {
            f.ws.read_with(vcx, |w, _| w.runs.is_empty()) && !editor.read_with(vcx, |e, _| e.is_running())
        });
    }

    fn column_names(f: &Fixture, vcx: &mut VisualTestContext, table: &str) -> Vec<String> {
        f.store
            .read_with(vcx, |s, _| s.catalog(&f.conn))
            .and_then(|c| {
                c.find_table(None, table).map(|(_, t)| t.columns.iter().map(|c| c.name.clone()).collect())
            })
            .unwrap_or_default()
    }

    #[gpui_kit::test]
    fn a_new_table_previews_exactly_what_create_table_generates(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        let (_, editor) = open_new(&f, vcx);
        assert_eq!(editor.read_with(vcx, |e, _| e.title()), "New Table");
        assert!(script(&editor, vcx).is_empty(), "nothing to run before the first edit");
        editor.update_in(vcx, |e, window, cx| e.set_name("things", window, cx));
        // Through the entity's API.
        editor.update_in(vcx, |e, window, cx| {
            e.add_row(false, window, cx);
            // Adding a row opens its name for typing; here the name is set through the API instead.
            e.cancel_edit(window, cx);
        });
        let col = editor.read_with(vcx, |e, _| e.current_row()).unwrap();
        editor.update(vcx, |e, cx| {
            assert!(e.set_cell(col, Field::Name, CellValue::Text("label".into()), cx));
            assert!(e.set_cell(col, Field::Type, CellValue::Text("TEXT".into()), cx));
        });
        // Through simulated keys: `o` adds a row with its name open for typing, Tab moves on, Enter applies.
        focus(&editor, vcx);
        press(vcx, &["o"]);
        vcx.update(|window, cx| window.input("note", cx));
        press(vcx, &["enter"]);
        let names: Vec<String> =
            editor.read_with(vcx, |e, _| e.model().draft().columns.iter().map(|c| c.name.clone()).collect());
        assert_eq!(names, vec!["id", "label", "note"]);
        let (draft, preview) = editor.read_with(vcx, |e, _| (e.model().draft().clone(), e.script()));
        let expected = ddl::create_table(&draft, DatabaseKind::Sqlite)
            .into_iter()
            .map(|s| format!("{};", s.sql))
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(preview, expected);
        assert!(preview.starts_with("CREATE TABLE things ("), "{preview}");
        assert_eq!(editor.read_with(vcx, |e, _| e.title()), "New Table things");
        // The tab is marked as having unapplied changes.
        let modified = f.ws.read_with(vcx, |ws, _| ws.tabs.last().map(|t| (t.modified, t.title.to_string())));
        assert_eq!(modified, Some((true, "New Table things".to_string())));
    }

    #[gpui_kit::test]
    fn a_new_table_suggests_foreign_keys_before_it_exists(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "create table stores (id integer primary key, name text)").unwrap();
        refresh(&f, vcx);
        let (_, editor) = open_new(&f, vcx);
        editor.update_in(vcx, |e, window, cx| e.set_name("visits", window, cx));
        assert!(editor.read_with(vcx, |e, _| e.open_suggestions().is_empty()));
        editor.update_in(vcx, |e, window, cx| {
            e.add_row(false, window, cx);
            e.cancel_edit(window, cx);
        });
        let col = editor.read_with(vcx, |e, _| e.current_row()).unwrap();
        editor.update(vcx, |e, cx| {
            assert!(e.set_cell(col, Field::Name, CellValue::Text("store_id".into()), cx));
            assert!(e.set_cell(col, Field::Type, CellValue::Text("integer".into()), cx));
        });
        let open = editor.read_with(vcx, |e, _| e.open_suggestions());
        let got: Vec<_> = open
            .iter()
            .map(|v| (v.column.as_str(), v.referenced_table.as_str(), v.referenced_column.as_str()))
            .collect();
        assert_eq!(got, vec![("store_id", "stores", "id")], "{open:?}");
    }

    #[gpui_kit::test]
    fn execute_creates_the_table_and_the_tab_reloads_as_modify(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        let (tab, editor) = open_new(&f, vcx);
        editor.update_in(vcx, |e, window, cx| e.set_name("things", window, cx));
        editor.update_in(vcx, |e, window, cx| e.add_row(false, window, cx));
        let col = editor.read_with(vcx, |e, _| e.current_row()).unwrap();
        editor.update(vcx, |e, cx| {
            e.set_cell(col, Field::Name, CellValue::Text("label".into()), cx);
        });
        execute(&f, vcx, tab, &editor);
        wait_for(vcx, "the catalog to show the table", |vcx| !column_names(&f, vcx, "things").is_empty());
        assert_eq!(column_names(&f, vcx, "things"), vec!["id", "label"]);
        // The tab reloads from the database: a clean diff, titled Modify.
        wait_for(vcx, "the tab to reload", |vcx| !editor.read_with(vcx, |e, _| e.model().is_new()));
        assert_eq!(editor.read_with(vcx, |e, _| e.title()), "Modify things");
        assert!(!editor.read_with(vcx, |e, _| e.dirty()));
        assert!(script(&editor, vcx).is_empty());
        assert_eq!(
            f.ws.read_with(vcx, |ws, _| ws.tabs.last().map(|t| (t.modified, t.title.to_string()))),
            Some((false, "Modify things".to_string()))
        );
        // The explorer selected it.
        let selected =
            f.ws.read_with(vcx, |ws, cx| ws.explorer.read(cx).selected_key().map(|k| format!("{k:?}")));
        assert!(
            selected.as_deref().is_some_and(|k| k.starts_with("Table(") && k.contains("things")),
            "{selected:?}"
        );
    }

    #[gpui_kit::test]
    fn rename_add_and_drop_are_previewed_in_engine_order_and_applied(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, email TEXT, city TEXT)")
            .unwrap();
        sql(&f, vcx, "INSERT INTO people VALUES (1, 'ann', 'a@x', 'Oslo')").unwrap();
        refresh(&f, vcx);
        let (tab, editor) = open_modify(&f, vcx, "people");
        assert_eq!(editor.read_with(vcx, |e, _| e.title()), "Modify people");
        let (email, city) = editor.read_with(vcx, |e, _| {
            let d = e.model().draft();
            (
                d.columns.iter().find(|c| c.name == "email").unwrap().id,
                d.columns.iter().find(|c| c.name == "city").unwrap().id,
            )
        });
        editor.update(vcx, |e, cx| {
            e.set_cell(RowId::Column(email), Field::Name, CellValue::Text("mail".into()), cx);
        });
        editor.update_in(vcx, |e, window, cx| e.add_row(false, window, cx));
        let new = editor.read_with(vcx, |e, _| e.current_row()).unwrap();
        editor.update(vcx, |e, cx| {
            e.set_cell(new, Field::Name, CellValue::Text("phone".into()), cx);
            e.model.delete_row(RowId::Column(city)).unwrap();
        });
        assert_eq!(
            script(&editor, vcx),
            vec![
                "ALTER TABLE people DROP COLUMN city",
                "ALTER TABLE people RENAME COLUMN email TO mail",
                "ALTER TABLE people ADD COLUMN phone TEXT",
            ]
        );
        let flags: Vec<bool> =
            editor.read_with(vcx, |e, _| e.statements().iter().map(|s| s.is_destructive()).collect());
        assert_eq!(flags, vec![true, false, false]);
        assert_eq!(
            editor.read_with(vcx, |e, _| e.model().analysis().summary.clone()),
            "3 statements · 1 destructive"
        );
        execute(&f, vcx, tab, &editor);
        wait_for(vcx, "the reload", |vcx| !editor.read_with(vcx, |e, _| e.dirty()));
        assert_eq!(column_names(&f, vcx, "people"), vec!["id", "name", "mail", "phone"]);
        let rows = sql(&f, vcx, "SELECT name, mail FROM people").unwrap();
        assert_eq!(rows[0][1], Value::Text("a@x".into()), "a rename keeps the data");
        assert!(script(&editor, vcx).is_empty(), "re-introspection matches: nothing left to do");
    }

    #[gpui_kit::test]
    fn a_failing_change_keeps_the_edits_marks_the_statement_and_rolls_back(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)").unwrap();
        sql(&f, vcx, "INSERT INTO t VALUES (1, 'a'), (2, NULL)").unwrap();
        refresh(&f, vcx);
        let fk_before = sql(&f, vcx, "PRAGMA foreign_keys").unwrap();
        let (tab, editor) = open_modify(&f, vcx, "t");
        let v = editor
            .read_with(vcx, |e, _| e.model().draft().columns.iter().find(|c| c.name == "v").unwrap().id);
        editor.update(vcx, |e, cx| {
            // NOT NULL over a NULL: SQLite rebuilds the table, and the copy hits the NULL.
            e.set_cell(RowId::Column(v), Field::Nullable, CellValue::Flip, cx);
        });
        let before = script(&editor, vcx);
        assert!(before.iter().any(|s| s.starts_with("INSERT INTO t__new")), "{before:?}");
        execute(&f, vcx, tab, &editor);
        let run = editor.read_with(vcx, |e, _| e.run_state().clone());
        let Run::Failed { statement, message, note } = run else { panic!("expected a failure, got {run:?}") };
        let at = statement.expect("the failing statement is identified");
        assert!(before[at].starts_with("INSERT INTO t__new"), "{}", before[at]);
        assert!(message.to_lowercase().contains("not null"), "{message}");
        assert!(note.contains("rolled back"), "{note}");
        // The edit is still there, and so is every row.
        assert!(editor.read_with(vcx, |e, _| e.dirty()));
        assert_eq!(script(&editor, vcx), before);
        assert_eq!(sql(&f, vcx, "SELECT count(*) FROM t").unwrap()[0][0], Value::Int(2));
        assert!(sql(&f, vcx, "SELECT * FROM t__new").is_err(), "the half-built table is gone");
        // Foreign keys are enforced again afterwards: the pragma step runs even after a failure.
        assert_eq!(sql(&f, vcx, "PRAGMA foreign_keys").unwrap(), fk_before);
        // Editing makes the failure stale.
        editor.update(vcx, |e, cx| {
            e.set_cell(RowId::Column(v), Field::Nullable, CellValue::Flip, cx);
        });
        assert_eq!(editor.read_with(vcx, |e, _| e.run_state().clone()), Run::Idle);
    }

    #[gpui_kit::test]
    fn a_read_only_connection_refuses_to_execute(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, true);
        let (tab, editor) = open_new(&f, vcx);
        editor.update_in(vcx, |e, window, cx| e.set_name("nope", window, cx));
        // Read-only: even the add-row key says no, and Execute reports why.
        editor.update_in(vcx, |e, window, cx| e.add_row(false, window, cx));
        assert_eq!(editor.read_with(vcx, |e, _| e.model().draft().columns.len()), 1);
        let problem = editor.read_with(vcx, |e, _| e.execute_problem());
        assert!(problem.as_deref().is_some_and(|p| p.contains("read-only")), "{problem:?}");
        // Even a script handed straight to the runner is refused.
        let stmts = vec![theta_sql::ddl::Statement {
            sql: "CREATE TABLE nope (a int)".into(),
            flags: vec![],
            explanation: String::new(),
        }];
        f.ws.update_in(vcx, |ws, window, cx| ws.run_table_script(tab, "t".into(), stmts, window, cx));
        vcx.run_until_parked();
        refresh(&f, vcx);
        assert!(column_names(&f, vcx, "nope").is_empty());
    }

    #[gpui_kit::test]
    fn problems_block_execute_and_a_fix_button_resolves_a_warning(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        let (_, editor) = open_new(&f, vcx);
        // No table name yet: an error, so Execute is not available.
        editor.update(vcx, |e, cx| {
            e.set_cell(RowId::Column(1), Field::Name, CellValue::Text("key".into()), cx);
        });
        let problem = editor.read_with(vcx, |e, _| e.execute_problem());
        assert!(problem.as_deref().is_some_and(|p| p.contains("problem")), "{problem:?}");
        editor.update(vcx, |e, cx| {
            e.request_execute(cx);
        });
        assert!(
            f.ws.read_with(vcx, |ws, _| ws.execute_asked.is_empty()),
            "no dialog for an unrunnable script"
        );
        // A warning with a fix: nullable key column.
        editor.update_in(vcx, |e, window, cx| e.set_name("keys", window, cx));
        editor.update(vcx, |e, cx| {
            e.set_cell(RowId::Column(1), Field::Nullable, CellValue::Flip, cx);
        });
        let fix =
            editor.read_with(vcx, |e, _| e.model().analysis().issues.iter().find_map(|i| i.fix.clone()));
        let fix = fix.expect("a NULLable key column offers a fix");
        editor.update(vcx, |e, cx| e.apply_fix(&fix, cx));
        assert!(!editor.read_with(vcx, |e, _| e.model().draft().columns[0].nullable));
        // Now it is runnable, and ctrl-enter on the tab asks for confirmation.
        editor.update_in(vcx, |e, window, cx| window.focus(&e.focus_handle_ref(), cx));
        press(vcx, &["escape", &primary("enter")]);
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.execute_asked.len()), 1);
    }

    #[gpui_kit::test]
    fn closing_a_tab_with_unapplied_changes_asks_first(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        let (tab, editor) = open_new(&f, vcx);
        // Untouched: closes at once.
        f.ws.update_in(vcx, |ws, window, cx| ws.request_close_tab(tab, window, cx));
        assert!(f.ws.read_with(vcx, |ws, _| ws.table_editor(tab).is_none()));
        let (tab, editor2) = open_new(&f, vcx);
        let _ = editor;
        editor2.update_in(vcx, |e, window, cx| e.set_name("draft", window, cx));
        assert!(f.ws.read_with(vcx, |ws, cx| ws.table_tab_dirty(tab, cx)));
        f.ws.update_in(vcx, |ws, window, cx| ws.request_close_tab(tab, window, cx));
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.discard_asked.clone()), vec![tab]);
        assert!(
            f.ws.read_with(vcx, |ws, _| ws.table_editor(tab).is_some()),
            "still open until the user decides"
        );
        // Ctrl-W goes through the same door.
        f.ws.update_in(vcx, |ws, window, cx| ws.activate_tab(tab, window, cx));
        press(vcx, &[&primary("w")]);
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.discard_asked.len()), 2);
        // Discard closes it.
        f.ws.update_in(vcx, |ws, window, cx| ws.close_tab_now(tab, window, cx));
        assert!(f.ws.read_with(vcx, |ws, _| ws.table_editor(tab).is_none()));
        assert!(f.ws.read_with(vcx, |ws, cx| ws.dirty_table_tabs(cx).is_empty()));
    }

    #[gpui_kit::test]
    fn the_same_table_opens_in_one_tab_and_the_state_survives_switching_tabs(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT)").unwrap();
        refresh(&f, vcx);
        let (tab, editor) = open_modify(&f, vcx, "a");
        editor.update(vcx, |e, cx| {
            e.set_cell(RowId::Column(2), Field::Name, CellValue::Text("y".into()), cx);
        });
        f.ws.update_in(vcx, |ws, window, cx| {
            ws.open_console(None, None, window, cx);
        });
        let (again, _) = open_modify(&f, vcx, "a");
        assert_eq!(again, tab, "no second editor for the same table");
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.active_tab), Some(tab));
        assert_eq!(script(&editor, vcx), vec!["ALTER TABLE a RENAME COLUMN x TO y"]);
    }

    #[gpui_kit::test]
    fn the_keyboard_alone_edits_a_row_and_undo_walks_it_back(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE k (id INTEGER PRIMARY KEY, a TEXT, b TEXT)").unwrap();
        refresh(&f, vcx);
        let (_, editor) = open_modify(&f, vcx, "k");
        focus(&editor, vcx);
        // Helix is the default keymap: j moves down, Enter edits, Space toggles, dd deletes, u undoes.
        press(vcx, &["j", "enter"]);
        assert!(editor.read_with(vcx, |e, _| e.is_editing()));
        press(vcx, &["x", "enter"]);
        assert_eq!(editor.read_with(vcx, |e, _| e.model().draft().columns[1].name.clone()), "x");
        press(vcx, &["l", "l", "space"]);
        assert!(!editor.read_with(vcx, |e, _| e.model().draft().columns[1].nullable));
        press(vcx, &["j", "d", "d"]);
        assert_eq!(editor.read_with(vcx, |e, _| e.model().draft().columns.len()), 2);
        // The struck-through row is still on screen and can be restored with dd again.
        assert_eq!(editor.read_with(vcx, |e, _| e.model().spec(Section::Columns).rows.len()), 3);
        press(vcx, &["u"]);
        assert_eq!(editor.read_with(vcx, |e, _| e.model().draft().columns.len()), 3);
        press(vcx, &["u", "u"]);
        assert!(!editor.read_with(vcx, |e, _| e.dirty()));
        press(vcx, &[&primary("r")]);
        assert_eq!(
            editor.read_with(vcx, |e, _| e.model().draft().columns[1].name.clone()),
            "x",
            "ctrl-r redoes"
        );
    }

    #[gpui_kit::test]
    fn explorer_and_palette_entry_points_open_the_editor(cx: &mut TestAppContext) {
        use crate::explorer::ExplorerEvent;
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, city TEXT)").unwrap();
        refresh(&f, vcx);
        let explorer = f.ws.read_with(vcx, |ws, _| ws.explorer.clone());
        // "Modify Column…" on a column opens the table with the cursor on it.
        explorer.update(vcx, |_, cx| {
            cx.emit(ExplorerEvent::ModifyTable {
                connection_id: f.conn.clone(),
                schema: "main".into(),
                table: "people".into(),
                column: Some("city".into()),
            })
        });
        vcx.run_until_parked();
        let (tab, editor) = {
            let tab = f.ws.read_with(vcx, |ws, _| ws.active_tab).unwrap();
            (tab, f.ws.read_with(vcx, |ws, _| ws.table_editor(tab)).expect("a table tab"))
        };
        assert_eq!(editor.read_with(vcx, |e, _| (e.section(), e.cursor().0)), (Section::Columns, 2));
        // "New Table…" from the Tables group.
        explorer.update(vcx, |_, cx| {
            cx.emit(ExplorerEvent::NewTable { connection_id: f.conn.clone(), schema: "main".into() })
        });
        vcx.run_until_parked();
        let new_tab = f.ws.read_with(vcx, |ws, _| ws.active_tab).unwrap();
        assert_ne!(new_tab, tab);
        assert!(f.ws.read_with(vcx, |ws, cx| ws.table_editor(new_tab).unwrap().read(cx).model().is_new()));
        // `:table modify NAME` finds the open tab instead of opening a second one.
        f.ws.update_in(vcx, |ws, window, cx| ws.table_host_command("modify people", window, cx));
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.active_tab), Some(tab));
        // `:table new` and the palette action open a fresh editor.
        let before = f.ws.read_with(vcx, |ws, _| ws.tabs.len());
        f.ws.update_in(vcx, |ws, window, cx| ws.table_host_command("new", window, cx));
        vcx.update(|window, cx| window.dispatch_action(Box::new(crate::actions::NewTable), cx));
        vcx.run_until_parked();
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.tabs.len()), before + 2);
        // A name that is not a table says so and opens nothing.
        f.ws.update_in(vcx, |ws, window, cx| ws.table_host_command("modify nothing_here", window, cx));
        assert_eq!(f.ws.read_with(vcx, |ws, _| ws.tabs.len()), before + 2);
    }

    #[gpui_kit::test]
    fn dropping_a_column_index_or_key_is_scripted_from_the_same_model(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT, y TEXT)").unwrap();
        sql(&f, vcx, "CREATE INDEX a_x_idx ON a (x)").unwrap();
        refresh(&f, vcx);
        let target = f.ws.read_with(vcx, |ws, cx| ws.ddl_target(&f.conn, "main", "a", cx)).unwrap();
        // SQLite cannot drop an indexed column in place: the whole consequence (a rebuild without the index,
        // and a destructive DROP TABLE of the old one) is in the script the confirmation shows.
        let stmts = f.ws.read_with(vcx, |ws, _| {
            ws.script_for(&target, |m| {
                let id = m.draft().columns.iter().find(|c| c.name == "x").unwrap().id;
                m.delete_row(RowId::Column(id)).unwrap();
            })
        });
        let sql_text: Vec<&str> = stmts.iter().map(|s| s.sql.as_str()).collect();
        assert!(sql_text.contains(&"DROP TABLE a"), "{sql_text:?}");
        assert!(
            !sql_text.iter().any(|s| s.contains("CREATE INDEX")),
            "the index goes with the column: {sql_text:?}"
        );
        assert!(stmts.iter().any(|s| s.is_destructive()));
        // A rename is one RENAME.
        let stmts = f.ws.read_with(vcx, |ws, _| {
            ws.script_for(&target, |m| {
                let id = m.draft().columns.iter().find(|c| c.name == "y").unwrap().id;
                m.set_cell(RowId::Column(id), Field::Name, CellValue::Text("z".into())).unwrap();
            })
        });
        assert_eq!(
            stmts.iter().map(|s| s.sql.as_str()).collect::<Vec<_>>(),
            vec!["ALTER TABLE a RENAME COLUMN y TO z"]
        );
        // Dropping an index alone.
        let stmts = f.ws.read_with(vcx, |ws, _| {
            ws.script_for(&target, |m| {
                let id = m.draft().indexes.iter().find(|i| i.name == "a_x_idx").unwrap().id;
                m.delete_row(RowId::Index(id)).unwrap();
            })
        });
        assert_eq!(stmts.iter().map(|s| s.sql.as_str()).collect::<Vec<_>>(), vec!["DROP INDEX a_x_idx"]);
    }

    #[gpui_kit::test]
    fn sqlite_triggers_survive_a_rebuild_because_the_editor_reads_and_replays_them(cx: &mut TestAppContext) {
        let (f, vcx) = setup(cx, false);
        sql(&f, vcx, "CREATE TABLE log (msg TEXT)").unwrap();
        sql(&f, vcx, "CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT)").unwrap();
        sql(&f, vcx, "CREATE TRIGGER a_log AFTER INSERT ON a BEGIN INSERT INTO log VALUES (new.x); END")
            .unwrap();
        refresh(&f, vcx);
        let (tab, editor) = open_modify(&f, vcx, "a");
        wait_for(vcx, "the triggers", |vcx| {
            !editor.read_with(vcx, |e, _| e.model().draft().recreate.is_empty())
        });
        editor.update(vcx, |e, cx| {
            e.set_cell(RowId::Column(2), Field::Type, CellValue::Text("VARCHAR(20)".into()), cx);
        });
        assert!(script(&editor, vcx).iter().any(|s| s.starts_with("CREATE TRIGGER a_log")));
        execute(&f, vcx, tab, &editor);
        wait_for(vcx, "the reload", |vcx| !editor.read_with(vcx, |e, _| e.dirty()));
        sql(&f, vcx, "INSERT INTO a (x) VALUES ('hi')").unwrap();
        assert_eq!(
            sql(&f, vcx, "SELECT count(*) FROM log").unwrap()[0][0],
            Value::Int(1),
            "the trigger still fires"
        );
    }
}
