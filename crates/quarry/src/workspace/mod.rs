//! The application window: title bar, explorer, tabbed console area, bottom
//! panel and status bar.
//!
//! The editor, results grid, plan tree and map are separate crates mounted
//! later. This module defines the *seams* they plug into:
//!
//! * [`Workspace::open_console`] creates a tab and emits
//!   [`WorkspaceEvent::ConsoleOpened`]; the integrator answers with
//!   [`Workspace::set_console_view`].
//! * [`Workspace::set_bottom_view`] / [`Workspace::set_bottom_tab`] fill and
//!   switch the Results | Messages | Plan | Map | History panel.
//! * `set_*` status-bar setters feed the editor mode, cursor, selection and
//!   result-info slots.
//! * Run / cancel / explain / format actions are registered here and re-emitted
//!   as [`WorkspaceEvent`]s.

mod bottom;
pub mod console;
pub mod persist;
mod run;
mod status_bar;
mod tabs;
mod title_bar;
mod welcome;

#[cfg(test)]
mod isolation_tests;
#[cfg(test)]
mod shortcut_tests;

use crate::actions::*;
use crate::connections::{ConnState, ConnectionStore, NoticeKind, StoreEvent};
use crate::exec::history::HistoryStore;
use crate::explorer::{Explorer, ExplorerEvent};
use crate::palette;
use crate::panels::history::{HistoryEvent, HistoryPanel};
use crate::panels::messages::MessagesPanel;
use crate::panels::results::{ResultsEvent, ResultsPanel};
use crate::sqlgen;
use console::Console;
use gpui_kit::assets::IconName;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, DragMoveEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, deferred, div, px,
};
use persist::*;
use quarry_editor_view::EditorStatus;
use quarry_map_view::{MapEvent, MapPanel};
use quarry_plan_view::{PlanEvent, PlanPanel};
use quarry_ui::picker::{Picker, PickerDelegate, PickerEvent};
use quarry_ui::{STATUS_BAR_HEIGHT, Settings, TAB_BAR_HEIGHT, TITLE_BAR_HEIGHT, data_surface};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BottomTab {
    Results,
    Messages,
    Plan,
    Map,
    History,
}

impl BottomTab {
    pub const ALL: [BottomTab; 5] =
        [BottomTab::Results, BottomTab::Messages, BottomTab::Plan, BottomTab::Map, BottomTab::History];

    pub fn label(self) -> &'static str {
        match self {
            BottomTab::Results => "Results",
            BottomTab::Messages => "Messages",
            BottomTab::Plan => "Plan",
            BottomTab::Map => "Map",
            BottomTab::History => "History",
        }
    }
}

/// One open console tab.
#[derive(Clone)]
pub struct ConsoleTab {
    pub id: u64,
    pub title: SharedString,
    pub connection_id: Option<String>,
    pub modified: bool,
    pub initial_sql: Option<String>,
}

/// Values shown in the status bar's right-hand slots. `None` hides a slot.
#[derive(Clone, Default)]
pub struct StatusSlots {
    pub editor_mode: Option<SharedString>,
    /// 1-based line and column.
    pub cursor: Option<(usize, usize)>,
    pub selection_count: Option<usize>,
    /// e.g. "1,000 rows in 42 ms".
    pub result_info: Option<SharedString>,
    /// Transaction state text for the title bar, e.g. "Transaction open".
    pub transaction: Option<SharedString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Statement,
    All,
}

/// What the integrator listens for.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceEvent {
    ConsoleOpened { tab_id: u64, connection_id: Option<String>, initial_sql: Option<String> },
    ConsoleClosed { tab_id: u64 },
    ActiveConsoleChanged { tab_id: Option<u64> },
    ActiveConnectionChanged { connection_id: Option<String> },
    OpenTableRequested { connection_id: String, schema: String, table: String },
    BottomTabChanged(BottomTab),
    Run(RunKind),
    Cancel,
    Explain { analyze: bool },
    Format,
}

#[derive(Clone, Copy)]
enum Resize {
    Explorer,
    Bottom,
}

/// Drag payload for the panel resize handles (needs a view to render while dragging).
#[derive(Clone, Copy)]
struct ResizeDrag(Resize);

impl Render for ResizeDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_0()
    }
}

struct Modal {
    view: AnyView,
    restore_focus: Option<FocusHandle>,
    _subscription: Subscription,
}

pub struct Workspace {
    store: Entity<ConnectionStore>,
    explorer: Entity<Explorer>,
    focus: FocusHandle,
    tabs: Vec<ConsoleTab>,
    active_tab: Option<u64>,
    next_tab_id: u64,
    console_views: HashMap<u64, AnyView>,
    bottom_views: HashMap<BottomTab, AnyView>,
    state: WorkspaceState,
    active_connection: Option<String>,
    status: StatusSlots,
    running: bool,
    modal: Option<Modal>,
    layout_dirty: bool,
    tabs_scroll: gpui_kit::ScrollHandle,
    /// The tab last scrolled into view, so activation reveals it once.
    revealed_tab: Option<u64>,
    consoles: HashMap<u64, Console>,
    console_counter: usize,
    results: Entity<ResultsPanel>,
    messages: Entity<MessagesPanel>,
    history: Entity<HistoryPanel>,
    plan: Entity<PlanPanel>,
    map: Entity<MapPanel>,
    map_badge: Option<usize>,
    runs: HashMap<String, run::RunInfo>,
    txn: HashSet<String>,
    /// Consoles whose Run / Explain is waiting for their connection to come up.
    awaiting_connection: HashSet<u64>,
    /// Settings/theme warnings waiting for a window to show them in.
    startup_warnings: Vec<String>,
    ticker: Option<gpui_kit::Task<()>>,
    editor_status: Option<EditorStatus>,
    /// Console whose plan the Plan tab currently shows.
    plan_owner: Option<u64>,
    startup: Option<run::Startup>,
    /// Tests run in windows without a `Root`, where toasts cannot be shown.
    quiet: bool,
    data_dir: std::path::PathBuf,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(store: Entity<ConnectionStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_data_dir(store, quarry_ui::settings::config_dir(), window, cx)
    }

    /// Like [`Workspace::new`] with an explicit directory for consoles, history and layout.
    pub fn with_data_dir(
        store: Entity<ConnectionStore>,
        data_dir: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let explorer = cx.new(|cx| Explorer::new(store.clone(), window, cx));
        let results = cx.new(|cx| ResultsPanel::new(window, cx));
        let messages = cx.new(|_| MessagesPanel::new());
        let history = cx.new(|cx| {
            HistoryPanel::new(HistoryStore::load(Some(data_dir.join("history.jsonl"))), window, cx)
        });
        let plan = cx.new(|cx| PlanPanel::new(window, cx));
        let map = cx.new(|cx| MapPanel::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&store, window, Self::on_store_event),
            cx.subscribe_in(&explorer, window, Self::on_explorer_event),
            cx.subscribe_in(&results, window, |this, _, e: &ResultsEvent, window, cx| {
                let ResultsEvent::ActivateTab(i) = e;
                this.activate_result(*i, window, cx);
            }),
            cx.subscribe_in(&history, window, |this, _, e: &HistoryEvent, window, cx| {
                this.on_history_event(e.clone(), window, cx)
            }),
            cx.subscribe_in(&plan, window, |this, _, e: &PlanEvent, window, cx| {
                this.on_plan_event(e.clone(), window, cx)
            }),
            cx.subscribe_in(&map, window, |this, _, e: &MapEvent, window, cx| {
                this.on_map_event(e.clone(), window, cx)
            }),
            cx.observe_global::<quarry_ui::settings::SettingsStore>(|_, cx| cx.notify()),
        ];
        let focus = cx.focus_handle();
        let mut ws = Self {
            store,
            explorer,
            focus,
            tabs: Vec::new(),
            active_tab: None,
            next_tab_id: 1,
            console_views: HashMap::new(),
            bottom_views: HashMap::new(),
            state: WorkspaceState::load(&data_dir.join("workspace.json")),
            active_connection: None,
            status: StatusSlots::default(),
            running: false,
            modal: None,
            layout_dirty: false,
            tabs_scroll: gpui_kit::ScrollHandle::new(),
            revealed_tab: None,
            consoles: HashMap::new(),
            console_counter: 0,
            results: results.clone(),
            messages: messages.clone(),
            history: history.clone(),
            plan: plan.clone(),
            map: map.clone(),
            map_badge: None,
            runs: HashMap::new(),
            txn: HashSet::new(),
            awaiting_connection: HashSet::new(),
            startup_warnings: Vec::new(),
            ticker: None,
            editor_status: None,
            plan_owner: None,
            startup: None,
            quiet: false,
            data_dir,
            _subscriptions: subscriptions,
        };
        ws.bottom_views.insert(BottomTab::Results, results.into());
        ws.bottom_views.insert(BottomTab::Messages, messages.into());
        ws.bottom_views.insert(BottomTab::History, history.into());
        ws.bottom_views.insert(BottomTab::Plan, plan.into());
        ws.bottom_views.insert(BottomTab::Map, map.into());
        ws.active_connection = ws.store.read(cx).first_connected().map(str::to_string);
        ws.restore_consoles(window, cx);
        if ws.tabs.is_empty() && !ws.store.read(cx).configs().is_empty() {
            ws.open_console(None, None, window, cx);
        }
        // Problems found while loading settings/themes are shown once the window exists:
        // toasts live in the window's `Root`, which wraps this view only after it is built, so
        // pushing one from here panics.
        ws.startup_warnings = Settings::take_warnings(cx);
        ws
    }

    /// Show the settings/theme load warnings collected at start-up (once).
    pub fn show_startup_warnings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for warning in std::mem::take(&mut self.startup_warnings) {
            if !self.quiet {
                window.push_notification(Notification::warning(warning), cx);
            }
        }
    }

    // ---------------------------------------------------------------- seams

    pub fn store(&self) -> &Entity<ConnectionStore> {
        &self.store
    }

    pub fn explorer(&self) -> &Entity<Explorer> {
        &self.explorer
    }

    pub fn tabs(&self) -> &[ConsoleTab] {
        &self.tabs
    }

    pub fn active_console(&self) -> Option<&ConsoleTab> {
        let id = self.active_tab?;
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn bottom_tab(&self) -> BottomTab {
        self.state.bottom_tab
    }

    /// The connection new consoles use and the title bar shows: the explicit
    /// choice, else the active tab's, else the first connected one.
    pub fn active_connection_id(&self, cx: &App) -> Option<String> {
        let store = self.store.read(cx);
        let exists = |id: &String| store.config(id).is_some();
        self.active_console()
            .and_then(|t| t.connection_id.clone())
            .filter(exists)
            .or_else(|| self.active_connection.clone().filter(exists))
            .or_else(|| store.first_connected().map(str::to_string))
            .or_else(|| store.configs().first().map(|c| c.id.clone()))
    }

    pub fn set_active_connection(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_connection != id {
            self.active_connection = id.clone();
            cx.emit(WorkspaceEvent::ActiveConnectionChanged { connection_id: id });
            cx.notify();
        }
    }

    /// Open a console tab bound to `connection_id` (defaults to the active one).
    pub fn open_console(
        &mut self,
        connection_id: Option<String>,
        initial_sql: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let connection_id = connection_id.or_else(|| self.active_connection_id(cx));
        self.console_counter += 1;
        let title = format!("console {}", self.console_counter);
        let key = crate::exec::consoles::new_key();
        let id = self.create_console(
            key,
            title,
            connection_id,
            initial_sql.unwrap_or_default(),
            0,
            None,
            window,
            cx,
        );
        self.activate_tab(id, window, cx);
        self.save_all_consoles(cx);
        id
    }

    pub fn activate_tab(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.iter().any(|t| t.id == id) && self.active_tab != Some(id) {
            self.active_tab = Some(id);
            if let Some(c) = self.active_console().and_then(|t| t.connection_id.clone()) {
                self.active_connection = Some(c);
            }
            cx.emit(WorkspaceEvent::ActiveConsoleChanged { tab_id: Some(id) });
            self.on_console_activated(id, _window, cx);
        }
        cx.notify();
    }

    pub fn close_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else { return };
        // Closing must never silently lose work: the console goes on the reopen stack first.
        let saved = self.snapshot_console(id, cx);
        self.cancel_run_of(id, cx);
        self.tabs.remove(pos);
        self.console_views.remove(&id);
        if let Some(c) = self.consoles.remove(&id) {
            if let Some(saved) = saved {
                let _ = crate::exec::consoles::push_closed(
                    &crate::exec::consoles::closed_dir(&self.data_dir),
                    &saved,
                );
            }
            crate::exec::consoles::remove(&self.data_dir.join("consoles"), &c.key);
        }
        cx.emit(WorkspaceEvent::ConsoleClosed { tab_id: id });
        if self.active_tab == Some(id) {
            self.active_tab = self.tabs.get(pos.min(self.tabs.len().saturating_sub(1))).map(|t| t.id);
            cx.emit(WorkspaceEvent::ActiveConsoleChanged { tab_id: self.active_tab });
        }
        cx.notify();
    }

    pub fn set_console_view(&mut self, tab_id: u64, view: Option<AnyView>, cx: &mut Context<Self>) {
        match view {
            Some(v) => self.console_views.insert(tab_id, v),
            None => self.console_views.remove(&tab_id),
        };
        cx.notify();
    }

    pub fn set_bottom_view(&mut self, tab: BottomTab, view: Option<AnyView>, cx: &mut Context<Self>) {
        match view {
            Some(v) => self.bottom_views.insert(tab, v),
            None => self.bottom_views.remove(&tab),
        };
        cx.notify();
    }

    pub fn set_bottom_tab(&mut self, tab: BottomTab, cx: &mut Context<Self>) {
        if self.state.bottom_tab != tab {
            self.state.bottom_tab = tab;
            cx.emit(WorkspaceEvent::BottomTabChanged(tab));
            self.save_layout();
        }
        if !self.state.bottom_open {
            self.state.bottom_open = true;
            self.save_layout();
        }
        cx.notify();
    }

    pub fn set_tab_modified(&mut self, tab_id: u64, modified: bool, cx: &mut Context<Self>) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.modified = modified;
            cx.notify();
        }
    }

    pub fn set_tab_title(&mut self, tab_id: u64, title: impl Into<SharedString>, cx: &mut Context<Self>) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.title = title.into();
            cx.notify();
            // The title is part of what is restored on the next launch.
            self.schedule_save(tab_id, cx);
        }
    }

    /// Enables Cancel (and disables Run) in the title bar while a query runs.
    pub fn set_running(&mut self, running: bool, cx: &mut Context<Self>) {
        self.running = running;
        cx.notify();
    }

    pub fn set_editor_mode(&mut self, mode: Option<SharedString>, cx: &mut Context<Self>) {
        self.status.editor_mode = mode;
        cx.notify();
    }
    pub fn set_cursor_position(&mut self, position: Option<(usize, usize)>, cx: &mut Context<Self>) {
        self.status.cursor = position;
        cx.notify();
    }
    pub fn set_selection_count(&mut self, count: Option<usize>, cx: &mut Context<Self>) {
        self.status.selection_count = count;
        cx.notify();
    }
    pub fn set_result_info(&mut self, info: Option<SharedString>, cx: &mut Context<Self>) {
        self.status.result_info = info;
        cx.notify();
    }
    pub fn set_transaction_state(&mut self, state: Option<SharedString>, cx: &mut Context<Self>) {
        self.status.transaction = state;
        cx.notify();
    }

    /// Show a toast.
    pub fn notify(
        &self,
        kind: NoticeKind,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let message = message.into();
        // Toasts live in the window's Root; headless test windows have none.
        if self.quiet {
            return;
        }
        {
            let t = gpui_kit::component::Theme::global_mut(cx);
            t.notification.placement = gpui_kit::Anchor::BottomRight;
            t.notification.margins.bottom = STATUS_BAR_HEIGHT + px(12.);
            t.notification.margins.right = px(12.);
        }
        let note = match kind {
            NoticeKind::Info => Notification::info(message),
            NoticeKind::Success => Notification::success(message),
            NoticeKind::Error => Notification::error(message),
        };
        window.push_notification(note, cx);
    }

    // ------------------------------------------------------------ internals

    fn save_layout(&mut self) {
        self.state.save(&self.data_dir.join("workspace.json"));
        self.layout_dirty = false;
    }

    fn on_store_event(
        &mut self,
        _: &Entity<ConnectionStore>,
        event: &StoreEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_store_event_extra(event, window, cx);
        match event {
            StoreEvent::Notice { kind, message } => self.notify(*kind, message.clone(), window, cx),
            StoreEvent::StateChanged(id) => {
                if self.store.read(cx).state(id).is_connected() && self.active_connection.is_none() {
                    self.set_active_connection(Some(id.clone()), cx);
                }
                if self.store.read(cx).state(id).is_connected()
                    && self.tabs.is_empty()
                    && self.startup.is_none()
                {
                    self.open_console(Some(id.clone()), None, window, cx);
                }
                cx.notify();
            }
            StoreEvent::ListChanged | StoreEvent::CatalogChanged(_) => cx.notify(),
        }
    }

    fn on_explorer_event(
        &mut self,
        _: &Entity<Explorer>,
        event: &ExplorerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.clone() {
            ExplorerEvent::OpenTable { connection_id, schema, table } => {
                self.open_table(connection_id, schema, table, window, cx)
            }
            ExplorerEvent::ReturnFocus => self.focus_editor(window, cx),
            ExplorerEvent::NewConsole { connection_id, sql } => {
                self.open_console(Some(connection_id), sql, window, cx);
            }
            ExplorerEvent::AddConnection => self.open_connection_dialog(None, window, cx),
            ExplorerEvent::EditConnection(id) => self.open_connection_dialog(Some(id), window, cx),
            ExplorerEvent::DeleteConnection(id) => self.confirm_delete_connection(id, window, cx),
            ExplorerEvent::ConnectionActivated(id) => self.set_active_connection(Some(id), cx),
            ExplorerEvent::RelatedTables { connection_id, schema, table } => {
                let (catalog, kind) = {
                    let store = self.store.read(cx);
                    (store.catalog(&connection_id), store.config(&connection_id).map(|c| c.kind))
                };
                let (Some(catalog), Some(kind)) = (catalog, kind) else { return };
                let inferred = self.explorer.read(cx).inferred_for(&connection_id);
                let items = crate::exec::fk_ui::related_tables(&catalog, &inferred, &schema, &table, kind);
                let delegate =
                    palette::RelatedPicker::new(items, cx.entity().downgrade(), connection_id, table);
                self.open_picker(delegate, window, cx);
            }
        }
    }

    /// Double-click / Enter on a table: open a console pre-filled with a SELECT
    /// and announce the request so the results integration can run it.
    pub fn open_table(
        &mut self,
        connection_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sql = {
            let store = self.store.read(cx);
            let catalog = store.catalog(&connection_id);
            let kind = store.config(&connection_id).map(|c| c.kind);
            match (catalog, kind) {
                (Some(catalog), Some(kind)) => catalog
                    .find_table(Some(&schema), &table)
                    .map(|(_, t)| sqlgen::select(kind, &schema, t, catalog.default_schema.as_deref())),
                _ => None,
            }
        };
        let has_sql = sql.is_some();
        let id = self.open_console(Some(connection_id.clone()), sql, window, cx);
        self.set_tab_title(id, table.clone(), cx);
        // Opening a table means "show me its rows": the generated query is bounded by a LIMIT.
        if has_sql {
            self.run_request(id, run::RunReq::Statement, window, cx);
        }
        cx.emit(WorkspaceEvent::OpenTableRequested { connection_id, schema, table });
    }

    pub fn open_connection_dialog(
        &mut self,
        edit: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::connection::open(self.store.clone(), edit, cx.entity().downgrade(), window, cx);
    }

    fn confirm_delete_connection(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::button::ButtonVariant;
        let name = self.store.read(cx).config(&id).map(|c| c.name.clone()).unwrap_or_default();
        let store = self.store.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let store = store.clone();
            let id = id.clone();
            alert
                .title(format!("Delete “{name}”?"))
                .description("The saved connection and its stored password are removed. The database itself is not touched.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    store.update(cx, |s, cx| s.remove(&id, cx));
                    true
                })
        });
    }

    // ---------------------------------------------------------------- modal

    pub fn open_picker<D: PickerDelegate>(
        &mut self,
        delegate: D,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_modal(window, cx);
        let restore = window.focused(cx);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        let subscription = cx.subscribe_in(&picker, window, |this, _, _: &PickerEvent, window, cx| {
            this.close_modal(window, cx)
        });
        self.modal = Some(Modal { view: picker.into(), restore_focus: restore, _subscription: subscription });
        cx.notify();
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(modal) = self.modal.take() {
            match modal.restore_focus {
                Some(handle) => window.focus(&handle, cx),
                None => self.focus.focus(window, cx),
            }
            cx.notify();
        }
    }

    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = palette::CommandPalette::new(window);
        self.open_picker(delegate, window, cx);
    }

    fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(palette::ThemePicker::new(cx), window, cx);
    }

    fn open_connection_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = palette::ConnectionPicker::new(
            self.store.clone(),
            cx.entity().downgrade(),
            self.active_connection_id(cx),
        );
        self.open_picker(delegate, window, cx);
    }

    fn open_table_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_connection_id(cx) else {
            self.notify(NoticeKind::Info, "Add a connection first", window, cx);
            return;
        };
        if self.store.read(cx).catalog(&id).is_none() {
            self.notify(NoticeKind::Info, "Connect to a database to browse its tables", window, cx);
            return;
        }
        let delegate = palette::TablePicker::new(self.store.clone(), cx.entity().downgrade(), id);
        self.open_picker(delegate, window, cx);
    }

    /// Make `id` the active connection, connecting it first if needed.
    pub fn use_connection(&mut self, id: &str, cx: &mut Context<Self>) {
        self.set_active_connection(Some(id.to_string()), cx);
        self.rebind_active_console(id, cx);
        let needs_connect =
            matches!(self.store.read(cx).state(id), ConnState::Disconnected | ConnState::Failed(_));
        if needs_connect {
            self.store.update(cx, |s, cx| {
                if matches!(s.state(id), ConnState::Failed(_)) {
                    s.disconnect(id, cx);
                }
                s.connect(id, cx);
            });
        }
    }

    // ------------------------------------------------------------ app actions

    fn toggle_explorer(&mut self, cx: &mut Context<Self>) {
        self.state.explorer_open = !self.state.explorer_open;
        self.save_layout();
        cx.notify();
    }

    fn toggle_bottom(&mut self, cx: &mut Context<Self>) {
        self.state.bottom_open = !self.state.bottom_open;
        self.save_layout();
        cx.notify();
    }

    /// Zoom scales the editor and the results grid (the buffer font). The chrome is laid out in
    /// fixed-size units, so scaling only its text would leave it half-resized: it stays put.
    fn zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        Settings::update(cx, |s| {
            if delta == 0.0 {
                s.buffer_font_size = quarry_ui::Settings::default().buffer_font_size;
            } else {
                s.buffer_font_size = (s.buffer_font_size + delta).clamp(8.0, 32.0);
            }
        });
    }

    pub fn set_keymap(&mut self, keymap: quarry_ui::KeymapMode, window: &mut Window, cx: &mut Context<Self>) {
        Settings::update(cx, |s| s.keymap = keymap);
        self.notify(NoticeKind::Info, format!("Keymap: {}", keymap.label()), window, cx);
    }

    fn open_keymap_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(palette::KeymapPicker::new(cx.entity().downgrade()), window, cx);
    }

    fn open_settings_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = Settings::path(cx);
        if !path.exists() {
            // Create it from the current settings so there is something to edit.
            if let Err(e) = Settings::get(cx).save(&path) {
                self.notify(
                    NoticeKind::Error,
                    format!("Could not create {}: {e}", path.display()),
                    window,
                    cx,
                );
                return;
            }
        }
        cx.open_with_system(&path);
    }

    fn import_zed_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Zed Theme".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update_in(cx, |this, window, cx| match quarry_ui::import_zed_theme_file(&path, cx) {
                Ok(names) => {
                    let list = names.join(", ");
                    this.notify(NoticeKind::Success, format!("Imported {list}"), window, cx);
                    this.open_theme_picker(window, cx);
                }
                Err(e) => this.notify(NoticeKind::Error, format!("Could not import theme: {e}"), window, cx),
            })
            .ok();
        })
        .detach();
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let cur =
            self.active_tab.and_then(|a| self.tabs.iter().position(|t| t.id == a)).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(self.tabs.len() as isize) as usize;
        let id = self.tabs[next].id;
        self.activate_tab(id, window, cx);
    }

    fn max_bottom_height(&self, window: &Window) -> f32 {
        let total: f32 = window.viewport_size().height.into();
        (total
            - f32::from(TITLE_BAR_HEIGHT)
            - f32::from(STATUS_BAR_HEIGHT)
            - f32::from(TAB_BAR_HEIGHT)
            - 140.0)
            .max(MIN_BOTTOM_HEIGHT)
    }

    fn on_resize(&mut self, event: &DragMoveEvent<ResizeDrag>, window: &mut Window, cx: &mut Context<Self>) {
        let drag = event.drag(cx).0;
        match drag {
            Resize::Explorer => {
                let x: f32 = event.event.position.x.into();
                self.state.explorer_width = x.clamp(MIN_EXPLORER_WIDTH, MAX_EXPLORER_WIDTH);
            }
            Resize::Bottom => {
                let y: f32 = event.event.position.y.into();
                let total: f32 = window.viewport_size().height.into();
                let h = total - f32::from(STATUS_BAR_HEIGHT) - y;
                self.state.bottom_height = h.clamp(MIN_BOTTOM_HEIGHT, self.max_bottom_height(window));
            }
        }
        self.layout_dirty = true;
        cx.notify();
    }
}

fn resize_handle(id: &'static str, which: Resize, vertical: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let line = if vertical {
        div().absolute().top_0().bottom_0().left(px(-3.)).w(px(7.)).cursor_col_resize()
    } else {
        div().absolute().left_0().right_0().top(px(-3.)).h(px(7.)).cursor_row_resize()
    };
    let accent = theme.drag_border;
    div()
        .relative()
        .flex_shrink_0()
        .when(vertical, |d| d.w(px(1.)).h_full())
        .when(!vertical, |d| d.h(px(1.)).w_full())
        .bg(theme.border)
        .child(
            line.id(id)
                .on_drag(ResizeDrag(which), |drag, _, _, cx| cx.new(|_| *drag))
                .hover(move |s| s.bg(accent.opacity(0.35))),
        )
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let explorer_open = self.state.explorer_open;
        let bottom_open = self.state.bottom_open;
        let bottom_height = self.state.bottom_height.min(self.max_bottom_height(window));
        let explorer_width = self.state.explorer_width;
        self.explorer.update(cx, |e, cx| e.set_width(explorer_width, cx));
        let title_bar = self.render_title_bar(window, cx);
        let tab_bar = self.render_tab_bar(window, cx);
        let console = self.render_console_area(window, cx);
        let bottom = bottom_open.then(|| self.render_bottom_panel(window, cx));
        let status = self.render_status_bar(window, cx);

        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_size(px(13.))
            .on_action(
                cx.listener(|this, _: &OpenCommandPalette, window, cx| this.open_command_palette(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenTablePicker, window, cx| this.open_table_picker(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenConnectionPicker, window, cx| {
                this.open_connection_picker(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ConnectTo, window, cx| this.open_connection_picker(window, cx)))
            .on_action(cx.listener(|this, _: &SelectTheme, window, cx| this.open_theme_picker(window, cx)))
            .on_action(cx.listener(|this, _: &NewConsole, window, cx| {
                this.open_console(None, None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
                this.open_connection_dialog(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Disconnect, _, cx| {
                if let Some(id) = this.active_connection_id(cx) {
                    this.store.update(cx, |s, cx| s.disconnect(&id, cx));
                }
            }))
            .on_action(cx.listener(
                |this, _: &RefreshSchema, window, cx| match this.active_connection_id(cx) {
                    Some(id) if this.store.read(cx).state(&id).is_connected() => {
                        this.store.update(cx, |s, cx| s.refresh_catalog(&id, cx))
                    }
                    _ => this.notify(
                        NoticeKind::Info,
                        "Connect to a database to refresh its schema",
                        window,
                        cx,
                    ),
                },
            ))
            .on_action(cx.listener(|this, _: &ToggleExplorer, _, cx| this.toggle_explorer(cx)))
            .on_action(cx.listener(|this, _: &ToggleBottomPanel, _, cx| this.toggle_bottom(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(id) = this.active_tab {
                    this.request_close_tab(id, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &ReopenClosedTab, window, cx| this.reopen_closed_tab(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &FocusExplorer, window, cx| {
                if !this.state.explorer_open {
                    this.toggle_explorer(cx);
                }
                this.explorer.update(cx, |e, cx| e.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &FocusEditor, window, cx| this.focus_editor(window, cx)))
            .on_action(cx.listener(|this, _: &FocusResults, window, cx| this.focus_results(window, cx)))
            .on_action(cx.listener(|this, _: &SelectKeymap, window, cx| this.open_keymap_picker(window, cx)))
            .on_action(cx.listener(|this, _: &UseHelixKeymap, window, cx| {
                this.set_keymap(quarry_ui::KeymapMode::Helix, window, cx)
            }))
            .on_action(cx.listener(|this, _: &UseVimKeymap, window, cx| {
                this.set_keymap(quarry_ui::KeymapMode::Vim, window, cx)
            }))
            .on_action(cx.listener(|this, _: &UseStandardKeymap, window, cx| {
                this.set_keymap(quarry_ui::KeymapMode::Standard, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ImportZedTheme, window, cx| this.import_zed_theme(window, cx)))
            .on_action(
                cx.listener(|this, _: &OpenSettingsFile, window, cx| this.open_settings_file(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1.0, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(-1.0, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom(0.0, cx)))
            .on_action(cx.listener(|this, _: &Quit, window, cx| this.request_quit(window, cx)))
            .on_action(
                cx.listener(|this, _: &BeginTransaction, window, cx| this.txn_command("BEGIN", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CommitTransaction, window, cx| this.txn_command("COMMIT", window, cx)),
            )
            .on_action(cx.listener(|this, _: &RollbackTransaction, window, cx| {
                this.txn_command("ROLLBACK", window, cx)
            }))
            .on_action(
                cx.listener(|_, _: &OpenSettings, window, cx| crate::dialogs::settings::open(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenSqlFile, window, cx| this.open_sql_file(window, cx)))
            .on_action(cx.listener(|this, _: &SaveConsole, window, cx| this.save_console_as(window, cx)))
            .on_action(cx.listener(|this, _: &WelcomeDemo, window, cx| this.start_demo(window, cx)))
            .on_action(cx.listener(|this, _: &RunStatement, window, cx| {
                cx.emit(WorkspaceEvent::Run(RunKind::Statement));
                this.run_active(run::RunReq::Statement, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RunAll, window, cx| {
                cx.emit(WorkspaceEvent::Run(RunKind::All));
                this.run_active(run::RunReq::All, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CancelQuery, window, cx| {
                cx.emit(WorkspaceEvent::Cancel);
                this.cancel_active(window, cx)
            }))
            .on_action(cx.listener(|this, _: &Explain, window, cx| {
                cx.emit(WorkspaceEvent::Explain { analyze: false });
                this.explain_active(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExplainAnalyze, window, cx| {
                cx.emit(WorkspaceEvent::Explain { analyze: true });
                this.explain_active(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatDocument, window, cx| {
                cx.emit(WorkspaceEvent::Format);
                this.format_active(false, window, cx)
            }))
            .on_drag_move(
                cx.listener(|this, e: &DragMoveEvent<ResizeDrag>, window, cx| this.on_resize(e, window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if this.layout_dirty {
                        this.save_layout();
                    }
                }),
            )
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .when(explorer_open, |this| {
                        this.child(
                            div()
                                .w(px(self.state.explorer_width))
                                .flex_shrink_0()
                                .h_full()
                                .overflow_hidden()
                                .bg(theme.sidebar)
                                .child(self.explorer.clone()),
                        )
                        .child(resize_handle(
                            "resize-explorer",
                            Resize::Explorer,
                            true,
                            cx,
                        ))
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(tab_bar)
                            .child(div().flex_1().min_h_0().bg(data_surface(cx)).child(console))
                            .when_some(bottom, |this, panel| {
                                this.child(resize_handle("resize-bottom", Resize::Bottom, false, cx)).child(
                                    div()
                                        .h(px(bottom_height))
                                        .flex_shrink_0()
                                        .overflow_hidden()
                                        .bg(data_surface(cx))
                                        .child(panel),
                                )
                            }),
                    ),
            )
            // Deferred so it paints above the toast layer: toasts slide 96 px as they leave and
            // would otherwise be seen sliding over the status bar.
            .child(deferred(status).priority(1))
            .when_some(self.modal.as_ref().map(|m| m.view.clone()), |this, view| this.child(view))
    }
}

impl Workspace {
    pub fn icon_for_bottom(tab: BottomTab) -> IconName {
        match tab {
            BottomTab::Results => IconName::Table,
            BottomTab::Messages => IconName::MessageSquare,
            BottomTab::Plan => IconName::Network,
            BottomTab::Map => IconName::Map,
            BottomTab::History => IconName::Clock,
        }
    }
}
