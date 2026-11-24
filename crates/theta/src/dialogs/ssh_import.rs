//! Import from `~/.ssh/config`: tick the `Host` entries to turn into SSH configurations.

use crate::connections::ConnectionStore;
use crate::connections::ssh;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use std::path::PathBuf;
use theta_drivers::ssh_config::{Env, HostEntry};

struct Row {
    entry: HostEntry,
    checked: bool,
    /// A configuration for this server, user and port exists already.
    present: bool,
}

pub struct SshImport {
    store: Entity<ConnectionStore>,
    path: PathBuf,
    local_user: String,
    /// `None`: the file does not exist; `Some(empty)`: it has no importable `Host` entries.
    rows: Option<Vec<Row>>,
    cursor: usize,
    focus: FocusHandle,
}

/// The local account name, for `User` when the file gives none.
fn local_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
}

pub fn open(store: Entity<ConnectionStore>, window: &mut Window, cx: &mut App) {
    let home = std::env::home_dir().unwrap_or_default();
    let path = home.join(".ssh").join("config");
    let user = local_user();
    let env = Env { home, local_user: user.clone() };
    let existing = store.read(cx).ssh_profiles().to_vec();
    let rows = theta_drivers::ssh_config::read_file(&path, &env).map(|entries| {
        entries
            .into_iter()
            .map(|entry| {
                let present = ssh::already_imported(&existing, &entry, &user);
                Row { entry, checked: false, present }
            })
            .collect()
    });
    let view = cx.new(|cx| SshImport { store, path, local_user: user, rows, cursor: 0, focus: cx.focus_handle() });
    let (shown, focus) = (view.clone(), view.clone());
    window.open_dialog(cx, move |dialog, _, cx| {
        let on_ok = shown.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Import SSH Configurations")
            .w(px(640.))
            .overlay_closable(true)
            .child(shown.clone())
            .on_ok(move |_, window, cx| {
                on_ok.update(cx, |v, cx| v.import(window, cx));
                false
            })
    });
    window.defer(cx, move |window, cx| {
        let handle = focus.read(cx).focus.clone();
        window.focus(&handle, cx);
    });
}

impl SshImport {
    fn chosen(&self) -> usize {
        self.rows.iter().flatten().filter(|r| r.checked).count()
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.rows.as_ref().map_or(0, |r| r.len().saturating_sub(1)) as isize;
        self.cursor = (self.cursor as isize + by).clamp(0, last) as usize;
        cx.notify();
    }

    fn toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.as_mut().and_then(|r| r.get_mut(i)) {
            r.checked = !r.checked;
            self.cursor = i;
            cx.notify();
        }
    }

    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        if let Some(rows) = self.rows.as_mut() {
            let all = rows.iter().all(|r| r.checked);
            for r in rows {
                r.checked = !all;
            }
            cx.notify();
        }
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Enter reaches here while the button is disabled, so the same condition guards both.
        if self.chosen() == 0 {
            return;
        }
        let entries: Vec<&HostEntry> = self.rows.iter().flatten().filter(|r| r.checked).map(|r| &r.entry).collect();
        let existing = self.store.read(cx).ssh_profiles().to_vec();
        let made = ssh::plan_import(&entries, &existing, &self.local_user);
        self.store.update(cx, |s, cx| {
            for p in made {
                s.add_ssh(p, cx);
            }
        });
        window.close_dialog(cx);
    }
}

impl Render for SshImport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let chosen = self.chosen();
        let focused = self.focus.is_focused(window);
        let cursor = self.cursor;
        let shown_path = self.path.display().to_string();

        let list = match &self.rows {
            None => message(&theme, format!("{shown_path} does not exist, so there is nothing to import.")),
            Some(rows) if rows.is_empty() => {
                message(&theme, format!("{shown_path} has no Host entries without wildcards."))
            }
            Some(rows) => v_flex()
                .id("ssh-import-list")
                .max_h(px(300.))
                .overflow_y_scroll()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .children(rows.iter().enumerate().map(|(i, r)| {
                    let e = &r.entry;
                    let user = e.user.clone().unwrap_or_else(|| self.local_user.clone());
                    let how = match &e.identity_file {
                        Some(f) => format!("key {f}"),
                        None => "ssh-agent".to_string(),
                    };
                    let jump = e
                        .proxy_jump
                        .as_deref()
                        .filter(|j| !j.eq_ignore_ascii_case("none"))
                        .map(|j| format!(" · via {j}"))
                        .unwrap_or_default();
                    h_flex()
                        .id(("ssh-import-row", i))
                        .h(px(40.))
                        .px_2()
                        .gap_2()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .hover(|s| s.bg(theme.list_hover))
                        .when(focused && i == cursor, |d| d.bg(theme.list_active))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.focus, cx);
                            this.toggle(i, cx);
                        }))
                        .child(div().w(px(14.)).child(if r.checked { "☑" } else { "☐" }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().font_weight(FontWeight::MEDIUM).child(e.alias.clone()))
                                .child(
                                    div().truncate().text_color(theme.muted_foreground).child(format!(
                                        "{user}@{}:{} · {how}{jump}",
                                        e.address(),
                                        e.port.unwrap_or(22)
                                    )),
                                ),
                        )
                        .when(r.present, |d| d.child(div().text_color(theme.muted_foreground).child("Already imported")))
                }))
                .into_any_element(),
        };

        let body = v_flex()
            .gap_3()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                match ev.keystroke.key.as_str() {
                    "down" | "j" => this.step(1, cx),
                    "up" | "k" => this.step(-1, cx),
                    "space" | "x" => this.toggle(this.cursor, cx),
                    "a" => this.toggle_all(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(format!(
                "Host entries of {shown_path}. {chosen} chosen · up / down move, space ticks, a ticks all, enter imports. \
                 A key file is used where the entry names one, otherwise the ssh-agent."
            )))
            .child(list);
        let footer = h_flex()
            .justify_end()
            .gap_2()
            .child(Button::new("ssh-import-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)))
            .child(
                Button::new("ssh-import-go")
                    .primary()
                    .label(if chosen == 0 { "Import".to_string() } else { format!("Import {chosen}") })
                    .disabled(chosen == 0)
                    .on_click(cx.listener(|this, _, w, cx| this.import(w, cx))),
            );
        super::framed(window, body, footer, 150.)
    }
}

fn message(theme: &gpui_kit::component::Theme, text: String) -> gpui_kit::AnyElement {
    div()
        .h(px(100.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .px_3()
        .text_size(px(12.))
        .text_color(theme.muted_foreground)
        .child(text)
        .into_any_element()
}
