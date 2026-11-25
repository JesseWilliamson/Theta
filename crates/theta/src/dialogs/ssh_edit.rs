//! The SSH configuration editor: one named tunnel (server, user, how to authenticate, keep-alive and an
//! optional jump host), with a test of the SSH login alone.
//!
//! Opened from the SSH Configurations manager and from the connection dialog; `on_saved` tells the
//! opener which configuration was saved (the connection dialog selects it).

use super::connection::field;
use crate::connections::ssh::{self, SshProfile};
use crate::connections::{ConnectionStore, test_ssh};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use std::rc::Rc;
use std::time::Duration;
use theta_core::{SshAuth, SshConfig};

const AUTH_LABELS: [&str; 3] = ["Key file", "Password", "SSH agent"];

/// Called with the id of the configuration that was saved.
pub type Saved = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// The outcome of a Test button, drawn in a fixed slot so the buttons below do not move.
pub enum TestState {
    Idle,
    Testing,
    Ok(Duration),
    Failed(String, Option<String>),
}

impl TestState {
    /// The banner for a finished or running test; `None` when idle.
    pub fn banner(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        let theme = cx.theme().clone();
        match self {
            TestState::Idle => None,
            TestState::Testing => Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .p_2()
                    .child(Spinner::new().xsmall())
                    .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("Testing the SSH login…"))
                    .into_any_element(),
            ),
            TestState::Ok(took) => Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .p_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.success.opacity(0.5))
                    .bg(theme.success.opacity(0.1))
                    .child(Icon::new(IconName::CircleCheck).size(px(14.)).text_color(theme.success))
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(format!("SSH login successful · {}", theta_core::text::duration(*took))),
                    )
                    .into_any_element(),
            ),
            TestState::Failed(message, details) => Some(
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
                            .child(div().text_size(px(12.)).child(message.clone()))
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
        }
    }
}

/// One row of the jump-host dropdown: a configuration, or "None".
#[derive(Clone)]
pub struct SshChoice {
    pub id: Option<String>,
    pub name: String,
    pub address: String,
}

impl SshChoice {
    pub fn none(label: &str) -> Self {
        Self { id: None, name: label.to_string(), address: String::new() }
    }

    pub fn of(p: &SshProfile) -> Self {
        Self { id: Some(p.id.clone()), name: p.name.clone(), address: p.address() }
    }

    fn row(&self, cx: &App) -> gpui_kit::AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_2()
            .items_baseline()
            .min_w_0()
            .child(div().text_size(px(13.)).child(SharedString::from(self.name.clone())))
            .when(!self.address.is_empty(), |d| {
                d.child(div().text_size(px(12.)).text_color(muted).child(SharedString::from(self.address.clone())))
            })
            .into_any_element()
    }
}

impl gpui_kit::component::select::SelectItem for SshChoice {
    type Value = Option<String>;

    fn title(&self) -> SharedString {
        if self.address.is_empty() {
            self.name.clone().into()
        } else {
            format!("{} · {}", self.name, self.address).into()
        }
    }

    fn display_title(&self) -> Option<gpui_kit::AnyElement> {
        // The trigger is drawn without an `App`; the dropdown rows (below) carry the muted address.
        Some(
            h_flex()
                .gap_2()
                .items_baseline()
                .min_w_0()
                .child(div().text_size(px(13.)).child(SharedString::from(self.name.clone())))
                .when(!self.address.is_empty(), |d| {
                    d.child(div().text_size(px(12.)).opacity(0.6).child(SharedString::from(self.address.clone())))
                })
                .into_any_element(),
        )
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.row(cx)
    }

    fn value(&self) -> &Option<String> {
        &self.id
    }

    fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        self.name.to_lowercase().contains(&q) || self.address.to_lowercase().contains(&q)
    }
}

pub fn open(
    store: Entity<ConnectionStore>,
    edit: Option<String>,
    // The connection being edited, if the connection dialog opened this: it is not counted among the
    // "other connections" that share the configuration.
    exclude: Option<String>,
    on_saved: Option<Saved>,
    window: &mut Window,
    cx: &mut App,
) {
    let editing = edit.is_some();
    let form = cx.new(|cx| SshForm::new(store, edit, exclude, on_saved, window, cx));
    let (shown, focus) = (form.clone(), form.clone());
    let title = if editing { "Edit SSH Configuration" } else { "New SSH Configuration" };
    window.open_dialog(cx, move |dialog, _, cx| {
        let on_ok = shown.clone();
        dialog.bg(cx.theme().popover).title(title).w(px(560.)).child(shown.clone()).on_ok(move |_, window, cx| {
            on_ok.update(cx, |f, cx| f.save(window, cx));
            false
        })
    });
    window.defer(cx, move |window, cx| {
        let first = focus.read(cx).name.clone();
        first.update(cx, |s, cx| s.focus(window, cx));
    });
}

pub struct SshForm {
    store: Entity<ConnectionStore>,
    id: String,
    editing: bool,
    /// How many other saved connections use this configuration, for the note under the title.
    shared_by: Vec<String>,
    /// Opened from a connection's dialog: the count says "other", as that connection is one of the users.
    others: bool,
    on_saved: Option<Saved>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    auth: Entity<SelectState<Vec<&'static str>>>,
    key_path: Entity<InputState>,
    secret: Entity<InputState>,
    keepalive: Entity<InputState>,
    via: Entity<SelectState<SearchableVec<SshChoice>>>,
    errors: Vec<(&'static str, String)>,
    test: TestState,
    test_epoch: u64,
    keychain_ok: Option<bool>,
    _subscriptions: Vec<Subscription>,
}

fn input(window: &mut Window, cx: &mut Context<SshForm>, placeholder: &str, value: &str) -> Entity<InputState> {
    let (placeholder, value) = (placeholder.to_string(), value.to_string());
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

fn secret_input(window: &mut Window, cx: &mut Context<SshForm>, placeholder: &str, value: &str) -> Entity<InputState> {
    let (placeholder, value) = (placeholder.to_string(), value.to_string());
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value).masked(true))
}

/// The dropdown's rows: "None", then every configuration that may be this one's jump host (not itself,
/// and not one that already jumps through it, which would make a loop).
fn jump_choices(profiles: &[SshProfile], this: &str) -> Vec<SshChoice> {
    std::iter::once(SshChoice::none("None (connect directly)"))
        .chain(
            profiles
                .iter()
                .filter(|p| p.id != this && !ssh::depends_on(profiles, &p.id, this))
                .map(SshChoice::of),
        )
        .collect()
}

impl SshForm {
    fn new(
        store: Entity<ConnectionStore>,
        edit: Option<String>,
        exclude: Option<String>,
        on_saved: Option<Saved>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let existing = edit.as_ref().and_then(|id| store.read(cx).ssh_profile_with_secrets(id));
        let p = existing.clone().unwrap_or_else(|| SshProfile::new("", "", "", SshAuth::Agent));
        let (key, secret, auth_ix) = match &p.auth {
            SshAuth::KeyFile { path, passphrase } => (path.clone(), passphrase.clone().unwrap_or_default(), 0),
            SshAuth::Password { password } => (String::new(), password.clone(), 1),
            SshAuth::Agent => (String::new(), String::new(), if existing.is_some() { 2 } else { 0 }),
        };
        let shared_by: Vec<String> = match &existing {
            Some(e) => {
                let s = store.read(cx);
                s.ssh_users(&e.id)
                    .into_iter()
                    .filter(|c| exclude.as_deref() != Some(c.id.as_str()))
                    // The label tells namesakes apart (two `quarry_demo @ 127.0.0.1`).
                    .map(|c| s.label(&c.id).unwrap_or_else(|| c.name.clone()))
                    .collect()
            }
            None => Vec::new(),
        };
        let choices = jump_choices(store.read(cx).ssh_profiles(), &p.id);
        let via_ix = p.via.as_ref().and_then(|v| choices.iter().position(|c| c.id.as_ref() == Some(v))).unwrap_or(0);

        let name = input(window, cx, "Defaults to user@host:port", &p.name);
        let host = input(window, cx, "bastion.example.com", &p.host);
        let port = input(window, cx, "22", &p.port.to_string());
        let user = input(window, cx, "ssh user", &p.user);
        let key_path = input(window, cx, "~/.ssh/id_ed25519", &key);
        let secret = secret_input(window, cx, "Passphrase", &secret);
        let keepalive = input(window, cx, "30", &p.keepalive_secs.to_string());
        let auth = cx.new(|cx| SelectState::new(AUTH_LABELS.to_vec(), Some(IndexPath::new(auth_ix)), window, cx));
        let via = cx.new(|cx| {
            SelectState::new(SearchableVec::new(choices), Some(IndexPath::new(via_ix)), window, cx).searchable(true)
        });

        let mut subscriptions = Vec::new();
        for state in [&name, &host, &port, &user, &key_path, &secret, &keepalive] {
            subscriptions.push(cx.subscribe_in(state, window, |this, state, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    // A test result describes the settings it was run with.
                    InputEvent::Change => this.forget_test(cx),
                    InputEvent::Focus => state.update(cx, |s, cx| s.select_all(window, cx)),
                    _ => {}
                }
            }));
        }
        subscriptions.push(cx.subscribe_in(
            &auth,
            window,
            |this, _, _: &SelectEvent<Vec<&'static str>>, _, cx| {
                this.forget_test(cx);
                cx.notify();
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &via,
            window,
            |this, _, _: &SelectEvent<SearchableVec<SshChoice>>, _, cx| this.forget_test(cx),
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
            id: p.id,
            editing: existing.is_some(),
            shared_by,
            others: exclude.is_some(),
            on_saved,
            name,
            host,
            port,
            user,
            auth,
            key_path,
            secret,
            keepalive,
            via,
            errors: Vec::new(),
            test: TestState::Idle,
            test_epoch: 0,
            keychain_ok: None,
            _subscriptions: subscriptions,
        }
    }

    fn text(&self, state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().trim().to_string()
    }

    fn error_for(&self, field: &str) -> Option<&String> {
        self.errors.iter().find(|(f, _)| *f == field).map(|(_, m)| m)
    }

    fn forget_test(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.test, TestState::Idle) {
            self.test_epoch += 1;
            self.test = TestState::Idle;
            cx.notify();
        }
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let value = path.to_string_lossy().into_owned();
                this.key_path.update(cx, |s, cx| s.set_value(value, window, cx));
            })
            .ok();
        })
        .detach();
    }

    /// Validate the fields and build the configuration, or record what is wrong per field.
    fn build(&mut self, cx: &App) -> Option<SshProfile> {
        self.errors.clear();
        let mut errors: Vec<(&'static str, String)> = Vec::new();
        let host = self.text(&self.host, cx);
        let user = self.text(&self.user, cx);
        if host.is_empty() {
            errors.push(("host", "Enter the SSH host.".into()));
        }
        if user.is_empty() {
            errors.push(("user", "Enter the SSH user.".into()));
        }
        let port = match self.text(&self.port, cx).parse::<u16>() {
            Ok(p) if p > 0 => p,
            _ => {
                errors.push(("port", "Port must be between 1 and 65535.".into()));
                22
            }
        };
        let keepalive_text = self.text(&self.keepalive, cx);
        let keepalive_secs = if keepalive_text.is_empty() {
            SshConfig::DEFAULT_KEEPALIVE_SECS
        } else {
            keepalive_text.parse::<u32>().unwrap_or_else(|_| {
                errors.push(("keepalive", "Enter a number of seconds, or 0 for none.".into()));
                SshConfig::DEFAULT_KEEPALIVE_SECS
            })
        };
        let secret = self.secret.read(cx).value().to_string();
        let auth = match self.auth.read(cx).selected_index(cx).map_or(0, |i| i.row) {
            1 => {
                if secret.is_empty() {
                    errors.push(("secret", "Enter the SSH password.".into()));
                }
                SshAuth::Password { password: secret }
            }
            2 => SshAuth::Agent,
            _ => {
                let path = self.text(&self.key_path, cx);
                if path.is_empty() {
                    errors.push(("key", "Choose a private key file.".into()));
                }
                SshAuth::KeyFile { path, passphrase: (!secret.is_empty()).then_some(secret) }
            }
        };
        let mut name = self.text(&self.name, cx);
        let others: Vec<SshProfile> =
            self.store.read(cx).ssh_profiles().iter().filter(|p| p.id != self.id).cloned().collect();
        if name.is_empty() {
            name = format!("{user}@{host}:{port}");
        }
        if others.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
            errors.push(("name", format!("Another SSH configuration is called “{name}”.")));
        }
        if !errors.is_empty() {
            self.errors = errors;
            return None;
        }
        let via = self.via.read(cx).selected_value().cloned().flatten();
        Some(SshProfile { id: self.id.clone(), name, host, port, user, auth, keepalive_secs, via })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.build(cx) else {
            self.focus_first_error(window, cx);
            cx.notify();
            return;
        };
        let id = profile.id.clone();
        let editing = self.editing;
        self.store.update(cx, |s, cx| {
            if editing {
                s.update_ssh(profile, cx);
            } else {
                s.add_ssh(profile, cx);
            }
        });
        window.close_dialog(cx);
        if let Some(done) = self.on_saved.clone() {
            done(id, window, cx);
        }
    }

    fn focus_first_error(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.errors.iter().find_map(|(f, _)| match *f {
            "name" => Some(&self.name),
            "host" => Some(&self.host),
            "port" => Some(&self.port),
            "user" => Some(&self.user),
            "key" => Some(&self.key_path),
            "secret" => Some(&self.secret),
            "keepalive" => Some(&self.keepalive),
            _ => None,
        });
        if let Some(input) = input {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn run_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.build(cx) else {
            self.focus_first_error(window, cx);
            cx.notify();
            return;
        };
        // The form's values for this server; the jump hosts are the saved ones.
        let via = match &profile.via {
            Some(id) => match self.store.read(cx).resolver().ssh(id) {
                Ok(c) => Some(Box::new(c)),
                Err(e) => {
                    self.test = TestState::Failed(e.to_string(), None);
                    cx.notify();
                    return;
                }
            },
            None => None,
        };
        let config = SshConfig {
            host: profile.host,
            port: profile.port,
            user: profile.user,
            auth: profile.auth,
            keepalive_secs: profile.keepalive_secs,
            via,
        };
        self.test_epoch += 1;
        let epoch = self.test_epoch;
        self.test = TestState::Testing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = test_ssh(config).await;
            this.update(cx, |this, cx| {
                if this.test_epoch != epoch {
                    return;
                }
                this.test = match result {
                    Ok(took) => TestState::Ok(took),
                    Err((message, details)) => TestState::Failed(message, details),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for SshForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let auth_ix = self.auth.read(cx).selected_index(cx).map_or(0, |i| i.row);
        let testing = matches!(self.test, TestState::Testing);
        let other = if self.others { "other " } else { "" };
        let shared = match self.shared_by.len() {
            0 => None,
            1 => Some(format!("1 {other}connection uses this configuration: {}.", self.shared_by[0])),
            n => Some(format!("{n} {other}connections use this configuration: {}.", names(&self.shared_by))),
        };

        let body = v_flex()
            .gap_3()
            .pt_1()
            .px_1p5()
            .pb_1()
            .when_some(shared, |d, text| {
                d.child(
                    h_flex()
                        .gap_1p5()
                        .items_start()
                        .child(Icon::new(IconName::Info).size(px(12.)).mt(px(2.)).text_color(theme.warning))
                        .child(
                            div().flex_1().min_w_0().text_size(px(12.)).text_color(theme.muted_foreground).child(format!(
                                "{text} Changes apply the next time they connect."
                            )),
                        ),
                )
            })
            .child(field("Name", Input::new(&self.name), self.error_for("name"), cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(field("SSH host", Input::new(&self.host), self.error_for("host"), cx))
                    .child(div().w(px(110.)).child(field("Port", Input::new(&self.port), self.error_for("port"), cx))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(field("User", Input::new(&self.user), self.error_for("user"), cx))
                    .child(field("Authentication", Select::new(&self.auth), None, cx)),
            )
            .when(auth_ix == 0, |d| {
                d.child(field(
                    "Private key",
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.key_path)))
                        .child(Button::new("ssh-browse-key").label("Browse…").on_click(
                            cx.listener(|this, _, window, cx| this.browse(window, cx)),
                        )),
                    self.error_for("key"),
                    cx,
                ))
                .child(field("Passphrase", Input::new(&self.secret).mask_toggle(), None, cx))
            })
            .when(auth_ix == 1, |d| {
                d.child(field("SSH password", Input::new(&self.secret).mask_toggle(), self.error_for("secret"), cx))
            })
            .when(auth_ix != 2 && self.keychain_ok == Some(false), |d| {
                d.child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(Icon::new(IconName::Info).size(px(12.)).text_color(theme.muted_foreground))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child("The secret will not be saved (no system keychain)"),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(field("Jump host", Select::new(&self.via).placeholder("None"), None, cx))
                    .child(div().w(px(150.)).child(field(
                        "Keep-alive (seconds)",
                        Input::new(&self.keepalive),
                        self.error_for("keepalive"),
                        cx,
                    ))),
            );

        let result_slot = div().min_h(px(56.)).flex().items_center().child(div().w_full().child(
            self.test.banner(cx).unwrap_or_else(|| {
                div()
                    .px_2()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("Test logs in to the SSH server (and its jump host) without opening a database.")
                    .into_any_element()
            }),
        ));

        let max_h = (f32::from(window.viewport_size().height) - 230.0).max(200.0);
        v_flex()
            .gap_3()
            .child(div().max_h(px(max_h)).overflow_y_scrollbar().child(body))
            .child(result_slot)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .mt_1()
                    .pt_3()
                    .pr(px(7.))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("ssh-test")
                            .icon(Icon::new(IconName::Zap))
                            .label("Test")
                            .loading(testing)
                            .on_click(cx.listener(|this, _, window, cx| this.run_test(window, cx))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Button::new("ssh-cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                            .child(
                                Button::new("ssh-save")
                                    .primary()
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                            ),
                    ),
            )
    }
}

/// "a, b and c", at most three names and a count for the rest.
fn names(all: &[String]) -> String {
    match all {
        [] => String::new(),
        [one] => one.clone(),
        _ if all.len() <= 3 => format!("{} and {}", all[..all.len() - 1].join(", "), all[all.len() - 1]),
        _ => format!("{}, {} and {} more", all[0], all[1], all.len() - 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::SshAuth;

    fn p(id: &str, via: Option<&str>) -> SshProfile {
        let mut p = SshProfile::new(id, "h", "u", SshAuth::Agent);
        p.id = id.into();
        p.via = via.map(str::to_string);
        p
    }

    #[test]
    fn the_jump_host_choices_leave_out_this_one_and_anything_that_would_loop() {
        let profiles = vec![p("a", None), p("b", Some("a")), p("c", None)];
        let ids = |this: &str| jump_choices(&profiles, this).into_iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids("new"), [None, Some("a".into()), Some("b".into()), Some("c".into())]);
        assert_eq!(ids("a"), [None, Some("c".into())], "b jumps through a, so a cannot jump through b");
        assert_eq!(ids("c"), [None, Some("a".into()), Some("b".into())]);
    }

    #[test]
    fn names_are_listed_briefly() {
        let n = |v: &[&str]| names(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(n(&["a"]), "a");
        assert_eq!(n(&["a", "b"]), "a and b");
        assert_eq!(n(&["a", "b", "c"]), "a, b and c");
        assert_eq!(n(&["a", "b", "c", "d", "e"]), "a, b and 3 more");
    }
}
