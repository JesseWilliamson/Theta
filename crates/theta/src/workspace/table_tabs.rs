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

    /// Open the editor for a new table or an existing one. `None` (after telling the user why) when the
    /// connection is not ready or the table cannot be edited.
    pub fn open_table_editor(
        &mut self,
        conn_id: String,
        target: EditorTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
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
        });
        let sub = cx.subscribe_in(&editor, window, move |this, _, ev: &TableEditorEvent, window, cx| {
            this.on_table_event(id, ev.clone(), window, cx)
        });
        self.table_subs.insert(id, sub);
        self.console_views.insert(id, editor.clone().into());
        self.active_connection = Some(conn_id);
        self.activate_tab(id, window, cx);
        if let EditorTarget::Modify { column: Some(col), .. } = &target {
            editor.update(cx, |e, cx| e.focus_column(col, window, cx));
        }
        if new {
            editor.update(cx, |e, cx| e.focus_name(window, cx));
        }
        self.save_all_consoles(cx);
        Some(id)
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
            TableEditorEvent::Notice { error, text } => {
                self.notify(if error { NoticeKind::Error } else { NoticeKind::Info }, text, window, cx)
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
        editor.update(cx, |e, cx| e.set_running(cx));
        let shown: Vec<String> = stmts.iter().map(|s| s.sql.clone()).collect();
        let own = script.own_transaction;
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
                    None if failure.is_none() && step.statement.is_some() => completed += 1,
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
                        let (schema, name) = editor.update(cx, |e, cx| {
                            e.applied(in_session, cx);
                            (e.model().draft().schema.clone(), e.model().draft().name.clone())
                        });
                        this.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
                        let schema = schema
                            .or_else(|| {
                                this.store.read(cx).catalog(&conn).and_then(|c| c.default_schema.clone())
                            })
                            .unwrap_or_default();
                        this.explorer.update(cx, |ex, cx| ex.reveal_table(&conn, &schema, &name, cx));
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
                        this.notify(NoticeKind::Error, format!("{title} failed: {message}"), window, cx);
                        editor.update(cx, |e, cx| e.set_failed(statement, message, note, cx));
                        if completed > 0 && !rolled_back {
                            // Something did change (MySQL commits each statement): show it in the explorer.
                            this.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
                        }
                    }
                    Err(e) => {
                        editor.update(cx, |e2, cx| e2.set_failed(None, e.to_string(), String::new(), cx));
                        this.notify(NoticeKind::Error, format!("{title} failed: {e}"), window, cx);
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
    if kind == DatabaseKind::MySql && completed > 0 {
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
                "Usage: :table new [SCHEMA]  or  :table modify [SCHEMA.]TABLE",
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
