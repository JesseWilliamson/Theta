//! SSH Configurations: the list of saved tunnels, kept apart from the connections that use them.
//!
//! New, Edit, Duplicate, Delete, a Test of the SSH login, and an import from `~/.ssh/config`. Deleting a
//! configuration that connections use asks first, naming them, and then detaches them: they connect
//! directly from then on (refusing would leave the user to hunt for them one by one).

use super::confirm::{self, Choice, Tone};
use super::ssh_edit::{self, TestState};
use crate::connections::ssh::SshProfile;
use crate::connections::{ConnectionStore, test_ssh};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, px,
};

pub struct SshManager {
    store: Entity<ConnectionStore>,
    /// Row the keyboard and the buttons act on.
    selected: usize,
    focus: FocusHandle,
    test: TestState,
    /// The configuration the test result belongs to.
    tested: Option<String>,
    test_epoch: u64,
    _subs: Vec<Subscription>,
}

pub fn open(store: Entity<ConnectionStore>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        // The list follows the store: a configuration saved in the editor on top of this dialog, or a
        // connection that starts using one, shows up without reopening.
        let subs = vec![cx.observe(&store, |this: &mut SshManager, _, cx| {
            let n = this.store.read(cx).ssh_profiles().len();
            this.selected = this.selected.min(n.saturating_sub(1));
            cx.notify();
        })];
        SshManager {
            store,
            selected: 0,
            focus: cx.focus_handle(),
            test: TestState::Idle,
            tested: None,
            test_epoch: 0,
            _subs: subs,
        }
    });
    let (shown, focus) = (view.clone(), view.clone());
    window.open_dialog(cx, move |dialog, _, cx| {
        let on_ok = shown.clone();
        // Enter edits the highlighted configuration, or starts a new one when the list is empty.
        dialog
            .bg(cx.theme().popover)
            .title("SSH Configurations")
            .w(px(680.))
            .overlay_closable(true)
            .child(shown.clone())
            .on_ok(move |_, window, cx| {
                on_ok.update(cx, |v, cx| v.edit_or_new(window, cx));
                false
            })
    });
    window.defer(cx, move |window, cx| {
        let handle = focus.read(cx).focus.clone();
        window.focus(&handle, cx);
    });
}

impl SshManager {
    fn profiles(&self, cx: &App) -> Vec<SshProfile> {
        self.store.read(cx).ssh_profiles().to_vec()
    }

    fn current(&self, cx: &App) -> Option<SshProfile> {
        self.store.read(cx).ssh_profiles().get(self.selected).cloned()
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.store.read(cx).ssh_profiles().len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + by).clamp(0, last) as usize;
        cx.notify();
    }

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        self.selected = i;
        cx.notify();
    }

    fn new_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let view = cx.entity();
        // Select what was just created: it is added at the end of the list.
        let done: ssh_edit::Saved = std::rc::Rc::new(move |id, _, cx| {
            let ix = store.read(cx).ssh_profiles().iter().position(|p| p.id == id);
            view.update(cx, |v, cx| {
                if let Some(ix) = ix {
                    v.selected = ix;
                }
                cx.notify();
            });
        });
        ssh_edit::open(self.store.clone(), None, None, Some(done), window, cx);
    }

    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.current(cx) {
            ssh_edit::open(self.store.clone(), Some(p.id), None, None, window, cx);
        }
    }

    fn edit_or_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.current(cx).is_some() {
            self.edit(window, cx);
        } else {
            self.new_config(window, cx);
        }
    }

    fn duplicate(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.current(cx) else { return };
        let copy = self.store.update(cx, |s, cx| s.duplicate_ssh(&p.id, cx));
        if let Some(copy) = copy {
            if let Some(ix) = self.store.read(cx).ssh_profiles().iter().position(|q| q.id == copy) {
                self.selected = ix;
            }
            cx.notify();
        }
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.current(cx) else { return };
        let users: Vec<String> = {
            let s = self.store.read(cx);
            s.ssh_users(&p.id).iter().map(|c| s.label(&c.id).unwrap_or_else(|| c.name.clone())).collect()
        };
        let (title, intro) = if users.is_empty() {
            (format!("Delete “{}”?", p.name), "The SSH configuration and its stored secret are removed. No connection uses it.".to_string())
        } else {
            (
                format!("Delete “{}”?", p.name),
                format!(
                    "{} {} it: {}. {} will connect directly, without a tunnel, until you choose another SSH configuration. The SSH configuration and its stored secret are removed.",
                    users.len(),
                    if users.len() == 1 { "connection uses" } else { "connections use" },
                    users.join(", "),
                    if users.len() == 1 { "It" } else { "They" },
                ),
            )
        };
        let (store, id) = (self.store.clone(), p.id);
        confirm::open(
            window,
            cx,
            title,
            intro,
            Vec::new(),
            vec![Choice::new("Delete", Tone::Danger, move |_, cx| {
                store.update(cx, |s, cx| s.remove_ssh(&id, cx));
            })],
        );
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.current(cx) else { return };
        let config = match self.store.read(cx).resolver().ssh(&p.id) {
            Ok(c) => c,
            Err(e) => {
                self.test = TestState::Failed(e.to_string(), None);
                self.tested = Some(p.id);
                cx.notify();
                return;
            }
        };
        self.test_epoch += 1;
        let epoch = self.test_epoch;
        self.test = TestState::Testing;
        self.tested = Some(p.id);
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

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::ssh_import::open(self.store.clone(), window, cx);
    }
}

fn count_label(n: usize) -> String {
    match n {
        0 => "Not used".to_string(),
        1 => "1 connection".to_string(),
        n => format!("{n} connections"),
    }
}

impl Render for SshManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let profiles = self.profiles(cx);
        let counts: Vec<usize> = {
            let store = self.store.read(cx);
            profiles.iter().map(|p| store.ssh_users(&p.id).len()).collect()
        };
        let selected_id = profiles.get(self.selected).map(|p| p.id.clone());
        let have = selected_id.is_some();
        let focused = self.focus.is_focused(window);
        let testing = matches!(self.test, TestState::Testing);
        // A result belongs to the configuration it was run for; selecting another hides it.
        let banner = if self.tested == selected_id { self.test.banner(cx) } else { None };

        let list = if profiles.is_empty() {
            div()
                .h(px(120.))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child("No SSH configurations yet. Create one, or import your ~/.ssh/config.")
                .into_any_element()
        } else {
            v_flex()
                .id("ssh-list")
                .max_h(px(260.))
                .overflow_y_scroll()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .children(profiles.iter().enumerate().map(|(i, p)| {
                    h_flex()
                        .id(("ssh-config", i))
                        .h(px(40.))
                        .px_2()
                        .gap_3()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .hover(|s| s.bg(theme.list_hover))
                        .when(i == self.selected, |d| d.bg(if focused { theme.list_active } else { theme.list_hover }))
                        .on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.select(i, cx);
                            if ev.click_count() >= 2 {
                                this.edit(window, cx);
                            }
                        }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().font_weight(FontWeight::MEDIUM).child(p.name.clone()))
                                .child(div().truncate().text_color(theme.muted_foreground).child(p.address())),
                        )
                        .child(div().w(px(80.)).text_color(theme.muted_foreground).child(p.auth_label()))
                        .child(div().w(px(100.)).text_color(theme.muted_foreground).child(count_label(counts[i])))
                }))
                .into_any_element()
        };

        let toolbar = h_flex()
            .gap_2()
            .child(Button::new("ssh-new").label("New…").on_click(cx.listener(|this, _, w, cx| this.new_config(w, cx))))
            .child(
                Button::new("ssh-edit")
                    .label("Edit…")
                    .disabled(!have)
                    .on_click(cx.listener(|this, _, w, cx| this.edit(w, cx))),
            )
            .child(
                Button::new("ssh-duplicate")
                    .label("Duplicate")
                    .disabled(!have)
                    .on_click(cx.listener(|this, _, _, cx| this.duplicate(cx))),
            )
            .child(
                Button::new("ssh-delete")
                    .label("Delete…")
                    .disabled(!have)
                    .on_click(cx.listener(|this, _, w, cx| this.delete(w, cx))),
            )
            .child(
                Button::new("ssh-test-selected")
                    .label("Test")
                    .disabled(!have)
                    .loading(testing)
                    .on_click(cx.listener(|this, _, _, cx| this.run_test(cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("ssh-import")
                    .label("Import from ~/.ssh/config…")
                    .on_click(cx.listener(|this, _, w, cx| this.import(w, cx))),
            );

        let body = v_flex()
            .gap_3()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                match ev.keystroke.key.as_str() {
                    "down" | "j" => this.step(1, cx),
                    "up" | "k" => this.step(-1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                div().text_size(px(12.)).text_color(theme.muted_foreground).child(
                    "A connection picks one of these to tunnel through. Edit one here and every connection that uses it follows.",
                ),
            )
            .child(toolbar)
            .child(list)
            // A fixed slot, like the connection dialog's: a test result does not move the button below.
            .child(div().min_h(px(56.)).flex().items_center().child(div().w_full().child(banner.unwrap_or_else(|| {
                div()
                    .px_2()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("Test logs in to the selected SSH server without opening a database.")
                    .into_any_element()
            }))));
        let footer = h_flex()
            .justify_end()
            .gap_2()
            .child(Button::new("ssh-close").primary().label("Close").on_click(|_, w, cx| w.close_dialog(cx)));
        super::framed(window, body, footer, 150.)
    }
}
