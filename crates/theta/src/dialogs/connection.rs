//! The connection dialog (new / edit): kind selector, per-kind fields, SSH
//! tunnel, options, URL paste, inline test and validation.

use crate::connections::url::parse_connection_url;
use crate::connections::{ConnectionStore, TestOutcome, new_connection_id, test_connection};
use crate::explorer::db_icon;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};
use std::path::PathBuf;
use theta_core::Family;
use theta_core::txn::{SqliteBegin, TransactionConfig, isolation_levels};
use theta_core::{ConnectionConfig, DatabaseKind, SshAuth, SshConfig, SslMode};
use theta_ui::DbKindIcon;

const COLORS: [(&str, &str); 5] = [
    ("Red", "#e5534b"),
    ("Orange", "#e0823d"),
    ("Green", "#57ab5a"),
    ("Blue", "#539bf5"),
    ("Purple", "#986ee2"),
];

const SSL_MODES: [SslMode; 4] = [SslMode::Disable, SslMode::Prefer, SslMode::Require, SslMode::VerifyFull];

/// The TLS dropdown's rows for a kind: the modes it implements, with the product's own wording
/// (`KindInfo::ssl_labels`).
fn ssl_choices(kind: DatabaseKind) -> Vec<(SslMode, &'static str)> {
    SSL_MODES.iter().zip(kind.info().ssl_labels).filter_map(|(m, l)| l.map(|l| (*m, l))).collect()
}

fn ssl_labels(kind: DatabaseKind) -> Vec<&'static str> {
    ssl_choices(kind).into_iter().map(|(_, l)| l).collect()
}

fn ssl_index(kind: DatabaseKind, mode: SslMode) -> usize {
    let choices = ssl_choices(kind);
    // A mode the kind does not offer (a config edited by hand, or switched from another kind) falls
    // back to the kind's default rather than to an arbitrary row.
    choices
        .iter()
        .position(|(m, _)| *m == mode)
        .or_else(|| choices.iter().position(|(m, _)| *m == kind.default_ssl_mode()))
        .unwrap_or(0)
}

/// What the TLS dropdown is called: SQL Server says "Encryption", ClickHouse picks a protocol.
fn ssl_title(kind: DatabaseKind) -> &'static str {
    match kind {
        DatabaseKind::SqlServer => "Encryption",
        DatabaseKind::ClickHouse => "Protocol",
        _ => "SSL mode",
    }
}
/// What the URL field suggests for `kind`: its own scheme and default port, not PostgreSQL's.
fn url_placeholder(kind: DatabaseKind) -> String {
    let info = kind.info();
    let scheme = info.url_schemes.first().copied().unwrap_or("postgres");
    if info.sections.file {
        return format!("Paste a connection URL, e.g. {scheme}:///path/to/database");
    }
    let port = info.default_port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("Paste a connection URL, e.g. {scheme}://user:password@host{port}/database")
}

/// The file field's placeholder, marked as an example like the other fields' hints.
fn path_hint(kind: DatabaseKind) -> String {
    let p = kind.info().path_placeholder;
    if p.is_empty() { String::new() } else { format!("e.g. {p}") }
}

const SSH_AUTH_LABELS: [&str; 3] = ["Key file", "Password", "SSH agent"];

pub fn open(
    store: Entity<ConnectionStore>,
    edit: Option<String>,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let form = cx.new(|cx| ConnectionForm::new(store, edit.clone(), workspace, window, cx));
    let title = if edit.is_some() { "Edit Connection" } else { "New Connection" };
    let (shown, focus) = (form.clone(), form.clone());
    // Dialogs sit on the popover colour, not the window background: the editor behind them is the darkest
    // surface, and the dialog would otherwise merge into it.
    window.open_dialog(cx, move |dialog, _, cx| {
        let on_ok = shown.clone();
        // Enter does the same wherever the focus is: save and connect, or say what is missing. Left to
        // the dialog it closed the form without a word.
        dialog.bg(cx.theme().popover).title(title).w(px(600.)).child(shown.clone()).on_ok(
            move |_, window, cx| {
                on_ok.update(cx, |f, cx| f.save(true, window, cx));
                false
            },
        )
    });
    // The first field that is typed into: the URL for a new connection (pasting one fills the form), the
    // name when editing. Taken after the dialog has opened, which moves the focus to itself.
    let editing = edit.is_some();
    window.defer(cx, move |window, cx| {
        let field = focus.read(cx).initial_field(editing);
        field.update(cx, |s, cx| s.focus(window, cx));
    });
}

enum TestState {
    Idle,
    Testing,
    Ok(TestOutcome),
    Failed(String, Option<String>),
}

/// One row of the database-type dropdown, drawn from the kind's descriptor.
#[derive(Clone)]
pub struct KindItem(pub DatabaseKind);

impl gpui_kit::component::select::SelectItem for KindItem {
    type Value = DatabaseKind;

    fn title(&self) -> SharedString {
        self.0.info().label.into()
    }

    fn display_title(&self) -> Option<gpui_kit::AnyElement> {
        Some(self.row(false))
    }

    fn render(&self, _: &mut Window, _: &mut gpui_kit::App) -> impl IntoElement {
        self.row(true)
    }

    fn value(&self) -> &DatabaseKind {
        &self.0
    }

    /// Type-to-filter finds a kind by its label, its URL schemes and its keywords.
    fn matches(&self, query: &str) -> bool {
        kind_matches(self.0, query)
    }
}

/// Does `query` select `kind` in the dropdown?
///
/// A word of the label, a URL scheme or a keyword must start with the query; only from three
/// characters on is a match inside a word enough. A plain substring match made "sq" list
/// PostgreSQL and MySQL next to SQLite, which is not what someone typing the start of a name
/// means, while "sql" should still find all three.
fn kind_matches(kind: DatabaseKind, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let i = kind.info();
    let label = i.label.to_lowercase();
    let fields = std::iter::once(label.as_str())
        .chain(i.url_schemes.iter().copied())
        .chain(i.keywords.iter().copied());
    fields.into_iter().any(|f| {
        let word_start =
            f.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).any(|w| w.starts_with(&q));
        word_start || (q.chars().count() >= 3 && f.contains(&q))
    })
}

impl KindItem {
    fn row(&self, _in_list: bool) -> gpui_kit::AnyElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(DbKindIcon::new(db_icon(self.0)).size(px(15.)))
            .child(div().text_size(px(13.)).child(SharedString::from(self.0.info().label)))
            .into_any_element()
    }
}

pub struct ConnectionForm {
    store: Entity<ConnectionStore>,
    workspace: WeakEntity<Workspace>,
    config_id: String,
    kind: DatabaseKind,
    kind_select: Entity<SelectState<SearchableVec<KindItem>>>,
    url: Entity<InputState>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    database: Entity<InputState>,
    path: Entity<InputState>,
    ssl: Entity<SelectState<Vec<&'static str>>>,
    ssh_enabled: bool,
    ssh_host: Entity<InputState>,
    ssh_port: Entity<InputState>,
    ssh_user: Entity<InputState>,
    ssh_auth: Entity<SelectState<Vec<&'static str>>>,
    ssh_key_path: Entity<InputState>,
    ssh_secret: Entity<InputState>,
    create_if_missing: bool,
    read_only: bool,
    /// Manual transactions by default for this connection (the title bar can still change it per session).
    manual_txn: bool,
    isolation: Entity<SelectState<Vec<&'static str>>>,
    color: Option<&'static str>,
    errors: Vec<(&'static str, String)>,
    test: TestState,
    keychain_ok: Option<bool>,
    test_epoch: u64,
    /// The name this form filled in from a URL. While Name still holds it, a longer URL replaces it, so
    /// typing `postgres://…` one character at a time ends with the final name, not `p`; once the user
    /// writes their own name it stays.
    auto_name: String,
    /// The last input event was a mouse press, so a focus change should place the caret rather
    /// than select the field (Tab selects, like every native form).
    pointer_focus: bool,
    _subscriptions: Vec<Subscription>,
}

/// The choices of the isolation select: isolation levels, or SQLite's BEGIN modes.
fn level_labels(kind: DatabaseKind) -> Vec<&'static str> {
    match kind.family() {
        Family::Sqlite => SqliteBegin::ALL.iter().map(|m| m.label()).collect(),
        _ => isolation_levels(kind).iter().map(|l| l.label()).collect(),
    }
}

fn level_index(kind: DatabaseKind, t: &TransactionConfig) -> usize {
    match kind.family() {
        Family::Sqlite => SqliteBegin::ALL.iter().position(|m| *m == t.sqlite_begin).unwrap_or(0),
        _ => isolation_levels(kind).iter().position(|l| *l == t.isolation).unwrap_or(0),
    }
}

fn input(
    window: &mut Window,
    cx: &mut Context<ConnectionForm>,
    placeholder: &str,
    value: &str,
) -> Entity<InputState> {
    let placeholder = placeholder.to_string();
    let value = value.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

fn secret_input(
    window: &mut Window,
    cx: &mut Context<ConnectionForm>,
    placeholder: &str,
    value: &str,
) -> Entity<InputState> {
    let placeholder = placeholder.to_string();
    let value = value.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value).masked(true))
}

impl ConnectionForm {
    fn new(
        store: Entity<ConnectionStore>,
        edit: Option<String>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let existing = edit.as_ref().and_then(|id| store.read(cx).config_with_secrets(id));
        let c = existing
            .clone()
            .unwrap_or_else(|| ConnectionConfig::new(new_connection_id(), "", DatabaseKind::Postgres));
        let ssh = c.ssh.clone();
        let (ssh_key, ssh_secret, ssh_auth_ix) = match ssh.as_ref().map(|s| &s.auth) {
            Some(SshAuth::KeyFile { path, passphrase }) => {
                (path.clone(), passphrase.clone().unwrap_or_default(), 0)
            }
            Some(SshAuth::Password { password }) => (String::new(), password.clone(), 1),
            Some(SshAuth::Agent) => (String::new(), String::new(), 2),
            None => (String::new(), String::new(), 0),
        };
        let info = c.kind.info();
        let port = if c.port == 0 { String::new() } else { c.port.to_string() };
        let ssl_ix = ssl_index(c.kind, c.ssl_mode);

        let url = input(window, cx, &url_placeholder(c.kind), "");
        let name = input(window, cx, "Production (read-only)", &c.name);
        let host = input(window, cx, info.host_placeholder, &c.host);
        let port = input(window, cx, &info.default_port.map(|p| p.to_string()).unwrap_or_default(), &port);
        // "e.g." keeps an example from passing for a value that is filled in.
        let user = input(window, cx, &format!("e.g. {}", info.user_placeholder), &c.user);
        let password = secret_input(window, cx, "Password", c.password.as_deref().unwrap_or(""));
        let database = input(window, cx, info.database_placeholder, &c.database);
        let path = input(window, cx, &path_hint(c.kind), &c.path);
        let ssh_host =
            input(window, cx, "bastion.example.com", ssh.as_ref().map(|s| s.host.as_str()).unwrap_or(""));
        let ssh_port =
            input(window, cx, "22", &ssh.as_ref().map(|s| s.port.to_string()).unwrap_or_else(|| "22".into()));
        let ssh_user = input(window, cx, "ssh user", ssh.as_ref().map(|s| s.user.as_str()).unwrap_or(""));
        let ssh_key_path = input(window, cx, "~/.ssh/id_ed25519", &ssh_key);
        let ssh_secret = secret_input(window, cx, "Passphrase", &ssh_secret);
        let kind_ix = DatabaseKind::ALL.iter().position(|k| *k == c.kind).unwrap_or(0);
        let kind_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(DatabaseKind::ALL.iter().map(|k| KindItem(*k)).collect::<Vec<_>>()),
                Some(IndexPath::new(kind_ix)),
                window,
                cx,
            )
            .searchable(true)
        });
        let ssl = cx.new(|cx| SelectState::new(ssl_labels(c.kind), Some(IndexPath::new(ssl_ix)), window, cx));
        let isolation = cx.new(|cx| {
            SelectState::new(
                level_labels(c.kind),
                Some(IndexPath::new(level_index(c.kind, &c.transaction))),
                window,
                cx,
            )
        });
        let ssh_auth = cx.new(|cx| {
            SelectState::new(SSH_AUTH_LABELS.to_vec(), Some(IndexPath::new(ssh_auth_ix)), window, cx)
        });

        let mut subscriptions = Vec::new();
        // Pasting a URL fills the form as soon as it parses.
        subscriptions.push(cx.subscribe_in(&url, window, |this, state, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.forget_test(cx);
            }
            if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                let text = state.read(cx).value().to_string();
                if text.contains("://") || text.starts_with("sqlite:") || text.starts_with("duckdb:") {
                    this.apply_url(&text, window, cx);
                }
            }
        }));
        // Enter in any field triggers the primary action.
        for input in [
            &name,
            &host,
            &port,
            &user,
            &password,
            &database,
            &path,
            &ssh_host,
            &ssh_port,
            &ssh_user,
            &ssh_key_path,
            &ssh_secret,
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.save(true, window, cx),
                        // A test result describes the settings it was run with.
                        InputEvent::Change => this.forget_test(cx),
                        // Tabbing into a field selects its text so typing replaces a default.
                        InputEvent::Focus if !this.pointer_focus => {
                            state.update(cx, |s, cx| s.select_all(window, cx))
                        }
                        _ => {}
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &kind_select,
            window,
            |this, _, ev: &gpui_kit::component::select::SelectEvent<SearchableVec<KindItem>>, window, cx| {
                if let gpui_kit::component::select::SelectEvent::Confirm(Some(item)) = ev {
                    this.set_kind(*item, window, cx);
                }
            },
        ));
        // ClickHouse's HTTPS port differs from its HTTP one: follow the protocol while the port is a default.
        subscriptions.push(cx.subscribe_in(
            &ssl,
            window,
            |this, _, ev: &gpui_kit::component::select::SelectEvent<Vec<&'static str>>, window, cx| {
                let gpui_kit::component::select::SelectEvent::Confirm(Some(_)) = ev else { return };
                let Some(tls) = this.kind.info().tls_port else { return };
                let plain = this.kind.default_port().map(|p| p.to_string()).unwrap_or_default();
                let current = this.text(&this.port, cx);
                let secure = this
                    .ssl
                    .read(cx)
                    .selected_index(cx)
                    .and_then(|ix| ssl_choices(this.kind).get(ix.row).map(|(m, _)| *m))
                    .is_some_and(|m| m != SslMode::Disable);
                if current.is_empty() || current == plain || current == tls.to_string() {
                    Self::set_text(&this.port, &if secure { tls.to_string() } else { plain }, window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &ssh_auth,
            window,
            |_, _, _: &gpui_kit::component::select::SelectEvent<Vec<&'static str>>, _, cx| cx.notify(),
        ));

        // Probing the keychain can block on D-Bus, so do it off the UI thread.
        let secrets = store.read(cx).secrets();
        cx.spawn(async move |this, cx| {
            let ok = cx.background_spawn(async move { secrets.persistent() }).await;
            this.update(cx, |this, cx| {
                this.keychain_ok = Some(ok);
                cx.notify();
            })
            .ok();
        })
        .detach();

        Self {
            store,
            workspace,
            config_id: c.id.clone(),
            kind: c.kind,
            kind_select,
            url,
            name,
            host,
            port,
            user,
            password,
            database,
            path,
            ssl,
            ssh_enabled: ssh.is_some(),
            ssh_host,
            ssh_port,
            ssh_user,
            ssh_auth,
            ssh_key_path,
            ssh_secret,
            create_if_missing: false,
            read_only: c.read_only,
            manual_txn: c.transaction.manual,
            isolation,
            color: COLORS.iter().find(|(_, hex)| Some(*hex) == c.color.as_deref()).map(|(_, h)| *h),
            errors: Vec::new(),
            test: TestState::Idle,
            pointer_focus: false,
            keychain_ok: None,
            test_epoch: 0,
            auto_name: String::new(),
            _subscriptions: subscriptions,
        }
    }

    fn initial_field(&self, editing: bool) -> Entity<InputState> {
        if editing { self.name.clone() } else { self.url.clone() }
    }

    /// Put the cursor in the first field that failed validation.
    fn focus_first_error(&self, window: &mut Window, cx: &mut Context<Self>) {
        let field = |name: &str| match name {
            "url" => Some(&self.url),
            "name" => Some(&self.name),
            "host" => Some(&self.host),
            "port" => Some(&self.port),
            "user" => Some(&self.user),
            "path" => Some(&self.path),
            "ssh_host" => Some(&self.ssh_host),
            "ssh_port" => Some(&self.ssh_port),
            "ssh_user" => Some(&self.ssh_user),
            "ssh_key" => Some(&self.ssh_key_path),
            "ssh_secret" => Some(&self.ssh_secret),
            _ => None,
        };
        if let Some(input) = self.errors.iter().find_map(|(f, _)| field(f)) {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn text(&self, state: &Entity<InputState>, cx: &gpui_kit::App) -> String {
        state.read(cx).value().trim().to_string()
    }

    fn set_text(state: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.to_string();
        state.update(cx, |s, cx| s.set_value(value, window, cx));
    }

    fn apply_url(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match parse_connection_url(text) {
            Ok(p) => {
                self.set_kind(p.kind, window, cx);
                Self::set_text(&self.host, &p.host, window, cx);
                Self::set_text(
                    &self.port,
                    &if p.port == 0 { String::new() } else { p.port.to_string() },
                    window,
                    cx,
                );
                Self::set_text(&self.user, &p.user, window, cx);
                Self::set_text(&self.password, p.password.as_deref().unwrap_or(""), window, cx);
                Self::set_text(&self.database, &p.database, window, cx);
                Self::set_text(&self.path, &p.path, window, cx);
                let current = self.text(&self.name, cx);
                if current.is_empty() || current == self.auto_name {
                    let suggested = p.suggested_name();
                    Self::set_text(&self.name, &suggested, window, cx);
                    self.auto_name = suggested;
                }
                if let Some(ssl) = p.ssl_mode {
                    let ix = ssl_index(p.kind, ssl);
                    self.ssl.update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(ix)), window, cx));
                }
                self.errors.retain(|(f, _)| *f != "url");
            }
            Err(e) => {
                self.errors.retain(|(f, _)| *f != "url");
                self.errors.push(("url", e));
            }
        }
        cx.notify();
    }

    fn set_kind(&mut self, kind: DatabaseKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.kind == kind {
            return;
        }
        // Only replace a port the user has not customised.
        let current_port = self.text(&self.port, cx);
        let default_of_old = self.kind.default_port().map(|p| p.to_string()).unwrap_or_default();
        if current_port.is_empty() || current_port == default_of_old {
            Self::set_text(
                &self.port,
                &kind.default_port().map(|p| p.to_string()).unwrap_or_default(),
                window,
                cx,
            );
        }
        self.kind = kind;
        // The hints are the new product's: a SQL Server user is `sa`, not `postgres`.
        let info = kind.info();
        for (state, hint) in [
            (&self.url, url_placeholder(kind)),
            (&self.host, info.host_placeholder.to_string()),
            (&self.user, format!("e.g. {}", info.user_placeholder)),
            (&self.database, info.database_placeholder.to_string()),
            (&self.path, path_hint(kind)),
        ] {
            state.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
        }
        let ix = ssl_index(kind, kind.default_ssl_mode());
        self.ssl.update(cx, |s, cx| {
            s.set_items(ssl_labels(kind), window, cx);
            s.set_selected_index(Some(IndexPath::new(ix)), window, cx);
        });
        self.isolation.update(cx, |s, cx| {
            s.set_items(level_labels(kind), window, cx);
            s.set_selected_index(Some(IndexPath::new(0)), window, cx);
        });
        // By value, not row: with a filter typed the rows of the dropdown are not `ALL`'s.
        self.kind_select.update(cx, |s, cx| s.set_selected_value(&kind, window, cx));
        self.errors.clear();
        self.forget_test(cx);
        cx.notify();
    }

    fn browse(&mut self, target: BrowseTarget, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update_in(cx, |this, window, cx| {
                let state = match target {
                    BrowseTarget::Database => &this.path,
                    BrowseTarget::SshKey => &this.ssh_key_path,
                };
                Self::set_text(state, &path.to_string_lossy(), window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Validate and build the config. Errors are recorded per field.
    fn build(&mut self, cx: &gpui_kit::App) -> Option<ConnectionConfig> {
        self.errors.clear();
        let mut errors: Vec<(&'static str, String)> = Vec::new();
        let name = self.text(&self.name, cx);
        if name.is_empty() {
            errors.push(("name", "Give the connection a name.".into()));
        }
        let mut config = ConnectionConfig::new(self.config_id.clone(), name, self.kind);
        config.read_only = self.read_only;
        config.transaction = {
            let ix = self.isolation.read(cx).selected_index(cx).map_or(0, |i| i.row);
            let mut t = TransactionConfig { manual: self.manual_txn, ..Default::default() };
            match self.kind.family() {
                Family::Sqlite => t.sqlite_begin = SqliteBegin::ALL.get(ix).copied().unwrap_or_default(),
                _ => t.isolation = isolation_levels(self.kind).get(ix).copied().unwrap_or_default(),
            }
            t
        };
        config.color = self.color.map(str::to_string);
        let sections = self.kind.info().sections;
        if sections.file {
            config.path = self.text(&self.path, cx);
            config.host.clear();
            config.port = 0;
            if config.path.is_empty() {
                errors.push(("path", "Choose a database file.".into()));
            } else if !(sections.create_file && self.create_if_missing)
                && config.path != ":memory:"
                && !std::path::Path::new(&config.path).exists()
            {
                errors.push((
                    "path",
                    "This file does not exist. Turn on “Create if missing” to create it.".into(),
                ));
            }
        }
        if sections.network {
            config.host = self.text(&self.host, cx);
            if config.host.is_empty() {
                errors.push(("host", "Enter a host name.".into()));
            }
            let port = self.text(&self.port, cx);
            match port.parse::<u16>() {
                Ok(p) if p > 0 => config.port = p,
                _ if port.is_empty() => config.port = self.kind.default_port().unwrap_or(0),
                _ => errors.push(("port", "Port must be a number between 1 and 65535.".into())),
            }
            config.user = self.text(&self.user, cx);
            if config.user.is_empty() {
                errors.push(("user", "Enter a user name.".into()));
            }
            let password = state_raw(&self.password, cx);
            config.password = (!password.is_empty()).then_some(password);
            config.database = self.text(&self.database, cx);
            if sections.ssl {
                config.ssl_mode = self
                    .ssl
                    .read(cx)
                    .selected_index(cx)
                    .and_then(|ix| ssl_choices(self.kind).get(ix.row).map(|(m, _)| *m))
                    .unwrap_or_else(|| self.kind.default_ssl_mode());
            }
        }
        if self.ssh_enabled && sections.ssh {
            let host = self.text(&self.ssh_host, cx);
            let user = self.text(&self.ssh_user, cx);
            if host.is_empty() {
                errors.push(("ssh_host", "Enter the SSH host.".into()));
            }
            if user.is_empty() {
                errors.push(("ssh_user", "Enter the SSH user.".into()));
            }
            let port = match self.text(&self.ssh_port, cx).parse::<u16>() {
                Ok(p) if p > 0 => p,
                _ => {
                    errors.push(("ssh_port", "SSH port must be between 1 and 65535.".into()));
                    22
                }
            };
            let secret = state_raw(&self.ssh_secret, cx);
            let auth_ix = self.ssh_auth.read(cx).selected_index(cx).map(|i| i.row).unwrap_or(0);
            let auth = match auth_ix {
                1 => {
                    if secret.is_empty() {
                        errors.push(("ssh_secret", "Enter the SSH password.".into()));
                    }
                    SshAuth::Password { password: secret }
                }
                2 => SshAuth::Agent,
                _ => {
                    let key = self.text(&self.ssh_key_path, cx);
                    if key.is_empty() {
                        errors.push(("ssh_key", "Choose a private key file.".into()));
                    }
                    SshAuth::KeyFile { path: key, passphrase: (!secret.is_empty()).then_some(secret) }
                }
            };
            config.ssh = Some(SshConfig { host, port, user, auth });
        }
        if errors.is_empty() {
            Some(config)
        } else {
            self.errors = errors;
            None
        }
    }

    fn create_file_if_needed(&self, config: &ConnectionConfig) -> Result<(), String> {
        if config.kind.is_file_based()
            && self.create_if_missing
            && config.path != ":memory:"
            && !std::path::Path::new(&config.path).exists()
        {
            let path = PathBuf::from(&config.path);
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| {
                    format!(
                        "The folder {} could not be created. {}",
                        dir.display(),
                        theta_core::friendly::plain(&e.to_string())
                    )
                })?;
            }
            std::fs::File::create(&path)
                .map_err(|e| theta_core::friendly::io_message(&e, &path, theta_core::friendly::IoOp::Write))?;
        }
        Ok(())
    }

    fn save(&mut self, connect: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.build(cx) else {
            self.focus_first_error(window, cx);
            cx.notify();
            return;
        };
        if let Err(e) = self.create_file_if_needed(&config) {
            self.errors.push(("path", e));
            cx.notify();
            return;
        }
        let id = config.id.clone();
        let exists = self.store.read(cx).config(&id).is_some();
        self.store.update(cx, |s, cx| {
            if exists {
                s.update(config, cx);
            } else {
                s.add(config, cx);
            }
        });
        window.close_dialog(cx);
        if connect {
            self.workspace.update(cx, |ws, cx| ws.use_connection(&id, cx)).ok();
        }
    }

    /// Drop the banner (and ignore a test still running) when the settings it speaks about change.
    fn forget_test(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.test, TestState::Idle) {
            self.test_epoch += 1;
            self.test = TestState::Idle;
            cx.notify();
        }
    }

    fn run_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.build(cx) else {
            self.focus_first_error(window, cx);
            cx.notify();
            return;
        };
        if self.kind.is_file_based() && self.create_if_missing && !std::path::Path::new(&config.path).exists()
        {
            self.test = TestState::Failed("The file will be created when you save the connection.".into(), None);
            cx.notify();
            return;
        }
        self.test_epoch += 1;
        let epoch = self.test_epoch;
        self.test = TestState::Testing;
        cx.notify();
        let _ = window;
        let shown_config = config.clone();
        cx.spawn(async move |this, cx| {
            let result = test_connection(config).await;
            this.update(cx, |this, cx| {
                if this.test_epoch != epoch {
                    return;
                }
                this.test = match result {
                    Ok(outcome) => TestState::Ok(outcome),
                    Err(e) => TestState::Failed(
                        crate::connections::connect_failure_reason(&e, Some(&shown_config)),
                        crate::connections::connect_failure_details(&e, Some(&shown_config)),
                    ),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn error_for(&self, field: &str) -> Option<&String> {
        self.errors.iter().find(|(f, _)| *f == field).map(|(_, m)| m)
    }
}

fn state_raw(state: &Entity<InputState>, cx: &gpui_kit::App) -> String {
    state.read(cx).value().to_string()
}

#[derive(Clone, Copy)]
enum BrowseTarget {
    Database,
    SshKey,
}

fn field(
    label: &'static str,
    control: impl IntoElement,
    error: Option<&String>,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(control)
        .when_some(error.cloned(), |d, e| d.child(div().text_size(px(12.)).text_color(theme.danger).child(e)))
}

impl Render for ConnectionForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sections = self.kind.info().sections;
        // Header, the result slot and the footer stay; the fields scroll in what is left of the window.
        let max_h = (f32::from(window.viewport_size().height) - 276.0).max(200.0);

        let test_row = match &self.test {
            TestState::Idle => None,
            TestState::Testing => Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .p_2()
                    .child(Spinner::new().xsmall())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("Testing connection…"),
                    )
                    .into_any_element(),
            ),
            TestState::Ok(o) => Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .p_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.success.opacity(0.5))
                    .bg(theme.success.opacity(0.1))
                    .child(Icon::new(IconName::CircleCheck).size(px(14.)).text_color(theme.success))
                    .child(div().text_size(px(12.)).child(format!(
                        "Connection successful · {} · {}",
                        o.server_version,
                        theta_core::text::duration(o.latency)
                    )))
                    .into_any_element(),
            ),
            TestState::Failed(e, details) => Some(
                h_flex()
                    .gap_2()
                    .items_start()
                    .p_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.danger.opacity(0.5))
                    .bg(theme.danger.opacity(0.1))
                    .child(Icon::new(IconName::CircleAlert).size(px(14.)).text_color(theme.danger).mt(px(1.)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_size(px(12.)).child(e.clone()))
                            .when_some(details.clone(), |d, t| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(format!("Details: {t}")),
                                )
                            }),
                    )
                    .into_any_element(),
            ),
        };
        let testing = matches!(self.test, TestState::Testing);

        let body = v_flex()
            .gap_3()
            .pt_3()
            .px_1p5()
            .pb_1()
            .child(field(
                "Database type",
                Select::new(&self.kind_select).placeholder("Choose a database type"),
                None,
                cx,
            ))
            .child(field(
                "Connection URL",
                Input::new(&self.url)
                    .small()
                    .prefix(Icon::new(IconName::Link).size(px(13.)).text_color(theme.muted_foreground)),
                self.error_for("url"),
                cx,
            ))
            .child(field("Name", Input::new(&self.name), self.error_for("name"), cx))
            .when(sections.network, |d| {
                d.child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(field("Host", Input::new(&self.host), self.error_for("host"), cx))
                        .child(div().w(px(110.)).child(field(
                            "Port",
                            Input::new(&self.port),
                            self.error_for("port"),
                            cx,
                        ))),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(field("User", Input::new(&self.user), self.error_for("user"), cx))
                        .child(field("Password", Input::new(&self.password).mask_toggle(), None, cx)),
                )
                .when(self.keychain_ok == Some(false), |d| {
                    d.child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(Icon::new(IconName::Info).size(px(12.)).text_color(theme.muted_foreground))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child("Password will not be saved (no system keychain)"),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(field("Database", Input::new(&self.database), None, cx))
                        .children(
                            sections
                                .ssl
                                .then(|| field(ssl_title(self.kind), Select::new(&self.ssl), None, cx)),
                        ),
                )
            })
            .when(sections.file, |d| {
                d.child(field(
                    "Database file",
                    h_flex().gap_2().child(div().flex_1().child(Input::new(&self.path))).child(
                        Button::new("browse-db").label("Browse…").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.browse(BrowseTarget::Database, window, cx)
                            }),
                        ),
                    ),
                    self.error_for("path"),
                    cx,
                ))
                .when(sections.create_file, |d| {
                    d.child(
                        Switch::new("create-missing")
                            .label("Create if missing")
                            .checked(self.create_if_missing)
                            .on_change(cx.listener(|this, v, _, cx| {
                                this.create_if_missing = *v;
                                cx.notify();
                            })),
                    )
                })
            })
            // SSH tunnel (kinds that connect over a network)
            .when(sections.ssh, |d| {
                d.child(div().h(px(1.)).bg(theme.border))
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::KeyRound)
                                            .size(px(14.))
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(div().font_weight(FontWeight::MEDIUM).child("SSH tunnel")),
                            )
                            .child(Switch::new("ssh-enabled").checked(self.ssh_enabled).on_change(
                                cx.listener(|this, v, _, cx| {
                                    this.ssh_enabled = *v;
                                    cx.notify();
                                }),
                            )),
                    )
                    .when(self.ssh_enabled, |d| {
                        let auth_ix = self.ssh_auth.read(cx).selected_index(cx).map(|i| i.row).unwrap_or(0);
                        d.child(
                            h_flex()
                                .gap_3()
                                .items_start()
                                .child(field(
                                    "SSH host",
                                    Input::new(&self.ssh_host),
                                    self.error_for("ssh_host"),
                                    cx,
                                ))
                                .child(div().w(px(110.)).child(field(
                                    "Port",
                                    Input::new(&self.ssh_port),
                                    self.error_for("ssh_port"),
                                    cx,
                                ))),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .items_start()
                                .child(field(
                                    "User",
                                    Input::new(&self.ssh_user),
                                    self.error_for("ssh_user"),
                                    cx,
                                ))
                                .child(field("Authentication", Select::new(&self.ssh_auth), None, cx)),
                        )
                        .when(auth_ix == 0, |d| {
                            d.child(field(
                                "Private key",
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().child(Input::new(&self.ssh_key_path)))
                                    .child(Button::new("browse-key").label("Browse…").on_click(cx.listener(
                                        |this, _, window, cx| this.browse(BrowseTarget::SshKey, window, cx),
                                    ))),
                                self.error_for("ssh_key"),
                                cx,
                            ))
                            .child(field(
                                "Passphrase",
                                Input::new(&self.ssh_secret).mask_toggle(),
                                None,
                                cx,
                            ))
                        })
                        .when(auth_ix == 1, |d| {
                            d.child(field(
                                "SSH password",
                                Input::new(&self.ssh_secret).mask_toggle(),
                                self.error_for("ssh_secret"),
                                cx,
                            ))
                        })
                    })
            })
            // Options
            .child(div().h(px(1.)).bg(theme.border))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Switch::new("read-only").label("Read-only").checked(self.read_only).on_change(
                        cx.listener(|this, v, _, cx| {
                            this.read_only = *v;
                            cx.notify();
                        }),
                    ))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .mr_1()
                                    .child("Colour"),
                            )
                            .child(self.color_swatch(None, cx))
                            .children(COLORS.iter().map(|(_, hex)| self.color_swatch(Some(*hex), cx))),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    // ClickHouse over HTTP has no transactions to make manual.
                    .children(self.kind.unsupported(theta_core::Feature::Transactions).is_none().then(|| {
                        Switch::new("manual-txn")
                            .label("Manual transactions")
                            .checked(self.manual_txn)
                            .on_change(cx.listener(|this, v, _, cx| {
                                this.manual_txn = *v;
                                cx.notify();
                            }))
                    }))
                    // A product with no isolation levels to choose (DuckDB, ClickHouse) shows no dropdown.
                    .children((!level_labels(self.kind).is_empty()).then(|| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(
                                if self.kind.family() == Family::Sqlite { "BEGIN mode" } else { "Isolation" },
                            ))
                            .child(div().w(px(180.)).child(Select::new(&self.isolation)))
                    })),
            );
        // A fixed slot: the result of a test appears in it without moving the buttons below, and an
        // untouched form says what the button does instead of leaving a gap.
        let result_slot = div().min_h(px(56.)).flex().items_center().child(div().w_full().child(test_row.unwrap_or_else(|| {
            div()
                .px_2()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child("Test Connection checks these settings without saving them.")
                .into_any_element()
        })));

        v_flex()
            .gap_3()
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.pointer_focus = true))
            .capture_key_down(cx.listener(|this, _, _, _| this.pointer_focus = false))
            .child(div().max_h(px(max_h)).overflow_y_scrollbar().child(body))
            .child(result_slot)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .mt_1()
                    .pt_3()
                    // The form above reserves a scrollbar gutter; keep the footer on the same grid.
                    .pr(px(7.))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("test-connection")
                            .icon(Icon::new(IconName::Zap))
                            .label("Test Connection")
                            .loading(testing)
                            .on_click(cx.listener(|this, _, window, cx| this.run_test(window, cx))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("save").label("Save").on_click(
                                    cx.listener(|this, _, window, cx| this.save(false, window, cx)),
                                ),
                            )
                            .child(
                                Button::new("save-connect")
                                    .primary()
                                    .label("Save & Connect")
                                    .on_click(cx.listener(|this, _, window, cx| this.save(true, window, cx))),
                            ),
                    ),
            )
    }
}

impl ConnectionForm {
    fn color_swatch(&self, hex: Option<&'static str>, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selected = self.color == hex;
        let id = SharedString::from(format!("color-{}", hex.unwrap_or("none")));
        let fill = hex.and_then(|h| u32::from_str_radix(&h[1..], 16).ok()).map(gpui_kit::rgb);
        div()
            .id(id)
            .size(px(18.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .border_2()
            .border_color(if selected { theme.ring } else { theme.transparent })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.color = hex;
                cx.notify();
            }))
            .child(match fill {
                Some(c) => div().size(px(12.)).rounded_full().bg(c).into_any_element(),
                None => div()
                    .size(px(12.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.muted_foreground)
                    .into_any_element(),
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use gpui_kit::component::select::SelectDelegate as _;

    fn matching(q: &str) -> Vec<DatabaseKind> {
        DatabaseKind::ALL.iter().copied().filter(|k| kind_matches(*k, q)).collect()
    }

    #[test]
    fn short_queries_match_word_starts_only() {
        assert_eq!(matching(""), DatabaseKind::ALL.to_vec());
        assert_eq!(matching("sq"), vec![DatabaseKind::Sqlite, DatabaseKind::SqlServer]);
        assert_eq!(
            matching("po"),
            vec![DatabaseKind::Postgres, DatabaseKind::CockroachDb, DatabaseKind::TimescaleDb],
            "PostgreSQL, and the products whose keywords say postgres wire / postgis"
        );
        assert_eq!(matching("pg"), vec![DatabaseKind::Postgres], "keyword");
        assert_eq!(matching("maria"), vec![DatabaseKind::MySql], "keyword of a combined label");
        assert_eq!(matching("zz"), vec![]);
    }

    #[test]
    fn longer_queries_match_inside_words() {
        let sql = matching("sql");
        for k in [DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::Sqlite, DatabaseKind::SqlServer]
        {
            assert!(sql.contains(&k), "{k:?} contains it");
        }
        assert_eq!(matching("duck"), vec![DatabaseKind::DuckDb]);
        assert_eq!(matching("cockroach"), vec![DatabaseKind::CockroachDb]);
        assert_eq!(matching("sqli"), vec![DatabaseKind::Sqlite]);
        assert_eq!(matching("SQLITE"), vec![DatabaseKind::Sqlite], "case-insensitive");
    }

    /// The dropdown filtered nothing while its delegate was a plain `Vec` (whose
    /// `perform_search` does nothing); `SearchableVec` is what actually applies `matches`.
    #[gpui_kit::test]
    fn the_dropdown_delegate_filters(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            let mut items =
                SearchableVec::new(DatabaseKind::ALL.iter().map(|k| KindItem(*k)).collect::<Vec<_>>());
            assert_eq!(items.items_count(0), DatabaseKind::ALL.len());
            items.perform_search("sq", window, cx).detach();
            assert_eq!(items.items_count(0), 2);
            assert_eq!(items.item(IndexPath::new(0)).map(|i| i.0), Some(DatabaseKind::Sqlite));
            items.perform_search("", window, cx).detach();
            assert_eq!(items.items_count(0), DatabaseKind::ALL.len());
        });
    }
}
