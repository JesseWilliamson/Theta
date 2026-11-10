//! Picker delegates: command palette, theme picker, connection picker, table
//! picker. Each is a thin adapter over `quarry_ui::picker`.

use crate::actions::*;
use crate::connections::ConnectionStore;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName;
use gpui_kit::{Action, App, Entity, SharedString, WeakEntity, Window};
use quarry_ui::picker::{PickerDelegate, PickerItem};
use quarry_ui::themes;

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
        cmd!("Toggle Explorer", "View", IconName::PanelLeft, ToggleExplorer),
        cmd!("Toggle Bottom Panel", "View", IconName::PanelBottom, ToggleBottomPanel),
        cmd!("Focus Explorer", "View", IconName::FolderTree, FocusExplorer),
        cmd!("Close Tab", "View", IconName::X, CloseTab),
        cmd!("Select Theme…", "Appearance", IconName::Palette, SelectTheme),
        cmd!("Import Zed Theme…", "Appearance", IconName::FileDown, ImportZedTheme),
        cmd!("Toggle Keymap (Helix / Standard)", "Appearance", IconName::Keyboard, ToggleKeymap),
        cmd!("Zoom In", "Appearance", IconName::ZoomIn, ZoomIn),
        cmd!("Zoom Out", "Appearance", IconName::ZoomOut, ZoomOut),
        cmd!("Reset Zoom", "Appearance", IconName::Search, ZoomReset),
        cmd!("Open Settings File", "App", IconName::Settings, OpenSettingsFile),
        cmd!("Quit", "App", IconName::Power, Quit),
    ]
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
                        .map(|k| k.unparse(quarry_ui::host_os()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    item = item.key_hint(quarry_ui::format_keystrokes(&spec, quarry_ui::is_mac()));
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
                let section = if t.name.starts_with("Quarry") {
                    "Quarry"
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
                PickerItem::new(c.id.clone(), c.name.clone())
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

pub struct TablePicker {
    store: Entity<ConnectionStore>,
    workspace: WeakEntity<Workspace>,
    connection_id: String,
}

impl TablePicker {
    pub fn new(
        store: Entity<ConnectionStore>,
        workspace: WeakEntity<Workspace>,
        connection_id: String,
    ) -> Self {
        Self { store, workspace, connection_id }
    }
}

impl PickerDelegate for TablePicker {
    fn placeholder(&self) -> SharedString {
        "Go to table…".into()
    }
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem> {
        let Some(catalog) = self.store.read(cx).catalog(&self.connection_id) else { return vec![] };
        catalog
            .schemas
            .iter()
            .flat_map(|s| {
                s.tables.iter().map(move |t| {
                    let kind = match t.kind {
                        quarry_core::TableKind::Table => "table",
                        quarry_core::TableKind::View => "view",
                        quarry_core::TableKind::MaterializedView => "materialized view",
                        quarry_core::TableKind::ForeignTable => "foreign table",
                    };
                    PickerItem::new(format!("{}\u{1}{}", s.name, t.name), format!("{}.{}", s.name, t.name))
                        .icon(IconName::Table2)
                        .detail(kind)
                })
            })
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some((schema, table)) = item.id.split_once('\u{1}') else { return };
        let (schema, table, conn) = (schema.to_string(), table.to_string(), self.connection_id.clone());
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| ws.open_table(conn, schema, table, window, cx)).ok();
        });
    }
}
