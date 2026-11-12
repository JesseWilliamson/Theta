//! Picker delegates: command palette, theme picker, connection picker, table
//! picker. Each is a thin adapter over `theta_ui::picker`.

use crate::actions::*;
use crate::connections::ConnectionStore;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName;
use gpui_kit::{Action, App, Entity, SharedString, WeakEntity, Window};
use theta_ui::Settings;
use theta_ui::picker::{PickerDelegate, PickerItem};
use theta_ui::settings::KeymapMode;
use theta_ui::themes;

struct Command {
    label: &'static str,
    section: &'static str,
    icon: IconName,
    action: fn() -> Box<dyn Action>,
}

macro_rules! cmd {
    ($label:expr, $section:expr, $icon:expr, $action:expr) => {
        Command { label: $label, section: $section, icon: $icon, action: || Box::new($action) }
    };
}

fn commands() -> Vec<Command> {
    vec![
        cmd!("New Console", "Console", IconName::SquareTerminal, NewConsole),
        cmd!("New Connection…", "Connection", IconName::DatabasePlus, NewConnection),
        cmd!("New Connection Folder…", "Connection", IconName::FolderPlus, NewConnectionFolder),
        cmd!("Move Connection to Folder…", "Connection", IconName::Folder, MoveConnectionToFolder),
        cmd!("Connect…", "Connection", IconName::Plug, ConnectTo),
        cmd!("Disconnect", "Connection", IconName::Unplug, Disconnect),
        cmd!("Refresh Schema", "Connection", IconName::RefreshCw, RefreshSchema),
        cmd!("Go to Table…", "Connection", IconName::Table, OpenTablePicker),
        cmd!("Run Statement", "Query", IconName::Play, RunStatement),
        cmd!("Run All", "Query", IconName::ListVideo, RunAll),
        cmd!("Cancel Query", "Query", IconName::Square, CancelQuery),
        cmd!("Explain", "Query", IconName::Network, Explain),
        cmd!("Explain Analyze", "Query", IconName::Activity, ExplainAnalyze),
        cmd!("Format Document", "Query", IconName::Clock, FormatDocument),
        cmd!("Surround Selection with…", "Query", IconName::Scissors, SurroundWith),
        cmd!("Edit Live Templates…", "Query", IconName::Scissors, EditLiveTemplates),
        cmd!("Toggle Query Parameters", "Query", IconName::Settings, ToggleParameters),
        cmd!("Begin Transaction", "Query", IconName::Play, BeginTransaction),
        cmd!("Commit Transaction", "Query", IconName::Check, CommitTransaction),
        cmd!("Roll Back Transaction", "Query", IconName::Undo2, RollbackTransaction),
        cmd!("Transactions: Auto-commit", "Query", IconName::Check, SetAutoCommit),
        cmd!("Transactions: Manual", "Query", IconName::Play, SetManualTransactions),
        cmd!("Transaction Isolation Level…", "Query", IconName::Settings, SelectIsolationLevel),
        cmd!("Open SQL File…", "Console", IconName::FileDown, OpenSqlFile),
        cmd!("Import Data from CSV/TSV…", "Data", IconName::FileDown, ImportData),
        cmd!("Import CSV/TSV as New Table…", "Data", IconName::FileDown, ImportAsNewTable),
        cmd!("Export Table Data…", "Data", IconName::FileDown, ExportTableData),
        cmd!("New Table…", "Table", IconName::Plus, NewTable),
        cmd!("Modify Table…", "Table", IconName::Settings2, ModifyTable),
        cmd!("Rename Table…", "Table", IconName::Pencil, RenameTable),
        cmd!("Duplicate Table Structure…", "Table", IconName::Copy, DuplicateTableStructure),
        cmd!("Truncate Table…", "Table", IconName::Eraser, TruncateTable),
        cmd!("Drop Table…", "Table", IconName::Trash, DropTable),
        cmd!("Sessions…", "Server", IconName::Activity, OpenSessions),
        cmd!("Roles and Users…", "Server", IconName::Users, OpenRoles),
        cmd!("Search Across Tables…", "Data", IconName::Search, SearchTables),
        cmd!("Export Database…", "Server", IconName::HardDriveDownload, ExportDatabase),
        cmd!("Run SQL Script…", "Server", IconName::FileUp, RunSqlScript),
        cmd!("Save Console As…", "Console", IconName::FileDown, SaveConsole),
        cmd!("Settings…", "App", IconName::Settings, OpenSettings),
        cmd!("Toggle Explorer", "View", IconName::PanelLeft, ToggleExplorer),
        cmd!("Toggle Bottom Panel", "View", IconName::PanelBottom, ToggleBottomPanel),
        cmd!("Focus Explorer", "View", IconName::FolderTree, FocusExplorer),
        cmd!("Focus Editor", "View", IconName::SquareTerminal, FocusEditor),
        cmd!("Focus Results", "View", IconName::Table, FocusResults),
        cmd!("Toggle Cell Viewer", "View", IconName::PanelRight, ToggleCellViewer),
        cmd!("Copy Cell Value", "Data", IconName::Copy, CopyCellValue),
        cmd!("Close Tab", "View", IconName::X, CloseTab),
        cmd!("Reopen Closed Tab", "View", IconName::Undo2, ReopenClosedTab),
        cmd!("Select Theme…", "Appearance", IconName::Palette, SelectTheme),
        cmd!("Import Zed Theme…", "Appearance", IconName::FileDown, ImportZedTheme),
        cmd!("Select Keymap…", "Appearance", IconName::Keyboard, SelectKeymap),
        cmd!("Use Helix Keymap", "Appearance", IconName::Keyboard, UseHelixKeymap),
        cmd!("Use Vim Keymap", "Appearance", IconName::Keyboard, UseVimKeymap),
        cmd!("Use Standard Keymap", "Appearance", IconName::Keyboard, UseStandardKeymap),
        cmd!("Increase Editor Font Size", "Appearance", IconName::ZoomIn, ZoomIn),
        cmd!("Decrease Editor Font Size", "Appearance", IconName::ZoomOut, ZoomOut),
        cmd!("Reset Editor Font Size", "Appearance", IconName::Search, ZoomReset),
        cmd!("Open Settings File", "App", IconName::Settings, OpenSettingsFile),
        cmd!("Quit", "App", IconName::Power, Quit),
    ]
}

/// (section, label, key hint) of every command, for the keymap reference.
pub fn command_list(window: &Window) -> Vec<(String, String, String)> {
    commands()
        .into_iter()
        .map(|c| {
            let action = (c.action)();
            let hint = window
                .bindings_for_action(&*action)
                .last()
                .map(|b| {
                    let spec = b
                        .keystrokes()
                        .iter()
                        .map(|k| k.unparse(theta_ui::host_os()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    theta_ui::format_keystrokes(&spec, theta_ui::is_mac()).to_string()
                })
                .unwrap_or_default();
            (c.section.to_string(), c.label.to_string(), hint)
        })
        .collect()
}

pub struct CommandPalette {
    items: Vec<PickerItem>,
}

impl CommandPalette {
    /// Key hints are resolved up front because they need the window's keymap.
    pub fn new(window: &Window) -> Self {
        let items = commands()
            .into_iter()
            .map(|c| {
                let action = (c.action)();
                let mut item = PickerItem::new(c.label, c.label).section(c.section).icon(c.icon);
                if let Some(b) = window.bindings_for_action(&*action).last() {
                    let spec = b
                        .keystrokes()
                        .iter()
                        .map(|k| k.unparse(theta_ui::host_os()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    item = item.key_hint(theta_ui::format_keystrokes(&spec, theta_ui::is_mac()));
                }
                item
            })
            .collect();
        Self { items }
    }
}

impl PickerDelegate for CommandPalette {
    fn placeholder(&self) -> SharedString {
        "Type a command…".into()
    }
    fn empty_message(&self) -> SharedString {
        "No matching commands".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.items.clone()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some(command) = commands().into_iter().find(|c| c.label == item.id.as_ref()) else { return };
        let action = (command.action)();
        // Two deferrals: the picker is removed and focus restored first, so the
        // action runs from the place the user was working in.
        window.defer(cx, move |window, cx| {
            window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        });
    }
}

pub struct ThemePicker {
    original: String,
    confirmed: bool,
}

impl ThemePicker {
    pub fn new(cx: &App) -> Self {
        Self { original: themes::current_name(cx).to_string(), confirmed: false }
    }
}

impl PickerDelegate for ThemePicker {
    fn placeholder(&self) -> SharedString {
        "Select a theme…".into()
    }
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem> {
        let current = themes::current_name(cx);
        themes::available(cx)
            .into_iter()
            .map(|t| {
                let section = if t.name.starts_with("Theta") {
                    "Theta"
                } else if t.dark {
                    "Dark"
                } else {
                    "Light"
                };
                PickerItem::new(t.name.clone(), t.name.clone()).section(section).checked(t.name == current)
            })
            .collect()
    }
    fn initial_selection(&self, items: &[PickerItem]) -> Option<usize> {
        items.iter().position(|i| i.checked)
    }
    fn selection_changed(&mut self, item: &PickerItem, cx: &mut App) {
        themes::preview(&item.id, cx);
    }
    fn confirm(&mut self, item: &PickerItem, _: &mut Window, cx: &mut App) {
        self.confirmed = true;
        themes::set_theme(&item.id, cx);
    }
    fn dismissed(&mut self, confirmed: bool, _: &mut Window, cx: &mut App) {
        if !confirmed && !self.confirmed {
            themes::preview(&self.original, cx);
        }
    }
}

/// Picks the editing keymap. Confirming applies it at once (every editor observes the settings).
pub struct KeymapPicker {
    workspace: WeakEntity<Workspace>,
}

impl KeymapPicker {
    pub fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace }
    }
}

impl PickerDelegate for KeymapPicker {
    fn placeholder(&self) -> SharedString {
        "Select a keymap…".into()
    }
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem> {
        let current = Settings::get(cx).keymap;
        KeymapMode::ALL
            .into_iter()
            .map(|k| {
                let detail = match k {
                    KeymapMode::Helix => "select, then act; multiple selections",
                    KeymapMode::Vim => "verb, then motion; registers and macros",
                    KeymapMode::Standard => "a conventional editor",
                };
                PickerItem::new(k.label(), k.label())
                    .detail(detail)
                    .icon(IconName::Keyboard)
                    .checked(k == current)
            })
            .collect()
    }
    fn initial_selection(&self, items: &[PickerItem]) -> Option<usize> {
        items.iter().position(|i| i.checked)
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some(keymap) = KeymapMode::ALL.into_iter().find(|k| k.label() == item.id.as_ref()) else {
            return;
        };
        let _ = self.workspace.update(cx, |ws, cx| ws.set_keymap(keymap, window, cx));
    }
}

pub struct ConnectionPicker {
    store: Entity<ConnectionStore>,
    workspace: WeakEntity<Workspace>,
    current: Option<String>,
}

impl ConnectionPicker {
    pub fn new(
        store: Entity<ConnectionStore>,
        workspace: WeakEntity<Workspace>,
        current: Option<String>,
    ) -> Self {
        Self { store, workspace, current }
    }
}

impl PickerDelegate for ConnectionPicker {
    fn placeholder(&self) -> SharedString {
        "Switch to connection…".into()
    }
    fn empty_message(&self) -> SharedString {
        "No saved connections".into()
    }
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem> {
        let store = self.store.read(cx);
        store
            .configs()
            .iter()
            .map(|c| {
                let state = match store.state(&c.id) {
                    crate::connections::ConnState::Connected { .. } => "connected",
                    crate::connections::ConnState::Connecting => "connecting…",
                    crate::connections::ConnState::Failed(_) => "failed",
                    crate::connections::ConnState::Disconnected => "",
                };
                let detail = if state.is_empty() {
                    c.kind.label().to_string()
                } else {
                    format!("{} · {state}", c.kind.label())
                };
                PickerItem::new(c.id.clone(), store.display_name(&c.id))
                    .icon(IconName::Database)
                    .detail(detail)
                    .checked(self.current.as_deref() == Some(c.id.as_str()))
            })
            .collect()
    }
    fn initial_selection(&self, items: &[PickerItem]) -> Option<usize> {
        items.iter().position(|i| i.checked)
    }
    fn confirm(&mut self, item: &PickerItem, _: &mut Window, cx: &mut App) {
        let id = item.id.to_string();
        self.workspace.update(cx, |ws, cx| ws.use_connection(&id, cx)).ok();
    }
}

/// What choosing a table in the picker does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TablePurpose {
    Open,
    Import,
    Export,
    Modify,
    Rename,
    Truncate,
    Drop,
    Duplicate,
}

pub struct TablePicker {
    store: Entity<ConnectionStore>,
    workspace: WeakEntity<Workspace>,
    connection_id: String,
    purpose: TablePurpose,
}

impl TablePicker {
    pub fn new(
        store: Entity<ConnectionStore>,
        workspace: WeakEntity<Workspace>,
        connection_id: String,
    ) -> Self {
        Self { store, workspace, connection_id, purpose: TablePurpose::Open }
    }

    pub fn for_purpose(mut self, purpose: TablePurpose) -> Self {
        self.purpose = purpose;
        self
    }
}

impl PickerDelegate for TablePicker {
    fn placeholder(&self) -> SharedString {
        match self.purpose {
            TablePurpose::Open => "Go to table…",
            TablePurpose::Import => "Import into which table…",
            TablePurpose::Export => "Export which table…",
            TablePurpose::Modify => "Modify which table…",
            TablePurpose::Rename => "Rename which table…",
            TablePurpose::Truncate => "Truncate which table…",
            TablePurpose::Drop => "Drop which table…",
            TablePurpose::Duplicate => "Duplicate the structure of which table…",
        }
        .into()
    }
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem> {
        let Some(catalog) = self.store.read(cx).catalog(&self.connection_id) else { return vec![] };
        let tables_only = self.purpose != TablePurpose::Open;
        catalog
            .schemas
            .iter()
            .flat_map(|s| {
                s.tables.iter().filter(move |t| !tables_only || t.kind == theta_core::TableKind::Table).map(
                    move |t| {
                        let kind = match t.kind {
                            theta_core::TableKind::Table => "table",
                            theta_core::TableKind::View => "view",
                            theta_core::TableKind::MaterializedView => "materialized view",
                            theta_core::TableKind::ForeignTable => "foreign table",
                        };
                        PickerItem::new(
                            format!("{}\u{1}{}", s.name, t.name),
                            format!("{}.{}", s.name, t.name),
                        )
                        .icon(IconName::Table2)
                        .detail(kind)
                    },
                )
            })
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some((schema, table)) = item.id.split_once('\u{1}') else { return };
        let (schema, table, conn) = (schema.to_string(), table.to_string(), self.connection_id.clone());
        let workspace = self.workspace.clone();
        let purpose = self.purpose;
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |ws, cx| match purpose {
                    TablePurpose::Open => ws.open_table(conn, schema, table, window, cx),
                    TablePurpose::Import => ws.open_import_dialog(
                        crate::dialogs::import::Request {
                            conn_id: conn,
                            table: Some((schema, table)),
                            path: None,
                            new_table: false,
                        },
                        window,
                        cx,
                    ),
                    TablePurpose::Export => ws.open_export_dialog(conn, schema, table, window, cx),
                    TablePurpose::Modify => {
                        ws.open_table_editor(
                            conn,
                            crate::workspace::EditorTarget::Modify { schema, table, column: None },
                            window,
                            cx,
                        );
                    }
                    TablePurpose::Rename => ws.rename_table_dialog(conn, schema, table, window, cx),
                    TablePurpose::Truncate => ws.truncate_table_dialog(conn, schema, table, window, cx),
                    TablePurpose::Drop => ws.drop_table_dialog(conn, schema, table, window, cx),
                    TablePurpose::Duplicate => ws.duplicate_structure_dialog(conn, schema, table, window, cx),
                })
                .ok();
        });
    }
}

/// Tables reachable from one table through real and suggested foreign keys.
pub struct RelatedPicker {
    items: Vec<crate::exec::fk_ui::Related>,
    workspace: WeakEntity<Workspace>,
    connection_id: String,
    from: String,
}

impl RelatedPicker {
    pub fn new(
        items: Vec<crate::exec::fk_ui::Related>,
        workspace: WeakEntity<Workspace>,
        connection_id: String,
        from: String,
    ) -> Self {
        Self { items, workspace, connection_id, from }
    }
}

impl PickerDelegate for RelatedPicker {
    fn placeholder(&self) -> SharedString {
        format!("Tables related to {}…", self.from).into()
    }
    fn empty_message(&self) -> SharedString {
        "No related tables found".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let detail = match r.confidence {
                    Some(c) => format!("{} · {:.0}%", r.via, c * 100.0),
                    None => r.via.clone(),
                };
                PickerItem::new(i.to_string(), format!("{}.{}", r.schema, r.table))
                    .icon(if r.inferred { IconName::Unlink } else { IconName::Link })
                    .detail(detail)
                    .section(if r.inferred { "Suggested" } else { "Foreign keys" })
            })
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Ok(i) = item.id.parse::<usize>() else { return };
        let Some(r) = self.items.get(i) else { return };
        let (sql, conn) = (r.sql.clone(), self.connection_id.clone());
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| ws.open_console(Some(conn), Some(sql), window, cx)).ok();
        });
    }
}

/// Picks the folder a connection goes into; the first entry takes it out of its folder.
pub struct FolderPicker {
    store: Entity<ConnectionStore>,
    connection_id: String,
}

impl FolderPicker {
    pub fn new(store: Entity<ConnectionStore>, connection_id: String) -> Self {
        Self { store, connection_id }
    }
}

impl PickerDelegate for FolderPicker {
    fn placeholder(&self) -> SharedString {
        "Move to folder…".into()
    }
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem> {
        let store = self.store.read(cx);
        let current = store.layout().folder_of.get(&self.connection_id);
        let mut items = vec![
            PickerItem::new("", "No folder (top level)").icon(IconName::Database).checked(current.is_none()),
        ];
        items.extend(store.layout().folders.iter().map(|f| {
            PickerItem::new(f.id.clone(), f.name.clone())
                .icon(IconName::Folder)
                .checked(current == Some(&f.id))
        }));
        items
    }
    fn initial_selection(&self, items: &[PickerItem]) -> Option<usize> {
        items.iter().position(|i| i.checked)
    }
    fn confirm(&mut self, item: &PickerItem, _: &mut Window, cx: &mut App) {
        let folder = (!item.id.is_empty()).then(|| item.id.to_string());
        let conn = self.connection_id.clone();
        self.store.update(cx, |s, cx| s.move_to_folder(&conn, folder.as_deref(), cx));
    }
}
