//! Saved connections and their live sessions.
//!
//! [`ConnectionStore`] is an app-level entity: the explorer, title bar, status
//! bar and pickers observe it and react to [`StoreEvent`]s. It owns:
//!
//! * the list of saved [`ConnectionConfig`]s, persisted to
//!   `<config_dir>/connections.json` **without secrets**;
//! * the saved SSH configurations connections tunnel through, in `<config_dir>/ssh_configs.json`
//!   (see [`ssh`]);
//! * secrets, in the OS keychain (see [`secrets`]);
//! * per-connection live state ([`ConnState`]) including the introspected catalog.
//!
//! All database I/O goes through [`crate::runtime`]; nothing here blocks the UI.

pub mod folders;
pub mod secrets;
pub mod ssh;
pub mod url;

use crate::runtime;
use gpui_kit::{Context, EventEmitter};
use secrets::{Secret, SecretStore, strip_secrets};
use ssh::{SshProfile, StoredSsh};
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
    Warning,
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
    /// Saved SSH configurations with secrets stripped (`ssh_configs.json`, next to `connections.json`).
    ssh: Vec<SshProfile>,
    secrets: Arc<SecretStore>,
    path: Option<PathBuf>,
}

/// Where the SSH configurations are kept, given where the connections are.
fn ssh_path_for(connections: &std::path::Path) -> PathBuf {
    connections.with_file_name("ssh_configs.json")
}

/// Turns what the store holds into what the driver layer takes: the connection with its secrets and its
/// saved SSH configuration resolved to a plain `SshConfig`. Cheap to clone into an async task, which is
/// the only reason it exists apart from the store.
#[derive(Clone)]
pub struct Resolver {
    profiles: Vec<SshProfile>,
    secrets: Arc<SecretStore>,
}

impl Resolver {
    /// `config` with the saved tunnel it names resolved into `ssh`. Secrets other than the tunnel's are
    /// left as they are, so a form's unsaved password survives (Test Connection).
    pub fn attach_ssh(&self, mut config: ConnectionConfig) -> Result<ConnectionConfig, Error> {
        if let Some(id) = config.ssh_profile.as_deref().filter(|_| !config.kind.is_file_based()) {
            config.ssh = Some(ssh::resolve(&self.profiles, &self.secrets, id)?);
        }
        Ok(config)
    }

    /// A saved SSH configuration as the driver takes it, jump hosts and secrets included.
    pub fn ssh(&self, id: &str) -> Result<theta_core::SshConfig, Error> {
        ssh::resolve(&self.profiles, &self.secrets, id)
    }

    /// The saved connection as the drivers take it: secrets filled in, tunnel resolved. The one place
    /// that happens, just before connecting.
    pub fn runnable(&self, config: &ConnectionConfig) -> Result<ConnectionConfig, Error> {
        self.attach_ssh(self.secrets.with_secrets(config))
    }
}

impl EventEmitter<StoreEvent> for ConnectionStore {}

fn folder_name_problem(name: &str) -> String {
    if name.trim().is_empty() {
        "A folder needs a name".into()
    } else {
        format!("A folder named “{}” already exists", name.trim())
    }
}

/// Why a connection failed, in words for the user: the one text shown by the toast, the explorer and the
/// status bar (the mapping lives in `theta_core::friendly`; the driver's own prefix "could not connect: "
/// is dropped because every place that shows it already says so). A server that asks for a password the connection does not have is told as what
/// it is, not as the driver's "invalid configuration".
pub fn connect_failure_reason(e: &theta_core::Error, config: Option<&ConnectionConfig>) -> String {
    let no_password = config.is_some_and(|c| !c.kind.is_file_based() && c.password.as_deref().is_none_or(str::is_empty));
    if matches!(e, theta_core::Error::Auth(_)) && no_password {
        return "No password is saved for this connection. Use Edit Connection to enter one.".into();
    }
    theta_core::friendly::connect_error(e, config).message
}

/// The original driver text of a failed connection, for a tooltip or the log (`None` when the sentence
/// already says everything).
pub fn connect_failure_details(e: &theta_core::Error, config: Option<&ConnectionConfig>) -> Option<String> {
    theta_core::friendly::connect_error(e, config).details
}

/// What tells two connections with the same name apart.
pub struct LabelParts {
    pub name: String,
    pub product: &'static str,
    pub address: String,
    pub user: String,
}

/// The names of `parts`, each made unique only if it is shared: the product if that separates the
/// namesakes, else the address, else the user, else all three.
pub fn disambiguated_labels(parts: &[LabelParts]) -> Vec<String> {
    let same = |a: &LabelParts, b: &LabelParts| a.name.eq_ignore_ascii_case(&b.name);
    parts
        .iter()
        .map(|p| {
            let group: Vec<&LabelParts> = parts.iter().filter(|q| same(p, q)).collect();
            if group.len() == 1 {
                return p.name.clone();
            }
            let distinct = |f: &dyn Fn(&LabelParts) -> String| {
                let mut seen: Vec<String> = group.iter().map(|q| f(q)).collect();
                seen.sort();
                seen.dedup();
                seen.len() == group.len()
            };
            let product = |q: &LabelParts| q.product.to_string();
            let address = |q: &LabelParts| q.address.clone();
            let user = |q: &LabelParts| q.user.clone();
            let all = |q: &LabelParts| format!("{} · {} · {}", q.product, q.address, q.user);
            let pick: &dyn Fn(&LabelParts) -> String = if distinct(&product) {
                &product
            } else if distinct(&address) {
                &address
            } else if distinct(&user) {
                &user
            } else {
                &all
            };
            format!("{} · {}", p.name, pick(p))
        })
        .collect()
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
        let mut ssh_profiles = Vec::new();
        if let Some(p) = &path {
            let sp = ssh_path_for(p);
            match std::fs::read_to_string(&sp) {
                Ok(text) if text.trim().is_empty() => {}
                Ok(text) => match serde_json::from_str::<StoredSsh>(&text) {
                    Ok(stored) => ssh_profiles = stored.configs,
                    Err(e) => {
                        let backup = sp.with_extension("json.broken");
                        let moved = std::fs::rename(&sp, &backup).is_ok();
                        warning = Some(format!(
                            "ssh_configs.json could not be read: {e}.{}",
                            if moved { format!(" It was kept as {}.", backup.display()) } else { String::new() }
                        ));
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warning = Some(theta_core::friendly::io_message(&e, &sp, theta_core::friendly::IoOp::Read)),
            }
        }
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
                            "connections.json could not be read: {e}.{}",
                            if moved {
                                format!(" It was kept as {}.", backup.display())
                            } else {
                                String::new()
                            }
                        ));
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warning = Some(theta_core::friendly::io_message(&e, p, theta_core::friendly::IoOp::Read)),
            }
        }
        // Files from before SSH configurations kept the tunnel inline in each connection.
        let migrated = ssh::migrate_inline(&mut configs, &mut ssh_profiles, &secrets);
        let store = Self {
            configs,
            states: HashMap::new(),
            catalog_errors: HashMap::new(),
            temporary: HashSet::new(),
            layout,
            ssh: ssh_profiles,
            secrets,
            path,
        };
        if migrated && let Some(p) = &store.path {
            // An older Theta cannot read references, so keep what it wrote, once.
            let backup = p.with_extension("json.bak");
            if !backup.exists() {
                let _ = std::fs::copy(p, &backup);
            }
            // The configurations first: if the second write fails, the connections still carry their
            // inline tunnel and the next start reuses the configurations written here.
            if let Err(e) = store.write_ssh().and_then(|()| store.write_connections()) {
                warning.get_or_insert(theta_core::friendly::io_message(&e, p, theta_core::friendly::IoOp::Write));
            }
        }
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

    /// Open a session of its own to the same server, for tools that must keep working while a console's
    /// session is busy (the session manager has to list and cancel the very query a console is running) and
    /// that must not share a console's transaction. The caller owns it and closes it. `None` when the
    /// connection does not exist. The saved secrets are applied here, so no password leaves the store.
    pub fn open_side_connection(
        &self,
        id: &str,
    ) -> Option<impl std::future::Future<Output = Result<Arc<dyn Connection>, Error>> + use<>> {
        let config = self.config(id)?.clone();
        let resolver = self.resolver();
        Some(runtime::spawn_result(async move {
            let config = resolver.runnable(&config)?;
            // The reason is worded here, where the host and port are known; callers show it as it is.
            let (connection, _notices) = open_connection(&config)
                .await
                .map_err(|e| Error::Other(connect_failure_reason(&e, Some(&config))))?;
            Ok(Arc::from(connection))
        }))
    }

    pub fn resolver(&self) -> Resolver {
        Resolver { profiles: self.ssh.clone(), secrets: self.secrets.clone() }
    }

    /// The saved connection as it is run, best effort, for wording an error: the tunnel's host is what
    /// a failed tunnel's message names. Never fails; an unresolvable tunnel is just absent.
    pub fn config_for_messages(&self, id: &str) -> Option<ConnectionConfig> {
        let c = self.config_with_secrets(id)?;
        Some(self.resolver().attach_ssh(c.clone()).unwrap_or(c))
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

    fn write_connections(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
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
        theta_ui::settings::write_atomic(path, json.as_bytes())
    }

    fn write_ssh(&self) -> std::io::Result<()> {
        let Some(path) = self.path.as_deref().map(ssh_path_for) else { return Ok(()) };
        let stored = StoredSsh { configs: self.ssh.iter().map(SshProfile::stripped).collect() };
        let json = serde_json::to_string_pretty(&stored).unwrap_or_default() + "\n";
        theta_ui::settings::write_atomic(&path, json.as_bytes())
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else { return };
        if let Err(e) = self.write_connections() {
            cx.emit(StoreEvent::Notice {
                kind: NoticeKind::Error,
                message: theta_core::friendly::io_message(&e, &path, theta_core::friendly::IoOp::Write),
            });
        }
    }

    fn persist_ssh(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.write_ssh() {
            let path = self.path.as_deref().map(ssh_path_for).unwrap_or_default();
            cx.emit(StoreEvent::Notice {
                kind: NoticeKind::Error,
                message: theta_core::friendly::io_message(&e, &path, theta_core::friendly::IoOp::Write),
            });
        }
    }

    // ---- SSH configurations ----

    pub fn ssh_profiles(&self) -> &[SshProfile] {
        &self.ssh
    }

    pub fn ssh_profile(&self, id: &str) -> Option<&SshProfile> {
        self.ssh.iter().find(|p| p.id == id)
    }

    /// The configuration with its secrets filled in (for the edit dialog and the test button).
    pub fn ssh_profile_with_secrets(&self, id: &str) -> Option<SshProfile> {
        self.ssh_profile(id).map(|p| p.clone().with_secret(&self.secrets.load(&ssh::secret_key(id))))
    }

    /// The saved connections that depend on a configuration, directly or through a jump host.
    pub fn ssh_users(&self, id: &str) -> Vec<&ConnectionConfig> {
        ssh::users_of(&self.ssh, &self.configs, id)
    }

    /// Add a configuration (its secrets go to the keychain). Returns its id.
    pub fn add_ssh(&mut self, mut profile: SshProfile, cx: &mut Context<Self>) -> String {
        if profile.id.is_empty() {
            profile.id = ssh::new_id();
        }
        let id = profile.id.clone();
        self.secrets.save(&ssh::secret_key(&id), &profile.secret());
        self.ssh.push(profile.stripped());
        self.ssh_changed(cx);
        id
    }

    /// Replace the configuration with the same id. Sessions that are open keep the tunnel they were
    /// opened with; the change applies the next time a connection connects.
    pub fn update_ssh(&mut self, profile: SshProfile, cx: &mut Context<Self>) {
        let Some(slot) = self.ssh.iter_mut().find(|p| p.id == profile.id) else { return };
        self.secrets.save(&ssh::secret_key(&profile.id), &profile.secret());
        *slot = profile.stripped();
        self.ssh_changed(cx);
    }

    pub fn duplicate_ssh(&mut self, id: &str, cx: &mut Context<Self>) -> Option<String> {
        let mut copy = self.ssh_profile_with_secrets(id)?;
        copy.id = ssh::new_id();
        copy.name = ssh::unique_name(&self.ssh, &format!("{} copy", copy.name));
        Some(self.add_ssh(copy, cx))
    }

    /// Delete a configuration. The connections that use it, and the configurations that jump through
    /// it, are detached: they connect directly from now on. (The dialog tells the user first.)
    pub fn remove_ssh(&mut self, id: &str, cx: &mut Context<Self>) {
        for c in self.configs.iter_mut().filter(|c| c.ssh_profile.as_deref() == Some(id)) {
            c.ssh_profile = None;
        }
        for p in self.ssh.iter_mut().filter(|p| p.via.as_deref() == Some(id)) {
            p.via = None;
        }
        self.ssh.retain(|p| p.id != id);
        self.secrets.remove(&ssh::secret_key(id));
        self.persist(cx);
        self.ssh_changed(cx);
    }

    fn ssh_changed(&mut self, cx: &mut Context<Self>) {
        self.persist_ssh(cx);
        cx.emit(StoreEvent::ListChanged);
        cx.notify();
    }

    // ---- folders and order ----

    pub fn layout(&self) -> &folders::Layout {
        &self.layout
    }

    /// `Folder / Name` for pickers, with the product or address appended when another connection has the
    /// same name (two `quarry_demo @ 127.0.0.1`, one PostgreSQL and one MariaDB).
    pub fn display_name(&self, id: &str) -> String {
        match self.config(id) {
            Some(c) => self.layout.display_name(id, &self.label(id).unwrap_or_else(|| c.name.clone())),
            None => String::new(),
        }
    }

    /// The connection's name, made unique among the saved connections only where it has to be.
    pub fn label(&self, id: &str) -> Option<String> {
        let parts: Vec<LabelParts> = self
            .configs()
            .iter()
            .map(|c| LabelParts {
                name: c.name.clone(),
                product: c.kind.label(),
                address: if c.kind.is_file_based() {
                    c.path.clone()
                } else {
                    format!("{}:{}", c.host, c.port)
                },
                user: c.user.clone(),
            })
            .collect();
        let at = self.configs().iter().position(|c| c.id == id)?;
        disambiguated_labels(&parts).into_iter().nth(at)
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
        let resolver = self.resolver();
        let work = runtime::spawn_result(async move {
            let config = resolver.runnable(&config)?;
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
                        // A connection set up as the wrong product still works (the wire protocol is the
                        // same), but with the wrong catalogs and EXPLAIN; say so once, on connect.
                        let mismatch = this.config(&id).and_then(|c| c.kind.mismatch(&server.banner));
                        this.set_state(
                            &id,
                            ConnState::Connected { connection, server, catalog: None, catalog_loading: true },
                            cx,
                        );
                        if let Some(message) = mismatch {
                            cx.emit(StoreEvent::Notice { kind: NoticeKind::Warning, message });
                        }
                        cx.emit(StoreEvent::Notice {
                            kind: NoticeKind::Success,
                            message: format!("Connected to {name}"),
                        });
                        this.load_catalog(&id, cx);
                    }
                    Err(e) => {
                        let name = this.config(&id).map(|c| c.name.clone()).unwrap_or_default();
                        let reason = connect_failure_reason(&e, this.config_for_messages(&id).as_ref());
                        cx.emit(StoreEvent::Notice {
                            kind: NoticeKind::Error,
                            message: format!("Could not connect to {name}: {reason}"),
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
                            message: format!("Could not read the schema. {}", theta_core::friendly::plain(&e.to_string())),
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
                    banner: String::new(),
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

/// What "Test" says about an SSH configuration: how long the SSH login took, or why it failed in words
/// for the user (and the driver's own text, when the sentence leaves something out).
pub async fn test_ssh(config: theta_core::SshConfig) -> Result<Duration, (String, Option<String>)> {
    let started = Instant::now();
    // The wording of a tunnel error names the SSH server from a connection that has one; a stand-in
    // carries it here, since only the SSH server is tested, not any database behind it.
    let mut stand_in = ConnectionConfig::new("", "", theta_core::DatabaseKind::Postgres);
    stand_in.ssh = Some(config.clone());
    let result = runtime::spawn_result(async move { theta_drivers::ssh::check(&config).await.map(|_| ()) }).await;
    match result {
        Ok(()) => Ok(started.elapsed()),
        Err(e) => {
            let said = theta_core::friendly::connect_error(&e, Some(&stand_in));
            Err((said.message, said.details))
        }
    }
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

#[cfg(test)]
mod ssh_store_tests {
    use super::secrets::Secret;
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};
    use theta_core::{DatabaseKind, SshAuth};

    /// A connections.json as the version before SSH configurations wrote it: the tunnel inline, no
    /// secrets (the keychain holds the passphrase and the password).
    const OLD_FORMAT: &str = r#"{
  "connections": [
    {"id": "c-prod", "name": "prod", "kind": "mysql", "host": "10.0.0.5", "port": 3306, "user": "app",
     "database": "shop", "path": "", "ssl_mode": "prefer",
     "ssh": {"host": "bastion.example.com", "port": 22, "user": "deploy",
             "auth": {"method": "key_file", "path": "/home/me/.ssh/id_ed25519", "passphrase": null}},
     "read_only": false},
    {"id": "c-stage", "name": "stage", "kind": "postgres", "host": "10.0.1.5", "port": 5432, "user": "app",
     "database": "shop", "path": "", "ssl_mode": "prefer",
     "ssh": {"host": "bastion.example.com", "port": 22, "user": "deploy",
             "auth": {"method": "key_file", "path": "/home/me/.ssh/id_ed25519", "passphrase": null}},
     "read_only": true, "folder": "f1"},
    {"id": "c-old", "name": "legacy", "kind": "postgres", "host": "db.old", "port": 5432, "user": "u",
     "database": "", "path": "", "ssl_mode": "prefer",
     "ssh": {"host": "gate.old", "port": 2222, "user": "ops", "auth": {"method": "password", "password": ""}}},
    {"id": "c-direct", "name": "local", "kind": "postgres", "host": "localhost", "port": 5432, "user": "me",
     "database": "", "path": "", "ssl_mode": "prefer"}
  ],
  "folders": [{"id": "f1", "name": "Staging"}]
}"#;

    fn seed_old_secrets(secrets: &SecretStore) {
        secrets.save(
            "c-prod",
            &Secret {
                password: Some("dbpw".into()),
                ssh_passphrase: Some("key-phrase".into()),
                ..Default::default()
            },
        );
        secrets.save("c-stage", &Secret { ssh_passphrase: Some("key-phrase".into()), ..Default::default() });
        secrets.save("c-old", &Secret { ssh_password: Some("gate-pw".into()), ..Default::default() });
    }

    fn old_store(dir: &std::path::Path) -> (ConnectionStore, Arc<SecretStore>, Option<String>) {
        std::fs::write(dir.join("connections.json"), OLD_FORMAT).unwrap();
        let secrets = Arc::new(SecretStore::memory_only());
        seed_old_secrets(&secrets);
        let (s, w) = ConnectionStore::load(Some(dir.join("connections.json")), secrets.clone());
        (s, secrets, w)
    }

    #[test]
    fn an_old_connections_file_becomes_shared_ssh_configurations_without_losing_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let (store, secrets, warning) = old_store(dir.path());
        assert!(warning.is_none(), "{warning:?}");

        // Two connections with the same tunnel share one configuration; the third has its own.
        assert_eq!(store.ssh_profiles().len(), 2);
        let prod = store.config("c-prod").unwrap();
        let stage = store.config("c-stage").unwrap();
        assert_eq!(prod.ssh_profile, stage.ssh_profile);
        let shared = store.ssh_profile(prod.ssh_profile.as_deref().unwrap()).unwrap();
        assert_eq!(shared.name, "deploy@bastion.example.com:22");
        assert_eq!(
            (shared.host.as_str(), shared.port, shared.user.as_str()),
            ("bastion.example.com", 22, "deploy")
        );
        let legacy = store.ssh_profile(store.config("c-old").unwrap().ssh_profile.as_deref().unwrap()).unwrap();
        assert_eq!(legacy.name, "ops@gate.old:2222");
        assert!(store.config("c-direct").unwrap().ssh_profile.is_none());
        assert!(store.configs().iter().all(|c| c.ssh.is_none()), "no inline tunnel survives");

        // Secrets moved to the configurations' own keychain keys; the connection keeps its password.
        let with_secret = store.ssh_profile_with_secrets(&shared.id).unwrap();
        assert_eq!(
            with_secret.auth,
            SshAuth::KeyFile { path: "/home/me/.ssh/id_ed25519".into(), passphrase: Some("key-phrase".into()) }
        );
        assert_eq!(
            store.ssh_profile_with_secrets(&legacy.id).unwrap().auth,
            SshAuth::Password { password: "gate-pw".into() }
        );
        assert_eq!(secrets.load("c-prod"), Secret { password: Some("dbpw".into()), ..Default::default() });
        assert!(secrets.load("c-stage").is_empty() && secrets.load("c-old").is_empty());

        // Resolution gives the driver the full tunnel.
        let run = store.resolver().runnable(store.config("c-stage").unwrap()).unwrap();
        let tunnel = run.ssh.unwrap();
        assert_eq!(tunnel.host, "bastion.example.com");
        assert_eq!(tunnel.auth, with_secret.auth);

        // The files on disk: references, no inline tunnel, no secret anywhere; the old file is kept.
        let connections = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        let configs = std::fs::read_to_string(dir.path().join("ssh_configs.json")).unwrap();
        assert!(!connections.contains("\"ssh\"") && connections.contains("ssh_profile"), "{connections}");
        assert!(connections.contains("Staging") && connections.contains("\"folder\": \"f1\""));
        for text in [&connections, &configs] {
            for secret in ["key-phrase", "gate-pw", "dbpw"] {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
        let backup = std::fs::read_to_string(dir.path().join("connections.json.bak")).unwrap();
        assert_eq!(backup, OLD_FORMAT);
    }

    #[test]
    fn migrating_twice_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (first, secrets, _) = old_store(dir.path());
        let ids: Vec<_> = first.ssh_profiles().iter().map(|p| p.id.clone()).collect();
        let (again, w) = ConnectionStore::load(Some(dir.path().join("connections.json")), secrets);
        assert!(w.is_none());
        assert_eq!(again.ssh_profiles().iter().map(|p| p.id.clone()).collect::<Vec<_>>(), ids);
        assert_eq!(again.config("c-prod").unwrap().ssh_profile, first.config("c-prod").unwrap().ssh_profile);
    }

    #[test]
    fn an_interrupted_migration_reuses_the_configurations_already_written() {
        // The configurations were written, the connections file still has the inline tunnels.
        let dir = tempfile::tempdir().unwrap();
        let (first, secrets, _) = old_store(dir.path());
        let ids: Vec<_> = first.ssh_profiles().iter().map(|p| p.id.clone()).collect();
        std::fs::write(dir.path().join("connections.json"), OLD_FORMAT).unwrap();
        // The connection entries carry their secrets again, as they did before the first run.
        seed_old_secrets(&secrets);
        let (again, _) = ConnectionStore::load(Some(dir.path().join("connections.json")), secrets);
        assert_eq!(again.ssh_profiles().iter().map(|p| p.id.clone()).collect::<Vec<_>>(), ids, "no duplicates");
    }

    fn profile(name: &str, secret: &str) -> SshProfile {
        SshProfile::new(
            name,
            "jump.example",
            "me",
            SshAuth::KeyFile { path: "/k".into(), passphrase: Some(secret.into()) },
        )
    }

    #[gpui_kit::test]
    fn configurations_persist_without_secrets_and_come_back(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let secrets = Arc::new(SecretStore::memory_only());
        let id = {
            let (s, _) = ConnectionStore::load(Some(path.clone()), secrets.clone());
            let store = cx.new(|_| s);
            store.update(cx, |s, cx| s.add_ssh(profile("Bastion", "p4ss"), cx))
        };
        let text = std::fs::read_to_string(dir.path().join("ssh_configs.json")).unwrap();
        assert!(text.contains("Bastion") && !text.contains("p4ss"), "{text}");
        let (again, w) = ConnectionStore::load(Some(path), secrets);
        assert!(w.is_none());
        assert_eq!(again.ssh_profile(&id).unwrap().name, "Bastion");
        assert_eq!(
            again.ssh_profile_with_secrets(&id).unwrap().auth,
            SshAuth::KeyFile { path: "/k".into(), passphrase: Some("p4ss".into()) }
        );
    }

    #[gpui_kit::test]
    fn deleting_a_configuration_detaches_its_connections_and_removes_its_secret(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let secrets = Arc::new(SecretStore::memory_only());
        let (s, _) = ConnectionStore::load(Some(path.clone()), secrets.clone());
        let store = cx.new(|_| s);
        store.update(cx, |s, cx| {
            let outer = s.add_ssh(profile("Outer", "o"), cx);
            let mut inner = profile("Inner", "i");
            inner.via = Some(outer.clone());
            let inner = s.add_ssh(inner, cx);
            for (id, ssh) in [("a", &outer), ("b", &outer), ("c", &inner)] {
                let mut c = ConnectionConfig::new(id, id, DatabaseKind::Postgres);
                c.ssh_profile = Some(ssh.clone());
                s.add(c, cx);
            }
            s.add(ConnectionConfig::new("d", "d", DatabaseKind::Postgres), cx);
            let users: Vec<_> = s.ssh_users(&outer).iter().map(|c| c.id.clone()).collect();
            assert_eq!(users, ["a", "b", "c"], "c reaches the outer one through Inner");
            assert_eq!(s.ssh_users(&inner).len(), 1);

            s.remove_ssh(&outer, cx);
            assert!(s.ssh_profile(&outer).is_none());
            assert!(secrets.load(&ssh::secret_key(&outer)).is_empty());
            assert_eq!(s.config("a").unwrap().ssh_profile, None);
            assert_eq!(s.config("b").unwrap().ssh_profile, None);
            assert_eq!(s.config("c").unwrap().ssh_profile.as_deref(), Some(inner.as_str()));
            assert_eq!(s.ssh_profile(&inner).unwrap().via, None, "the jump host was detached");
            assert_eq!(s.resolver().runnable(s.config("a").unwrap()).unwrap().ssh, None);
        });
        let (again, _) = ConnectionStore::load(Some(path), secrets);
        assert_eq!(again.config("a").unwrap().ssh_profile, None);
        assert_eq!(again.ssh_profiles().len(), 1);
    }

    #[gpui_kit::test]
    fn duplicate_copies_the_secret_and_picks_a_free_name(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(SecretStore::memory_only());
        let (s, _) = ConnectionStore::load(Some(dir.path().join("connections.json")), secrets.clone());
        let store = cx.new(|_| s);
        store.update(cx, |s, cx| {
            let id = s.add_ssh(profile("Bastion", "p4ss"), cx);
            let copy = s.duplicate_ssh(&id, cx).unwrap();
            let again = s.duplicate_ssh(&id, cx).unwrap();
            assert_eq!(s.ssh_profile(&copy).unwrap().name, "Bastion copy");
            assert_eq!(s.ssh_profile(&again).unwrap().name, "Bastion copy (2)");
            assert_eq!(secrets.load(&ssh::secret_key(&copy)).ssh_passphrase.as_deref(), Some("p4ss"));
            // Editing the original leaves the copy alone.
            let mut edited = s.ssh_profile_with_secrets(&id).unwrap();
            edited.host = "elsewhere".into();
            s.update_ssh(edited, cx);
            assert_eq!(s.ssh_profile(&copy).unwrap().host, "jump.example");
        });
    }

    #[test]
    fn an_unresolved_reference_is_an_error_not_a_direct_connection() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = ConnectionStore::load(
            Some(dir.path().join("connections.json")),
            Arc::new(SecretStore::memory_only()),
        );
        let mut c = ConnectionConfig::new("a", "a", DatabaseKind::Postgres);
        c.ssh_profile = Some("gone".into());
        assert!(s.resolver().runnable(&c).is_err());
        // A file database never tunnels, so a stale reference on one does not matter.
        let mut f = ConnectionConfig::new("f", "f", DatabaseKind::Sqlite);
        f.ssh_profile = Some("gone".into());
        assert!(s.resolver().runnable(&f).unwrap().ssh.is_none());
    }

    /// A connection that uses a shared SSH configuration reaches MariaDB through the tunnel. Needs the
    /// local sshd and MariaDB (see crates/theta-drivers/tests/ssh/setup.sh): `THETA_TEST_SSH=1`.
    #[test]
    fn a_shared_configuration_reaches_mariadb_through_the_tunnel() {
        if std::env::var("THETA_TEST_SSH").as_deref() != Ok("1") {
            eprintln!("skipped: set THETA_TEST_SSH=1");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let keys = std::env::var("THETA_TEST_SSH_DIR").unwrap_or_else(|_| "/tmp/theta-ssh-test".into());
        let port = std::env::var("THETA_TEST_SSH_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(2222);
        let secrets = Arc::new(SecretStore::memory_only());
        let (mut store, _) = ConnectionStore::load(Some(dir.path().join("connections.json")), secrets);
        let mut p = SshProfile::new(
            "local sshd",
            "127.0.0.1",
            "thetatest",
            SshAuth::KeyFile { path: format!("{keys}/client_ed25519"), passphrase: None },
        );
        p.port = port;
        let id = p.id.clone();
        store.ssh.push(p);
        let make = |cid: &str| {
            let mut c = ConnectionConfig::new(cid, cid, DatabaseKind::MySql);
            c.host = "127.0.0.1".into();
            c.user = "quarry".into();
            c.password = Some("quarry".into());
            c.database = "quarry_demo".into();
            c.ssh_profile = Some(id.clone());
            c
        };
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        for cid in ["first", "second"] {
            let config = store.resolver().runnable(&make(cid)).unwrap();
            assert!(config.ssh.is_some());
            rt.block_on(async {
                let (conn, _) = open_connection(&config).await.expect("connect through the shared tunnel");
                assert!(!conn.server_info().await.unwrap().version.is_empty());
                conn.close().await.unwrap();
            });
        }
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    fn part(name: &str, product: &'static str, address: &str, user: &str) -> LabelParts {
        LabelParts { name: name.into(), product, address: address.into(), user: user.into() }
    }

    #[test]
    fn unique_names_are_left_alone() {
        let l = disambiguated_labels(&[part("a", "PostgreSQL", "h:1", "u"), part("b", "PostgreSQL", "h:1", "u")]);
        assert_eq!(l, ["a", "b"]);
    }

    #[test]
    fn namesakes_get_the_product_then_the_address_then_the_user() {
        let l = disambiguated_labels(&[
            part("db @ 127.0.0.1", "PostgreSQL", "127.0.0.1:5432", "u"),
            part("db @ 127.0.0.1", "MySQL / MariaDB", "127.0.0.1:3306", "u"),
        ]);
        assert_eq!(l, ["db @ 127.0.0.1 · PostgreSQL", "db @ 127.0.0.1 · MySQL / MariaDB"]);
        let l = disambiguated_labels(&[
            part("db", "PostgreSQL", "h:5432", "a"),
            part("DB", "PostgreSQL", "h:5433", "a"),
        ]);
        assert_eq!(l, ["db · h:5432", "DB · h:5433"]);
        let l = disambiguated_labels(&[part("db", "PostgreSQL", "h:1", "a"), part("db", "PostgreSQL", "h:1", "b")]);
        assert_eq!(l, ["db · a", "db · b"]);
    }
}
