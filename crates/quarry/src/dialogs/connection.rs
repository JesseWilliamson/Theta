//! The connection dialog (new / edit): kind selector, per-kind fields, SSH
//! tunnel, options, URL paste, inline test and validation.

use crate::connections::url::parse_connection_url;
use crate::connections::{ConnectionStore, TestOutcome, new_connection_id, test_connection};
use crate::explorer::db_icon;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};
use quarry_core::{ConnectionConfig, DatabaseKind, SshAuth, SshConfig, SslMode};
use quarry_ui::DbKindIcon;
use std::path::PathBuf;

const COLORS: [(&str, &str); 5] = [
    ("Red", "#e5534b"),
    ("Orange", "#e0823d"),
    ("Green", "#57ab5a"),
    ("Blue", "#539bf5"),
    ("Purple", "#986ee2"),
];

const SSL_LABELS: [&str; 4] = ["Disable", "Prefer", "Require", "Verify full"];
const SSL_MODES: [SslMode; 4] = [SslMode::Disable, SslMode::Prefer, SslMode::Require, SslMode::VerifyFull];
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
    window.open_dialog(cx, move |dialog, _, _| dialog.title(title).w(px(600.)).child(form.clone()));
}

enum TestState {
    Idle,
    Testing,
    Ok(TestOutcome),
    Failed(String),
}

pub struct ConnectionForm {
    store: Entity<ConnectionStore>,
    workspace: WeakEntity<Workspace>,
    config_id: String,
    kind: DatabaseKind,
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
    color: Option<&'static str>,
    errors: Vec<(&'static str, String)>,
    test: TestState,
    keychain_ok: Option<bool>,
    test_epoch: u64,
    _subscriptions: Vec<Subscription>,
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
        let port = if c.port == 0 { String::new() } else { c.port.to_string() };
        let ssl_ix = SSL_MODES.iter().position(|m| *m == c.ssl_mode).unwrap_or(1);

        let url =
            input(window, cx, "Paste a connection URL, e.g. postgres://user:password@host:5432/database", "");
        let name = input(window, cx, "Production (read-only)", &c.name);
        let host = input(window, cx, "localhost", &c.host);
        let port = input(window, cx, "5432", &port);
        let user = input(window, cx, "postgres", &c.user);
        let password = secret_input(window, cx, "Password", c.password.as_deref().unwrap_or(""));
        let database = input(window, cx, "database name", &c.database);
        let path = input(window, cx, "/path/to/database.sqlite", &c.path);
        let ssh_host =
            input(window, cx, "bastion.example.com", ssh.as_ref().map(|s| s.host.as_str()).unwrap_or(""));
        let ssh_port =
            input(window, cx, "22", &ssh.as_ref().map(|s| s.port.to_string()).unwrap_or_else(|| "22".into()));
        let ssh_user = input(window, cx, "ssh user", ssh.as_ref().map(|s| s.user.as_str()).unwrap_or(""));
        let ssh_key_path = input(window, cx, "~/.ssh/id_ed25519", &ssh_key);
        let ssh_secret = secret_input(window, cx, "Passphrase", &ssh_secret);
        let ssl =
            cx.new(|cx| SelectState::new(SSL_LABELS.to_vec(), Some(IndexPath::new(ssl_ix)), window, cx));
        let ssh_auth = cx.new(|cx| {
            SelectState::new(SSH_AUTH_LABELS.to_vec(), Some(IndexPath::new(ssh_auth_ix)), window, cx)
        });

        let mut subscriptions = Vec::new();
        // Pasting a URL fills the form as soon as it parses.
        subscriptions.push(cx.subscribe_in(&url, window, |this, state, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                let text = state.read(cx).value().to_string();
                if text.contains("://") || text.starts_with("sqlite:") {
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
            subscriptions.push(cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save(true, window, cx);
                }
            }));
        }
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
            color: COLORS.iter().find(|(_, hex)| Some(*hex) == c.color.as_deref()).map(|(_, h)| *h),
            errors: Vec::new(),
            test: TestState::Idle,
            keychain_ok: None,
            test_epoch: 0,
            _subscriptions: subscriptions,
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
                self.kind = p.kind;
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
                if self.text(&self.name, cx).is_empty() {
                    Self::set_text(&self.name, &p.suggested_name(), window, cx);
                }
                if let Some(ssl) = p.ssl_mode
                    && let Some(ix) = SSL_MODES.iter().position(|m| *m == ssl)
                {
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
        self.errors.clear();
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
        config.color = self.color.map(str::to_string);
        if self.kind.is_file_based() {
            config.path = self.text(&self.path, cx);
            config.host.clear();
            config.port = 0;
            if config.path.is_empty() {
                errors.push(("path", "Choose a database file.".into()));
            } else if !self.create_if_missing
                && config.path != ":memory:"
                && !std::path::Path::new(&config.path).exists()
            {
                errors.push((
                    "path",
                    "This file does not exist. Turn on “Create if missing” to create it.".into(),
                ));
            }
        } else {
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
            let password = state_raw(&self.password, cx);
            config.password = (!password.is_empty()).then_some(password);
            config.database = self.text(&self.database, cx);
            config.ssl_mode = self
                .ssl
                .read(cx)
                .selected_index(cx)
                .and_then(|ix| SSL_MODES.get(ix.row).copied())
                .unwrap_or_default();
        }
        if self.ssh_enabled && !self.kind.is_file_based() {
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
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::File::create(&path).map_err(|e| format!("Could not create {}: {e}", path.display()))?;
        }
        Ok(())
    }

    fn save(&mut self, connect: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.build(cx) else {
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

    fn run_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.build(cx) else {
            cx.notify();
            return;
        };
        if self.kind.is_file_based() && self.create_if_missing && !std::path::Path::new(&config.path).exists()
        {
            self.test = TestState::Failed("The file will be created when you save the connection.".into());
            cx.notify();
            return;
        }
        self.test_epoch += 1;
        let epoch = self.test_epoch;
        self.test = TestState::Testing;
        cx.notify();
        let _ = window;
        cx.spawn(async move |this, cx| {
            let result = test_connection(config).await;
            this.update(cx, |this, cx| {
                if this.test_epoch != epoch {
                    return;
                }
                this.test = match result {
                    Ok(outcome) => TestState::Ok(outcome),
                    Err(e) => TestState::Failed(e.to_string()),
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
        let file_based = self.kind.is_file_based();
        let max_h = (f32::from(window.viewport_size().height) - 200.0).max(240.0);

        let kind_button = |kind: DatabaseKind, cx: &mut Context<Self>| {
            let selected = self.kind == kind;
            let label: SharedString = kind.label().into();
            let theme = theme.clone();
            h_flex()
                .id(SharedString::from(format!("kind-{kind:?}")))
                .flex_1()
                .h(px(34.))
                .gap_2()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(if selected { theme.ring } else { theme.border })
                .when(selected, |d| d.bg(theme.list_active))
                .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
                .cursor_default()
                .on_click(cx.listener(move |this, _, window, cx| this.set_kind(kind, window, cx)))
                .child(DbKindIcon::new(db_icon(kind)).size(px(15.)))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(if selected { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                        .child(label),
                )
        };

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
                        "Connection successful · {} · {} ms",
                        o.server_version,
                        o.latency.as_millis()
                    )))
                    .into_any_element(),
            ),
            TestState::Failed(e) => Some(
                h_flex()
                    .gap_2()
                    .items_start()
                    .p_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.danger.opacity(0.5))
                    .bg(theme.danger.opacity(0.1))
                    .child(Icon::new(IconName::CircleAlert).size(px(14.)).text_color(theme.danger).mt(px(1.)))
                    .child(div().flex_1().min_w_0().text_size(px(12.)).child(e.clone()))
                    .into_any_element(),
            ),
        };
        let testing = matches!(self.test, TestState::Testing);

        let body = v_flex()
            .gap_3()
            .pt_3()
            .pr_2()
            .child(h_flex().gap_2().children(DatabaseKind::ALL.iter().map(|k| kind_button(*k, cx))))
            .child(field(
                "Connection URL",
                Input::new(&self.url)
                    .small()
                    .prefix(Icon::new(IconName::Link).size(px(13.)).text_color(theme.muted_foreground)),
                self.error_for("url"),
                cx,
            ))
            .child(field("Name", Input::new(&self.name), self.error_for("name"), cx))
            .when(!file_based, |d| {
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
                        .child(field("User", Input::new(&self.user), None, cx))
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
                        .child(field("SSL mode", Select::new(&self.ssl), None, cx)),
                )
            })
            .when(file_based, |d| {
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
                .child(
                    Switch::new("create-missing")
                        .label("Create if missing")
                        .checked(self.create_if_missing)
                        .on_change(cx.listener(|this, v, _, cx| {
                            this.create_if_missing = *v;
                            cx.notify();
                        })),
                )
            })
            // SSH tunnel (network databases only)
            .when(!file_based, |d| {
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
            .when_some(test_row, |d, row| d.child(row));

        v_flex()
            .gap_3()
            .child(div().id("connection-form-scroll").max_h(px(max_h)).overflow_y_scroll().child(body))
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
