//! Saved connections and their live sessions.
//!
//! [`ConnectionStore`] is an app-level entity: the explorer, title bar, status
//! bar and pickers observe it and react to [`StoreEvent`]s. It owns:
//!
//! * the list of saved [`ConnectionConfig`]s, persisted to
//!   `<config_dir>/connections.json` **without secrets**;
//! * secrets, in the OS keychain (see [`secrets`]);
//! * per-connection live state ([`ConnState`]) including the introspected catalog.
//!
//! All database I/O goes through [`crate::runtime`]; nothing here blocks the UI.

pub mod folders;
pub mod secrets;
pub mod url;

use crate::runtime;
use gpui_kit::{Context, EventEmitter};
use secrets::{Secret, SecretStore, strip_secrets};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theta_core::{Catalog, Connection, ConnectionConfig, Error, ServerInfo};

#[derive(Clone)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected {
        connection: Arc<dyn Connection>,
        server: ServerInfo,
        catalog: Option<Arc<Catalog>>,
        catalog_loading: bool,
    },
    Failed(Error),
}

impl ConnState {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnState::Connected { .. })
    }
    pub fn is_busy(&self) -> bool {
        matches!(self, ConnState::Connecting)
            || matches!(self, ConnState::Connected { catalog_loading: true, .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreEvent {
    /// A configuration was added, edited, removed or reordered.
    ListChanged,
    /// The live state of one connection changed.
    StateChanged(String),
    /// The catalog of one connection finished loading (or failed).
    CatalogChanged(String),
    /// Something the user should be told about (shown as a toast by the shell).
    Notice { kind: NoticeKind, message: String },
}

#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub server_version: String,
    pub latency: Duration,
}

/// One saved connection: its config plus the folder it is in. The folder id is flattened into the
/// same JSON object, so a file written before folders existed reads as "no folder" and a file with
/// folders still reads in an older version (which ignores the key).
#[derive(Serialize, Deserialize)]
struct StoredEntry {
    #[serde(flatten)]
    config: ConnectionConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct StoredConnections {
    #[serde(default)]
    connections: Vec<StoredEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    folders: Vec<folders::Folder>,
}

pub struct ConnectionStore {
    /// Configs with all secrets stripped; secrets live in `secrets`.
    configs: Vec<ConnectionConfig>,
    states: HashMap<String, ConnState>,
    catalog_errors: HashMap<String, Error>,
    /// Ids that are not written to disk (`--connect`).
    temporary: HashSet<String>,
    layout: folders::Layout,
    secrets: Arc<SecretStore>,
    path: Option<PathBuf>,
}

impl EventEmitter<StoreEvent> for ConnectionStore {}

fn folder_name_problem(name: &str) -> String {
    if name.trim().is_empty() {
        "A folder needs a name".into()
    } else {
        format!("A folder named “{}” already exists", name.trim())
    }
}

pub fn new_connection_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl ConnectionStore {
    /// Load `connections.json`. A missing file is an empty store; an unreadable
    /// or malformed one is moved aside (never overwritten) and reported.
    pub fn load(path: Option<PathBuf>, secrets: Arc<SecretStore>) -> (Self, Option<String>) {
        let mut warning = None;
        let mut configs = Vec::new();
        let mut layout = folders::Layout::default();
        if let Some(p) = &path {
            match std::fs::read_to_string(p) {
                Ok(text) if text.trim().is_empty() => {}
                Ok(text) => match serde_json::from_str::<StoredConnections>(&text) {
                    Ok(stored) => {
                        layout.folders = stored.folders;
                        for e in stored.connections {
                            if let Some(f) = e.folder {
                                layout.folder_of.insert(e.config.id.clone(), f);
                            }
                            configs.push(strip_secrets(&e.config));
                        }
                        layout.sanitize();
                    }
                    Err(e) => {
                        let backup = p.with_extension("json.broken");
                        let moved = std::fs::rename(p, &backup).is_ok();
                        warning = Some(format!(
                            "connections.json could not be read ({e}){}",
                            if moved {
                                format!("; it was kept as {}", backup.display())
                            } else {
                                String::new()
                            }
                        ));
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warning = Some(format!("could not read {}: {e}", p.display())),
            }
        }
        let store = Self {
            configs,
            states: HashMap::new(),
            catalog_errors: HashMap::new(),
            temporary: HashSet::new(),
            layout,
            secrets,
            path,
        };
        (store, warning)
    }

    pub fn secrets(&self) -> Arc<SecretStore> {
        self.secrets.clone()
    }

    pub fn configs(&self) -> &[ConnectionConfig] {
        &self.configs
    }

    pub fn config(&self, id: &str) -> Option<&ConnectionConfig> {
        self.configs.iter().find(|c| c.id == id)
    }

    /// The saved config with its secrets filled in (for the edit dialog).
    pub fn config_with_secrets(&self, id: &str) -> Option<ConnectionConfig> {
        self.config(id).map(|c| self.secrets.with_secrets(c))
    }

    pub fn state(&self, id: &str) -> &ConnState {
        self.states.get(id).unwrap_or(&ConnState::Disconnected)
    }

    pub fn catalog(&self, id: &str) -> Option<Arc<Catalog>> {
        match self.state(id) {
            ConnState::Connected { catalog, .. } => catalog.clone(),
            _ => None,
        }
    }

    pub fn connection(&self, id: &str) -> Option<Arc<dyn Connection>> {
        match self.state(id) {
            ConnState::Connected { connection, .. } => Some(connection.clone()),
            _ => None,
        }
    }

    pub fn catalog_error(&self, id: &str) -> Option<&Error> {
        self.catalog_errors.get(id)
    }

    pub fn is_temporary(&self, id: &str) -> bool {
        self.temporary.contains(id)
    }

    pub fn first_connected(&self) -> Option<&str> {
        self.configs.iter().find(|c| self.state(&c.id).is_connected()).map(|c| c.id.as_str())
    }

    // ---- persistence ----

    fn persist(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.path else { return };
        let stored = StoredConnections {
            connections: self
                .configs
                .iter()
                .filter(|c| !self.temporary.contains(&c.id))
                .map(|c| StoredEntry {
                    config: strip_secrets(c),
                    folder: self.layout.folder_of.get(&c.id).cloned(),
                })
                .collect(),
            folders: self.layout.folders.clone(),
        };
        let json = serde_json::to_string_pretty(&stored).unwrap_or_default() + "\n";
        if let Err(e) = theta_ui::settings::write_atomic(path, json.as_bytes()) {
            cx.emit(StoreEvent::Notice {
                kind: NoticeKind::Error,
                message: format!("Could not save connections: {e}"),
            });
        }
    }

    // ---- folders and order ----

    pub fn layout(&self) -> &folders::Layout {
        &self.layout
    }

    /// `Folder / Name` for pickers.
    pub fn display_name(&self, id: &str) -> String {
        match self.config(id) {
            Some(c) => self.layout.display_name(id, &c.name),
            None => String::new(),
        }
    }

    fn layout_changed(&mut self, cx: &mut Context<Self>) {
        self.persist(cx);
        cx.emit(StoreEvent::ListChanged);
        cx.notify();
    }

    /// Create a folder. `Err` carries the reason (empty or duplicate name).
    pub fn add_folder(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        match self.layout.add_folder(name) {
            Some(id) => {
                self.layout_changed(cx);
                Ok(id)
            }
            None => Err(folder_name_problem(name)),
        }
    }

    pub fn rename_folder(&mut self, id: &str, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.layout.rename_folder(id, name) {
            self.layout_changed(cx);
            Ok(())
        } else {
            Err(folder_name_problem(name))
        }
    }

    pub fn delete_folder(&mut self, id: &str, cx: &mut Context<Self>) {
        self.layout.delete_folder(id);
        self.layout_changed(cx);
    }

    pub fn set_folder_collapsed(&mut self, id: &str, collapsed: bool, cx: &mut Context<Self>) {
        if self.layout.folder(id).is_some_and(|f| f.collapsed != collapsed) {
            self.layout.set_collapsed(id, collapsed);
            self.layout_changed(cx);
        }
    }

    pub fn move_to_folder(&mut self, conn: &str, folder: Option<&str>, cx: &mut Context<Self>) {
        self.layout.move_to(conn, folder);
        self.layout_changed(cx);
    }

    pub fn move_folder(&mut self, id: &str, delta: i32, cx: &mut Context<Self>) {
        if self.layout.move_folder(id, delta) {
            self.layout_changed(cx);
        }
    }

    /// Move a connection up (`-1`) or down (`1`) within its folder (or the top level).
    pub fn move_connection(&mut self, id: &str, delta: i32, cx: &mut Context<Self>) {
        let mut order: Vec<String> = self.configs.iter().map(|c| c.id.clone()).collect();
        if self.layout.move_connection(&mut order, id, delta) {
            self.configs.sort_by_key(|c| order.iter().position(|o| *o == c.id));
            self.layout_changed(cx);
        }
    }

    // ---- editing the list ----

    /// Add a new connection (its secrets go to the keychain). Returns its id.
    pub fn add(&mut self, mut config: ConnectionConfig, cx: &mut Context<Self>) -> String {
        if config.id.is_empty() {
            config.id = new_connection_id();
        }
        let id = config.id.clone();
        self.secrets.save(&id, &Secret::from_config(&config));
        self.configs.push(strip_secrets(&config));
        self.persist(cx);
        cx.emit(StoreEvent::ListChanged);
        cx.notify();
        id
    }

    /// Add a connection that is kept for this session only (`--connect`).
    pub fn add_temporary(&mut self, config: ConnectionConfig, cx: &mut Context<Self>) -> String {
        let id = config.id.clone();
        self.temporary.insert(id.clone());
        self.secrets.save(&id, &Secret::from_config(&config));
        self.configs.push(strip_secrets(&config));
        cx.emit(StoreEvent::ListChanged);
        cx.notify();
        id
    }

    /// Replace the config with the same id. Any live session is closed because
    /// it was opened with the old settings.
    pub fn update(&mut self, config: ConnectionConfig, cx: &mut Context<Self>) {
        let id = config.id.clone();
        let Some(slot) = self.configs.iter_mut().find(|c| c.id == id) else { return };
        self.secrets.save(&id, &Secret::from_config(&config));
        *slot = strip_secrets(&config);
        if self.state(&id).is_connected() || matches!(self.state(&id), ConnState::Failed(_)) {
            self.disconnect(&id, cx);
        }
        self.persist(cx);
        cx.emit(StoreEvent::ListChanged);
        cx.notify();
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.disconnect(id, cx);
        self.configs.retain(|c| c.id != id);
        self.states.remove(id);
        self.temporary.remove(id);
        self.layout.folder_of.remove(id);
        self.secrets.remove(id);
        self.persist(cx);
        cx.emit(StoreEvent::ListChanged);
        cx.notify();
    }

    pub fn duplicate(&mut self, id: &str, cx: &mut Context<Self>) -> Option<String> {
        let mut copy = self.config_with_secrets(id)?;
        copy.id = new_connection_id();
        copy.name = format!("{} copy", copy.name);
        let new_id = self.add(copy, cx);
        // The copy lands next to the original's group, not at the top level.
        if let Some(f) = self.layout.folder_of.get(id).cloned() {
            self.layout.folder_of.insert(new_id.clone(), f);
            self.layout_changed(cx);
        }
        Some(new_id)
    }

    // ---- live state ----

    fn set_state(&mut self, id: &str, state: ConnState, cx: &mut Context<Self>) {
        self.states.insert(id.to_string(), state);
        cx.emit(StoreEvent::StateChanged(id.to_string()));
        cx.notify();
    }

    pub fn connect(&mut self, id: &str, cx: &mut Context<Self>) {
        if matches!(self.state(id), ConnState::Connecting | ConnState::Connected { .. }) {
            return;
        }
        let Some(config) = self.config(id).cloned() else { return };
        self.catalog_errors.remove(id);
        self.set_state(id, ConnState::Connecting, cx);
        let secrets = self.secrets.clone();
        let work = runtime::spawn_result(async move {
            let config = secrets.with_secrets(&config);
            let (connection, notices) = open_connection(&config).await?;
            let connection: Arc<dyn Connection> = Arc::from(connection);
            let server = connection.server_info().await?;
            Ok((connection, server, notices))
        });
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                // The user may have disconnected or deleted it while we waited.
                if !matches!(this.state(&id), ConnState::Connecting) {
                    if let Ok((connection, ..)) = result {
                        runtime::spawn(async move {
                            let _ = connection.close().await;
                        });
                    }
                    return;
                }
                match result {
                    Ok((connection, server, notices)) => {
                        for message in notices {
                            cx.emit(StoreEvent::Notice { kind: NoticeKind::Info, message });
                        }
                        let name = this.config(&id).map(|c| c.name.clone()).unwrap_or_default();
                        this.set_state(
                            &id,
                            ConnState::Connected { connection, server, catalog: None, catalog_loading: true },
                            cx,
                        );
                        cx.emit(StoreEvent::Notice {
                            kind: NoticeKind::Success,
                            message: format!("Connected to {name}"),
                        });
                        this.load_catalog(&id, cx);
                    }
                    Err(e) => {
                        let name = this.config(&id).map(|c| c.name.clone()).unwrap_or_default();
                        cx.emit(StoreEvent::Notice {
                            kind: NoticeKind::Error,
                            message: format!("Could not connect to {name}: {e}"),
                        });
                        this.set_state(&id, ConnState::Failed(e), cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn disconnect(&mut self, id: &str, cx: &mut Context<Self>) {
        let previous = self.states.remove(id);
        self.catalog_errors.remove(id);
        if let Some(ConnState::Connected { connection, .. }) = previous {
            runtime::spawn(async move {
                // Stop whatever is running first: closing alone leaves a long statement
                // running on the server (and, for SQLite, in the worker thread) until it ends.
                let _ = connection.cancel().await;
                let _ = connection.close().await;
            });
        }
        cx.emit(StoreEvent::StateChanged(id.to_string()));
        cx.notify();
    }

    /// Re-read the catalog of a connected connection.
    pub fn refresh_catalog(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.state(id).is_connected() {
            return;
        }
        self.catalog_errors.remove(id);
        if let Some(ConnState::Connected { catalog_loading, .. }) = self.states.get_mut(id) {
            *catalog_loading = true;
        }
        cx.emit(StoreEvent::StateChanged(id.to_string()));
        cx.notify();
        self.load_catalog(id, cx);
    }

    fn load_catalog(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(connection) = self.connection(id) else { return };
        let work = runtime::spawn_result(async move { connection.introspect().await });
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let Some(ConnState::Connected { catalog, catalog_loading, .. }) = this.states.get_mut(&id)
                else {
                    return;
                };
                *catalog_loading = false;
                match result {
                    Ok(c) => *catalog = Some(Arc::new(c)),
                    Err(e) => {
                        cx.emit(StoreEvent::Notice {
                            kind: NoticeKind::Error,
                            message: format!("Could not read the schema: {e}"),
                        });
                        this.catalog_errors.insert(id.clone(), e);
                    }
                }
                cx.emit(StoreEvent::CatalogChanged(id.clone()));
                cx.emit(StoreEvent::StateChanged(id.clone()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Install a synthetic catalog without a database (`--stress-catalog`, tests).
    pub fn set_catalog_synthetic(
        &mut self,
        id: &str,
        catalog: Catalog,
        connection: Arc<dyn Connection>,
        cx: &mut Context<Self>,
    ) {
        self.set_state(
            id,
            ConnState::Connected {
                connection,
                server: ServerInfo {
                    version: "synthetic".into(),
                    current_database: None,
                    current_user: None,
                },
                catalog: Some(Arc::new(catalog)),
                catalog_loading: false,
            },
            cx,
        );
        cx.emit(StoreEvent::CatalogChanged(id.to_string()));
    }
}

/// Open a throw-away connection with `config`, read the server version and
/// close it again. Used by the dialog's "Test Connection" button.
pub async fn test_connection(config: ConnectionConfig) -> Result<TestOutcome, Error> {
    runtime::spawn_result(async move {
        let started = Instant::now();
        let (connection, _) = open_connection(&config).await?;
        let info = connection.server_info().await;
        let latency = started.elapsed();
        let _ = connection.close().await;
        Ok(TestOutcome { server_version: info?.version, latency })
    })
    .await
}

/// A connection that does nothing, behind synthetic catalogs (`--stress-catalog`).
pub struct NullConnection;

#[async_trait::async_trait]
impl Connection for NullConnection {
    fn kind(&self) -> theta_core::DatabaseKind {
        theta_core::DatabaseKind::Postgres
    }
    async fn server_info(&self) -> Result<ServerInfo, Error> {
        Ok(ServerInfo::default())
    }
    async fn execute(
        &self,
        _: &str,
        _: &theta_core::ExecOptions,
    ) -> Result<theta_core::StatementOutcome, Error> {
        Err(Error::Unsupported("synthetic connection".into()))
    }
    async fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
    async fn introspect(&self) -> Result<Catalog, Error> {
        Ok(Catalog::default())
    }
    async fn explain(&self, _: &str, _: bool) -> Result<theta_core::Plan, Error> {
        Err(Error::Unsupported("synthetic connection".into()))
    }
    async fn in_transaction(&self) -> bool {
        false
    }
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// The single place the app calls into the driver crate.
async fn open_connection(config: &ConnectionConfig) -> Result<(Box<dyn Connection>, Vec<String>), Error> {
    theta_drivers::connect_with_notices(config).await
}

pub fn connections_path() -> PathBuf {
    theta_ui::config_dir().join("connections.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};
    use std::sync::Arc;
    use theta_core::DatabaseKind;

    fn sample(id: &str) -> ConnectionConfig {
        let mut c = ConnectionConfig::new(id, format!("conn {id}"), DatabaseKind::Postgres);
        c.password = Some("s3cret".into());
        c.user = "me".into();
        c
    }

    fn store_at(path: PathBuf) -> (ConnectionStore, Arc<SecretStore>) {
        let secrets = Arc::new(SecretStore::memory_only());
        let (s, w) = ConnectionStore::load(Some(path), secrets.clone());
        assert!(w.is_none());
        (s, secrets)
    }

    #[gpui_kit::test]
    fn persistence_never_writes_secrets(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let (store, secrets) = store_at(path.clone());
        let store = cx.new(|_| store);
        let id = store.update(cx, |s, cx| s.add(sample("c1"), cx));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("conn c1"));
        assert!(!on_disk.contains("s3cret"), "password leaked to disk: {on_disk}");
        assert_eq!(secrets.load(&id).password.as_deref(), Some("s3cret"));
        // The edit dialog still gets the password back.
        let full = store.read_with(cx, |s, _| s.config_with_secrets("c1").unwrap());
        assert_eq!(full.password.as_deref(), Some("s3cret"));
    }

    #[gpui_kit::test]
    fn reload_restores_configs_without_passwords(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        {
            let (store, _) = store_at(path.clone());
            let store = cx.new(|_| store);
            store.update(cx, |s, cx| {
                s.add(sample("a"), cx);
                s.add(sample("b"), cx);
            });
        }
        let (reloaded, _) = store_at(path);
        assert_eq!(reloaded.configs().len(), 2);
        assert!(reloaded.configs().iter().all(|c| c.password.is_none()));
        assert_eq!(reloaded.config("b").unwrap().name, "conn b");
    }

    #[gpui_kit::test]
    fn update_remove_duplicate(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (store, secrets) = store_at(dir.path().join("c.json"));
        let store = cx.new(|_| store);
        store.update(cx, |s, cx| {
            s.add(sample("a"), cx);
            let mut edited = sample("a");
            edited.name = "renamed".into();
            s.update(edited, cx);
            assert_eq!(s.config("a").unwrap().name, "renamed");
            let copy = s.duplicate("a", cx).unwrap();
            assert_ne!(copy, "a");
            assert_eq!(s.config(&copy).unwrap().name, "renamed copy");
            assert_eq!(secrets.load(&copy).password.as_deref(), Some("s3cret"));
            s.remove("a", cx);
            assert!(s.config("a").is_none());
            assert!(secrets.load("a").is_empty());
        });
    }

    #[gpui_kit::test]
    fn temporary_connections_are_not_persisted(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let (store, _) = store_at(path.clone());
        let store = cx.new(|_| store);
        store.update(cx, |s, cx| {
            s.add(sample("keep"), cx);
            s.add_temporary(sample("tmp"), cx);
        });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("keep") && !text.contains("conn tmp"));
    }

    #[test]
    fn malformed_file_is_moved_aside_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, "{ nope").unwrap();
        let (s, warning) = ConnectionStore::load(Some(path.clone()), Arc::new(SecretStore::memory_only()));
        assert!(s.configs().is_empty());
        assert!(warning.unwrap().contains("connections.json.broken"));
        assert!(dir.path().join("connections.json.broken").exists());
        assert!(!path.exists());
    }

    #[test]
    fn missing_file_is_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let (s, warning) =
            ConnectionStore::load(Some(dir.path().join("none.json")), Arc::new(SecretStore::memory_only()));
        assert!(s.configs().is_empty() && warning.is_none());
    }

    #[test]
    fn a_file_from_before_folders_loads_and_gains_them_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(
            &path,
            r#"{"connections":[{"id":"a","name":"A","kind":"postgres"},{"id":"b","name":"B","kind":"sqlite"}]}"#,
        )
        .unwrap();
        let (s, w) = ConnectionStore::load(Some(path), Arc::new(SecretStore::memory_only()));
        assert!(w.is_none());
        assert_eq!(s.configs().len(), 2);
        assert!(s.layout().folders.is_empty() && s.layout().folder_of.is_empty());
    }

    #[gpui_kit::test]
    fn folders_order_and_collapsed_state_survive_a_restart(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        {
            let (store, _) = store_at(path.clone());
            let store = cx.new(|_| store);
            store.update(cx, |s, cx| {
                for id in ["a", "b", "c"] {
                    s.add(sample(id), cx);
                }
                let f = s.add_folder("Prod", cx).unwrap();
                assert!(s.add_folder("prod", cx).is_err());
                s.move_to_folder("b", Some(&f), cx);
                s.move_to_folder("c", Some(&f), cx);
                s.move_connection("c", -1, cx);
                s.set_folder_collapsed(&f, true, cx);
            });
        }
        let (r, _) = store_at(path.clone());
        let ids: Vec<_> = r.configs().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "b"], "c moved above b within Prod");
        let f = &r.layout().folders[0];
        assert!(f.collapsed && f.name == "Prod");
        assert_eq!(r.display_name("c"), "Prod / conn c");
        assert_eq!(r.display_name("a"), "conn a");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("s3cret"));
    }

    #[gpui_kit::test]
    fn deleting_things_cleans_up_the_layout(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = store_at(dir.path().join("c.json"));
        let store = cx.new(|_| store);
        store.update(cx, |s, cx| {
            s.add(sample("a"), cx);
            let f = s.add_folder("F", cx).unwrap();
            s.move_to_folder("a", Some(&f), cx);
            let copy = s.duplicate("a", cx).unwrap();
            assert_eq!(s.layout().folder_of.get(&copy), Some(&f), "a duplicate stays in the folder");
            s.remove("a", cx);
            assert!(!s.layout().folder_of.contains_key("a"));
            s.delete_folder(&f, cx);
            assert!(s.layout().folder_of.is_empty(), "the copy went back to the top level");
            assert!(s.config(&copy).is_some(), "deleting a folder never deletes connections");
        });
    }
}
