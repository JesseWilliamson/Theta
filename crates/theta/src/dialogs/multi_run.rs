//! Run on Several Connections: choose the connections and whether it is the statement or the script.
//!
//! The run itself is `Workspace::run_on_several`; this dialog only collects the choice, and says in advance
//! what will be refused (a read-only connection, a statement that is a write) and warns about a statement
//! that is dangerous on every one of them.

use crate::connections::ConnectionStore;
use crate::workspace::Workspace;
use crate::workspace::multi_run::Scope;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};

const SCOPES: [&str; 2] = ["Current statement (or selection)", "Whole script"];

struct Item {
    id: String,
    name: String,
    product: &'static str,
    read_only: bool,
    checked: bool,
}

pub struct MultiRunView {
    ws: WeakEntity<Workspace>,
    console: u64,
    items: Vec<Item>,
    /// The row the keyboard acts on. The list is a handful of rows, so a plain index is enough.
    cursor: usize,
    focus: FocusHandle,
    scope: Entity<SelectState<Vec<String>>>,
    /// For each scope, the first line of each picked statement and whether it writes, for the warnings.
    previews: [Vec<(String, bool)>; 2],
    preview: Vec<(String, bool)>,
    _subs: Vec<Subscription>,
}

pub fn open(
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    console: u64,
    first: Option<String>,
    previews: [Vec<(String, bool)>; 2],
    window: &mut Window,
    cx: &mut App,
) {
    let items: Vec<Item> = {
        let s = store.read(cx);
        s.configs()
            .iter()
            .filter(|c| s.state(&c.id).is_connected())
            .map(|c| Item {
                id: c.id.clone(),
                name: c.name.clone(),
                product: c.kind.label(),
                read_only: c.read_only,
                checked: first.as_deref() == Some(c.id.as_str()),
            })
            .collect()
    };
    if items.len() < 2 {
        if let Some(ws) = ws.upgrade() {
            ws.update(cx, |w, cx| {
                w.notify(
                    crate::connections::NoticeKind::Info,
                    "Connect at least two connections to run on several".to_string(),
                    window,
                    cx,
                )
            });
        }
        return;
    }
    let view = cx.new(|cx| {
        let scope = cx.new(|cx| {
            SelectState::new(
                SCOPES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let subs = vec![cx.subscribe_in(
            &scope,
            window,
            |this: &mut MultiRunView, _, _: &SelectEvent<Vec<String>>, _, cx| {
                this.refresh_preview(cx);
            },
        )];
        let mut v = MultiRunView {
            ws: ws.clone(),
            console,
            cursor: items.iter().position(|i| i.checked).unwrap_or(0),
            items,
            focus: cx.focus_handle(),
            scope,
            previews,
            preview: Vec::new(),
            _subs: subs,
        };
        v.refresh_preview(cx);
        v
    });
    let focus = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .bg(cx.theme().popover)
            .title("Run on Several Connections")
            .w(px(560.))
            .overlay_closable(true)
            // Enter is the dialog's OK. `run` closes the dialog itself, and only when something ran, so
            // the answer here is always "do not close".
            .on_ok({
                let view = view.clone();
                move |_, window, cx| {
                    view.update(cx, |v, cx| v.run(window, cx));
                    false
                }
            })
            .child(view.clone())
    });
    // Without this the editor behind the dialog keeps the focus and the list cannot be reached by keys.
    window.defer(cx, move |window, cx| {
        let handle = focus.read(cx).focus.clone();
        window.focus(&handle, cx);
    });
}

impl MultiRunView {
    fn chosen_scope(&self, cx: &App) -> Scope {
        match self.scope.read(cx).selected_value().map(String::as_str) {
            Some("Whole script") => Scope::Script,
            _ => Scope::Statement,
        }
    }

    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = match self.chosen_scope(cx) {
            Scope::Statement => self.previews[0].clone(),
            Scope::Script => self.previews[1].clone(),
        };
        cx.notify();
    }

    fn can_run(&self) -> bool {
        self.items.iter().any(|i| i.checked) && !self.preview.is_empty()
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.items.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + by).clamp(0, last) as usize;
        cx.notify();
    }

    fn toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(it) = self.items.get_mut(i) {
            it.checked = !it.checked;
            self.cursor = i;
            cx.notify();
        }
    }

    /// Tick every connection, or untick them all when they are all ticked already.
    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let all = self.items.iter().all(|i| i.checked);
        for it in &mut self.items {
            it.checked = !all;
        }
        cx.notify();
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Enter reaches here even while the Run button is disabled, so the same condition guards both.
        if !self.can_run() {
            return;
        }
        let ids: Vec<String> = self.items.iter().filter(|i| i.checked).map(|i| i.id.clone()).collect();
        let scope = self.chosen_scope(cx);
        if let Some(ws) = self.ws.upgrade() {
            let console = self.console;
            ws.update(cx, |w, cx| w.run_on_several(console, ids, scope, window, cx));
        }
        window.close_dialog(cx);
    }
}

impl Render for MultiRunView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let chosen = self.items.iter().filter(|i| i.checked).count();
        let writes = self.preview.iter().any(|(_, w)| *w);
        let can_run = self.can_run();
        let focused = self.focus.is_focused(window);
        let cursor = self.cursor;
        let n_read_only = self.items.iter().filter(|i| i.checked && i.read_only).count();
        let caption = |t: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t)
        };
        v_flex()
            .gap_3()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                match ev.keystroke.key.as_str() {
                    "down" | "j" => this.step(1, cx),
                    "up" | "k" => this.step(-1, cx),
                    "space" | "x" => this.toggle(this.cursor, cx),
                    "a" => this.toggle_all(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(v_flex().gap_1().flex_1().child(caption("Run")).child(Select::new(&self.scope))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(h_flex().gap_2().child(caption("Connections")).child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(format!("{chosen} chosen · up / down move, space ticks, a ticks all, enter runs")),
                    ))
                    .child(
                        v_flex().border_1().border_color(theme.border).rounded_md().children(
                            self.items.iter().enumerate().map(|(i, it)| {
                                h_flex()
                                    .id(("multi-conn", i))
                                    .h(px(28.))
                                    .px_2()
                                    .gap_2()
                                    .items_center()
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .hover(|s| s.bg(theme.list_hover))
                                    .when(focused && i == cursor, |d| d.bg(theme.list_active))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        // A click on a row must not strand the keys on nothing.
                                        window.focus(&this.focus, cx);
                                        this.toggle(i, cx);
                                    }))
                                    .child(div().w(px(14.)).child(if it.checked { "☑" } else { "☐" }))
                                    .child(div().flex_1().truncate().child(it.name.clone()))
                                    .child(div().text_color(theme.muted_foreground).child(it.product))
                                    .when(it.read_only, |d| {
                                        d.child(div().text_color(theme.warning).child("read-only"))
                                    })
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child("Each connection runs the same text on its own session, in its own transaction mode. A failure on one does not stop the others; the result is a Summary tab and one tab per connection."),
            )
            .when(writes && n_read_only > 0, |d| {
                d.child(div().text_size(px(11.)).text_color(theme.warning).child(format!(
                    "{n_read_only} of the chosen connections are read-only and will refuse this statement; the others will run it."
                )))
            })
            .when(writes, |d| {
                d.child(div().text_size(px(11.)).text_color(theme.warning).child(
                    "This changes data. It will run on every chosen connection that allows writes, in that connection's transaction mode.",
                ))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("multi-run")
                            .primary()
                            .label(if writes { "Run on All" } else { "Run" })
                            .disabled(!can_run)
                            .on_click(cx.listener(|this, _, w, cx| this.run(w, cx))),
                    )
                    .child(Button::new("multi-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx))),
            )
    }
}
