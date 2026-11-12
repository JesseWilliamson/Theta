//! Consoles and everything that happens when you run SQL: statement
//! execution, results, messages, history, transactions, editing results,
//! export, map, explain and formatting.

use super::console::{CatalogSlot, Console, PlanState, ResultItem, SqlCompletion, fmt_ms, plural, verb};
use super::{BottomTab, ConsoleTab, Workspace, WorkspaceEvent};
use crate::connections::{ConnState, NoticeKind, StoreEvent};
use crate::dialogs::confirm::{self, Choice, Tone};
use crate::exec::history::{HistoryEntry, now_ms};
use crate::exec::local_history::{Kind as LocalKind, LocalHistory, Recorded};
use crate::exec::pick::{self, Picked};
use crate::exec::{changes, consoles, editable, geo};
use crate::panels::history::HistoryEvent;
use crate::panels::messages::{Message, MessageKind, clock_now};
use crate::runtime;
use gpui_kit::{App, AppContext as _, ClipboardItem, Context, Entity, Window};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use theta_core::{Connection, DatabaseKind, Error, ExecOptions, StatementOutcome};
use theta_editor::{Effect, Selection};
use theta_editor_view::{EditorEvent, StatementStatus};
use theta_grid::{GridEvent, ResultGrid};
use theta_map_view::MapEvent;
use theta_plan_view::{PlanEvent, PlanInput};
use theta_sql::{ExportFormat, FormatOptions, StatementClass};
use theta_ui::Settings;

/// Text, cursor byte offset and the selection (start byte, text) of a console.
type Snapshot = (String, usize, Option<(usize, String)>);

pub enum RunReq {
    Statement,
    Selection(String),
    All,
}

pub struct RunInfo {
    /// The console that started it; closing that tab cancels the run (nobody could otherwise).
    pub console: u64,
    pub started: Instant,
    pub cancelled: bool,
    pub failed: bool,
}

#[derive(Clone)]
pub struct ExplainTarget {
    pub console: u64,
    pub connection_id: String,
    pub sql: String,
}

/// SQL to run once a start-up connection is ready (`--run`, `--script`).
pub struct Startup {
    pub sql: String,
}

impl Workspace {
    // ------------------------------------------------------------ consoles

    #[allow(clippy::too_many_arguments)]
    pub(super) fn create_console(
        &mut self,
        key: String,
        title: String,
        connection_id: Option<String>,
        text: String,
        cursor: usize,
        file: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let editor = cx.new(|cx| theta_editor_view::EditorView::new(&text, window, cx));
        let sub = cx.subscribe_in(&editor, window, move |this, _, ev: &EditorEvent, window, cx| {
            this.on_editor_event(id, ev.clone(), window, cx)
        });
        // Grid events carry no console id, so each grid is subscribed with its console's: a
        // submit or an export always acts on the console the grid belongs to.
        let grid = cx.new(|cx| ResultGrid::new(window, cx));
        let grid_sub = cx.subscribe_in(&grid, window, move |this, _, ev: &GridEvent, window, cx| {
            this.on_grid_event(id, ev.clone(), window, cx)
        });
        if cursor > 0 {
            editor.update(cx, |e, cx| {
                e.update_editor(cx, |ed| {
                    let c = cursor.min(ed.len_chars());
                    ed.set_selections(vec![Selection::point(c)], 0)
                })
            });
        }
        self.tabs.push(ConsoleTab {
            id,
            title: title.into(),
            connection_id: connection_id.clone(),
            modified: false,
            initial_sql: None,
            table: None,
            admin: None,
            tool: None,
        });
        if let Some(c) = &connection_id {
            self.active_connection = Some(c.clone());
        }
        self.console_views.insert(id, editor.clone().into());
        self.consoles.insert(
            id,
            Console {
                key,
                editor,
                grid,
                plan: PlanState::Empty,
                last_explain: None,
                explain_gen: 0,
                file,
                results: vec![],
                active_result: 0,
                summary: None,
                catalog: CatalogSlot::default(),
                save_task: None,
                param_values: Default::default(),
                _subs: vec![sub, grid_sub],
            },
        );
        self.configure_console(id, cx);
        cx.emit(WorkspaceEvent::ConsoleOpened { tab_id: id, connection_id, initial_sql: None });
        id
    }

    /// Point a console's completion/diagnostics at its connection's kind and catalog.
    pub(super) fn configure_console(&mut self, id: u64, cx: &mut Context<Self>) {
        let conn = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone());
        let (kind, catalog) = {
            let store = self.store.read(cx);
            match conn.as_deref().and_then(|c| store.config(c).map(|cfg| (cfg.kind, store.catalog(c)))) {
                Some((k, c)) => (k, c),
                None => (DatabaseKind::Postgres, None),
            }
        };
        let Some(console) = self.consoles.get(&id) else { return };
        if let Ok(mut slot) = console.catalog.write() {
            *slot = catalog;
        }
        let slot = console.catalog.clone();
        let slot2 = slot.clone();
        let slot3 = slot.clone();
        console.editor.update(cx, |e, _| {
            e.set_database_kind(kind);
            e.set_completion_provider(Some(Rc::new(SqlCompletion {
                kind,
                catalog: slot,
                templates: crate::templates::TemplateStore::global(),
            })));
            e.set_diagnostics_provider(Some(Arc::new(move |text: &str| {
                let c = slot2.read().ok().and_then(|c| c.clone());
                theta_sql::lint(text, kind, c.as_deref())
            })));
            e.set_occurrence_provider(Some(Arc::new(move |text: &str, byte: usize| {
                let c = slot3.read().ok().and_then(|c| c.clone());
                crate::exec::intel::highlights(text, byte, kind, c.as_deref())
            })));
        });
    }

    /// Re-run [`Workspace::configure_console`] for a console whose catalog slot was written directly (tests).
    #[cfg(test)]
    pub(super) fn configure_console_catalog_only(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(console) = self.consoles.get(&id) else { return };
        let slot = console.catalog.clone();
        let kind = self.console_kind(id, cx);
        let slot2 = slot.clone();
        let slot3 = slot.clone();
        console.editor.update(cx, |e, _| {
            e.set_database_kind(kind);
            e.set_diagnostics_provider(Some(Arc::new(move |text: &str| {
                let c = slot2.read().ok().and_then(|c| c.clone());
                theta_sql::lint(text, kind, c.as_deref())
            })));
            e.set_occurrence_provider(Some(Arc::new(move |text: &str, byte: usize| {
                let c = slot3.read().ok().and_then(|c| c.clone());
                crate::exec::intel::highlights(text, byte, kind, c.as_deref())
            })));
            let _ = slot;
        });
    }

    pub(super) fn on_catalog_changed(&mut self, conn: &str, cx: &mut Context<Self>) {
        let ids: Vec<u64> =
            self.tabs.iter().filter(|t| t.connection_id.as_deref() == Some(conn)).map(|t| t.id).collect();
        for id in ids {
            self.configure_console(id, cx);
        }
    }

    /// Rebind the active console to another connection.
    pub fn rebind_active_console(&mut self, conn: &str, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab else { return };
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id)
            && t.connection_id.as_deref() != Some(conn)
        {
            t.connection_id = Some(conn.to_string());
            self.configure_console(id, cx);
            self.schedule_save(id, cx);
        }
    }

    pub(super) fn restore_consoles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = consoles::load_all(&self.data_dir.join("consoles"));
        let mut active = None;
        for s in saved {
            let conn = s.connection_id.filter(|c| self.store.read(cx).config(c).is_some());
            self.console_counter += 1;
            let id = self.create_console(s.key, s.title, conn, s.text, s.cursor, s.file, window, cx);
            if s.active {
                active = Some(id);
            }
        }
        if let Some(id) = active.or_else(|| self.tabs.last().map(|t| t.id)) {
            self.activate_tab(id, window, cx);
        }
    }

    fn console_text(&self, id: u64, cx: &App) -> Option<String> {
        self.consoles.get(&id).map(|c| c.editor.read(cx).text())
    }

    pub(super) fn save_all_consoles(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        for id in ids {
            self.save_console_now(id, cx);
        }
    }

    pub(super) fn schedule_save(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(c) = self.consoles.get_mut(&id) else { return };
        c.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            this.update(cx, |this, cx| this.save_console_now(id, cx)).ok();
        }));
    }

    /// Everything needed to bring console `id` back: text, connection, cursor, file.
    pub(super) fn snapshot_console(&self, id: u64, cx: &App) -> Option<consoles::SavedConsole> {
        let order = self.tabs.iter().position(|t| t.id == id)?;
        let tab = &self.tabs[order];
        let c = self.consoles.get(&id)?;
        let ed = c.editor.read(cx);
        Some(consoles::SavedConsole {
            key: c.key.clone(),
            order,
            title: tab.title.to_string(),
            connection_id: tab.connection_id.clone(),
            text: ed.text(),
            cursor: ed.editor().cursor_of(&ed.editor().primary()),
            active: self.active_tab == Some(id),
            file: c.file.clone(),
        })
    }

    fn save_console_now(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(saved) = self.snapshot_console(id, cx) {
            let _ = consoles::save(&self.data_dir.join("consoles"), &saved);
            self.record_local_history(&saved.key, &saved.text, LocalKind::Edit);
        }
    }

    /// Local history lives next to the console files: `<config>/local-history/<console key>/`.
    pub(super) fn local_history(&self) -> LocalHistory {
        LocalHistory::new(self.data_dir.join("local-history")).session_from(self.started_ms)
    }

    /// Snapshot `text` for a console. A failure to write must never get in the way of editing or running, so
    /// it is only logged.
    pub(super) fn record_local_history(&self, key: &str, text: &str, kind: LocalKind) {
        match self.local_history().record(key, text, kind, now_ms()) {
            Ok(Recorded::TooLarge) => {
                eprintln!("theta: local history: a console text over 1 MiB is not snapshotted")
            }
            Ok(_) => {}
            Err(e) => eprintln!("theta: local history: could not write a snapshot: {e}"),
        }
    }

    /// Show the Local History of the active console.
    pub(super) fn open_local_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab.filter(|id| self.consoles.contains_key(id)) else {
            self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx);
            return;
        };
        if self.quiet {
            return;
        }
        let Some(c) = self.consoles.get(&id) else { return };
        let (key, text) = (c.key.clone(), c.editor.read(cx).text());
        // What has been typed since the last pause is not on disk yet; it belongs in the list.
        self.record_local_history(&key, &text, LocalKind::Edit);
        crate::dialogs::local_history::open(
            cx.entity().downgrade(),
            id,
            key,
            text,
            self.local_history(),
            window,
            cx,
        );
    }

    /// Replace the console's text with a snapshot, as one undoable edit. The text that is replaced is
    /// snapshotted first, so the restore can be taken back from history as well as with undo.
    pub fn restore_snapshot(&mut self, id: u64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.consoles.get(&id) else { return };
        let (key, editor) = (c.key.clone(), c.editor.clone());
        let current = editor.read(cx).text();
        self.record_local_history(&key, &current, LocalKind::Restore);
        editor.update(cx, |e, cx| {
            let len = e.text().len();
            e.replace_byte_ranges(vec![(0..len, text)], cx);
        });
        self.schedule_save(id, cx);
        self.focus_editor(window, cx);
    }

    /// Bring back the most recently closed console (text, connection, cursor, file).
    pub(super) fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = consoles::pop_closed(&consoles::closed_dir(&self.data_dir)) else {
            self.notify(NoticeKind::Info, "No closed tab to reopen", window, cx);
            return;
        };
        let conn = s.connection_id.filter(|c| self.store.read(cx).config(c).is_some());
        // The key is free again: the old console's file was removed when it closed.
        let id = self.create_console(s.key, s.title, conn, s.text, s.cursor, s.file, window, cx);
        self.activate_tab(id, window, cx);
        self.save_all_consoles(cx);
    }

    pub(super) fn on_console_activated(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_console(id, window, cx);
        if self.plan_owner != Some(id) {
            self.show_plan_state(id, window, cx);
        }
        self.refresh_editor_status(id, cx);
        self.refresh_problems(cx);
        self.sync_running(cx);
        if let Some(c) = self.consoles.get(&id) {
            let handle = c.editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        } else if let Some(e) = self.table_editor(id) {
            let handle = e.read(cx).focus_handle_ref();
            window.focus(&handle, cx);
        } else if let Some(t) = self.tool_tab(id) {
            t.focus(window, cx);
        }
        self.save_all_consoles(cx);
    }

    fn refresh_editor_status(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.active_tab != Some(id) {
            return;
        }
        if let Some(c) = self.consoles.get(&id) {
            self.editor_status = Some(c.editor.read(cx).status());
            cx.notify();
        }
    }

    // ------------------------------------------------------- editor events

    fn on_editor_event(&mut self, id: u64, ev: EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::Edited { .. } => {
                let modified =
                    self.consoles.get(&id).is_some_and(|c| c.editor.read(cx).editor().is_modified());
                self.set_tab_modified(id, modified, cx);
                self.schedule_save(id, cx);
                self.refresh_editor_status(id, cx);
            }
            EditorEvent::StateChanged => self.refresh_editor_status(id, cx),
            EditorEvent::DiagnosticsChanged => {
                if self.active_tab == Some(id) {
                    self.refresh_problems(cx);
                }
            }
            EditorEvent::Effect(e) => self.on_editor_effect(id, e, window, cx),
            EditorEvent::ExpandSnippet { body, replace } => {
                self.expand_template(id, body, replace, window, cx)
            }
            EditorEvent::Focused | EditorEvent::Blurred => {}
        }
    }

    fn on_editor_effect(&mut self, id: u64, e: Effect, window: &mut Window, cx: &mut Context<Self>) {
        use crate::actions as a;
        let act = |window: &mut Window, cx: &mut App, action: Box<dyn gpui_kit::Action>| {
            window.dispatch_action(action, cx)
        };
        match e {
            Effect::RunStatementAtCursor => self.run_request(id, RunReq::Statement, window, cx),
            Effect::RunSelection(s) => self.run_request(id, RunReq::Selection(s), window, cx),
            Effect::RunAll => self.run_request(id, RunReq::All, window, cx),
            Effect::CancelQuery => self.cancel_active(window, cx),
            Effect::ExplainStatement { analyze } => self.explain_console(id, analyze, window, cx),
            Effect::FormatDocument => self.format_console(id, false, window, cx),
            Effect::FormatSelection => self.format_console(id, true, window, cx),
            Effect::GoToDefinition => self.go_to_definition(id, window, cx),
            Effect::FindUsages => self.find_usages(id, window, cx),
            Effect::RenameSymbol => self.rename_symbol(id, window, cx),
            Effect::CodeActions => self.code_actions(id, window, cx),
            Effect::ShowProblems => self.show_problems(window, cx),
            Effect::Save => self.save_console_as(window, cx),
            Effect::CloseTab => self.request_close_tab(id, window, cx),
            Effect::NewTab => {
                self.open_console(None, None, window, cx);
            }
            Effect::NextTab => act(window, cx, Box::new(a::NextTab)),
            Effect::PreviousTab => act(window, cx, Box::new(a::PreviousTab)),
            Effect::OpenCommandPalette => act(window, cx, Box::new(a::OpenCommandPalette)),
            Effect::OpenConnectionPicker => act(window, cx, Box::new(a::OpenConnectionPicker)),
            Effect::OpenTablePicker => act(window, cx, Box::new(a::OpenTablePicker)),
            Effect::OpenThemePicker => act(window, cx, Box::new(a::SelectTheme)),
            Effect::FocusExplorer => act(window, cx, Box::new(a::FocusExplorer)),
            Effect::FocusResults => self.focus_results(window, cx),
            Effect::ToggleBottomPanel => act(window, cx, Box::new(a::ToggleBottomPanel)),
            Effect::ToggleExplorer => act(window, cx, Box::new(a::ToggleExplorer)),
            Effect::Quit => act(window, cx, Box::new(a::Quit)),
            Effect::HostCommand(cmd) => self.host_command(&cmd, window, cx),
            // Handled by the view itself.
            Effect::SetClipboard(_)
            | Effect::RequestCompletion
            | Effect::DismissCompletion
            | Effect::RevealCursor
            | Effect::NextProblem
            | Effect::PreviousProblem
            | Effect::Scroll(_)
            | Effect::Status(_) => {}
        }
    }

    /// Where keyboard focus belongs when the window opens: the active editor if there is one
    /// (restored consoles included), else the workspace itself.
    pub fn focus_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab.is_some_and(|id| {
            self.consoles.contains_key(&id)
                || self.is_table_tab(id)
                || self.admin_tab(id).is_some()
                || self.tool_tab(id).is_some()
        }) {
            self.focus_editor(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// Keyboard focus to the active console's editor.
    pub(super) fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = self.active_tab.and_then(|id| self.consoles.get(&id)) {
            let handle = c.editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        } else if let Some(e) = self.active_tab.and_then(|id| self.table_editor(id)) {
            let handle = e.read(cx).focus_handle_ref();
            window.focus(&handle, cx);
        } else if let Some(a) = self.active_tab.and_then(|id| self.admin_tab(id)) {
            match a {
                super::AdminTab::Sessions(v) => v.update(cx, |v, cx| v.focus_list(window, cx)),
                super::AdminTab::Roles(v) => v.update(cx, |v, cx| v.focus_list(window, cx)),
            }
        } else if let Some(t) = self.active_tab.and_then(|id| self.tool_tab(id)) {
            t.focus(window, cx);
        }
    }

    /// Keyboard focus to the results grid (opening the bottom panel if it is hidden).
    pub(super) fn focus_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.bottom_open {
            self.toggle_bottom(cx);
        }
        if self.state.bottom_tab != BottomTab::Results {
            self.set_bottom_tab(BottomTab::Results, cx);
        }
        let h = self.results.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&h, cx);
    }

    /// Show the Chart tab of the active result and give it the keyboard, so the arrow keys walk the points
    /// and the digits pick the chart type.
    pub(super) fn show_chart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.bottom_open {
            self.toggle_bottom(cx);
        }
        self.set_bottom_tab(BottomTab::Chart, cx);
        let h = self.chart.read(cx).focus_handle();
        window.focus(&h, cx);
    }

    /// Show the value viewer for the results grid's focused cell, or hide it. The grid takes focus so
    /// the cursor keys move the viewer along; `esc` hands focus back to the editor as usual.
    pub(super) fn toggle_cell_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_results(window, cx);
        let grid = self.results.read(cx).grid.clone();
        grid.update(cx, |g, cx| g.toggle_cell_viewer(cx));
    }

    pub(super) fn copy_cell_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let grid = self.results.read(cx).grid.clone();
        let copied = grid.update(cx, |g, cx| g.copy_cell_value(cx));
        if !copied {
            self.notify(NoticeKind::Error, "Select a cell in the results first".to_string(), window, cx);
        }
    }

    fn host_command(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let t = cmd.trim().trim_start_matches(':');
        let (name, arg) = t.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((t, ""));
        match name {
            "theme" if !arg.is_empty() => {
                let names: Vec<String> =
                    theta_ui::themes::available(cx).iter().map(|n| n.name.to_string()).collect();
                match resolve_theme(&names, arg) {
                    Ok(name) => theta_ui::themes::set_theme(&name, cx),
                    Err(options) if options.is_empty() => self.notify(
                        NoticeKind::Error,
                        format!("No theme named “{}”", shorten(arg)),
                        window,
                        cx,
                    ),
                    Err(options) => self.notify(
                        NoticeKind::Error,
                        format!("“{}” matches several themes: {}", shorten(arg), options.join(", ")),
                        window,
                        cx,
                    ),
                }
            }
            "theme" => window.dispatch_action(Box::new(crate::actions::SelectTheme), cx),
            "connect" | "conn" if arg.is_empty() => {
                window.dispatch_action(Box::new(crate::actions::OpenConnectionPicker), cx)
            }
            "connect" | "conn" => {
                let found = self
                    .store
                    .read(cx)
                    .configs()
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(arg))
                    .map(|c| c.id.clone());
                match found {
                    Some(id) => {
                        self.use_connection(&id, cx);
                        self.rebind_active_console(&id, cx);
                    }
                    None => self.notify(
                        NoticeKind::Error,
                        format!("No connection named “{}”", shorten(arg)),
                        window,
                        cx,
                    ),
                }
            }
            "chart" => self.show_chart(window, cx),
            "diagram" | "er" | "erd" => self.diagram_from_palette(arg, window, cx),
            "schemacompare" | "scompare" | "schemadiff" => self.compare_schemas_dialog(None, window, cx),
            "datacompare" | "dcompare" | "datadiff" => self.compare_data_dialog(None, window, cx),
            "lhistory" | "localhistory" | "lh" => self.open_local_history(window, cx),
            "multirun" | "multi" => self.open_multi_run(window, cx),
            "grep" | "search" => self.search_from_palette(window, cx),
            "cell" | "viewer" if arg == "copy" => self.copy_cell_value(window, cx),
            "cell" | "viewer" => self.toggle_cell_viewer(window, cx),
            "reopen" | "reopen-tab" => self.reopen_closed_tab(window, cx),
            "run" => self.run_request(self.active_tab.unwrap_or(0), RunReq::Statement, window, cx),
            "format" | "fmt" => self.format_console(self.active_tab.unwrap_or(0), false, window, cx),
            "open" | "e" => self.open_sql_file(window, cx),
            "import" | "export" => self.bulk_host_command(cmd, arg, window, cx),
            "sessions" | "sess" | "roles" | "users" | "dump" | "script" | "source" | "restore" => {
                self.admin_host_command(cmd, arg, window, cx)
            }
            "table" | "tbl" => self.table_host_command(arg, window, cx),
            // Vim `:help`: the key reference of the active keymap is the nearest thing to it.
            "help" | "h" => crate::dialogs::settings::open_at(crate::dialogs::settings::Section::Keymap, window, cx),
            // The Vim engine handles its own options (ignorecase, smartcase, hlsearch) and passes
            // the rest on: say so instead of pretending it worked.
            "set" | "se" => self.notify(
                NoticeKind::Error,
                format!(
                    "Unknown option: {} (supported: ignorecase, smartcase, hlsearch; keymap and editor settings are in Settings)",
                    shorten(arg)
                ),
                window,
                cx,
            ),
            _ => self.notify(NoticeKind::Info, format!("Unknown command: {}", shorten(cmd)), window, cx),
        }
    }

    // ---------------------------------------------------------------- running

    pub(super) fn run_active(&mut self, req: RunReq, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab {
            Some(id) => self.run_request(id, req, window, cx),
            None => self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx),
        }
    }

    pub(super) fn console_kind(&self, id: u64, cx: &App) -> DatabaseKind {
        self.tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.connection_id.clone())
            .and_then(|c| self.store.read(cx).config(&c).map(|c| c.kind))
            .unwrap_or(DatabaseKind::Postgres)
    }

    /// Text, cursor byte offset and the selection (start byte, text) of a console.
    pub(super) fn editor_snapshot(&self, id: u64, cx: &App) -> Option<Snapshot> {
        let c = self.consoles.get(&id)?;
        let ed = c.editor.read(cx).editor();
        let text = ed.text();
        let p = ed.primary();
        let cursor = ed.char_to_byte(ed.cursor_of(&p));
        let sel = ed.selected_text().map(|s| (ed.char_to_byte(p.from()), s));
        Some((text, cursor, sel))
    }

    pub(super) fn run_request(&mut self, id: u64, req: RunReq, window: &mut Window, cx: &mut Context<Self>) {
        // Run (ctrl-enter, F5) in a Sessions or Roles tab means "refresh": there is no statement to run.
        if self.refresh_admin_tab(id, cx) {
            return;
        }
        let Some((text, cursor, sel)) = self.editor_snapshot(id, cx) else {
            self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx);
            return;
        };
        let kind = self.console_kind(id, cx);
        // What is about to run is the text worth remembering, exactly as it is (not the 600 ms-late save).
        if let Some(c) = self.consoles.get(&id) {
            self.record_local_history(&c.key.clone(), &text, LocalKind::Run);
        }
        let picks: Vec<Picked> = match req {
            RunReq::Statement => match sel {
                Some((start, s)) if !s.trim().is_empty() => pick::selection(&text, start, &s, kind),
                _ => pick::at_cursor(&text, cursor, kind).into_iter().collect(),
            },
            RunReq::Selection(s) => {
                let start = sel
                    .as_ref()
                    .map(|(st, _)| *st)
                    .filter(|st| text.get(*st..).is_some_and(|t| t.starts_with(&s)))
                    .or_else(|| text.find(&s))
                    .unwrap_or(0);
                pick::selection(&text, start, &s, kind)
            }
            RunReq::All => pick::all(&text, kind),
        };
        let picks: Vec<Picked> = picks.into_iter().filter(|p| !p.sql.trim().is_empty()).collect();
        if picks.is_empty() {
            self.notify(NoticeKind::Info, "There is no statement to run here", window, cx);
            return;
        }
        self.with_params(id, picks, window, cx, move |this, picks, window, cx| {
            this.start_run(id, picks, window, cx)
        });
    }

    /// Resolve the console's connection, or explain why there is none.
    fn connection_for(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, Arc<dyn Connection>)> {
        let Some(conn_id) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone())
        else {
            self.notify(NoticeKind::Info, "Choose a connection for this console first", window, cx);
            window.dispatch_action(Box::new(crate::actions::OpenConnectionPicker), cx);
            return None;
        };
        let state = self.store.read(cx).state(&conn_id).clone();
        match state {
            ConnState::Connected { connection, .. } => Some((conn_id, connection)),
            ConnState::Connecting => {
                self.notify(NoticeKind::Info, "Still connecting…", window, cx);
                None
            }
            ConnState::Disconnected | ConnState::Failed(_) => {
                self.use_connection(&conn_id, cx);
                self.notify(NoticeKind::Info, "Connecting… run again once connected", window, cx);
                None
            }
        }
    }

    /// Run `then` once console `id`'s connection is up. Returns false when it already is (the
    /// caller carries on inline) and true when the caller must stop: either `then` was queued
    /// behind a connect that is now in progress, or there is no connection to wait for.
    ///
    /// A restored console keeps its connection id but the connection itself starts
    /// disconnected; Run / Explain must connect, wait and then run, not ask the user to press
    /// the key twice. A failed connect is reported in Messages with the server's error.
    fn defer_until_connected(
        &mut self,
        id: u64,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        let Some(conn_id) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone())
        else {
            // Explains why (and opens the connection picker).
            self.connection_for(id, window, cx);
            return true;
        };
        let state = self.store.read(cx).state(&conn_id).clone();
        if state.is_connected() {
            return false;
        }
        if !self.awaiting_connection.insert(id) {
            self.notify(NoticeKind::Info, "Still connecting…", window, cx);
            return true;
        }
        if !matches!(state, ConnState::Connecting) {
            self.use_connection(&conn_id, cx);
        }
        let name = self.conn_name(&conn_id, cx);
        self.notify(NoticeKind::Info, format!("Connecting to {name}…"), window, cx);
        let what = what.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let waited = wait_connected(&this, cx, &conn_id).await;
            this.update_in(cx, |this, window, cx| {
                this.awaiting_connection.remove(&id);
                match waited {
                    Ok(_) => {
                        // The console may have been closed or re-pointed while connecting.
                        let same = this
                            .tabs
                            .iter()
                            .find(|t| t.id == id)
                            .is_some_and(|t| t.connection_id.as_deref() == Some(conn_id.as_str()));
                        if same {
                            then(this, window, cx);
                        }
                    }
                    Err(e) => {
                        let name = this.conn_name(&conn_id, cx);
                        this.log(
                            &conn_id,
                            &what,
                            MessageKind::Error,
                            vec![format!("Could not connect to {name}: {e}")],
                            Some(&e),
                            cx,
                        );
                        this.set_bottom_tab(BottomTab::Messages, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        true
    }

    fn conn_name(&self, conn_id: &str, cx: &App) -> String {
        self.store.read(cx).config(conn_id).map(|c| c.name.clone()).unwrap_or_default()
    }

    fn start_run(&mut self, id: u64, picks: Vec<Picked>, window: &mut Window, cx: &mut Context<Self>) {
        let what = picks.first().map(|p| p.sql.clone()).unwrap_or_default();
        let again = picks.clone();
        if self.defer_until_connected(id, &what, window, cx, move |this, window, cx| {
            this.start_run(id, again, window, cx)
        }) {
            return;
        }
        let Some((conn_id, _)) = self.connection_for(id, window, cx) else { return };
        if self.runs.contains_key(&conn_id) {
            // Policy: one query at a time per connection; a second run is refused, not queued.
            let name = self.conn_name(&conn_id, cx);
            self.notify(
                NoticeKind::Info,
                format!("A query is already running on {name}. Cancel it first (Ctrl+.)."),
                window,
                cx,
            );
            return;
        }
        let read_only = self.store.read(cx).config(&conn_id).is_some_and(|c| c.read_only);
        let kind = self.console_kind(id, cx);
        if let Some((p, why)) =
            picks.iter().find_map(|p| pick::read_only_refusal(p.class, read_only).map(|w| (p, w)))
        {
            self.log(&conn_id, &p.sql, MessageKind::Error, vec![why.clone()], None, cx);
            self.set_status(id, Some((p.range.clone(), StatementStatus::Failed)), cx);
            // The Messages tab (brought to the front) and the status bar already say it.
            self.set_bottom_tab(BottomTab::Messages, cx);
            return;
        }
        if let Some((p, why)) =
            picks.iter().find_map(|p| pick::dangerous_reason(&p.sql, kind, p.class).map(|w| (p, w)))
        {
            let ws = cx.entity().downgrade();
            let stmt = p.sql.clone();
            let picks2 = picks.clone();
            confirm::open(
                window,
                cx,
                "Run this statement?",
                format!("{why}. Nothing is wrapped in a transaction unless you opened one."),
                vec![stmt],
                vec![Choice::new("Run Anyway", Tone::Danger, move |window, cx| {
                    let picks = picks2.clone();
                    ws.update(cx, |this, cx| this.execute_picks(id, picks, window, cx)).ok();
                })],
            );
            return;
        }
        self.execute_picks(id, picks, window, cx);
    }

    fn set_status(
        &mut self,
        id: u64,
        status: Option<(std::ops::Range<usize>, StatementStatus)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(c) = self.consoles.get(&id) {
            c.editor.update(cx, |e, cx| e.set_statement_status(status, cx));
        }
    }

    fn execute_picks(&mut self, id: u64, picks: Vec<Picked>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((conn_id, connection)) = self.connection_for(id, window, cx) else { return };
        if self.runs.contains_key(&conn_id) {
            return;
        }
        self.runs.insert(
            conn_id.clone(),
            RunInfo { console: id, started: Instant::now(), cancelled: false, failed: false },
        );
        if let Some(c) = self.consoles.get_mut(&id) {
            c.results.clear();
            c.active_result = 0;
            c.summary = None;
            // The previous rows stay, dimmed with a progress bar, until the answer replaces them.
            c.grid.update(cx, |g, cx| g.set_loading(true, cx));
        }
        // With no rows to dim (the last statement returned none, or this is the first), say so.
        let nothing_on_show = self.consoles.get(&id).is_some_and(|c| c.grid.read(cx).result().is_none());
        if self.active_tab == Some(id) && nothing_on_show {
            self.results.update(cx, |r, cx| r.show_running(cx));
        }
        self.sync_running(cx);
        self.ensure_ticker(cx);
        let limit = Settings::get(cx).row_limit;
        let opts = ExecOptions { row_limit: (limit > 0).then_some(limit) };
        // Whether the session may hold uncommitted work that a reconnect would silently drop.
        let mut txn_open = self.txn.contains(&conn_id);
        // Manual transaction mode: what to start a transaction with, if the session is set to it.
        let manual = self.manual_begin(&conn_id, cx);
        cx.spawn_in(window, async move |this, cx| {
            let mut connection = connection;
            let total = picks.len();
            // Set once a DDL statement has been sent, whether or not it succeeded: in a script that
            // continues after an error an earlier one may have taken effect. One refresh at the end of the
            // run, not one per statement, because introspection is the expensive part.
            let mut ran_ddl = false;
            for (i, p) in picks.iter().enumerate() {
                let cancelled = this
                    .update(cx, |this, cx| {
                        this.set_status(id, Some((p.range.clone(), StatementStatus::Running)), cx);
                        this.runs.get(&conn_id).is_none_or(|r| r.cancelled)
                    })
                    .unwrap_or(true);
                if cancelled {
                    break;
                }
                let mut began = Vec::new();
                let mut res = match begin_transaction_if_needed(&connection, manual.as_ref(), p).await {
                    Ok(stmts) => {
                        began = stmts;
                        exec_statement(&connection, p, &opts).await
                    }
                    Err(e) => Err(e),
                };
                let mut recovery = Recovery::None;
                if res.as_ref().is_err_and(Error::is_connection_lost) {
                    let user_cancelled = this
                        .update(cx, |this, _| this.runs.get(&conn_id).is_some_and(|r| r.cancelled))
                        .unwrap_or(true);
                    if !user_cancelled {
                        txn_open |= p.class == StatementClass::Transaction;
                        (connection, res, recovery) =
                            recover_lost_session(&this, cx, &conn_id, connection, p, &opts, txn_open, res)
                                .await;
                    }
                }
                ran_ddl |= changes_schema(p.class);
                if matches!(p.class, StatementClass::Transaction) {
                    txn_open = true;
                }
                if let Some(m) = &manual {
                    // Ask the server rather than assume: a DDL statement on MySQL ends the transaction
                    // by itself, and the title bar must say so.
                    let was_open = txn_open || !began.is_empty();
                    let now_open = {
                        let c = connection.clone();
                        runtime::spawn(async move { c.in_transaction().await }).await.unwrap_or(false)
                    };
                    let note = theta_core::txn::implicit_commit_note(
                        m.kind,
                        was_open,
                        now_open,
                        txn_statement_kind(p.class),
                    );
                    txn_open = now_open;
                    let started = began.last().cloned();
                    this.update(cx, |this, cx| {
                        if let Some(first) = &started {
                            this.log(
                                &conn_id,
                                first,
                                MessageKind::Info,
                                vec![format!("Transaction started: {}", began.join("; "))],
                                None,
                                cx,
                            );
                        }
                        if let Some(n) = note {
                            this.log(&conn_id, &p.sql, MessageKind::Info, vec![n.to_string()], None, cx);
                        }
                    })
                    .ok();
                }
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        this.finish_statement(id, &conn_id, p, i, total, res, recovery, window, cx)
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.finish_run(id, &conn_id, connection, ran_ddl, window, cx)
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_statement(
        &mut self,
        id: u64,
        conn_id: &str,
        p: &Picked,
        index: usize,
        total: usize,
        res: Result<StatementOutcome, Error>,
        recovery: Recovery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let conn_name = self.conn_name(conn_id, cx);
        let read_only = self.store.read(cx).config(conn_id).is_some_and(|c| c.read_only);
        let cancelled = self.runs.get(conn_id).is_some_and(|r| r.cancelled);
        match res {
            Ok(out) => {
                let rows = out.result.as_ref().map(|r| r.rows.len() as u64).or(out.rows_affected);
                self.push_history(&conn_name, &p.sql, out.elapsed, rows, None, cx);
                self.set_status(id, Some((p.range.clone(), StatementStatus::Ok)), cx);
                if let Some(c) = self.consoles.get(&id) {
                    c.editor.update(cx, |ed, cx| ed.clear_transient_diagnostic(cx));
                }
                let v = verb(&p.sql);
                let mut lines = Vec::new();
                if let Some(rs) = out.result {
                    let n = rs.rows.len() as u64;
                    let catalog = self.store.read(cx).catalog(conn_id);
                    let meta = editable::grid_meta(
                        &rs,
                        catalog.as_deref(),
                        read_only,
                        self.store.read(cx).config(conn_id).map_or(DatabaseKind::Postgres, |c| c.kind),
                    )
                    .with_elapsed(out.elapsed)
                    .with_statement(p.sql.clone());
                    let truncated = rs.truncated;
                    let number = self.consoles.get(&id).map_or(0, |c| c.results.len()) + 1;
                    let title = super::console::result_title(number, &p.sql, &rs.columns);
                    let rs = Arc::new(rs);
                    let item_index = {
                        let Some(c) = self.consoles.get_mut(&id) else { return false };
                        c.results.push(ResultItem {
                            title,
                            statement: p.sql.clone(),
                            result: rs.clone(),
                            meta,
                            geo: None,
                        });
                        c.results.len() - 1
                    };
                    let mut line = format!("{v} — {} · {}", plural(n, "row", "rows"), fmt_ms(out.elapsed));
                    if truncated {
                        line += &format!(" (stopped at the row limit of {})", Settings::get(cx).row_limit);
                    }
                    lines.push(line);
                    self.detect_geometry(id, item_index, rs, window, cx);
                } else {
                    let line = match out.rows_affected {
                        Some(n) => {
                            format!("{v} — {} affected · {}", plural(n, "row", "rows"), fmt_ms(out.elapsed))
                        }
                        None => format!("{v} — OK · {}", fmt_ms(out.elapsed)),
                    };
                    if let Some(c) = self.consoles.get_mut(&id) {
                        c.summary = Some((line.clone(), false));
                    }
                    lines.push(line);
                }
                if total > 1 {
                    lines[0] = format!("[{}/{}] {}", index + 1, total, lines[0]);
                }
                lines.extend(out.notices.iter().map(|n| format!("Notice: {n}")));
                lines.extend(recovery.message());
                self.log(conn_id, &p.sql, MessageKind::Ok, lines, None, cx);
                true
            }
            Err(e) => {
                if let Some(r) = self.runs.get_mut(conn_id) {
                    r.failed = true;
                }
                let elapsed = self.runs.get(conn_id).map(|r| r.started.elapsed()).unwrap_or_default();
                if cancelled || matches!(e, Error::Cancelled) {
                    self.push_history(&conn_name, &p.sql, elapsed, None, Some("cancelled".into()), cx);
                    self.set_status(id, None, cx);
                    let line = format!("Cancelled after {}", fmt_ms(elapsed));
                    if let Some(c) = self.consoles.get_mut(&id) {
                        c.summary = Some((line.clone(), true));
                    }
                    self.log(conn_id, &p.sql, MessageKind::Info, vec![line], None, cx);
                    return false;
                }
                self.push_history(&conn_name, &p.sql, elapsed, None, Some(e.to_string()), cx);
                self.set_status(id, Some((p.range.clone(), StatementStatus::Failed)), cx);
                // A dropped connection says nothing about the statement's text: no squiggle.
                if !e.is_connection_lost()
                    && let Some(text) = self.console_text(id, cx)
                {
                    let range = pick::error_range(&p.range, &e, &text);
                    if let Some(c) = self.consoles.get(&id) {
                        c.editor.update(cx, |ed, cx| ed.set_transient_diagnostic(range, e.to_string(), cx));
                    }
                }
                if let Some(c) = self.consoles.get_mut(&id) {
                    c.summary = Some((e.to_string(), true));
                }
                // No toast: `finish_run` raises the Messages tab and the status bar shows the text.
                let mut lines = vec![e.to_string()];
                // A dropped session was already reconnected by `recover_lost_session`; say what
                // happened to this statement.
                lines.extend(recovery.message());
                self.log(conn_id, &p.sql, MessageKind::Error, lines, Some(&e), cx);
                false
            }
        }
    }

    fn finish_run(
        &mut self,
        id: u64,
        conn_id: &str,
        connection: Arc<dyn Connection>,
        ran_ddl: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let failed = self.runs.remove(conn_id).is_some_and(|r| r.failed);
        if ran_ddl {
            // The explorer, completion and the table editors all read the store's catalog; without this a
            // `create table` typed into a console (or given to `--run`) stays invisible until a manual refresh.
            self.store.update(cx, |s, cx| s.refresh_catalog(conn_id, cx));
        }
        self.sync_running(cx);
        // Only the console on screen may touch the shared panels: a statement that finishes in a
        // background tab stores its result with that console (see `finish_statement`) and is
        // shown when the tab is next activated.
        if self.active_tab == Some(id) {
            self.apply_console(id, window, cx);
            self.set_bottom_tab(if failed { BottomTab::Messages } else { BottomTab::Results }, cx);
        }
        self.refresh_txn(conn_id.to_string(), connection, cx);
    }

    pub(super) fn cancel_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conn_id) = self.active_connection_id(cx) else { return };
        let Some(run) = self.runs.get_mut(&conn_id) else {
            self.notify(NoticeKind::Info, "Nothing is running", window, cx);
            return;
        };
        run.cancelled = true;
        let ConnState::Connected { connection, .. } = self.store.read(cx).state(&conn_id).clone() else {
            return;
        };
        self.send_cancel(conn_id, connection, cx);
    }

    /// Ask the server to stop the statement, and keep asking until the run has ended.
    ///
    /// A cancel that reaches the server before it has started the statement (Run immediately
    /// followed by Cancel, or a console closed right after Run) is silently dropped by every
    /// driver, and the statement would then run to completion with nothing left to stop it.
    fn send_cancel(&mut self, conn_id: String, connection: Arc<dyn Connection>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            for attempt in 0..120 {
                let c = connection.clone();
                let r = runtime::spawn_result(async move { c.cancel().await }).await;
                if attempt == 0
                    && let Err(e) = r
                {
                    this.update(cx, |this, cx| {
                        this.log(
                            &conn_id,
                            "cancel",
                            MessageKind::Error,
                            vec![format!("Could not cancel: {e}")],
                            None,
                            cx,
                        )
                    })
                    .ok();
                    return;
                }
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let still = this
                    .update(cx, |this, _| this.runs.get(&conn_id).is_some_and(|r| r.cancelled))
                    .unwrap_or(false);
                if !still {
                    return;
                }
            }
        })
        .detach();
    }

    /// A closed console's query has nowhere to show its result and, with no tab left on the
    /// connection, nothing could cancel it: ask the server to stop it.
    pub(super) fn cancel_run_of(&mut self, console: u64, cx: &mut Context<Self>) {
        let Some(conn_id) = self.runs.iter().find(|(_, r)| r.console == console).map(|(c, _)| c.clone())
        else {
            return;
        };
        if let Some(r) = self.runs.get_mut(&conn_id) {
            r.cancelled = true;
        }
        if let Some(connection) = self.store.read(cx).connection(&conn_id) {
            self.send_cancel(conn_id, connection, cx);
        }
    }

    pub(super) fn sync_running(&mut self, cx: &mut Context<Self>) {
        let running = self.active_connection_id(cx).is_some_and(|c| self.runs.contains_key(&c));
        self.running = running;
        cx.notify();
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.runs.is_empty() {
                        this.ticker = None;
                        false
                    } else {
                        cx.notify();
                        true
                    }
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Elapsed time of the query the *active console* is running. A console that shares the
    /// connection with a running one shows nothing: its own state is idle (its Run is refused
    /// with an explanation, and Cancel in the title bar still stops the connection's query).
    pub(super) fn running_elapsed(&self, _cx: &App) -> Option<Duration> {
        let id = self.active_tab?;
        self.runs.values().find(|r| r.console == id).map(|r| r.started.elapsed())
    }

    // ------------------------------------------------------- logging/history

    pub(super) fn log(
        &mut self,
        conn_id: &str,
        sql: &str,
        kind: MessageKind,
        lines: Vec<String>,
        err: Option<&Error>,
        cx: &mut Context<Self>,
    ) {
        let (code, detail, hint) = match err {
            Some(Error::Query { code, detail, hint, .. }) => (code.clone(), detail.clone(), hint.clone()),
            _ => (None, None, None),
        };
        let m = Message {
            time: clock_now(),
            connection: self.conn_name(conn_id, cx),
            statement: pick::first_line(sql),
            kind,
            lines,
            code,
            detail,
            hint,
        };
        self.messages.update(cx, |p, cx| p.push(m, cx));
    }

    fn push_history(
        &mut self,
        conn_name: &str,
        sql: &str,
        elapsed: Duration,
        rows: Option<u64>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let e = HistoryEntry {
            at: now_ms(),
            connection: conn_name.to_string(),
            sql: sql.to_string(),
            duration_ms: elapsed.as_millis() as u64,
            rows,
            ok: error.is_none(),
            error,
        };
        self.history.update(cx, |h, cx| h.push(e, cx));
    }

    pub(super) fn on_history_event(&mut self, e: HistoryEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            HistoryEvent::Insert(sql) => match self.active_tab {
                Some(id) => self.insert_into_console(id, &sql, cx),
                None => {
                    self.open_console(None, Some(sql), window, cx);
                }
            },
            HistoryEvent::OpenNew { connection, sql } => {
                let conn =
                    self.store.read(cx).configs().iter().find(|c| c.name == connection).map(|c| c.id.clone());
                self.open_console(conn, Some(sql), window, cx);
            }
        }
    }

    fn insert_into_console(&mut self, id: u64, sql: &str, cx: &mut Context<Self>) {
        if let Some(c) = self.consoles.get(&id) {
            let sql = sql.to_string();
            c.editor.update(cx, |e, cx| {
                e.update_editor(cx, |ed| {
                    let end = ed.len_chars();
                    let prefix = if end == 0 { "" } else { "\n\n" };
                    ed.replace_range(end..end, &format!("{prefix}{sql};"));
                })
            });
        }
    }

    // ------------------------------------------------------ results display

    /// Make the shared bottom panels show console `id`: its grid, result tabs, map and status.
    ///
    /// Everything shown here is read from the console's own state; asynchronous completions
    /// only ever write to the console they belong to and call this when that console is the
    /// active one.
    pub(super) fn apply_console(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.consoles.get(&id) else {
            self.results.update(cx, |r, cx| {
                r.use_grid(None, cx);
                r.clear(cx);
            });
            self.results.read(cx).grid.clone().update(cx, |g, cx| g.clear(cx));
            self.map.update(cx, |m, cx| m.clear(cx));
            self.map_badge = None;
            self.chart.update(cx, |c, cx| c.clear(cx));
            self.plan_owner = None;
            self.plan.update(cx, |p, cx| p.clear(cx));
            self.set_result_info(None, cx);
            return;
        };
        let grid = c.grid.clone();
        self.results.update(cx, |r, cx| r.use_grid(Some(grid.clone()), cx));
        let running = self.runs.values().any(|r| r.console == id);
        let active = c.active_result.min(c.results.len().saturating_sub(1));
        if !c.results.is_empty() {
            let titles: Vec<String> = c.results.iter().map(|r| r.title.clone()).collect();
            let item = &c.results[active];
            let (rs, meta) = (item.result.clone(), item.meta.clone());
            self.results.update(cx, |r, cx| r.show_sets(titles, active, cx));
            // Re-installing the result the grid already shows would throw away its staged
            // edits, sort and scroll position: switching tabs must not cost the user that.
            if !grid.read(cx).result().is_some_and(|g| Arc::ptr_eq(g, &rs)) {
                grid.update(cx, |g, cx| g.set_result(rs.clone(), meta.clone(), window, cx));
            }
            grid.update(cx, |g, cx| g.set_loading(running, cx));
            let mut info = format!(
                "{} · {}",
                plural(rs.rows.len() as u64, "row", "rows"),
                meta.elapsed.map(fmt_ms).unwrap_or_default()
            );
            if rs.truncated {
                info += " · truncated";
            }
            self.set_result_info(Some(info.into()), cx);
            self.chart.update(cx, |c, cx| c.set_result(rs.clone(), window, cx));
            self.apply_map(id, active, window, cx);
        } else {
            let summary = c.summary.clone();
            grid.update(cx, |g, cx| g.clear(cx));
            match summary {
                _ if running => {
                    self.set_result_info(None, cx);
                    self.results.update(cx, |r, cx| r.show_running(cx));
                }
                Some((t, err)) => {
                    self.set_result_info(Some(t.clone().into()), cx);
                    self.results.update(cx, |r, cx| r.show_summary(t, err, cx));
                }
                None => {
                    self.results.update(cx, |r, cx| r.clear(cx));
                    self.set_result_info(None, cx);
                }
            }
            self.map.update(cx, |m, cx| m.clear(cx));
            self.map_badge = None;
            self.chart.update(cx, |c, cx| c.clear(cx));
        }
    }

    pub(super) fn activate_result(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab else { return };
        if let Some(c) = self.consoles.get_mut(&id) {
            c.active_result = i;
        }
        self.apply_console(id, window, cx);
    }

    fn apply_map(&mut self, id: u64, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let geo = self.consoles.get(&id).and_then(|c| c.results.get(index)).and_then(|r| r.geo.clone());
        match geo {
            Some(g) => {
                self.map_badge = Some(g.features.len());
                let (layers, features) = (g.layers.clone(), g.features.clone());
                self.map.update(cx, |m, cx| m.set_features(layers, features, window, cx));
            }
            None => {
                self.map_badge = None;
                self.map.update(cx, |m, cx| m.clear(cx));
            }
        }
        cx.notify();
    }

    fn detect_geometry(
        &mut self,
        id: u64,
        index: usize,
        rs: Arc<theta_core::ResultSet>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rs2 = rs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let data = runtime::spawn(async move { geo::extract(&rs2) }).await.ok().flatten();
            let Some(data) = data else { return };
            this.update_in(cx, |this, window, cx| {
                let still = this
                    .consoles
                    .get_mut(&id)
                    .and_then(|c| c.results.get_mut(index))
                    .filter(|r| Arc::ptr_eq(&r.result, &rs))
                    .map(|r| r.geo = Some(Arc::new(data)))
                    .is_some();
                let active = this.consoles.get(&id).map(|c| c.active_result);
                if still && this.active_tab == Some(id) && active == Some(index) {
                    this.apply_map(id, index, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ grid events

    /// The result console `id` is showing and the connection it belongs to.
    fn item_of(&self, id: u64) -> Option<(&ResultItem, String)> {
        let c = self.consoles.get(&id)?;
        let conn = self.tabs.iter().find(|t| t.id == id)?.connection_id.clone()?;
        c.results.get(c.active_result.min(c.results.len().saturating_sub(1))).map(|r| (r, conn))
    }

    /// An event from console `id`'s grid. The id comes from the subscription, not from whichever
    /// tab happens to be active when the event is handled.
    pub(super) fn on_grid_event(
        &mut self,
        id: u64,
        e: GridEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            GridEvent::SubmitChanges(changes) => self.submit_changes(id, changes, window, cx),
            GridEvent::Copied { cells } => self.notify(
                NoticeKind::Info,
                format!("Copied {}", plural(cells as u64, "cell", "cells")),
                window,
                cx,
            ),
            GridEvent::Export(req) => self.export_grid(id, req, window, cx),
            GridEvent::ShowOnMap { row, .. } => {
                // The map shows the active console's features; a stale grid has nothing to show.
                if self.active_tab == Some(id) {
                    self.set_bottom_tab(BottomTab::Map, cx);
                    self.map.update(cx, |m, cx| m.select_row(row, true, cx));
                }
            }
            GridEvent::SelectionChanged => {}
            GridEvent::ReturnFocus => self.focus_editor(window, cx),
            GridEvent::FollowReference { column, value } => {
                self.follow_reference(id, column, value, window, cx)
            }
        }
    }

    fn follow_reference(
        &mut self,
        console: u64,
        column: usize,
        value: theta_core::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((item, conn)) = self.item_of(console) else { return };
        let Some(catalog) = self.store.read(cx).catalog(&conn) else { return };
        let Some(target) = item.meta.editable.clone() else { return };
        let Some((schema, table, col)) =
            editable::reference_target(&item.result, &catalog, (&target.schema, &target.table), column)
        else {
            return;
        };
        let kind = self.store.read(cx).config(&conn).map(|c| c.kind).unwrap_or(DatabaseKind::Postgres);
        let sql = format!(
            "SELECT * FROM {} WHERE {} = {}",
            theta_sql::keywords::qualified_name(schema.as_deref().or(target.schema.as_deref()), &table, kind),
            theta_sql::keywords::quote_if_needed(&col, kind),
            theta_sql::sql_literal(&value, kind)
        );
        let id = self.open_console(Some(conn), Some(sql), window, cx);
        self.set_tab_title(id, table, cx);
        self.run_request(id, RunReq::All, window, cx);
    }

    fn submit_changes(
        &mut self,
        console: u64,
        changes: Vec<theta_grid::RowChange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(grid) = self.consoles.get(&console).map(|c| c.grid.clone()) else { return };
        let Some((item, conn)) = self.item_of(console) else { return };
        let Some(target) = item.meta.editable.clone() else {
            grid.update(cx, |g, cx| g.changes_failed("This result is not editable".into(), cx));
            return;
        };
        // The data the edits were staged on: the answer is folded into exactly this result.
        let submitted = item.result.clone();
        let kind = self.store.read(cx).config(&conn).map(|c| c.kind).unwrap_or(DatabaseKind::Postgres);
        let stmts = match changes::statements(kind, &target, &changes) {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => return,
            Err(m) => {
                grid.update(cx, |g, cx| g.changes_failed(m, cx));
                return;
            }
        };
        let ConnState::Connected { connection, .. } = self.store.read(cx).state(&conn).clone() else {
            grid.update(cx, |g, cx| g.changes_failed("The connection is closed".into(), cx));
            return;
        };
        if self.quiet {
            // Test windows have no `Root` to host the confirmation dialog: act as if confirmed.
            self.apply_changes(console, submitted, conn, connection, stmts, window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let n = stmts.len();
        let stmts2 = stmts.clone();
        let (conn2, connection2) = (conn.clone(), connection.clone());
        confirm::open(
            window,
            cx,
            "Apply changes?",
            format!("{} will be applied in one transaction.", plural(n as u64, "statement", "statements")),
            stmts,
            vec![Choice::new(
                format!("Apply {n} {}", if n == 1 { "change" } else { "changes" }),
                Tone::Primary,
                move |window, cx| {
                    let (stmts, conn, connection) = (stmts2.clone(), conn2.clone(), connection2.clone());
                    let submitted = submitted.clone();
                    ws.update(cx, |this, cx| {
                        this.apply_changes(console, submitted, conn, connection, stmts, window, cx)
                    })
                    .ok();
                },
            )],
        );
        // Cancelling the dialog leaves the edits staged; nothing else to do.
    }

    /// Run the staged edits in one transaction and report the outcome to console `console`'s
    /// grid. The user may switch tabs (or close this one) while the server works, so the answer
    /// is routed by id and never to whatever grid is on screen when it arrives.
    #[allow(clippy::too_many_arguments)]
    fn apply_changes(
        &mut self,
        console: u64,
        submitted: Arc<theta_core::ResultSet>,
        conn_id: String,
        connection: Arc<dyn Connection>,
        stmts: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let c = connection.clone();
        let list = stmts.clone();
        // In manual mode the edits join a transaction that stays open for the user to commit.
        let manual = self.manual_begin(&conn_id, cx);
        cx.spawn_in(window, async move |this, cx| {
            let res = runtime::spawn_result(async move {
                let opts = ExecOptions::default();
                let was_open = c.in_transaction().await;
                let mut began_here = false;
                if let (Some(m), false) = (&manual, was_open) {
                    for sql in &m.statements {
                        c.execute(sql, &opts).await?;
                    }
                    began_here = true;
                }
                let in_txn = was_open || began_here;
                let all = changes::with_transaction(list, in_txn);
                for sql in &all {
                    if let Err(e) = c.execute(sql, &opts).await {
                        if !was_open {
                            let _ = c.execute("ROLLBACK", &opts).await;
                        }
                        return Err(e);
                    }
                }
                Ok(in_txn)
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                let name = this.conn_name(&conn_id, cx);
                let grid = this.consoles.get(&console).map(|c| c.grid.clone());
                match res {
                    Ok(in_txn) => {
                        if let Some(grid) = grid {
                            this.fold_applied_changes(console, &grid, &submitted, cx);
                        }
                        for s in &stmts {
                            this.push_history(&name, s, Duration::ZERO, None, None, cx);
                        }
                        let tail = if in_txn { " (in your open transaction, not yet committed)" } else { "" };
                        this.log(
                            &conn_id,
                            &stmts[0],
                            MessageKind::Ok,
                            vec![format!(
                                "Applied {}{tail}",
                                plural(stmts.len() as u64, "change", "changes")
                            )],
                            None,
                            cx,
                        );
                        this.notify(
                            NoticeKind::Success,
                            format!("Applied {}{tail}", plural(stmts.len() as u64, "change", "changes")),
                            window,
                            cx,
                        );
                    }
                    Err(e) => {
                        // The staged edits stay in the grid so the user can fix and resubmit.
                        if let Some(grid) = grid {
                            grid.update(cx, |g, cx| g.changes_failed(e.to_string(), cx));
                        }
                        this.log(
                            &conn_id,
                            &stmts[0],
                            MessageKind::Error,
                            vec![format!("Changes rolled back: {e}")],
                            Some(&e),
                            cx,
                        );
                        this.notify(NoticeKind::Error, format!("Changes rolled back: {e}"), window, cx);
                    }
                }
                this.refresh_txn(conn_id.clone(), connection, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The submit succeeded: fold the staged edits into the console's grid, and keep the
    /// console's stored result in step with what the grid now shows (otherwise the next
    /// activation would see a different `Arc` and reload the stale rows).
    fn fold_applied_changes(
        &mut self,
        console: u64,
        grid: &Entity<ResultGrid>,
        submitted: &Arc<theta_core::ResultSet>,
        cx: &mut Context<Self>,
    ) {
        // A newer run replaced the rows the edits were staged on: nothing left to fold into.
        if !grid.read(cx).result().is_some_and(|r| Arc::ptr_eq(r, submitted)) {
            return;
        }
        grid.update(cx, |g, cx| g.changes_applied(cx));
        let folded = grid.read(cx).result().cloned();
        if let (Some(folded), Some(c)) = (folded, self.consoles.get_mut(&console))
            && let Some(item) = c.results.iter_mut().find(|r| Arc::ptr_eq(&r.result, submitted))
        {
            item.result = folded;
        }
    }

    fn export_grid(
        &mut self,
        console: u64,
        req: theta_grid::ExportRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use theta_grid::ExportFormatChoice as F;
        let Some((item, conn)) = self.item_of(console) else { return };
        let table = item.meta.editable.as_ref().map(|t| t.table.clone());
        let kind = self.store.read(cx).config(&conn).map(|c| c.kind).unwrap_or(DatabaseKind::Postgres);
        let Some(grid) = self.consoles.get(&console).map(|c| c.grid.clone()) else { return };
        let rs = grid.read(cx).export_view(req.selection_only);
        let n = rs.rows.len() as u64;
        let (fmt, ext) = match req.format {
            F::Csv => (ExportFormat::Csv, "csv"),
            F::Tsv => (ExportFormat::Tsv, "tsv"),
            F::Json => (ExportFormat::Json, "json"),
            F::Markdown => (ExportFormat::Markdown, "md"),
            F::SqlInserts => {
                (ExportFormat::SqlInserts { table: table.clone().unwrap_or_else(|| "result".into()) }, "sql")
            }
        };
        let text = theta_sql::export(&rs, &fmt, kind);
        if req.to_clipboard {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.notify(
                NoticeKind::Success,
                format!("Copied {} as {}", plural(n, "row", "rows"), req.format.label()),
                window,
                cx,
            );
        } else {
            let name = format!("{}.{ext}", table.unwrap_or_else(|| "result".into()));
            self.save_text_file(
                name,
                text,
                format!("Exported {} as {}", plural(n, "row", "rows"), req.format.label()),
                window,
                cx,
            );
        }
    }

    pub(super) fn save_text_file(
        &mut self,
        suggested: String,
        text: String,
        ok: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let r = std::fs::write(&path, text);
            this.update_in(cx, |this, window, cx| match r {
                Ok(()) => this.notify(NoticeKind::Success, format!("{ok}: {}", path.display()), window, cx),
                Err(e) => this.notify(
                    NoticeKind::Error,
                    format!("Could not write {}: {e}", path.display()),
                    window,
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn on_map_event(&mut self, e: MapEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            // Select the row in the grid without switching tabs or stealing focus from the map.
            MapEvent::FeatureClicked { row } => {
                let grid = self.results.read(cx).grid.clone();
                grid.update(cx, |g, cx| g.select_row(row, cx));
            }
            MapEvent::ExportGeoJson(text) => {
                self.save_text_file("result.geojson".into(), text, "Exported GeoJSON".into(), window, cx)
            }
            MapEvent::CopyText(_) => {}
        }
    }

    // ----------------------------------------------------------- explain/plan

    pub(super) fn explain_active(&mut self, analyze: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab {
            Some(id) => self.explain_console(id, analyze, window, cx),
            None => self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx),
        }
    }

    pub(super) fn explain_console(
        &mut self,
        id: u64,
        analyze: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((text, cursor, _)) = self.editor_snapshot(id, cx) else { return };
        let kind = self.console_kind(id, cx);
        let missing = kind.unsupported(if analyze {
            theta_core::Feature::ExplainAnalyze
        } else {
            theta_core::Feature::Explain
        });
        if let Some(reason) = missing {
            self.notify(NoticeKind::Info, format!("{}: {reason}", kind.label()), window, cx);
            return;
        }
        let Some(p) = pick::at_cursor(&text, cursor, kind) else {
            self.notify(NoticeKind::Info, "There is no statement to explain here", window, cx);
            return;
        };
        self.with_params(id, vec![p], window, cx, move |this, mut picks, window, cx| {
            if let Some(p) = picks.pop() {
                this.explain_picked(id, p, analyze, window, cx);
            }
        });
    }

    fn explain_picked(
        &mut self,
        id: u64,
        p: Picked,
        analyze: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let again = p.sql.clone();
        let p2 = p.clone();
        if self.defer_until_connected(id, &again, window, cx, move |this, window, cx| {
            this.explain_picked(id, p2, analyze, window, cx)
        }) {
            return;
        }
        let Some((conn_id, _)) = self.connection_for(id, window, cx) else { return };
        let target = ExplainTarget { console: id, connection_id: conn_id, sql: p.sql.clone() };
        if analyze && matches!(p.class, StatementClass::Write | StatementClass::Ddl) {
            let ws = cx.entity().downgrade();
            let t = target.clone();
            confirm::open(
                window,
                cx,
                "Explain Analyze runs the statement",
                "The database rolls the change back afterwards, but triggers, sequences and other side effects can still happen.",
                vec![p.sql],
                vec![Choice::new("Explain Analyze", Tone::Danger, move |window, cx| {
                    let t = t.clone();
                    ws.update(cx, |this, cx| this.run_explain(t, true, window, cx)).ok();
                })],
            );
            return;
        }
        self.run_explain(target, analyze, window, cx);
    }

    fn run_explain(&mut self, t: ExplainTarget, analyze: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ConnState::Connected { connection, .. } = self.store.read(cx).state(&t.connection_id).clone()
        else {
            self.notify(NoticeKind::Info, "Connect first", window, cx);
            return;
        };
        let kind = connection.kind();
        let catalog = self.store.read(cx).catalog(&t.connection_id);
        let console = t.console;
        let Some(c) = self.consoles.get_mut(&console) else { return };
        c.explain_gen += 1;
        let generation = c.explain_gen;
        c.last_explain = Some(t.clone());
        c.plan = match &c.plan {
            // The previous plan stays on screen, dimmed, until the new one arrives.
            PlanState::Ready(_) => c.plan.clone(),
            _ => PlanState::Loading,
        };
        if self.active_tab == Some(console) {
            if self.plan_owner == Some(console) {
                self.plan.update(cx, |p, cx| p.set_loading(true, cx));
            } else {
                self.show_plan_state(console, window, cx);
            }
            self.set_bottom_tab(BottomTab::Plan, cx);
        }
        let sql = t.sql.clone();
        cx.spawn_in(window, async move |this, cx| {
            let sql2 = sql.clone();
            let res = runtime::spawn_result(async move { connection.explain(&sql2, analyze).await }).await;
            this.update_in(cx, |this, window, cx| {
                let state = match res {
                    Ok(plan) => {
                        let (summary, insights) = theta_sql::analyze_plan(&plan, catalog.as_deref(), kind);
                        PlanState::Ready(PlanInput {
                            plan: Arc::new(plan),
                            summary,
                            insights,
                            kind,
                            statement: sql,
                        })
                    }
                    Err(e) => {
                        this.log(
                            &t.connection_id,
                            &t.sql,
                            MessageKind::Error,
                            vec![format!("Explain failed: {e}")],
                            Some(&e),
                            cx,
                        );
                        PlanState::Failed(e.to_string())
                    }
                };
                // Stored with the console that asked (it may have been closed meanwhile, or a
                // newer EXPLAIN may have started); shown only if that console is on screen.
                let Some(c) = this.consoles.get_mut(&console).filter(|c| c.explain_gen == generation) else {
                    return;
                };
                c.plan = state;
                if this.active_tab == Some(console) {
                    this.show_plan_state(console, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Make the Plan tab show console `id`'s plan state.
    pub(super) fn show_plan_state(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.consoles.get(&id).map(|c| c.plan.clone()).unwrap_or_default();
        match state {
            PlanState::Ready(input) => self.plan.update(cx, |p, cx| p.set_plan(input, window, cx)),
            PlanState::Loading => self.plan.update(cx, |p, cx| {
                p.clear(cx);
                p.set_loading(true, cx);
            }),
            PlanState::Failed(m) => self.plan.update(cx, |p, cx| {
                p.clear(cx);
                p.set_error(m, cx);
            }),
            PlanState::Empty => self.plan.update(cx, |p, cx| p.clear(cx)),
        }
        self.plan_owner = Some(id);
    }

    pub(super) fn on_plan_event(&mut self, e: PlanEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The panel shows the active console's plan, so its events belong to that console.
        let last = self.active_tab.and_then(|id| self.consoles.get(&id)).and_then(|c| c.last_explain.clone());
        match e {
            PlanEvent::OpenSuggestion(sql) => {
                let conn = last.map(|t| t.connection_id);
                self.open_console(conn, Some(sql), window, cx);
            }
            PlanEvent::CopyText(t) => cx.write_to_clipboard(ClipboardItem::new_string(t)),
            PlanEvent::Rerun { analyze } => {
                if let Some(t) = last {
                    if analyze {
                        self.explain_console(t.console, true, window, cx);
                    } else {
                        self.run_explain(t, false, window, cx);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ format

    pub(super) fn format_active(&mut self, selection: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_tab {
            self.format_console(id, selection, window, cx);
        }
    }

    fn format_console(&mut self, id: u64, selection: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.console_kind(id, cx);
        let s = Settings::get(cx).format.clone();
        let opts = FormatOptions {
            indent_width: s.indent,
            uppercase_keywords: s.uppercase_keywords,
            ..Default::default()
        };
        let Some(c) = self.consoles.get(&id) else { return };
        c.editor.update(cx, |e, cx| {
            e.update_editor(cx, |ed| {
                let sel = ed.primary();
                // Vim's `=` hands over the lines it formats as an ordinary (Normal mode) selection,
                // which `selected_text` does not report: there the range itself is the request.
                let has_selection = ed.selected_text().is_some()
                    || (ed.keymap() == theta_editor::Keymap::Vim && !sel.is_empty());
                if selection && has_selection {
                    let (from, to) = (sel.from(), sel.to());
                    let src = ed.slice(from..to);
                    let out = theta_sql::format(&src, kind, &opts);
                    ed.replace_range(from..to, out.trim_end());
                } else {
                    let before = ed.text();
                    let out = theta_sql::format(&before, kind, &opts);
                    let cursor = cursor_after_format(&before, &out, ed.cursor_of(&sel));
                    ed.set_text(&out);
                    // The old selection would span an arbitrary part of the re-flowed text (and
                    // "run selection" would then run a fragment): continue from the same token.
                    ed.set_selections(vec![theta_editor::Selection::point(cursor)], 0);
                }
            })
        });
    }

    // ------------------------------------------------------------ transactions

    pub(super) fn refresh_txn(
        &mut self,
        conn_id: String,
        connection: Arc<dyn Connection>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let open =
                runtime::spawn(async move { connection.in_transaction().await }).await.unwrap_or(false);
            this.update(cx, |this, cx| {
                if open {
                    this.txn.insert(conn_id);
                } else {
                    this.txn.remove(&conn_id);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn txn_open_for_active(&self, cx: &App) -> bool {
        self.active_connection_id(cx).is_some_and(|c| self.txn.contains(&c))
    }

    pub(super) fn txn_command(&mut self, sql: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab else {
            self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx);
            return;
        };
        if self.defer_until_connected(id, sql, window, cx, move |this, window, cx| {
            this.txn_command(sql, window, cx)
        }) {
            return;
        }
        let Some((conn_id, connection)) = self.connection_for(id, window, cx) else { return };
        if self.runs.contains_key(&conn_id) {
            self.notify(NoticeKind::Info, "A query is running on this connection", window, cx);
            return;
        }
        self.run_control(conn_id, connection, sql, window, cx);
    }

    fn run_control(
        &mut self,
        conn_id: String,
        connection: Arc<dyn Connection>,
        sql: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let c = connection.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = runtime::spawn_result(async move { c.execute(sql, &ExecOptions::default()).await }).await;
            this.update_in(cx, |this, _window, cx| {
                match r {
                    Ok(out) => {
                        this.log(
                            &conn_id,
                            sql,
                            MessageKind::Ok,
                            vec![format!("{sql} — OK · {}", fmt_ms(out.elapsed))],
                            None,
                            cx,
                        );
                        let n = this.conn_name(&conn_id, cx);
                        this.push_history(&n, sql, out.elapsed, None, None, cx);
                    }
                    Err(e) => {
                        this.log(&conn_id, sql, MessageKind::Error, vec![e.to_string()], Some(&e), cx);
                        this.set_bottom_tab(BottomTab::Messages, cx);
                    }
                }
                this.refresh_txn(conn_id.clone(), connection.clone(), cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn request_close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        // Unapplied table edits are never dropped silently.
        if self.table_tab_dirty(id, cx) {
            self.confirm_discard_table_tab(id, window, cx);
            return;
        }
        let conn = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone());
        let shared = conn.as_ref().is_some_and(|c| {
            self.tabs.iter().any(|t| t.id != id && t.connection_id.as_deref() == Some(c.as_str()))
        });
        if let Some(conn) = conn.filter(|c| self.txn.contains(c) && !shared)
            && let ConnState::Connected { connection, .. } = self.store.read(cx).state(&conn).clone()
        {
            let ws = cx.entity().downgrade();
            let (ws1, ws2) = (ws.clone(), ws.clone());
            let (c1, c2) = (conn.clone(), conn.clone());
            let (k1, k2) = (connection.clone(), connection.clone());
            confirm::open(
                window,
                cx,
                "Transaction still open",
                "This connection has an open transaction. Commit or roll it back before closing the console?",
                vec![],
                vec![
                    Choice::new("Roll Back and Close", Tone::Danger, move |window, cx| {
                        let (c, k) = (c2.clone(), k2.clone());
                        ws2.update(cx, |this, cx| {
                            this.run_control(c, k, "ROLLBACK", window, cx);
                            this.close_tab_now(id, window, cx);
                        })
                        .ok();
                    }),
                    Choice::new("Commit and Close", Tone::Primary, move |window, cx| {
                        let (c, k) = (c1.clone(), k1.clone());
                        ws1.update(cx, |this, cx| {
                            this.run_control(c, k, "COMMIT", window, cx);
                            this.close_tab_now(id, window, cx);
                        })
                        .ok();
                    }),
                ],
            );
            let _ = ws;
            return;
        }
        self.close_tab_now(id, window, cx);
    }

    pub(super) fn close_tab_now(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(id, cx);
        match self.active_tab {
            Some(a) => self.on_console_activated(a, window, cx),
            None => {
                self.apply_console(0, window, cx);
                self.editor_status = None;
            }
        }
        self.save_all_consoles(cx);
    }

    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all_consoles(cx);
        let dirty = self.dirty_table_tabs(cx);
        if !dirty.is_empty() {
            confirm::open(
                window,
                cx,
                "Quit with unapplied table changes?",
                format!(
                    "These tabs have changes that were not applied and are not saved: {}.",
                    dirty.join(", ")
                ),
                vec![],
                vec![Choice::new("Discard and Quit", Tone::Danger, |_, cx| cx.quit())],
            );
            return;
        }
        if self.txn.is_empty() {
            cx.quit();
            return;
        }
        let conns: Vec<(String, Arc<dyn Connection>)> = self
            .txn
            .iter()
            .filter_map(|c| match self.store.read(cx).state(c) {
                ConnState::Connected { connection, .. } => Some((c.clone(), connection.clone())),
                _ => None,
            })
            .collect();
        let c1 = conns.clone();
        confirm::open(
            window,
            cx,
            "Quit with an open transaction?",
            "A transaction is still open. Quitting rolls it back unless you commit it first.",
            vec![],
            vec![
                Choice::new("Commit and Quit", Tone::Primary, move |_, cx| {
                    let c = c1.clone();
                    runtime::spawn(async move {
                        for (_, k) in &c {
                            let _ = k.execute("COMMIT", &ExecOptions::default()).await;
                        }
                    });
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(Duration::from_millis(400)).await;
                        cx.update(|cx| cx.quit());
                    })
                    .detach();
                }),
                Choice::new("Roll Back and Quit", Tone::Danger, |_, cx| cx.quit()),
            ],
        );
    }

    // ------------------------------------------------------------------ files

    pub(super) fn save_console_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab else { return };
        let Some(text) = self.console_text(id, cx) else { return };
        let existing = self.consoles.get(&id).and_then(|c| c.file.clone());
        if let Some(path) = existing {
            self.write_console_file(id, path, text, window, cx);
            return;
        }
        let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let name = format!(
            "{}.sql",
            self.tabs
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.title.to_string())
                .unwrap_or_default()
                .replace(' ', "-")
        );
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            this.update_in(cx, |this, window, cx| this.write_console_file(id, path, text, window, cx)).ok();
        })
        .detach();
    }

    fn write_console_file(
        &mut self,
        id: u64,
        path: std::path::PathBuf,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match std::fs::write(&path, text) {
            Ok(()) => {
                if let Some(c) = self.consoles.get_mut(&id) {
                    c.file = Some(path.clone());
                    c.editor.update(cx, |e, cx| e.update_editor(cx, |ed| ed.mark_saved()));
                }
                if let Some(n) = path.file_name() {
                    self.set_tab_title(id, n.to_string_lossy().to_string(), cx);
                }
                self.set_tab_modified(id, false, cx);
                self.schedule_save(id, cx);
                self.notify(NoticeKind::Success, format!("Saved {}", path.display()), window, cx);
            }
            Err(e) => {
                self.notify(NoticeKind::Error, format!("Could not save {}: {e}", path.display()), window, cx)
            }
        }
    }

    pub(super) fn open_sql_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open SQL File".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            this.update_in(cx, |this, window, cx| {
                for path in paths {
                    this.open_sql_path(path, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn open_sql_path(&mut self, path: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let title = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "query.sql".into());
                let conn = self.active_connection_id(cx);
                let id =
                    self.create_console(consoles::new_key(), title, conn, text, 0, Some(path), window, cx);
                self.activate_tab(id, window, cx);
                self.save_all_consoles(cx);
            }
            Err(e) => {
                self.notify(NoticeKind::Error, format!("Could not open {}: {e}", path.display()), window, cx)
            }
        }
    }

    // ------------------------------------------------------------------ misc

    pub fn run_startup_sql(&mut self, sql: String) {
        self.startup = Some(Startup { sql });
    }

    pub(super) fn maybe_run_startup(&mut self, conn_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.startup.is_none() || !self.store.read(cx).state(conn_id).is_connected() {
            return;
        }
        if let Some(Startup { sql }) = self.startup.take() {
            let id = self.open_console(Some(conn_id.to_string()), Some(sql), window, cx);
            self.run_request(id, RunReq::All, window, cx);
        }
    }

    pub(super) fn on_store_event_extra(
        &mut self,
        ev: &StoreEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            StoreEvent::CatalogChanged(id) => {
                self.on_catalog_changed(id, cx);
                self.maybe_run_startup(id, window, cx);
                self.maybe_open_startup_import(id, window, cx);
                self.sync_table_editors(id, window, cx);
                self.maybe_open_startup_table(id, window, cx);
            }
            StoreEvent::StateChanged(id) => {
                self.on_catalog_changed(id, cx);
                if let ConnState::Connected { connection, catalog: None, .. } =
                    self.store.read(cx).state(id).clone()
                    && self.store.read(cx).catalog_error(id).is_some()
                {
                    let _ = connection;
                    self.maybe_run_startup(id, window, cx);
                }
            }
            _ => {}
        }
    }

    pub fn start_demo(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::demo::create_demo_database().await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    let id = store.update(cx, |s, cx| {
                        let existing = s
                            .configs()
                            .iter()
                            .find(|c| c.name == crate::demo::DEMO_NAME)
                            .map(|c| c.id.clone());
                        let id = existing.unwrap_or_else(|| {
                            s.add(crate::demo::demo_config(&crate::connections::new_connection_id()), cx)
                        });
                        s.connect(&id, cx);
                        id
                    });
                    this.set_active_connection(Some(id), cx);
                }
                Err(e) => eprintln!("theta: could not create the demo database: {e}"),
            })
            .ok();
        })
        .detach();
    }
}

#[allow(dead_code)]
fn _unused(_: Entity<()>, _: RwLock<()>) {}

/// The theme `arg` names: an exact name, else the only theme containing it (case-insensitively),
/// so `:theme tokyo` finds "Tokyo Night". `Err` carries the candidates when ambiguous (or none).
fn resolve_theme(names: &[String], arg: &str) -> Result<String, Vec<String>> {
    let want = arg.trim().to_lowercase();
    if let Some(n) = names.iter().find(|n| n.to_lowercase() == want) {
        return Ok(n.clone());
    }
    let hits: Vec<&String> = names.iter().filter(|n| n.to_lowercase().contains(&want)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Vec::new()),
        many => {
            // A prefix match on a unique theme wins over incidental substrings.
            let prefixed: Vec<&&String> =
                many.iter().filter(|n| n.to_lowercase().starts_with(&want)).collect();
            match prefixed.as_slice() {
                [one] => Ok((**one).clone()),
                _ => Err(many.iter().map(|n| (*n).clone()).collect()),
            }
        }
    }
}

#[cfg(test)]
mod theme_resolve_tests {
    use super::resolve_theme;

    #[test]
    fn exact_substring_and_ambiguous() {
        let names: Vec<String> =
            ["Theta Dark", "Theta Light", "Tokyo Night", "Catppuccin Latte", "Catppuccin Mocha"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(resolve_theme(&names, "tokyo"), Ok("Tokyo Night".into()));
        assert_eq!(resolve_theme(&names, "THETA LIGHT"), Ok("Theta Light".into()));
        assert_eq!(resolve_theme(&names, "mocha"), Ok("Catppuccin Mocha".into()));
        assert_eq!(resolve_theme(&names, "theta").unwrap_err().len(), 2);
        assert_eq!(resolve_theme(&names, "nope"), Err(vec![]));
    }
}

/// Where the cursor belongs after `before` was re-flowed into `after`: the position with the
/// same number of non-whitespace characters in front of it, since formatting only changes
/// whitespace and keyword case. `cursor` is a char index into `before`.
fn cursor_after_format(before: &str, after: &str, cursor: usize) -> usize {
    let wanted = before.chars().take(cursor).filter(|c| !c.is_whitespace()).count();
    let mut seen = 0;
    for (i, c) in after.chars().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        if seen == wanted {
            return i;
        }
        seen += 1;
    }
    after.chars().count()
}

#[cfg(test)]
mod format_cursor_tests {
    use super::cursor_after_format;

    #[test]
    fn cursor_follows_its_token() {
        let before = "select a,b from t where x=1";
        let after = "SELECT\n    a,\n    b\nFROM t\nWHERE x = 1";
        // On the `b`.
        let at = before.find('b').unwrap();
        let idx = cursor_after_format(before, after, at);
        assert_eq!(after.chars().nth(idx), Some('b'));
        assert_eq!(cursor_after_format(before, after, 0), 0);
        assert_eq!(cursor_after_format(before, after, before.len()), after.chars().count());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::ConnectionStore;
    use crate::connections::secrets::SecretStore;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use theta_core::ConnectionConfig;

    fn setup(
        cx: &mut TestAppContext,
        read_only: bool,
    ) -> (Entity<ConnectionStore>, Entity<Workspace>, &mut VisualTestContext, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        // Drivers wake GPUI tasks from Tokio threads; the test scheduler must allow that from the start.
        cx.executor().allow_parking();
        // Settings writes go to the config dir; keep tests away from the real one.
        // SAFETY: tests only ever point this at their own temporary directory.
        unsafe { std::env::set_var("THETA_CONFIG_DIR", dir.path().join("config")) };
        cx.update(|cx| {
            theta_ui::init(cx);
            crate::keymap::bind(cx);
        });
        let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
        let store = cx.new(|_| store);
        let mut config = ConnectionConfig::new("t1", "test", DatabaseKind::Sqlite);
        config.path = ":memory:".into();
        config.read_only = read_only;
        let id = store.update(cx, |s, cx| {
            let id = s.add_temporary(config, cx);
            s.connect(&id, cx);
            id
        });
        let (ws, vcx) = cx.add_window_view(|window, cx| {
            let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
            ws.quiet = true;
            ws
        });
        wait_for(vcx, |vcx| store.read_with(vcx, |s, _| s.state(&id).is_connected()));
        (store, ws, vcx, id, dir)
    }

    /// Database drivers run on a real Tokio runtime, so poll instead of parking.
    fn wait_for(vcx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        vcx.executor().allow_parking();
        for _ in 0..500 {
            vcx.run_until_parked();
            if done(vcx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for the condition");
    }

    #[gpui_kit::test]
    fn query_runs_end_to_end_into_the_grid(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, false);
        let tab = ws.update_in(vcx, |ws, window, cx| {
            let tab = ws.open_console(
                Some(id.clone()),
                Some("select 1 as a, 'x' as b union all select 2, 'y'".into()),
                window,
                cx,
            );
            ws.run_request(tab, RunReq::All, window, cx);
            tab
        });
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some()));
        ws.read_with(vcx, |ws, cx| {
            let grid = ws.results.read(cx).grid.read(cx);
            assert_eq!(grid.result().unwrap().rows.len(), 2);
            assert_eq!(ws.history.read(cx).len(), 1);
            assert!(ws.consoles[&tab].results[0].title.starts_with("Result 1"));
            assert!(ws.runs.is_empty(), "the run must have finished");
        });
    }

    #[gpui_kit::test]
    fn two_statements_give_two_result_sets_and_stop_at_error(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, false);
        let tab = ws.update_in(vcx, |ws, window, cx| {
            let tab = ws.open_console(
                Some(id.clone()),
                Some("select 1; select 2; select nope from nowhere; select 4".into()),
                window,
                cx,
            );
            ws.run_request(tab, RunReq::All, window, cx);
            tab
        });
        wait_for(vcx, |vcx| {
            ws.read_with(vcx, |ws, _| ws.runs.is_empty() && ws.consoles[&tab].summary.is_some())
        });
        ws.read_with(vcx, |ws, cx| {
            let c = &ws.consoles[&tab];
            assert_eq!(c.results.len(), 2, "the failing statement stops the run");
            assert!(c.summary.as_ref().unwrap().1, "the summary is an error");
            assert_eq!(ws.messages.read(cx).entries().len(), 3);
            assert_eq!(ws.history.read(cx).len(), 3);
        });
    }

    #[gpui_kit::test]
    fn parameters_are_asked_for_substituted_and_remembered(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, false);
        let tab = ws.update_in(vcx, |ws, window, cx| {
            ws.param_answer =
                Some(vec![theta_sql::params::ParamValue { ty: Default::default(), text: "41".into() }]);
            let tab =
                ws.open_console(Some(id.clone()), Some("select :n + 1 as r, '?' as lit".into()), window, cx);
            ws.run_request(tab, RunReq::All, window, cx);
            tab
        });
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some()));
        ws.read_with(vcx, |ws, cx| {
            let rs = ws.results.read(cx).grid.read(cx).result().unwrap().clone();
            assert_eq!(rs.rows[0][0], theta_core::Value::Int(42));
            // History records the substituted statement, the console remembers the value.
            assert_eq!(ws.history.read(cx).last_sql(), Some("select 41 + 1 as r, '?' as lit"));
            assert_eq!(ws.consoles[&tab].param_values[":n"].text, "41");
            let m = ws.messages.read(cx).entries();
            assert!(m.iter().any(|m| m.lines.iter().any(|l| l.contains(":n = 41"))));
        });
    }

    #[gpui_kit::test]
    fn parameters_setting_off_runs_text_as_is(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, false);
        vcx.update(|_, cx| theta_ui::Settings::update(cx, |s| s.editor.parameters = false));
        ws.update_in(vcx, |ws, window, cx| {
            let tab = ws.open_console(Some(id.clone()), Some("select 1 as r".into()), window, cx);
            ws.run_request(tab, RunReq::All, window, cx);
        });
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, cx| ws.results.read(cx).grid.read(cx).result().is_some()));
    }

    #[gpui_kit::test]
    fn manual_mode_begins_lazily_keeps_the_transaction_and_rolls_back(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, false);
        ws.update_in(vcx, |ws, window, cx| {
            ws.txn_prefs.insert(
                id.clone(),
                theta_core::TransactionConfig {
                    manual: true,
                    sqlite_begin: theta_core::SqliteBegin::Immediate,
                    ..Default::default()
                },
            );
            let tab = ws.open_console(
                Some(id.clone()),
                Some("create table t (a int); insert into t values (1)".into()),
                window,
                cx,
            );
            ws.run_request(tab, RunReq::All, window, cx);
        });
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, _| ws.runs.is_empty() && ws.txn.contains(&id)));
        ws.read_with(vcx, |ws, cx| {
            let m = ws.messages.read(cx).entries();
            assert!(
                m.iter().any(|m| m.lines.iter().any(|l| l.contains("Transaction started: BEGIN IMMEDIATE"))),
                "the lazy BEGIN is logged exactly once: {m:?}"
            );
            let begins =
                m.iter().filter(|m| m.lines.iter().any(|l| l.starts_with("Transaction started"))).count();
            assert_eq!(begins, 1, "the second statement joins the open transaction");
        });
        // A rollback undoes both statements: the table is gone again.
        ws.update_in(vcx, |ws, window, cx| ws.txn_command("ROLLBACK", window, cx));
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, _| !ws.txn.contains(&id)));
        ws.update_in(vcx, |ws, window, cx| {
            let check = ws.open_console(Some(id.clone()), Some("select count(*) from t".into()), window, cx);
            ws.run_request(check, RunReq::All, window, cx);
        });
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, _| ws.runs.is_empty()));
        ws.read_with(vcx, |ws, cx| {
            let last = ws.messages.read(cx).entries().last().unwrap().clone();
            assert!(last.lines.join(" ").contains("no such table"), "{last:?}");
        });
    }

    #[gpui_kit::test]
    fn auto_commit_mode_does_not_begin(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, false);
        ws.update_in(vcx, |ws, window, cx| {
            let tab = ws.open_console(Some(id.clone()), Some("create table t (a int)".into()), window, cx);
            ws.run_request(tab, RunReq::All, window, cx);
        });
        wait_for(vcx, |vcx| ws.read_with(vcx, |ws, _| ws.runs.is_empty()));
        ws.read_with(vcx, |ws, cx| {
            assert!(!ws.txn.contains(&id));
            assert!(
                !ws.messages
                    .read(cx)
                    .entries()
                    .iter()
                    .any(|m| m.lines.iter().any(|l| l.contains("Transaction started")))
            );
        });
    }

    #[gpui_kit::test]
    fn read_only_connection_refuses_writes(cx: &mut TestAppContext) {
        let (_, ws, vcx, id, _dir) = setup(cx, true);
        ws.update_in(vcx, |ws, window, cx| {
            let tab = ws.open_console(Some(id.clone()), Some("create table t (a int)".into()), window, cx);
            ws.run_request(tab, RunReq::All, window, cx);
        });
        vcx.run_until_parked();
        ws.read_with(vcx, |ws, cx| {
            assert!(ws.runs.is_empty());
            let m = ws.messages.read(cx).entries();
            assert_eq!(m.len(), 1);
            assert!(m[0].lines[0].contains("read-only"));
            assert_eq!(ws.history.read(cx).len(), 0, "a refused statement is not history");
        });
    }

    #[gpui_kit::test]
    fn consoles_persist_and_restore(cx: &mut TestAppContext) {
        let (store, ws, vcx, id, dir) = setup(cx, false);
        ws.update_in(vcx, |ws, window, cx| {
            ws.open_console(Some(id.clone()), Some("select 'kept'".into()), window, cx);
        });
        let saved = consoles::load_all(&dir.path().join("consoles"));
        assert!(saved.iter().any(|c| c.text == "select 'kept'" && c.connection_id.as_deref() == Some("t1")));
        // A second window over the same config dir restores it.
        let (ws2, vcx2) = cx.add_window_view(|window, cx| {
            let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
            ws.quiet = true;
            ws
        });
        ws2.read_with(vcx2, |ws, cx| {
            let texts: Vec<String> =
                ws.tabs().iter().map(|t| ws.consoles[&t.id].editor.read(cx).text()).collect();
            assert!(texts.iter().any(|t| t == "select 'kept'"), "{texts:?}");
            assert!(ws.tabs().iter().all(|t| t.connection_id.as_deref() == Some("t1")));
        });
    }
}

/// What happened to a statement whose session was lost.
pub(super) enum Recovery {
    /// The session was fine.
    None,
    /// Reconnected and ran the statement again; its result is the one shown.
    Reran,
    /// Reconnected, but the statement was not run again (the reason is for the user).
    NotRerun(String),
    /// Could not reconnect.
    ReconnectFailed(Error),
}

impl Recovery {
    fn message(&self) -> Option<String> {
        match self {
            Recovery::None => None,
            Recovery::Reran => Some("Connection lost — reconnected and re-ran the statement".into()),
            Recovery::NotRerun(why) => {
                Some(format!("Connection lost — reconnected. The statement was NOT re-run: {why}."))
            }
            Recovery::ReconnectFailed(e) => {
                Some(format!("Connection lost — reconnecting failed: {e}. The statement was NOT re-run."))
            }
        }
    }
}

/// What manual transaction mode needs to start a transaction before a statement.
#[derive(Clone)]
pub(super) struct ManualBegin {
    pub kind: DatabaseKind,
    pub statements: Vec<String>,
}

/// Whether a statement of this class can change what the explorer shows.
pub(super) fn changes_schema(class: StatementClass) -> bool {
    class == StatementClass::Ddl
}

pub(super) fn txn_statement_kind(class: StatementClass) -> theta_core::txn::StatementKind {
    use theta_core::txn::StatementKind as K;
    match class {
        StatementClass::Query => K::Query,
        StatementClass::Write => K::Write,
        StatementClass::Ddl => K::Ddl,
        StatementClass::Transaction => K::Transaction,
        StatementClass::Session | StatementClass::Other => K::Other,
    }
}

/// Manual mode: start the transaction before the first statement that needs one, unless the session
/// already has one (the user typed `BEGIN`, or an earlier statement started it). Returns the statements
/// that were run.
pub(super) async fn begin_transaction_if_needed(
    connection: &Arc<dyn Connection>,
    manual: Option<&ManualBegin>,
    p: &Picked,
) -> Result<Vec<String>, Error> {
    let Some(m) = manual else { return Ok(Vec::new()) };
    if !theta_core::txn::starts_transaction(txn_statement_kind(p.class)) {
        return Ok(Vec::new());
    }
    let (c, stmts) = (connection.clone(), m.statements.clone());
    runtime::spawn_result(async move {
        if c.in_transaction().await {
            return Ok(Vec::new());
        }
        for sql in &stmts {
            c.execute(sql, &ExecOptions::default()).await?;
        }
        Ok(stmts)
    })
    .await
}

pub(super) async fn exec_statement(
    connection: &Arc<dyn Connection>,
    p: &Picked,
    opts: &ExecOptions,
) -> Result<StatementOutcome, Error> {
    let (c, sql, o) = (connection.clone(), p.sql.clone(), opts.clone());
    runtime::spawn_result(async move { c.execute(&sql, &o).await }).await
}

/// Wait until the store has finished connecting `conn_id`: the live connection, or the
/// connect error. Polling keeps this independent of which event the store emits when.
async fn wait_connected(
    this: &gpui_kit::WeakEntity<Workspace>,
    cx: &mut gpui_kit::AsyncWindowContext,
    conn_id: &str,
) -> Result<Arc<dyn Connection>, Error> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let state = this
            .update(cx, |this, cx| this.store.read(cx).state(conn_id).clone())
            .map_err(|_| Error::Closed)?;
        match state {
            ConnState::Connected { connection, .. } => return Ok(connection),
            ConnState::Failed(e) => return Err(e),
            ConnState::Disconnected => return Err(Error::Connect("the connection was closed".into())),
            ConnState::Connecting if Instant::now() > deadline => {
                return Err(Error::Connect("timed out while connecting".into()));
            }
            ConnState::Connecting => {}
        }
        cx.background_executor().timer(Duration::from_millis(40)).await;
    }
}

/// The server ended the session under a running statement: mark it lost, reconnect at once
/// and, when that is safe, run the statement once more.
///
/// Re-running is only safe for a plain query outside a transaction: a write may or may not
/// have been applied before the connection died, and an open transaction's earlier statements
/// are gone with the session, so repeating just the last one would act on a different state.
#[allow(clippy::too_many_arguments)]
async fn recover_lost_session(
    this: &gpui_kit::WeakEntity<Workspace>,
    cx: &mut gpui_kit::AsyncWindowContext,
    conn_id: &str,
    old: Arc<dyn Connection>,
    p: &Picked,
    opts: &ExecOptions,
    txn_open: bool,
    first: Result<StatementOutcome, Error>,
) -> (Arc<dyn Connection>, Result<StatementOutcome, Error>, Recovery) {
    // The user may have disconnected on purpose while the statement ran (that is what made it
    // fail): do not undo that by reconnecting.
    let state = this.update(cx, |this, cx| this.store.read(cx).state(conn_id).clone());
    if matches!(state, Ok(ConnState::Disconnected)) {
        return (old, first, Recovery::None);
    }
    let reconnect = |cx: &mut gpui_kit::AsyncWindowContext| {
        let conn = conn_id.to_string();
        this.update(cx, |this, cx| {
            this.txn.remove(&conn);
            this.store.update(cx, |s, cx| {
                s.disconnect(&conn, cx);
                s.connect(&conn, cx);
            });
        })
    };
    if reconnect(cx).is_err() {
        return (old, first, Recovery::None);
    }
    let fresh = match wait_connected(this, cx, conn_id).await {
        Ok(c) => c,
        Err(e) => return (old, first, Recovery::ReconnectFailed(e)),
    };
    if txn_open {
        let why = "a transaction was open and its changes were lost with the session";
        return (fresh, first, Recovery::NotRerun(why.into()));
    }
    if p.class != StatementClass::Query {
        let why = "it changes data or session state, so running it again could apply it twice";
        return (fresh, first, Recovery::NotRerun(why.into()));
    }
    let second = exec_statement(&fresh, p, opts).await;
    if second.as_ref().is_err_and(Error::is_connection_lost) {
        // Lost again (e.g. the statement itself kills its backend): leave a healthy session.
        let _ = reconnect(cx);
        let fresh = wait_connected(this, cx, conn_id).await.unwrap_or(fresh);
        return (fresh, second, Recovery::NotRerun("it lost the connection again when re-run".into()));
    }
    (fresh, second, Recovery::Reran)
}

/// Text echoed back in a toast: a pasted novel as a `:` argument must not fill the screen.
fn shorten(text: &str) -> String {
    const MAX: usize = 48;
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX {
        return one_line;
    }
    let cut: String = one_line.chars().take(MAX).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod shorten_tests {
    use super::shorten;

    #[test]
    fn long_arguments_are_cut_and_whitespace_is_collapsed() {
        assert_eq!(shorten("tokyo   night"), "tokyo night");
        let long = "x".repeat(300);
        let s = shorten(&long);
        assert_eq!(s.chars().count(), 49);
        assert!(s.ends_with('…'));
        assert_eq!(shorten("é".repeat(60).as_str()).chars().count(), 49);
    }
}
