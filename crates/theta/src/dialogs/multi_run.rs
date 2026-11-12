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
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, div, px,
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
            items,
            scope,
            previews,
            preview: Vec::new(),
            _subs: subs,
        };
        v.refresh_preview(cx);
        v
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .bg(cx.theme().popover)
            .title("Run on Several Connections")
            .w(px(560.))
            .overlay_closable(true)
            .on_ok(|_, _, _| false)
            .child(view.clone())
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

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let chosen = self.items.iter().filter(|i| i.checked).count();
        let writes = self.preview.iter().any(|(_, w)| *w);
        let danger = self.preview.iter().any(|(_, w)| *w);
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
                        div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!("{chosen} chosen")),
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.items[i].checked = !this.items[i].checked;
                                        cx.notify();
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
            .when(danger, |d| {
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
                            .label(if danger { "Run on All" } else { "Run" })
                            .disabled(chosen == 0 || self.preview.is_empty())
                            .on_click(cx.listener(|this, _, w, cx| this.run(w, cx))),
                    )
                    .child(Button::new("multi-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx))),
            )
    }
}
