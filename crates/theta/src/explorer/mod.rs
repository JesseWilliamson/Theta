//! The explorer panel: connections ▸ schemas ▸ groups ▸ tables ▸ columns/keys.
//!
//! [`tree`] holds the pure model (flattening, filtering); this module is the
//! view: a virtualised list of 24px rows, a filter box, keyboard navigation and
//! context menus. It never opens editors itself: it emits [`ExplorerEvent`]s
//! that the workspace turns into tabs.

pub mod tree;

use crate::actions::*;
use crate::connections::{ConnState, ConnectionStore, StoreEvent};
use crate::sqlgen;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, KeyContext, MouseButton,
    MouseDownEvent, ParentElement as _, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, px, uniform_list,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use theta_core::{DatabaseKind, Table};
use theta_sql::VirtualForeignKey;
use theta_ui::{DbIcon, DbKindIcon, EmptyState, PanelHeader, ROW_HEIGHT, Settings, tool_button};
use tree::*;

#[derive(Debug, Clone, PartialEq)]
pub enum ExplorerEvent {
    OpenTable {
        connection_id: String,
        schema: String,
        table: String,
    },
    /// Server administration, from the connection's menu.
    OpenSessions {
        connection_id: String,
    },
    OpenRoles {
        connection_id: String,
    },
    ExportDatabase {
        connection_id: String,
    },
    RunScript {
        connection_id: String,
    },
    /// Search Across Tables, scoped to what was clicked: the connection, a schema or one table.
    SearchTables {
        connection_id: String,
        schema: Option<String>,
        table: Option<String>,
    },
    /// An ER diagram of a schema, or of a table and its neighbours.
    ShowDiagram {
        connection_id: String,
        schema: String,
        table: Option<String>,
    },
    /// Compare the rows of this table (as the source) with another table's.
    CompareData {
        connection_id: String,
        schema: String,
        table: String,
    },
    /// Compare this schema (as the source) with another.
    CompareSchema {
        connection_id: String,
        schema: String,
    },
    /// Open a new console, optionally pre-filled (generated SQL, DDL).
    NewConsole {
        connection_id: String,
        sql: Option<String>,
    },
    /// Show tables reachable through real and inferred foreign keys.
    RelatedTables {
        connection_id: String,
        schema: String,
        table: String,
    },
    /// Import a CSV/TSV file into this table.
    ImportData {
        connection_id: String,
        schema: String,
        table: String,
    },
    RenameTable {
        connection_id: String,
        schema: String,
        table: String,
    },
    TruncateTable {
        connection_id: String,
        schema: String,
        table: String,
    },
    DropTable {
        connection_id: String,
        schema: String,
        table: String,
    },
    DuplicateStructure {
        connection_id: String,
        schema: String,
        table: String,
    },
    /// Copy the table, with or without its rows, to another schema or connection.
    CopyTable {
        connection_id: String,
        schema: String,
        table: String,
    },
    /// Open the editor on an existing table; `column` puts the cursor on it.
    ModifyTable {
        connection_id: String,
        schema: String,
        table: String,
        column: Option<String>,
    },
    /// Open the editor on a table that does not exist yet.
    NewTable {
        connection_id: String,
        schema: String,
    },
    RenameColumn {
        connection_id: String,
        schema: String,
        table: String,
        column: String,
    },
    DropColumn {
        connection_id: String,
        schema: String,
        table: String,
        column: String,
    },
    /// Open the editor on the table's Indexes or Foreign Keys with a new row.
    NewIndex {
        connection_id: String,
        schema: String,
        table: String,
    },
    NewForeignKey {
        connection_id: String,
        schema: String,
        table: String,
    },
    DropIndex {
        connection_id: String,
        schema: String,
        table: String,
        index: String,
    },
    DropForeignKey {
        connection_id: String,
        schema: String,
        table: String,
        name: String,
    },
    /// Declare a suggested (inferred) foreign key for real.
    AddSuggestedForeignKey {
        connection_id: String,
        schema: String,
        table: String,
        column: String,
        referenced_schema: String,
        referenced_table: String,
        referenced_column: String,
    },
    /// Import a file as a new table.
    ImportNewTable {
        connection_id: String,
    },
    /// Export this table to a file.
    ExportData {
        connection_id: String,
        schema: String,
        table: String,
    },
    AddConnection,
    /// Ask for a name and create a connection folder.
    NewFolder,
    RenameFolder(String),
    DeleteFolder(String),
    /// Choose the folder of this connection (a picker).
    MoveToFolder(String),
    EditConnection(String),
    DeleteConnection(String),
    /// The user selected/used a connection: make it the active one.
    ConnectionActivated(String),
    /// Escape in the tree: hand keyboard focus back to the editor.
    ReturnFocus,
}

pub struct Explorer {
    store: Entity<ConnectionStore>,
    filter: Entity<InputState>,
    focus: FocusHandle,
    rows: Vec<Row>,
    expanded: HashSet<NodeKey>,
    /// Connections whose first catalog already opened the default schema.
    defaults_applied: HashSet<String>,
    selected: Option<NodeKey>,
    /// Inferred foreign keys per connection, keyed by the catalog they were computed from.
    inferred: HashMap<String, (usize, Arc<Vec<VirtualForeignKey>>)>,
    filter_text: String,
    scroll: UniformListScrollHandle,
    /// Panel width in px, used to wrap long error messages into detail rows.
    width: f32,
    /// A row context menu is open: tooltips would cover it, so rows render without them.
    menu_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn db_icon(kind: DatabaseKind) -> DbIcon {
    DbIcon::from_key(kind.info().icon)
}

impl Explorer {
    pub fn new(store: Entity<ConnectionStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter objects"));
        let mut subscriptions = vec![
            cx.subscribe(&store, Self::on_store_event),
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.filter_text = input.read(cx).value().to_string();
                    this.rebuild(cx);
                    if this.selected.is_none()
                        || !this.rows.iter().any(|r| Some(&r.key) == this.selected.as_ref())
                    {
                        this.selected = this.rows.first().map(|r| r.key.clone());
                    }
                }
                InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                _ => {}
            }),
        ];
        // Re-render when the keymap setting changes (modal keys on/off).
        subscriptions.push(cx.observe_global::<theta_ui::settings::SettingsStore>(|_, cx| cx.notify()));
        let mut explorer = Self {
            store,
            filter,
            focus: cx.focus_handle(),
            rows: Vec::new(),
            expanded: HashSet::new(),
            defaults_applied: HashSet::new(),
            inferred: HashMap::new(),
            selected: None,
            filter_text: String::new(),
            scroll: UniformListScrollHandle::new(),
            width: 280.0,
            menu_open: false,
            _subscriptions: subscriptions,
        };
        explorer.rebuild(cx);
        explorer
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| f.focus(window, cx));
    }

    fn on_store_event(&mut self, _: Entity<ConnectionStore>, event: &StoreEvent, cx: &mut Context<Self>) {
        match event {
            StoreEvent::StateChanged(id) => {
                let state = self.store.read(cx).state(id).clone();
                if matches!(state, ConnState::Connecting | ConnState::Failed(_)) {
                    self.expanded.insert(NodeKey::Connection(id.clone()));
                }
                if matches!(state, ConnState::Connected { .. }) {
                    self.expanded.insert(NodeKey::Connection(id.clone()));
                }
                if matches!(state, ConnState::Disconnected) {
                    self.defaults_applied.remove(id);
                }
            }
            StoreEvent::CatalogChanged(id) => {
                if self.defaults_applied.insert(id.clone())
                    && let Some(catalog) = self.store.read(cx).catalog(id)
                {
                    self.expanded.extend(default_expansion(id, &catalog));
                }
            }
            StoreEvent::ListChanged => {}
            StoreEvent::Notice { .. } => return,
        }
        self.rebuild(cx);
    }

    fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.menu_open != open {
            self.menu_open = open;
            cx.notify();
        }
    }

    /// Tell the explorer how wide it is drawn so error text can be wrapped to fit.
    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if (self.width - width).abs() >= 4.0 {
            self.width = width;
            self.rebuild(cx);
        }
    }

    /// Characters of 12 px text that fit beside the status-row icon (text starts 60 px in).
    fn wrap_cols(&self) -> usize {
        (((self.width - 60.0 - 12.0) / 6.3).floor().max(12.0)) as usize
    }

    /// Recompute the flat row list. Called on every model change; cheap because
    /// only expanded nodes are visited.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let catalogs: Vec<_> = store.configs().iter().map(|c| store.catalog(&c.id)).collect();
        for (c, cat) in store.configs().iter().zip(&catalogs) {
            match cat {
                Some(cat) => {
                    let ptr = Arc::as_ptr(cat) as usize;
                    if self.inferred.get(&c.id).is_none_or(|(p, _)| *p != ptr) {
                        let v = theta_sql::infer_foreign_keys(cat);
                        self.inferred.insert(c.id.clone(), (ptr, Arc::new(v)));
                    }
                }
                None => {
                    self.inferred.remove(&c.id);
                }
            }
        }
        let empty: Vec<VirtualForeignKey> = Vec::new();
        let views: Vec<ConnView> = store
            .configs()
            .iter()
            .zip(&catalogs)
            .map(|(c, catalog)| ConnView {
                folder: store.layout().folder_of.get(&c.id).map(String::as_str),
                id: &c.id,
                name: &c.name,
                kind: c.kind,
                color: c.color.as_deref(),
                inferred: self.inferred.get(&c.id).map(|(_, v)| v.as_slice()).unwrap_or(&empty),
                phase: match store.state(&c.id) {
                    ConnState::Disconnected => Phase::Disconnected,
                    ConnState::Connecting => Phase::Connecting,
                    ConnState::Failed(e) => Phase::Failed(failure_text(e, &c.id, store)),
                    ConnState::Connected { catalog_loading, .. } => Phase::Connected {
                        catalog: catalog.as_deref(),
                        loading: *catalog_loading,
                        catalog_error: store.catalog_error(&c.id).map(|e| e.to_string()),
                    },
                },
            })
            .collect();
        let folders: Vec<FolderView> = store
            .layout()
            .folders
            .iter()
            .map(|f| FolderView { id: &f.id, name: &f.name, collapsed: f.collapsed })
            .collect();
        self.rows = flatten_grouped(&folders, &views, &self.expanded, &self.filter_text, self.wrap_cols());
        if self.selected.as_ref().is_none_or(|k| !self.rows.iter().any(|r| &r.key == k)) {
            self.selected = self.rows.first().map(|r| r.key.clone());
        }
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn selected_key(&self) -> Option<&NodeKey> {
        self.selected.as_ref()
    }

    /// Expand the path to a table and select it (after a table was created or changed).
    pub fn reveal_table(&mut self, conn: &str, schema: &str, table: &str, cx: &mut Context<Self>) {
        for key in [
            NodeKey::Connection(conn.to_string()),
            NodeKey::Schema(conn.to_string(), schema.to_string()),
            NodeKey::Group(conn.to_string(), schema.to_string(), GroupKind::Tables),
        ] {
            self.expanded.insert(key);
        }
        self.selected = Some(NodeKey::Table(conn.to_string(), schema.to_string(), table.to_string()));
        self.rebuild(cx);
        if let Some(ix) = self.selected_index() {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.key == key)
    }

    fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            self.selected = Some(row.key.clone());
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            if let NodeKey::Connection(id) = &row.key {
                cx.emit(ExplorerEvent::ConnectionActivated(id.clone()));
            }
            cx.notify();
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let ix = self.selected_index().map(|i| (i + 1).min(self.rows.len().saturating_sub(1))).unwrap_or(0);
        self.select_index(ix, cx);
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let ix = self.selected_index().map(|i| i.saturating_sub(1)).unwrap_or(0);
        self.select_index(ix, cx);
    }

    fn set_expanded(&mut self, key: &NodeKey, open: bool, cx: &mut Context<Self>) {
        // A folder's state is part of the saved connection layout, not of this view.
        if let NodeKey::Folder(id) = key {
            self.store.update(cx, |s, cx| s.set_folder_collapsed(id, !open, cx));
            return;
        }
        if open {
            self.expanded.insert(key.clone());
        } else {
            self.expanded.remove(key);
        }
        self.rebuild(cx);
    }

    fn toggle(&mut self, key: &NodeKey, cx: &mut Context<Self>) {
        let open = self.rows.iter().find(|r| &r.key == key).is_some_and(|r| r.expanded);
        // A filter forces expansion; toggling then only collapses what the user opened.
        self.set_expanded(key, !open, cx);
    }

    pub fn expand_or_descend(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index() else { return };
        let row = self.rows[ix].clone();
        if row.expandable && !row.expanded {
            self.set_expanded(&row.key, true, cx);
        } else if row.expandable && self.rows.get(ix + 1).is_some_and(|r| r.depth > row.depth) {
            self.select_index(ix + 1, cx);
        }
    }

    pub fn collapse_or_ascend(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index() else { return };
        let row = self.rows[ix].clone();
        if row.expandable && row.expanded {
            self.set_expanded(&row.key, false, cx);
        } else if let Some(parent) = parent_index(&self.rows, ix) {
            self.select_index(parent, cx);
        }
    }

    /// `alt-up` / `alt-down`: move the selected connection (within its folder) or folder.
    pub fn move_selected(&mut self, delta: i32, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(NodeKey::Connection(id)) => self.store.update(cx, |s, cx| s.move_connection(&id, delta, cx)),
            Some(NodeKey::Folder(id)) => self.store.update(cx, |s, cx| s.move_folder(&id, delta, cx)),
            _ => {}
        }
    }

    /// `f2`: rename the selected folder.
    pub fn rename_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(NodeKey::Folder(id)) = self.selected.clone() {
            cx.emit(ExplorerEvent::RenameFolder(id));
        }
    }

    /// `delete`: delete the selected folder (after a confirmation).
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(NodeKey::Folder(id)) = self.selected.clone() {
            cx.emit(ExplorerEvent::DeleteFolder(id));
        }
    }

    /// Enter / double-click: the default action of the selected row.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index() else { return };
        self.activate_row(ix, cx);
    }

    fn activate_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else { return };
        match &row.key {
            NodeKey::Table(c, s, t) => cx.emit(ExplorerEvent::OpenTable {
                connection_id: c.clone(),
                schema: s.clone(),
                table: t.clone(),
            }),
            NodeKey::Connection(id) => {
                let state = self.store.read(cx).state(id).clone();
                if matches!(state, ConnState::Disconnected | ConnState::Failed(_)) {
                    self.store.update(cx, |s, cx| s.connect(id, cx));
                } else {
                    self.toggle(&row.key, cx);
                }
            }
            NodeKey::Status(id) if row.retry => self.retry(id, cx),
            _ if row.expandable => self.toggle(&row.key, cx),
            _ => {}
        }
    }

    fn retry(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            if matches!(s.state(id), ConnState::Failed(_)) {
                s.disconnect(id, cx);
                s.connect(id, cx);
            } else {
                s.refresh_catalog(id, cx);
            }
        });
    }

    fn table_of(
        &self,
        cx: &App,
        c: &str,
        schema: &str,
        table: &str,
    ) -> Option<(Table, Option<String>, DatabaseKind)> {
        let store = self.store.read(cx);
        let catalog = store.catalog(c)?;
        let kind = store.config(c)?.kind;
        let (_, t) = catalog.find_table(Some(schema), table)?;
        Some((t.clone(), catalog.default_schema.clone(), kind))
    }

    fn generate(&mut self, key: &NodeKey, what: Generate, cx: &mut Context<Self>) {
        let NodeKey::Table(c, s, t) = key else { return };
        let Some((table, default, kind)) = self.table_of(cx, c, s, t) else { return };
        let d = default.as_deref();
        let sql = match what {
            Generate::Select => sqlgen::select(kind, s, &table, d),
            Generate::Insert => sqlgen::insert(kind, s, &table, d),
            Generate::Update => sqlgen::update(kind, s, &table, d),
            Generate::Delete => sqlgen::delete(kind, s, &table, d),
            Generate::Ddl => sqlgen::ddl(kind, s, &table, d),
        };
        cx.emit(ExplorerEvent::NewConsole { connection_id: c.clone(), sql: Some(sql) });
    }

    /// The ALTER TABLE (or JOIN query) for a suggested foreign key leaf.
    fn suggested_sql(&self, key: &NodeKey, join: bool, cx: &App) -> Option<String> {
        let NodeKey::Leaf(c, s, t, Section::Suggested, name) = key else { return None };
        let (_, list) = self.inferred.get(c)?;
        let v =
            list.iter().find(|v| &v.schema == s && &v.table == t && &tree::suggested_leaf_name(v) == name)?;
        let store = self.store.read(cx);
        let kind = store.config(c)?.kind;
        let catalog = store.catalog(c)?;
        let default = catalog.default_schema.as_deref();
        Some(if join {
            crate::exec::fk_ui::join_sql(
                kind,
                default,
                (&v.schema, &v.table),
                (&v.referenced_schema, &v.referenced_table),
                &[(v.column.clone(), v.referenced_column.clone())],
            )
        } else {
            crate::exec::fk_ui::alter_table_sql(kind, default, v)
        })
    }

    /// The event that accepts the suggested foreign key a leaf stands for.
    fn suggested_event(&self, key: &NodeKey) -> Option<ExplorerEvent> {
        let NodeKey::Leaf(c, s, t, Section::Suggested, name) = key else { return None };
        let (_, list) = self.inferred.get(c)?;
        let v =
            list.iter().find(|v| &v.schema == s && &v.table == t && &tree::suggested_leaf_name(v) == name)?;
        Some(ExplorerEvent::AddSuggestedForeignKey {
            connection_id: c.clone(),
            schema: s.clone(),
            table: t.clone(),
            column: v.column.clone(),
            referenced_schema: v.referenced_schema.clone(),
            referenced_table: v.referenced_table.clone(),
            referenced_column: v.referenced_column.clone(),
        })
    }

    /// The inferred foreign keys of a connection (empty until its catalog loads).
    pub fn inferred_for(&self, conn: &str) -> Arc<Vec<VirtualForeignKey>> {
        self.inferred.get(conn).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    fn copy(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn qualified_copy(&self, key: &NodeKey, cx: &App) -> Option<String> {
        let NodeKey::Table(c, s, t) = key else { return None };
        let store = self.store.read(cx);
        let kind = store.config(c)?.kind;
        let default = store.catalog(c).and_then(|cat| cat.default_schema.clone());
        Some(sqlgen::qualified(kind, s, t, default.as_deref()))
    }

    fn context_menu_for(
        explorer: WeakEntity<Explorer>,
        key: NodeKey,
        state: Option<ConnState>,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        // One item that re-enters the explorer when clicked.
        fn item(
            explorer: &WeakEntity<Explorer>,
            label: &'static str,
            icon: IconName,
            f: impl Fn(&mut Explorer, &mut Context<Explorer>) + 'static,
        ) -> PopupMenuItem {
            let explorer = explorer.clone();
            PopupMenuItem::new(label).icon(Icon::new(icon)).on_click(move |_, _, cx| {
                explorer.update(cx, |this, cx| f(this, cx)).ok();
            })
        }
        match key.clone() {
            NodeKey::Connection(id) => {
                let connected = state.as_ref().is_some_and(|s| s.is_connected());
                let mut menu = menu;
                if connected {
                    let id1 = id.clone();
                    menu =
                        menu.item(item(&explorer, "New Console", IconName::SquareTerminal, move |_, cx| {
                            cx.emit(ExplorerEvent::NewConsole { connection_id: id1.clone(), sql: None });
                        }));
                    let id2 = id.clone();
                    menu = menu.item(item(&explorer, "Refresh", IconName::RefreshCw, move |this, cx| {
                        this.store.update(cx, |s, cx| s.refresh_catalog(&id2, cx));
                    }));
                    let id3 = id.clone();
                    menu = menu.item(item(&explorer, "Disconnect", IconName::Unplug, move |this, cx| {
                        this.store.update(cx, |s, cx| s.disconnect(&id3, cx));
                    }));
                    // Entries a product cannot do stay in the menu; choosing one says why.
                    let (a, b, c, d, e) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
                    menu = menu
                        .separator()
                        .item(item(&explorer, "Sessions", IconName::Activity, move |_, cx| {
                            cx.emit(ExplorerEvent::OpenSessions { connection_id: a.clone() });
                        }))
                        .item(item(&explorer, "Roles and Users", IconName::Users, move |_, cx| {
                            cx.emit(ExplorerEvent::OpenRoles { connection_id: b.clone() });
                        }))
                        .item(item(
                            &explorer,
                            "Export Database…",
                            IconName::HardDriveDownload,
                            move |_, cx| {
                                cx.emit(ExplorerEvent::ExportDatabase { connection_id: c.clone() });
                            },
                        ))
                        .item(item(&explorer, "Run SQL Script…", IconName::FileUp, move |_, cx| {
                            cx.emit(ExplorerEvent::RunScript { connection_id: d.clone() });
                        }))
                        .item(item(&explorer, "Search Across Tables…", IconName::Search, move |_, cx| {
                            cx.emit(ExplorerEvent::SearchTables {
                                connection_id: e.clone(),
                                schema: None,
                                table: None,
                            });
                        }));
                } else {
                    let id1 = id.clone();
                    menu = menu.item(item(&explorer, "Connect", IconName::Plug, move |this, cx| {
                        this.retry_or_connect(&id1, cx);
                    }));
                }
                let (id1, id2, id3) = (id.clone(), id.clone(), id.clone());
                let (m1, m2, m3) = (id.clone(), id.clone(), id.clone());
                menu.separator()
                    .item(item(&explorer, "Move to Folder…", IconName::Folder, move |_, cx| {
                        cx.emit(ExplorerEvent::MoveToFolder(m1.clone()));
                    }))
                    .item(item(&explorer, "Move Up", IconName::ArrowUp, move |this, cx| {
                        this.store.update(cx, |s, cx| s.move_connection(&m2, -1, cx));
                    }))
                    .item(item(&explorer, "Move Down", IconName::ArrowDown, move |this, cx| {
                        this.store.update(cx, |s, cx| s.move_connection(&m3, 1, cx));
                    }))
                    .separator()
                    .item(item(&explorer, "Edit…", IconName::Settings2, move |_, cx| {
                        cx.emit(ExplorerEvent::EditConnection(id1.clone()));
                    }))
                    .item(item(&explorer, "Duplicate", IconName::Copy, move |this, cx| {
                        this.store.update(cx, |s, cx| {
                            s.duplicate(&id2, cx);
                        });
                    }))
                    .item(item(&explorer, "Delete…", IconName::Trash, move |_, cx| {
                        cx.emit(ExplorerEvent::DeleteConnection(id3.clone()));
                    }))
            }
            NodeKey::Folder(id) => {
                let (r, d, u, dn) = (id.clone(), id.clone(), id.clone(), id.clone());
                menu.item(item(&explorer, "New Folder…", IconName::FolderPlus, |_, cx| {
                    cx.emit(ExplorerEvent::NewFolder);
                }))
                .item(item(&explorer, "Rename…", IconName::Pencil, move |_, cx| {
                    cx.emit(ExplorerEvent::RenameFolder(r.clone()));
                }))
                .separator()
                .item(item(&explorer, "Move Up", IconName::ArrowUp, move |this, cx| {
                    this.store.update(cx, |s, cx| s.move_folder(&u, -1, cx));
                }))
                .item(item(&explorer, "Move Down", IconName::ArrowDown, move |this, cx| {
                    this.store.update(cx, |s, cx| s.move_folder(&dn, 1, cx));
                }))
                .separator()
                .item(item(
                    &explorer,
                    "Delete Folder…",
                    IconName::Trash,
                    move |_, cx| {
                        cx.emit(ExplorerEvent::DeleteFolder(d.clone()));
                    },
                ))
            }
            NodeKey::Table(c, s, t) => {
                let (c1, s1, t1) = (c.clone(), s.clone(), t.clone());
                let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
                menu.item(item(&explorer, "Open Table", IconName::Table, move |_, cx| {
                    cx.emit(ExplorerEvent::OpenTable {
                        connection_id: c1.clone(),
                        schema: s1.clone(),
                        table: t1.clone(),
                    });
                }))
                .separator()
                .item(item(&explorer, "Copy Name", IconName::Copy, {
                    let t = t.clone();
                    move |this, cx| this.copy(t.clone(), cx)
                }))
                .item(item(&explorer, "Copy Qualified Name", IconName::Copy, move |this, cx| {
                    if let Some(q) = this.qualified_copy(&k1, cx) {
                        this.copy(q, cx);
                    }
                }))
                .separator()
                .submenu_with_icon(Some(Icon::new(IconName::SquareTerminal)), "Generate", window, cx, {
                    let explorer = explorer.clone();
                    move |menu, _, _| {
                        let mut menu = menu;
                        for (label, what) in [
                            ("SELECT", Generate::Select),
                            ("INSERT", Generate::Insert),
                            ("UPDATE", Generate::Update),
                            ("DELETE", Generate::Delete),
                        ] {
                            let k = k2.clone();
                            menu = menu.item(item(
                                &explorer,
                                label,
                                IconName::SquareTerminal,
                                move |this, cx| this.generate(&k, what, cx),
                            ));
                        }
                        menu
                    }
                })
                .item(item(&explorer, "Show DDL", IconName::FileCode, move |this, cx| {
                    this.generate(&k3, Generate::Ddl, cx);
                }))
                .separator()
                .item(item(&explorer, "Search in Table…", IconName::Search, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::SearchTables {
                            connection_id: c.clone(),
                            schema: Some(s.clone()),
                            table: Some(t.clone()),
                        })
                    }
                }))
                .item(item(&explorer, "ER Diagram…", IconName::Network, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::ShowDiagram {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: Some(t.clone()),
                        })
                    }
                }))
                .item(item(&explorer, "Compare Data…", IconName::Diff, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::CompareData {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .item(item(&explorer, "Related Tables…", IconName::Link, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::RelatedTables {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .separator()
                .item(item(&explorer, "Modify Table…", IconName::Settings2, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::ModifyTable {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                            column: None,
                        })
                    }
                }))
                .item(item(&explorer, "Rename…", IconName::Pencil, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::RenameTable {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .item(item(&explorer, "Copy Table to…", IconName::Copy, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::CopyTable {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .item(item(&explorer, "Duplicate Structure…", IconName::Copy, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::DuplicateStructure {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .item(item(&explorer, "Truncate…", IconName::Eraser, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::TruncateTable {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .item(item(&explorer, "Drop…", IconName::Trash, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::DropTable {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .separator()
                .item(item(&explorer, "Import Data…", IconName::FileDown, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::ImportData {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
                .item(item(&explorer, "Export Data…", IconName::FileDown, {
                    let (c, s, t) = (c.clone(), s.clone(), t.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::ExportData {
                            connection_id: c.clone(),
                            schema: s.clone(),
                            table: t.clone(),
                        })
                    }
                }))
            }
            NodeKey::Group(c, schema, GroupKind::Tables) => {
                let c2 = c.clone();
                menu.item(item(&explorer, "New Table…", IconName::Plus, move |_, cx| {
                    cx.emit(ExplorerEvent::NewTable { connection_id: c2.clone(), schema: schema.clone() });
                }))
                .item(item(
                    &explorer,
                    "Import as New Table…",
                    IconName::FileDown,
                    move |_, cx| {
                        cx.emit(ExplorerEvent::ImportNewTable { connection_id: c.clone() });
                    },
                ))
            }
            NodeKey::Section(c, s, t, Section::Indexes) => {
                menu.item(item(&explorer, "New Index…", IconName::Plus, move |_, cx| {
                    cx.emit(ExplorerEvent::NewIndex {
                        connection_id: c.clone(),
                        schema: s.clone(),
                        table: t.clone(),
                    });
                }))
            }
            NodeKey::Section(c, s, t, Section::ForeignKeys) => {
                menu.item(item(&explorer, "New Foreign Key…", IconName::Plus, move |_, cx| {
                    cx.emit(ExplorerEvent::NewForeignKey {
                        connection_id: c.clone(),
                        schema: s.clone(),
                        table: t.clone(),
                    });
                }))
            }
            NodeKey::Leaf(c, s, t, Section::Indexes, name) => {
                menu.item(item(&explorer, "Drop Index…", IconName::Trash, move |_, cx| {
                    cx.emit(ExplorerEvent::DropIndex {
                        connection_id: c.clone(),
                        schema: s.clone(),
                        table: t.clone(),
                        index: name.clone(),
                    });
                }))
            }
            NodeKey::Leaf(c, s, t, Section::ForeignKeys, name) => {
                menu.item(item(&explorer, "Drop Foreign Key…", IconName::Trash, move |_, cx| {
                    cx.emit(ExplorerEvent::DropForeignKey {
                        connection_id: c.clone(),
                        schema: s.clone(),
                        table: t.clone(),
                        name: name.clone(),
                    });
                }))
            }
            NodeKey::Column(c, s, t, name) => {
                let (c1, s1, t1, n1) = (c.clone(), s.clone(), t.clone(), name.clone());
                let (c2, s2, t2, n2) = (c.clone(), s.clone(), t.clone(), name.clone());
                let n3 = name.clone();
                menu.item(item(&explorer, "Modify Column…", IconName::Settings2, move |_, cx| {
                    cx.emit(ExplorerEvent::ModifyTable {
                        connection_id: c1.clone(),
                        schema: s1.clone(),
                        table: t1.clone(),
                        column: Some(n1.clone()),
                    });
                }))
                .item(item(&explorer, "Rename Column…", IconName::Pencil, move |_, cx| {
                    cx.emit(ExplorerEvent::RenameColumn {
                        connection_id: c2.clone(),
                        schema: s2.clone(),
                        table: t2.clone(),
                        column: n2.clone(),
                    });
                }))
                .item(item(&explorer, "Drop Column…", IconName::Trash, move |_, cx| {
                    cx.emit(ExplorerEvent::DropColumn {
                        connection_id: c.clone(),
                        schema: s.clone(),
                        table: t.clone(),
                        column: name.clone(),
                    });
                }))
                .separator()
                .item(item(&explorer, "Copy Name", IconName::Copy, move |this, cx| this.copy(n3.clone(), cx)))
            }
            NodeKey::Leaf(c, s, t, Section::Suggested, name) => {
                let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
                let _ = (c, s, t, name);
                menu.item(item(&explorer, "Add as Foreign Key…", IconName::Link, move |this, cx| {
                    if let Some(ev) = this.suggested_event(&k3) {
                        cx.emit(ev);
                    }
                }))
                .separator()
                .item(item(
                    &explorer,
                    "Copy ALTER TABLE … ADD FOREIGN KEY",
                    IconName::Copy,
                    move |this, cx| {
                        if let Some(sql) = this.suggested_sql(&k1, false, cx) {
                            this.copy(sql, cx);
                        }
                    },
                ))
                .item(item(
                    &explorer,
                    "Generate JOIN Query",
                    IconName::SquareTerminal,
                    move |this, cx| {
                        if let Some(sql) = this.suggested_sql(&k2, true, cx) {
                            let c = k2.connection_id().to_string();
                            cx.emit(ExplorerEvent::NewConsole { connection_id: c, sql: Some(sql) });
                        }
                    },
                ))
            }
            NodeKey::Schema(c, name) => {
                let (copy, schema) = (name.clone(), name.clone());
                menu.item(item(&explorer, "Copy Name", IconName::Copy, move |this, cx| {
                    this.copy(copy.clone(), cx)
                }))
                .item(item(&explorer, "Search in Schema…", IconName::Search, {
                    let (c, schema) = (c.clone(), schema.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::SearchTables {
                            connection_id: c.clone(),
                            schema: Some(schema.clone()),
                            table: None,
                        });
                    }
                }))
                .item(item(&explorer, "ER Diagram…", IconName::Network, {
                    let (c, schema) = (c.clone(), schema.clone());
                    move |_, cx| {
                        cx.emit(ExplorerEvent::ShowDiagram {
                            connection_id: c.clone(),
                            schema: schema.clone(),
                            table: None,
                        });
                    }
                }))
                .item(item(
                    &explorer,
                    "Compare Schema…",
                    IconName::Diff,
                    move |_, cx| {
                        cx.emit(ExplorerEvent::CompareSchema {
                            connection_id: c.clone(),
                            schema: schema.clone(),
                        });
                    },
                ))
            }
            NodeKey::Sequence(_, _, name) | NodeKey::Routine(_, _, name) => {
                menu.item(item(&explorer, "Copy Name", IconName::Copy, move |this, cx| {
                    this.copy(name.clone(), cx)
                }))
            }
            NodeKey::Status(id) | NodeKey::Detail(id, _) => {
                let (i1, i2, i3) = (id.clone(), id.clone(), id.clone());
                menu.item(item(&explorer, "Retry", IconName::RefreshCw, move |this, cx| this.retry(&i1, cx)))
                    .item(item(&explorer, "Edit Connection…", IconName::Settings2, move |_, cx| {
                        cx.emit(ExplorerEvent::EditConnection(i2.clone()));
                    }))
                    .item(item(&explorer, "Copy Error", IconName::Copy, move |this, cx| {
                        if let Some(text) = this.error_text(&i3, cx) {
                            this.copy(text, cx);
                        }
                    }))
            }
            _ => menu,
        }
    }

    /// The full error of a connection that failed to connect or to load its schema.
    fn error_text(&self, id: &str, cx: &App) -> Option<String> {
        let store = self.store.read(cx);
        match store.state(id) {
            ConnState::Failed(e) => Some(e.to_string()),
            _ => store.catalog_error(id).map(|e| e.to_string()),
        }
    }

    fn retry_or_connect(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            if matches!(s.state(id), ConnState::Failed(_)) {
                s.disconnect(id, cx);
            }
            s.connect(id, cx);
        });
    }

    fn render_row(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let row = &self.rows[ix];
        let selected = self.selected.as_ref() == Some(&row.key);
        let focused = self.focus.contains_focused(window, cx);
        let key = row.key.clone();
        let depth = row.depth as f32;
        const INDENT: f32 = 14.0;
        const BASE: f32 = 8.0;

        if let NodeKey::Detail(..) = row.key {
            // Continuation line of an error: aligned with the text of the status row above it.
            let menu_key = row.key.clone();
            let explorer = cx.entity().downgrade();
            let state = None;
            return h_flex()
                .id(SharedString::from(format!("row-{ix}")))
                .h(ROW_HEIGHT)
                .w_full()
                .flex_shrink_0()
                .pl(px(BASE + depth * INDENT + 38.))
                .pr_2()
                .items_center()
                .cursor_default()
                .bg(theme.danger.opacity(0.06))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .text_color(theme.danger.opacity(0.9))
                        .child(row.label.clone()),
                )
                .context_menu(move |menu, window, cx| {
                    Explorer::context_menu_for(
                        explorer.clone(),
                        menu_key.clone(),
                        state.clone(),
                        menu,
                        window,
                        cx,
                    )
                })
                .into_any_element();
        }

        let label_color = match (&row.icon, row.is_error) {
            (_, true) => theme.danger,
            (RowIcon::Column | RowIcon::PrimaryKeyColumn | RowIcon::ForeignKeyColumn, _) => theme.foreground,
            (RowIcon::Group(_), _) => theme.muted_foreground,
            _ if row.muted => theme.muted_foreground,
            _ => theme.foreground,
        };
        let ranges = theta_ui::fuzzy::highlight_ranges(&row.label, &row.highlights);
        let styled = StyledText::new(row.label.clone()).with_highlights(ranges.into_iter().map(|r| {
            (
                r,
                HighlightStyle {
                    color: Some(theme.ring),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            )
        }));

        let chevron: gpui_kit::AnyElement = if row.expandable {
            let icon = if row.expanded { IconName::ChevronDown } else { IconName::ChevronRight };
            let toggle_key = key.clone();
            div()
                .id(SharedString::from(format!("chev-{ix}")))
                .size(px(16.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle(&toggle_key, cx);
                }))
                .child(Icon::new(icon).size(px(12.)).text_color(theme.muted_foreground))
                .into_any_element()
        } else {
            div().size(px(16.)).flex_shrink_0().into_any_element()
        };

        let icon_el: gpui_kit::AnyElement = match row.icon {
            RowIcon::Connection(kind) => {
                let dim = matches!(row.dot, Some(ConnDot::Disconnected | ConnDot::Failed));
                DbKindIcon::new(db_icon(kind))
                    .size(px(14.))
                    .when(dim, |i| i.color(theme.muted_foreground))
                    .into_any_element()
            }
            RowIcon::Spinner => Spinner::new().xsmall().color(theme.muted_foreground).into_any_element(),
            other => {
                let (name, color) = match other {
                    RowIcon::Folder => (IconName::Folder, theme.muted_foreground),
                    RowIcon::FolderOpen => (IconName::FolderOpen, theme.muted_foreground),
                    RowIcon::Schema => (IconName::FolderTree, theme.muted_foreground),
                    RowIcon::Group(tree::GroupKind::Tables) => (IconName::Table, theme.muted_foreground),
                    RowIcon::Group(tree::GroupKind::Views) => (IconName::Eye, theme.muted_foreground),
                    RowIcon::Group(tree::GroupKind::Routines) => {
                        (IconName::SquareFunction, theme.muted_foreground)
                    }
                    RowIcon::Group(tree::GroupKind::Sequences) => {
                        (IconName::ListOrdered, theme.muted_foreground)
                    }
                    RowIcon::Table => (IconName::Table2, theme.blue),
                    RowIcon::View => (IconName::Eye, theme.cyan),
                    RowIcon::MaterializedView => (IconName::Layers, theme.cyan),
                    RowIcon::ForeignTable => (IconName::Globe, theme.muted_foreground),
                    RowIcon::Column => (IconName::Columns2, theme.muted_foreground.opacity(0.8)),
                    RowIcon::PrimaryKeyColumn => (IconName::Key, theme.yellow),
                    RowIcon::ForeignKeyColumn => (IconName::Link, theme.blue),
                    RowIcon::Key => (IconName::Key, theme.yellow),
                    RowIcon::ForeignKey => (IconName::Link, theme.blue),
                    RowIcon::SuggestedForeignKey => (IconName::Unlink, theme.blue.opacity(0.55)),
                    RowIcon::Index => (IconName::ListTree, theme.muted_foreground),
                    RowIcon::Routine => (IconName::SquareFunction, theme.magenta),
                    RowIcon::Sequence => (IconName::ListOrdered, theme.muted_foreground),
                    RowIcon::Error => (IconName::CircleAlert, theme.danger),
                    RowIcon::Info => (IconName::Info, theme.muted_foreground),
                    RowIcon::Connection(_) | RowIcon::Spinner | RowIcon::None => unreachable!(),
                };
                Icon::new(name).size(px(14.)).text_color(color).into_any_element()
            }
        };

        let state = match &row.key {
            NodeKey::Connection(id) => Some(self.store.read(cx).state(id).clone()),
            _ => None,
        };
        let is_connection = matches!(row.key, NodeKey::Connection(_));
        let menu_key = row.key.clone();
        let explorer = cx.entity().downgrade();
        let click_key = key.clone();
        let tint = row.color.as_deref().and_then(parse_hex_color);

        h_flex()
            .id(SharedString::from(format!("row-{ix}")))
            .h(ROW_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .relative()
            .pl(px(BASE + depth * INDENT))
            .pr_2()
            .gap_1()
            .items_center()
            .cursor_default()
            .when(selected, |this| {
                this.bg(if focused { theme.list_active } else { theme.list_active.opacity(0.55) })
            })
            .when(!selected, |this| this.hover(|s| s.bg(theme.list_hover)))
            .when_some(row.tooltip.clone().filter(|_| !self.menu_open), |this, t| {
                this.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(window, cx)
                })
            })
            // Indent guides: one hairline per ancestor level.
            .children((0..row.depth as usize).map(|level| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(BASE + level as f32 * INDENT + 7.5))
                    .w(px(1.))
                    .bg(theme.border.opacity(if selected { 0.0 } else { 0.55 }))
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let key = key.clone();
                    move |this, _: &MouseDownEvent, _, cx| {
                        this.selected = Some(key.clone());
                        cx.notify();
                    }
                }),
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.focus.focus(window, cx);
                let Some(ix) = this.rows.iter().position(|r| r.key == click_key) else { return };
                this.select_index(ix, cx);
                if e.click_count() >= 2 {
                    this.activate_row(ix, cx);
                } else if !matches!(click_key, NodeKey::Table(..) | NodeKey::Connection(..))
                    && this.rows[ix].expandable
                {
                    this.toggle(&click_key, cx);
                }
            }))
            .child(chevron)
            .when_some(tint, |this, c| {
                this.child(div().w(px(3.)).h(px(12.)).rounded(px(1.5)).flex_shrink_0().bg(c))
            })
            .child(icon_el)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .items_center()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w_full()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .when(is_connection || matches!(row.icon, RowIcon::Group(_)) && false, |d| {
                                d.font_weight(FontWeight::MEDIUM)
                            })
                            .text_color(label_color)
                            .child(styled),
                    )
                    .when_some(row.type_text.clone(), |this, t| {
                        this.child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(t),
                        )
                    })
                    .when(row.nullable == Some(false), |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .size(px(4.))
                                .rounded_full()
                                .bg(theme.muted_foreground.opacity(0.7)),
                        )
                    }),
            )
            .when(row.retry, |this| {
                let retry_id = row.key.connection_id().to_string();
                this.child(
                    Button::new(SharedString::from(format!("retry-{ix}")))
                        .xsmall()
                        .ghost()
                        .label("Retry")
                        .on_click(cx.listener(move |this, _, _, cx| this.retry(&retry_id, cx))),
                )
            })
            .when_some(row.detail.clone(), |this, d| {
                this.child(
                    div().flex_shrink_0().text_size(px(12.)).text_color(theme.muted_foreground).child(d),
                )
            })
            .when_some(row.dot, |this, dot| {
                let color = match dot {
                    ConnDot::Disconnected => None,
                    ConnDot::Connecting => Some(theme.warning),
                    ConnDot::Connected => Some(theme.success),
                    ConnDot::Failed => Some(theme.danger),
                };
                this.when_some(color, |this, c| {
                    this.child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(c))
                })
            })
            .context_menu(move |menu, window, cx| {
                // Hide tooltips while the menu is up (they would be drawn over it).
                let menu_owner = explorer.clone();
                menu_owner.update(cx, |this, cx| this.set_menu_open(true, cx)).ok();
                cx.subscribe(&cx.entity(), move |_, _, _: &gpui_kit::DismissEvent, cx| {
                    menu_owner.update(cx, |this, cx| this.set_menu_open(false, cx)).ok();
                })
                .detach();
                Explorer::context_menu_for(
                    explorer.clone(),
                    menu_key.clone(),
                    state.clone(),
                    menu,
                    window,
                    cx,
                )
            })
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Generate {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
}

/// The error text of a failed connection (see `connect_failure_reason`).
fn failure_text(e: &theta_core::Error, id: &str, store: &ConnectionStore) -> String {
    crate::connections::connect_failure_reason(e, store.config_with_secrets(id).as_ref())
}

fn parse_hex_color(hex: &str) -> Option<gpui_kit::Hsla> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(gpui_kit::rgb(v).into())
}

impl Render for Explorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_connections = !self.store.read(cx).configs().is_empty();
        let count = self.rows.len();
        let modal = Settings::get(cx).keymap.is_modal();
        let mut context = KeyContext::new_with_defaults(theta_ui::host_os());
        context.add("Explorer");
        if modal {
            context.add("modal");
        }
        let filtering = !self.filter_text.trim().is_empty();

        let body: gpui_kit::AnyElement = if !has_connections {
            EmptyState::new(IconName::Database, "No connections yet")
                .description("Add a database connection to browse its schemas, tables and columns.")
                .action(
                    Button::new("explorer-add-connection")
                        .primary()
                        .small()
                        .icon(IconName::Plus)
                        .label("Add Connection")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ExplorerEvent::AddConnection))),
                )
                .into_any_element()
        } else if count == 0 && filtering {
            EmptyState::new(IconName::Search, "No matching objects")
                .description(format!("Nothing matches “{}”.", self.filter_text.trim()))
                .into_any_element()
        } else {
            div()
                .id("explorer-list")
                .key_context(context)
                .track_focus(&self.focus)
                .size_full()
                .on_action(cx.listener(|this, _: &ExplorerNext, _, cx| this.select_next(cx)))
                .on_action(cx.listener(|this, _: &ExplorerPrevious, _, cx| this.select_previous(cx)))
                .on_action(cx.listener(|this, _: &ExplorerExpand, _, cx| this.expand_or_descend(cx)))
                .on_action(cx.listener(|this, _: &ExplorerCollapse, _, cx| this.collapse_or_ascend(cx)))
                .on_action(cx.listener(|this, _: &ExplorerActivate, _, cx| this.activate(cx)))
                .on_action(cx.listener(|this, _: &ExplorerFilter, window, cx| this.focus_filter(window, cx)))
                .on_action(cx.listener(|this, _: &ExplorerMoveUp, _, cx| this.move_selected(-1, cx)))
                .on_action(cx.listener(|this, _: &ExplorerMoveDown, _, cx| this.move_selected(1, cx)))
                .on_action(cx.listener(|this, _: &ExplorerRename, _, cx| this.rename_selected(cx)))
                .on_action(cx.listener(|this, _: &ExplorerDelete, _, cx| this.delete_selected(cx)))
                .on_action(cx.listener(|_, _: &ExplorerBack, _, cx| cx.emit(ExplorerEvent::ReturnFocus)))
                .child(
                    uniform_list(
                        "explorer-rows",
                        count,
                        cx.processor(|this, range: Range<usize>, window, cx| {
                            range.map(|ix| this.render_row(ix, window, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full()
                    .py_1(),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(
                PanelHeader::new("Explorer")
                    .action(
                        tool_button("explorer-add-folder", IconName::FolderPlus, "New Folder")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ExplorerEvent::NewFolder))),
                    )
                    .action(
                        tool_button("explorer-add", IconName::Plus, "Add Connection")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ExplorerEvent::AddConnection))),
                    )
                    .action(
                        tool_button("explorer-refresh", IconName::RefreshCw, "Refresh Schema")
                            .disabled(!has_connections)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(
                                    NodeKey::Connection(id)
                                    | NodeKey::Schema(id, _)
                                    | NodeKey::Group(id, _, _)
                                    | NodeKey::Table(id, _, _),
                                ) = this.selected.clone()
                                {
                                    this.store.update(cx, |s, cx| s.refresh_catalog(&id, cx));
                                } else if let Some(id) =
                                    this.store.read(cx).first_connected().map(str::to_string)
                                {
                                    this.store.update(cx, |s, cx| s.refresh_catalog(&id, cx));
                                }
                            })),
                    ),
            )
            .when(has_connections, |this| {
                this.child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex_shrink_0()
                        .on_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                            this.filter.update(cx, |f, cx| f.set_value("", window, cx));
                            this.focus.focus(window, cx);
                        }))
                        .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                            this.focus.focus(window, cx);
                            cx.stop_propagation();
                        }))
                        .child(Input::new(&self.filter).small().cleanable(true).prefix(
                            Icon::new(IconName::Search).size(px(13.)).text_color(theme.muted_foreground),
                        )),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // Clicking empty space below the rows still moves focus into the
                    // tree so the keys work (the filter box is outside this region).
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            if this.row_count() > 0 {
                                this.focus.focus(window, cx);
                            }
                        }),
                    )
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::secrets::SecretStore;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Arc;
    use theta_core::{Catalog, Schema, TableKind};

    struct NoConnection;
    #[async_trait::async_trait]
    impl theta_core::Connection for NoConnection {
        fn kind(&self) -> DatabaseKind {
            DatabaseKind::Sqlite
        }
        async fn server_info(&self) -> theta_core::Result<theta_core::ServerInfo> {
            Ok(Default::default())
        }
        async fn execute(
            &self,
            _: &str,
            _: &theta_core::ExecOptions,
        ) -> theta_core::Result<theta_core::StatementOutcome> {
            Err(theta_core::Error::Closed)
        }
        async fn cancel(&self) -> theta_core::Result<()> {
            Ok(())
        }
        async fn introspect(&self) -> theta_core::Result<Catalog> {
            Ok(Catalog::default())
        }
        async fn explain(&self, _: &str, _: bool) -> theta_core::Result<theta_core::Plan> {
            Err(theta_core::Error::Closed)
        }
        async fn in_transaction(&self) -> bool {
            false
        }
        async fn close(&self) -> theta_core::Result<()> {
            Ok(())
        }
    }

    pub(crate) fn stress_catalog(n: usize) -> Catalog {
        let tables = (0..n)
            .map(|i| Table {
                name: format!("table_{i:05}"),
                kind: TableKind::Table,
                columns: vec![],
                primary_key: vec![],
                foreign_keys: vec![],
                indexes: vec![],
                row_estimate: Some(i as u64),
                comment: None,
                ..Default::default()
            })
            .collect();
        Catalog {
            schemas: vec![Schema { name: "public".into(), tables, routines: vec![], sequences: vec![] }],
            default_schema: Some("public".into()),
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<ConnectionStore>, Entity<Explorer>, &mut VisualTestContext) {
        cx.update(|cx| {
            theta_ui::init(cx);
            crate::keymap::bind(cx);
        });
        let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
        let store = cx.new(|_| store);
        let (explorer, vcx) = cx.add_window_view(|window, cx| Explorer::new(store.clone(), window, cx));
        (store, explorer, vcx)
    }

    #[gpui_kit::test]
    fn empty_store_has_no_rows(cx: &mut TestAppContext) {
        let (_, explorer, vcx) = setup(cx);
        explorer.read_with(vcx, |e, _| assert_eq!(e.row_count(), 0));
    }

    #[gpui_kit::test]
    fn keyboard_navigation_moves_and_expands(cx: &mut TestAppContext) {
        let (store, explorer, vcx) = setup(cx);
        store.update(vcx, |s, cx| {
            let config = theta_core::ConnectionConfig::new("c1", "Demo", DatabaseKind::Sqlite);
            s.add_temporary(config, cx);
            s.set_catalog_synthetic("c1", stress_catalog(3), Arc::new(NoConnection), cx);
        });
        explorer.update_in(vcx, |e, window, cx| e.focus(window, cx));
        vcx.run_until_parked();
        // Connection, public, Tables, 3 tables (default expansion applied on catalog load).
        explorer.read_with(vcx, |e, _| {
            assert_eq!(e.row_count(), 6, "{:?}", e.rows.iter().map(|r| &r.label).collect::<Vec<_>>())
        });
        explorer.update(vcx, |e, cx| {
            assert!(matches!(e.selected_key(), Some(NodeKey::Connection(_))));
            e.select_next(cx);
            assert!(matches!(e.selected_key(), Some(NodeKey::Schema(..))));
            e.select_next(cx);
            e.select_next(cx);
            assert!(matches!(e.selected_key(), Some(NodeKey::Table(_, _, t)) if t == "table_00000"));
            e.select_previous(cx);
            e.collapse_or_ascend(cx); // Tables group open -> collapses
            assert_eq!(e.row_count(), 3);
            e.expand_or_descend(cx);
            assert_eq!(e.row_count(), 6);
            // Left on a table jumps to its parent group.
            e.select_next(cx);
            e.collapse_or_ascend(cx);
            assert!(matches!(e.selected_key(), Some(NodeKey::Group(..))));
        });
    }

    #[gpui_kit::test]
    fn enter_on_a_table_requests_opening_it(cx: &mut TestAppContext) {
        let (store, explorer, vcx) = setup(cx);
        let events = Arc::new(std::sync::Mutex::new(Vec::<ExplorerEvent>::new()));
        let sink = events.clone();
        let _sub = vcx.update(|_, cx| {
            cx.subscribe(&explorer, move |_, e: &ExplorerEvent, _| sink.lock().unwrap().push(e.clone()))
        });
        store.update(vcx, |s, cx| {
            s.add_temporary(theta_core::ConnectionConfig::new("c1", "Demo", DatabaseKind::Sqlite), cx);
            s.set_catalog_synthetic("c1", stress_catalog(2), Arc::new(NoConnection), cx);
        });
        vcx.run_until_parked();
        explorer.update(vcx, |e, cx| {
            for _ in 0..3 {
                e.select_next(cx);
            }
            e.activate(cx);
        });
        let got = events.lock().unwrap().clone();
        assert!(
            got.contains(&ExplorerEvent::OpenTable {
                connection_id: "c1".into(),
                schema: "public".into(),
                table: "table_00000".into()
            }),
            "{got:?}"
        );
    }

    #[gpui_kit::test]
    fn five_thousand_tables_build_quickly(cx: &mut TestAppContext) {
        let (store, explorer, vcx) = setup(cx);
        let started = std::time::Instant::now();
        store.update(vcx, |s, cx| {
            s.add_temporary(theta_core::ConnectionConfig::new("c1", "Big", DatabaseKind::Postgres), cx);
            s.set_catalog_synthetic("c1", stress_catalog(5000), Arc::new(NoConnection), cx);
        });
        vcx.run_until_parked();
        explorer.read_with(vcx, |e, _| assert_eq!(e.row_count(), 3 + 5000));
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
    }
}
