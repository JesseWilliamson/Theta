//! Server administration entry points: the Sessions and Roles tabs, Export Database and Run SQL Script.
//!
//! Each is reachable from the explorer's connection menu, the command palette and a `:` command, and each asks
//! `DatabaseKind::unsupported` first, so a product that cannot do it says why (`refuse_unsupported`) instead
//! of opening an empty tab. The tabs themselves live in `crate::admin`; this file hosts them.

use super::{AdminTab, ConsoleTab, Workspace};
use crate::admin::roles::RolesView;
use crate::admin::sessions::SessionsView;
use crate::admin::{AdminEvent, AdminKind};
use crate::connections::NoticeKind;
use crate::dialogs;
use gpui_kit::{AppContext as _, Context, Entity, Window};
use theta_core::Feature;

impl AdminTab {
    pub fn kind(&self) -> AdminKind {
        match self {
            AdminTab::Sessions(_) => AdminKind::Sessions,
            AdminTab::Roles(_) => AdminKind::Roles,
        }
    }

    pub fn conn_id(&self, cx: &gpui_kit::App) -> String {
        match self {
            AdminTab::Sessions(v) => v.read(cx).conn_id().to_string(),
            AdminTab::Roles(v) => v.read(cx).conn_id().to_string(),
        }
    }
}

impl Workspace {
    /// Whether a console statement, import or dump is running on `conn_id`'s shared session.
    pub fn is_busy(&self, conn_id: &str) -> bool {
        self.runs.contains_key(conn_id)
    }

    pub fn admin_tab(&self, tab: u64) -> Option<AdminTab> {
        self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.admin.clone())
    }

    /// The open admin tab of `kind` on `conn_id`, if any.
    pub fn find_admin_tab(&self, conn_id: &str, kind: AdminKind, cx: &gpui_kit::App) -> Option<u64> {
        self.tabs.iter().find_map(|t| {
            let a = t.admin.as_ref()?;
            (a.kind() == kind && a.conn_id(cx) == conn_id).then_some(t.id)
        })
    }

    /// Open (or switch to) the Sessions or Roles tab for `conn_id`. `None` after saying why when the
    /// connection is not ready or its product cannot do it.
    pub fn open_admin_tab(
        &mut self,
        conn_id: String,
        kind: AdminKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        if self.refuse_unsupported(&conn_id, kind.feature(), window, cx) {
            return None;
        }
        if !self.store.read(cx).state(&conn_id).is_connected() {
            self.notify(NoticeKind::Info, format!("Connect first, then open {}", kind.label()), window, cx);
            return None;
        }
        if let Some(id) = self.find_admin_tab(&conn_id, kind, cx) {
            self.activate_tab(id, window, cx);
            return Some(id);
        }
        let (store, ws) = (self.store.clone(), cx.entity().downgrade());
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let (tab, title, view): (AdminTab, String, gpui_kit::AnyView) = match kind {
            AdminKind::Sessions => {
                let v = cx.new(|cx| SessionsView::new(conn_id.clone(), store, ws, window, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &AdminEvent, window, cx| {
                    this.on_admin_event(ev.clone(), window, cx)
                });
                self.table_subs.insert(id, sub);
                let title = v.read(cx).title();
                (AdminTab::Sessions(v.clone()), title, v.into())
            }
            AdminKind::Roles => {
                let v = cx.new(|cx| RolesView::new(conn_id.clone(), store, ws, window, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &AdminEvent, window, cx| {
                    this.on_admin_event(ev.clone(), window, cx)
                });
                self.table_subs.insert(id, sub);
                let title = v.read(cx).title();
                (AdminTab::Roles(v.clone()), title, v.into())
            }
        };
        self.tabs.push(ConsoleTab {
            id,
            title: title.into(),
            connection_id: Some(conn_id.clone()),
            modified: false,
            initial_sql: None,
            table: None,
            admin: Some(tab.clone()),
        });
        self.console_views.insert(id, view);
        self.active_connection = Some(conn_id);
        self.activate_tab(id, window, cx);
        match &tab {
            AdminTab::Sessions(v) => v.update(cx, |v, cx| v.focus_list(window, cx)),
            AdminTab::Roles(v) => v.update(cx, |v, cx| v.focus_list(window, cx)),
        }
        Some(id)
    }

    fn on_admin_event(&mut self, ev: AdminEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            AdminEvent::Log { connection_id, title, kind, lines } => {
                self.log_bulk(&connection_id, &title, kind, lines, cx)
            }
            AdminEvent::Notice(kind, message) => self.notify(kind, message, window, cx),
            AdminEvent::OpenInConsole { connection_id, sql } => {
                self.open_console(Some(connection_id), Some(sql), window, cx);
            }
            AdminEvent::StateChanged => cx.notify(),
        }
    }

    /// Run in an admin tab means "refresh now".
    pub(super) fn refresh_admin_tab(&mut self, tab: u64, cx: &mut Context<Self>) -> bool {
        match self.admin_tab(tab) {
            Some(AdminTab::Sessions(v)) => {
                // This runs inside a workspace update, so the view must not read the workspace itself.
                let busy = self.admin_tab(tab).is_some_and(|a| self.is_busy(&a.conn_id(cx)));
                v.update(cx, |v, cx| v.refresh_with(busy, cx));
                true
            }
            Some(AdminTab::Roles(v)) => {
                v.update(cx, |v, cx| v.reload(cx));
                true
            }
            None => false,
        }
    }

    pub fn open_dump_dialog(&mut self, conn_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.refuse_unsupported(&conn_id, Feature::Dump, window, cx) {
            return;
        }
        if !self.store.read(cx).state(&conn_id).is_connected() {
            self.notify(NoticeKind::Info, "Connect first, then export the database", window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        dialogs::dump::open(ws, self.store.clone(), conn_id, window, cx);
    }

    pub fn open_script_dialog(
        &mut self,
        conn_id: String,
        file: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.store.read(cx).state(&conn_id).is_connected() {
            self.notify(NoticeKind::Info, "Connect first, then run a script", window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        dialogs::script::open(ws, self.store.clone(), conn_id, file, window, cx);
    }

    /// `:sessions`, `:roles`, `:dump`, `:script [FILE]`, on the active connection.
    pub(super) fn admin_host_command(
        &mut self,
        cmd: &str,
        arg: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.bulk_connection(cx) else {
            self.notify(NoticeKind::Info, "Connect to a database first", window, cx);
            return;
        };
        match cmd {
            "sessions" | "sess" => {
                self.open_admin_tab(conn, AdminKind::Sessions, window, cx);
            }
            "roles" | "users" => {
                self.open_admin_tab(conn, AdminKind::Roles, window, cx);
            }
            "dump" => self.open_dump_dialog(conn, window, cx),
            _ => {
                let file = (!arg.is_empty()).then(|| std::path::PathBuf::from(arg));
                self.open_script_dialog(conn, file, window, cx);
            }
        }
    }

    /// Palette and menu: the same entry points on the active connection.
    pub(super) fn admin_from_palette(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.admin_host_command(what, "", window, cx);
    }

    pub fn sessions_view(&self, tab: u64) -> Option<Entity<SessionsView>> {
        match self.admin_tab(tab)? {
            AdminTab::Sessions(v) => Some(v),
            _ => None,
        }
    }

    pub fn roles_view(&self, tab: u64) -> Option<Entity<RolesView>> {
        match self.admin_tab(tab)? {
            AdminTab::Roles(v) => Some(v),
            _ => None,
        }
    }
}
