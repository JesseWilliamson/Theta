//! Bulk data movement from the workspace's side: opening the import and export dialogs, keeping other
//! statements off the connection while one runs, and reporting what happened.
//!
//! The dialogs own the work (they hold the progress and the cancel flag); the workspace only knows three
//! things about it: which connections are busy with it (so a console cannot interleave statements into an
//! import's transaction), how to log the result, and what the session's transaction state is.

use super::Workspace;
use super::run::RunInfo;
use crate::connections::NoticeKind;
use crate::dialogs::{export, import};
use crate::panels::messages::MessageKind;
use gpui_kit::{Context, Window};
use std::time::Instant;
use theta_core::txn::TransactionConfig;
use theta_import::TxnPlan;

/// Marks a connection as busy with a bulk operation; no console owns it.
const BULK_CONSOLE: u64 = u64::MAX;

impl Workspace {
    /// Reserve `conn_id` for a bulk operation. `false` (after telling the user) when a query is running.
    pub fn begin_bulk(&mut self, conn_id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.runs.contains_key(conn_id) {
            self.notify(
                NoticeKind::Info,
                "A query is running on this connection. Wait for it or cancel it first.",
                window,
                cx,
            );
            return false;
        }
        self.runs.insert(
            conn_id.to_string(),
            RunInfo { console: BULK_CONSOLE, started: Instant::now(), cancelled: false, failed: false },
        );
        self.sync_running(cx);
        true
    }

    pub fn end_bulk(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        if self.runs.get(conn_id).is_some_and(|r| r.console == BULK_CONSOLE) {
            self.runs.remove(conn_id);
        }
        self.sync_running(cx);
        if let Some(connection) = self.store.read(cx).connection(conn_id) {
            // The import may have left a transaction open (manual mode); the title bar must say so.
            self.refresh_txn(conn_id.to_string(), connection, cx);
        }
    }

    /// How an import should relate to the session's transaction, from the mode and whether one is open.
    pub fn bulk_txn_plan(&self, conn_id: &str, cx: &gpui_kit::App) -> Option<TxnPlan> {
        let config = self.store.read(cx).config(conn_id)?;
        let open = self.txn.contains(conn_id);
        let manual = self.manual_begin(conn_id, cx);
        Some(match (open, manual) {
            (true, m) => {
                let begin = m.map(|m| m.statements).unwrap_or_default();
                TxnPlan::Session { open: true, begin }
            }
            (false, Some(m)) => TxnPlan::Session { open: false, begin: m.statements },
            (false, None) => {
                TxnPlan::Own { begin: TransactionConfig::default().begin_statements(config.kind, false) }
            }
        })
    }

    pub fn transaction_open(&self, conn_id: &str) -> bool {
        self.txn.contains(conn_id)
    }

    pub fn log_bulk(
        &mut self,
        conn_id: &str,
        title: &str,
        kind: MessageKind,
        lines: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.log(conn_id, title, kind, lines, None, cx);
    }

    pub fn open_import_dialog(
        &mut self,
        request: import::Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let conn = request.conn_id.clone();
        if !self.store.read(cx).state(&conn).is_connected() {
            self.notify(NoticeKind::Info, "Connect first, then import", window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        import::open(ws, self.store.clone(), request, window, cx);
    }

    pub fn open_export_dialog(
        &mut self,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.store.read(cx).state(&conn_id).is_connected() {
            self.notify(NoticeKind::Info, "Connect first, then export", window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        export::open(ws, self.store.clone(), conn_id, schema, table, window, cx);
    }

    /// The connection an import from the palette or a Tables group should use.
    pub fn bulk_connection(&self, cx: &gpui_kit::App) -> Option<String> {
        let id = self.active_connection_id(cx)?;
        self.store.read(cx).state(&id).is_connected().then_some(id)
    }

    /// Palette: open the import dialog on the active connection, as a new table or into a chosen one.
    pub(super) fn import_from_palette(
        &mut self,
        new_table: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn_id) = self.bulk_connection(cx) else {
            self.notify(NoticeKind::Info, "Connect to a database first", window, cx);
            return;
        };
        self.open_import_dialog(import::Request { conn_id, table: None, path: None, new_table }, window, cx);
    }

    pub(super) fn pick_table_for(
        &mut self,
        purpose: crate::palette::TablePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.bulk_connection(cx) else {
            self.notify(NoticeKind::Info, "Connect to a database first", window, cx);
            return;
        };
        let delegate = crate::palette::TablePicker::new(self.store.clone(), cx.entity().downgrade(), id)
            .for_purpose(purpose);
        self.open_picker(delegate, window, cx);
    }

    /// Vim / Helix `:import [FILE]` and `:export [SCHEMA.]TABLE`.
    pub(super) fn bulk_host_command(
        &mut self,
        cmd: &str,
        arg: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn_id) = self.bulk_connection(cx) else {
            self.notify(NoticeKind::Info, "Connect to a database first", window, cx);
            return;
        };
        if cmd == "import" {
            let path = (!arg.is_empty()).then(|| arg.to_string());
            self.open_import_dialog(
                import::Request { conn_id, table: None, path, new_table: true },
                window,
                cx,
            );
            return;
        }
        // :export needs a table: the name given, or a picker.
        if arg.is_empty() {
            self.pick_table_for(crate::palette::TablePurpose::Export, window, cx);
            return;
        }
        let found = self.store.read(cx).catalog(&conn_id).and_then(|c| {
            let (s, t) = match arg.split_once('.') {
                Some((s, t)) => (Some(s), t),
                None => (None, arg),
            };
            c.find_table(s, t).map(|(s, t)| (s.name.clone(), t.name.clone()))
        });
        match found {
            Some((s, t)) => self.open_export_dialog(conn_id, s, t, window, cx),
            None => self.notify(NoticeKind::Error, format!("No table named {arg}"), window, cx),
        }
    }

    /// `--import FILE [--into SCHEMA.TABLE]`: open the dialog as soon as the first connection is ready.
    pub fn import_at_startup(&mut self, file: String, into: Option<String>) {
        self.startup_import = Some((file, into));
    }

    pub(super) fn maybe_open_startup_import(
        &mut self,
        conn_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.startup_import.is_none() || !self.store.read(cx).state(conn_id).is_connected() {
            return;
        }
        if self.store.read(cx).catalog(conn_id).is_none() {
            // The mapping needs the catalog; wait for the next event.
            return;
        }
        let Some((file, into)) = self.startup_import.take() else { return };
        let table = into.map(|t| match t.split_once('.') {
            Some((s, t)) => (s.to_string(), t.to_string()),
            None => (
                self.store
                    .read(cx)
                    .catalog(conn_id)
                    .and_then(|c| c.default_schema.clone())
                    .unwrap_or_default(),
                t,
            ),
        });
        self.open_import_dialog(
            import::Request {
                conn_id: conn_id.to_string(),
                new_table: table.is_none(),
                table,
                path: Some(file),
            },
            window,
            cx,
        );
    }
}
