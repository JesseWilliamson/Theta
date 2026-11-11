//! The Roles tab: users and roles of the server, their attributes, memberships and privileges, and the
//! statements that change them.
//!
//! Every change is a `theta_sql::admin::roles::Change` built from a form (`form.rs`) and shown in a
//! confirmation before it runs. The password in a `Change` exists only in `Change::sql`, which goes to the
//! server and nowhere else: the Messages entry, the confirmation and the preview use `Change::shown`, and a
//! server error that echoes the statement is cleaned with `Change::scrub` first.

use super::form::{self, Field, FormSpec, Values};
use super::keys::{self, Cmd};
use super::{AdminEvent, SideConnection};
use crate::connections::{ConnectionStore, NoticeKind};
use crate::dialogs::confirm::{self, Choice, Item, Tone};
use crate::panels::messages::MessageKind;
use crate::runtime;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, WeakEntity, Window,
    div, px, uniform_list,
};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;
use theta_core::{DatabaseKind, ExecOptions, Family, Feature, Value};
use theta_sql::admin::roles::{self as logic, AlterRole, Change, NewRole, ObjectPrivileges, Role, Target};
use theta_ui::{ROW_HEIGHT, Settings, data_surface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Name,
    Host,
    Kind,
    Login,
    Super,
    CreateDb,
    CreateRole,
    Replication,
    Plugin,
    Locked,
    Expiry,
    Limit,
    ValidUntil,
    MemberOf,
}

impl Col {
    fn label(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Host => "Host",
            Col::Kind => "Type",
            Col::Login => "Login",
            Col::Super => "Superuser",
            Col::CreateDb => "Create DB",
            Col::CreateRole => "Create role",
            Col::Replication => "Replication",
            Col::Plugin => "Plugin",
            Col::Locked => "Locked",
            Col::Expiry => "Password",
            Col::Limit => "Conn limit",
            Col::ValidUntil => "Valid until",
            Col::MemberOf => "Member of",
        }
    }
}

fn columns(kind: DatabaseKind, narrow: bool) -> Vec<(Col, f32)> {
    let all = columns_all(kind);
    if !narrow {
        return all;
    }
    // Below ~1250 px wide the least used columns move to the detail pane.
    let drop: &[Col] = match kind.family() {
        Family::MySql => &[Col::Plugin, Col::Limit, Col::Kind],
        Family::SqlServer => &[],
        _ => &[Col::Replication, Col::ValidUntil, Col::Limit],
    };
    all.into_iter().filter(|(c, _)| !drop.contains(c)).collect()
}

fn columns_all(kind: DatabaseKind) -> Vec<(Col, f32)> {
    match kind.family() {
        Family::MySql => vec![
            (Col::Name, 150.),
            (Col::Host, 110.),
            (Col::Kind, 64.),
            (Col::Super, 84.),
            (Col::Locked, 64.),
            (Col::Plugin, 170.),
            (Col::Expiry, 110.),
            (Col::Limit, 84.),
            (Col::MemberOf, 0.),
        ],
        Family::SqlServer => {
            vec![
                (Col::Name, 220.),
                (Col::Kind, 64.),
                (Col::Login, 64.),
                (Col::Super, 84.),
                (Col::MemberOf, 0.),
            ]
        }
        _ => vec![
            (Col::Name, 170.),
            (Col::Login, 60.),
            (Col::Super, 84.),
            (Col::CreateDb, 80.),
            (Col::CreateRole, 90.),
            (Col::Replication, 90.),
            (Col::Limit, 84.),
            (Col::ValidUntil, 170.),
            (Col::MemberOf, 0.),
        ],
    }
}

fn yes(b: bool) -> String {
    if b { "yes".into() } else { String::new() }
}

fn cell(r: &Role, c: Col) -> String {
    match c {
        Col::Name => r.name.clone(),
        Col::Host => r.host.clone().unwrap_or_default(),
        Col::Kind => if r.is_role { "role" } else { "user" }.into(),
        Col::Login => yes(r.login),
        Col::Super => yes(r.superuser),
        Col::CreateDb => yes(r.create_db),
        Col::CreateRole => yes(r.create_role),
        Col::Replication => yes(r.replication),
        Col::Plugin => r.plugin.clone().unwrap_or_default(),
        Col::Locked => {
            if r.locked {
                "locked".into()
            } else {
                String::new()
            }
        }
        Col::Expiry => {
            if r.password_expired {
                "expired".into()
            } else {
                r.password_lifetime.as_ref().map(|d| format!("every {d} d")).unwrap_or_default()
            }
        }
        Col::Limit => r.conn_limit.map(|n| n.to_string()).unwrap_or_default(),
        Col::ValidUntil => r.valid_until.clone().unwrap_or_default(),
        Col::MemberOf => r.member_of.join(", "),
    }
}

pub struct RolesView {
    conn_id: String,
    name: String,
    kind: DatabaseKind,
    banner: String,
    store: Entity<ConnectionStore>,
    side: SideConnection,
    roles: Vec<Role>,
    shown: Vec<usize>,
    selected: Option<String>,
    privileges: Vec<ObjectPrivileges>,
    privileges_for: Option<String>,
    privileges_error: Option<String>,
    show_system: bool,
    filter: Entity<InputState>,
    loading: bool,
    last: Option<Instant>,
    error: Option<String>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    pending: keys::Pending,
    narrow: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AdminEvent> for RolesView {}

impl Focusable for RolesView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RolesView {
    pub fn new(
        conn_id: String,
        store: Entity<ConnectionStore>,
        _ws: WeakEntity<crate::workspace::Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name, kind, banner) = {
            let s = store.read(cx);
            let kind = s.config(&conn_id).map(|c| c.kind).unwrap_or(DatabaseKind::Postgres);
            let banner = match s.state(&conn_id) {
                crate::connections::ConnState::Connected { server, .. } => server.banner.clone(),
                _ => String::new(),
            };
            (s.display_name(&conn_id), kind, banner)
        };
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter roles  (/)"));
        let sub = cx.subscribe_in(&filter, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => this.reshape(cx),
            InputEvent::PressEnter { .. } => window.focus(&this.focus, cx),
            _ => {}
        });
        let mut this = RolesView {
            conn_id,
            name,
            kind,
            banner,
            store,
            side: SideConnection::default(),
            roles: Vec::new(),
            shown: Vec::new(),
            selected: None,
            privileges: Vec::new(),
            privileges_for: None,
            privileges_error: None,
            show_system: false,
            filter,
            loading: false,
            last: None,
            error: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            pending: None,
            narrow: false,
            _subs: vec![sub],
        };
        if kind.unsupported(Feature::Roles).is_none() {
            this.refresh(cx);
        }
        this
    }

    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    pub fn title(&self) -> String {
        format!("Roles · {}", self.name)
    }

    pub fn roles(&self) -> &[Role] {
        &self.roles
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn selected_role(&self) -> Option<&Role> {
        let id = self.selected.as_ref()?;
        self.roles.iter().find(|r| &r.display() == id)
    }

    pub fn privileges(&self) -> &[ObjectPrivileges] {
        &self.privileges
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        window.focus(&self.focus, cx);
    }

    /// Reload the roles and the selected role's privileges (what Run does in this tab).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.privileges_for = None;
        self.refresh(cx);
    }

    fn can_edit(&self) -> Option<&'static str> {
        self.kind.unsupported(Feature::RoleEdit)
    }

    // ------------------------------------------------------------ loading

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Ok(q) = logic::queries(self.kind, &self.banner, self.show_system) else { return };
        if self.loading {
            return;
        }
        self.loading = true;
        let open = self.side.take_or_open(&self.store, &self.conn_id, cx);
        let kind = self.kind;
        let task = runtime::spawn(async move {
            let side = open.await?;
            let opts = ExecOptions { row_limit: None };
            let rs = side.execute(&q.roles, &opts).await?.result.unwrap_or_default();
            let mut roles = logic::parse_roles(kind, &rs);
            let mut note = None;
            if let Some(m) = &q.members {
                match side.execute(m, &opts).await {
                    Ok(o) => logic::apply_members(&mut roles, &o.result.unwrap_or_default()),
                    Err(e) => note = Some(format!("Memberships are not available: {e}")),
                }
            }
            Ok::<_, theta_core::Error>((side, roles, note))
        });
        cx.spawn(async move |this, cx| {
            let r = task.await.and_then(|r| r);
            this.update(cx, |this, cx| {
                this.loading = false;
                match r {
                    Ok((side, roles, note)) => {
                        this.side.put(side);
                        this.roles = roles;
                        this.error = note;
                        this.last = Some(Instant::now());
                    }
                    Err(e) => {
                        this.side.discard();
                        this.error = Some(e.to_string());
                    }
                }
                this.reshape(cx);
            })
            .ok();
        })
        .detach();
    }

    fn reshape(&mut self, cx: &mut Context<Self>) {
        let text = self.filter.read(cx).value().to_lowercase();
        self.shown = (0..self.roles.len())
            .filter(|i| {
                let r = &self.roles[*i];
                text.split_whitespace().all(|t| {
                    r.display().to_lowercase().contains(t)
                        || r.member_of.iter().any(|m| m.to_lowercase().contains(t))
                })
            })
            .collect();
        if self.selected.as_ref().is_none_or(|id| !self.shown.iter().any(|i| &self.roles[*i].display() == id))
        {
            self.selected = self.shown.first().map(|i| self.roles[*i].display());
        }
        self.load_privileges(cx);
        cx.emit(AdminEvent::StateChanged);
        cx.notify();
    }

    fn load_privileges(&mut self, cx: &mut Context<Self>) {
        let Some(role) = self.selected_role().cloned() else {
            self.privileges.clear();
            self.privileges_for = None;
            return;
        };
        if self.privileges_for.as_deref() == Some(&role.display()) {
            return;
        }
        self.privileges_for = Some(role.display());
        self.privileges.clear();
        self.privileges_error = None;
        let Ok(sql) = logic::privileges_query(self.kind, &role) else {
            self.privileges_error = Some("Privileges are not listed for this product.".into());
            return;
        };
        let open = self.side.take_or_open(&self.store, &self.conn_id, cx);
        let kind = self.kind;
        let who = role.display();
        let task = runtime::spawn(async move {
            let side = open.await?;
            let out = side.execute(&sql, &ExecOptions { row_limit: None }).await;
            Ok::<_, theta_core::Error>((side, out))
        });
        cx.spawn(async move |this, cx| {
            let r = task.await.and_then(|r| r);
            this.update(cx, |this, cx| {
                if this.privileges_for.as_deref() != Some(who.as_str()) {
                    if let Ok((side, _)) = r {
                        this.side.put(side);
                    }
                    return;
                }
                match r {
                    Ok((side, out)) => {
                        this.side.put(side);
                        match out {
                            Ok(o) => {
                                let rs = o.result.unwrap_or_default();
                                let grants = if kind.family() == Family::MySql {
                                    let lines: Vec<String> = rs
                                        .rows
                                        .iter()
                                        .filter_map(|r| r.first())
                                        .map(Value::display)
                                        .collect();
                                    logic::parse_show_grants(&lines)
                                } else {
                                    logic::parse_pg_grants(&rs)
                                };
                                this.privileges = logic::group(&grants);
                            }
                            Err(e) => this.privileges_error = Some(e.to_string()),
                        }
                    }
                    Err(e) => this.privileges_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ commands

    fn cursor(&self) -> usize {
        self.selected
            .as_ref()
            .and_then(|id| self.shown.iter().position(|i| &self.roles[*i].display() == id))
            .unwrap_or(0)
    }

    fn select_at(&mut self, pos: usize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let pos = pos.min(self.shown.len() - 1);
        self.selected = Some(self.roles[self.shown[pos]].display());
        self.scroll.scroll_to_item(pos, gpui_kit::ScrollStrategy::Nearest);
        self.load_privileges(cx);
        cx.notify();
    }

    pub fn command(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Move(d) => self.select_at((self.cursor() as i32 + d).max(0) as usize, cx),
            Cmd::Top => self.select_at(0, cx),
            Cmd::Bottom => self.select_at(usize::MAX, cx),
            Cmd::Filter => {
                let f = self.filter.clone();
                f.update(cx, |s, cx| s.focus(window, cx));
            }
            Cmd::Clear => {
                if !self.filter.read(cx).value().is_empty() {
                    self.filter.update(cx, |s, cx| s.set_value("", window, cx));
                }
            }
            Cmd::Refresh => {
                self.privileges_for = None;
                self.refresh(cx);
            }
            Cmd::ToggleSystem => {
                self.show_system = !self.show_system;
                self.refresh(cx);
            }
            Cmd::Copy => {
                if let Some(r) = self.selected_role() {
                    cx.write_to_clipboard(ClipboardItem::new_string(r.display()));
                }
            }
            Cmd::New => self.form_new(window, cx),
            Cmd::Alter | Cmd::Enter => self.form_alter(window, cx),
            Cmd::Password => self.form_password(window, cx),
            Cmd::Membership => self.form_membership(window, cx),
            Cmd::Grant => self.form_privileges(true, window, cx),
            Cmd::Revoke => self.form_privileges(false, window, cx),
            Cmd::Drop => self.drop_role(window, cx),
            _ => {}
        }
    }

    /// `false` (after saying why) when the product cannot change roles.
    fn editable(&self, cx: &mut Context<Self>) -> bool {
        match self.can_edit() {
            Some(why) => {
                cx.emit(AdminEvent::Notice(NoticeKind::Info, why.to_string()));
                false
            }
            None => true,
        }
    }

    fn on_run(&self, cx: &mut Context<Self>) -> form::Run {
        let view = cx.entity().downgrade();
        Rc::new(move |change: Change, window, cx| {
            view.update(cx, |v, cx| v.confirm(change, None, window, cx)).ok();
        })
    }

    /// Show the statement (as `shown`) and run it on confirmation.
    fn confirm(
        &mut self,
        change: Change,
        extra: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let tone = if change.destructive { Tone::Danger } else { Tone::Primary };
        let mut intro = change.summary.clone();
        if let Some(e) = extra {
            intro.push_str("\n\n");
            intro.push_str(&e);
        }
        let item = Item {
            sql: change.shown.clone(),
            badges: if change.destructive { vec![("destructive".to_string(), true)] } else { Vec::new() },
            note: None,
        };
        confirm::open_items(
            window,
            cx,
            "Confirm",
            intro,
            vec![item],
            vec![Choice::new("Run", tone, move |_, cx| {
                let change = change.clone();
                view.update(cx, |v, cx| v.execute(change, cx)).ok();
            })],
        );
    }

    pub(crate) fn execute(&mut self, change: Change, cx: &mut Context<Self>) {
        let open = self.side.take_or_open(&self.store, &self.conn_id, cx);
        let sql = change.sql.clone();
        let task = runtime::spawn(async move {
            let side = open.await?;
            let out = side.execute(&sql, &ExecOptions { row_limit: Some(1) }).await;
            Ok::<_, theta_core::Error>((side, out))
        });
        cx.spawn(async move |this, cx| {
            let r = task.await.and_then(|r| r);
            this.update(cx, |this, cx| {
                let outcome = match r {
                    Ok((side, out)) => {
                        this.side.put(side);
                        out
                    }
                    Err(e) => Err(e),
                };
                let (kind, line) = match &outcome {
                    Ok(_) => (MessageKind::Ok, change.summary.clone()),
                    // A server may echo the statement in its error: scrub the password out of it.
                    Err(e) => (MessageKind::Error, change.scrub(&e.to_string())),
                };
                cx.emit(AdminEvent::Log {
                    connection_id: this.conn_id.clone(),
                    title: change.shown.clone(),
                    kind,
                    lines: vec![line.clone()],
                });
                cx.emit(AdminEvent::Notice(
                    if outcome.is_ok() { NoticeKind::Success } else { NoticeKind::Error },
                    line,
                ));
                this.privileges_for = None;
                this.loading = false;
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ forms

    fn form_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let kind = self.kind;
        let mysql = kind.family() == Family::MySql;
        let mut fields =
            vec![Field::text("name", if mysql { "User or role name" } else { "Role name" }, "", "")];
        if mysql {
            fields.push(Field::text("host", "Host", "%", "% means any host"));
            fields.push(Field::check("is_role", "A role (cannot log in)", false));
        }
        fields.push(Field::secret("password", "Password (optional)"));
        if mysql {
            fields.push(Field::text("limit", "Max connections (blank: unlimited)", "", ""));
            fields.push(Field::check("locked", "Account locked", false));
            fields.push(Field::check("expired", "Password must be changed at first login", false));
        } else {
            fields.push(Field::text("limit", "Connection limit (blank: unlimited)", "", ""));
            fields.push(Field::text("valid", "Valid until (blank: forever)", "", "2030-01-01"));
            fields.push(Field::check("login", "Can log in", true));
            fields.push(Field::check("superuser", "Superuser", false));
            fields.push(Field::check("createdb", "Create databases", false));
            fields.push(Field::check("createrole", "Create roles", false));
            fields.push(Field::check("replication", "Replication", false));
            fields.push(Field::check("inherit", "Inherit privileges", true));
        }
        let build: form::Build = Rc::new(move |v: &Values| {
            let limit = parse_limit(v.text("limit"))?;
            let password = Some(v.text("password").to_string()).filter(|p| !p.is_empty());
            logic::create_role(
                kind,
                &NewRole {
                    name: v.text("name").trim().to_string(),
                    host: v.text("host").to_string(),
                    is_role: v.flag("is_role"),
                    login: v.flag("login"),
                    superuser: v.flag("superuser"),
                    create_db: v.flag("createdb"),
                    create_role: v.flag("createrole"),
                    replication: v.flag("replication"),
                    inherit: v.flag("inherit"),
                    conn_limit: limit,
                    valid_until: Some(v.text("valid").trim().to_string()).filter(|s| !s.is_empty()),
                    password,
                    locked: v.flag("locked"),
                    password_expired: v.flag("expired"),
                },
            )
        });
        let on_run = self.on_run(cx);
        form::open(
            FormSpec {
                title: if mysql { "New User or Role".into() } else { "New Role".into() },
                intro: None,
                fields,
                submit: "Review…".into(),
                build,
                on_run,
            },
            window,
            cx,
        );
    }

    fn form_alter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let Some(role) = self.selected_role().cloned() else { return };
        let kind = self.kind;
        let mysql = kind.family() == Family::MySql;
        let mut fields = Vec::new();
        if mysql {
            fields.push(Field::text("limit", "Max connections (blank: unlimited)", &limit_text(&role), ""));
            fields.push(Field::check("locked", "Account locked", role.locked));
            fields.push(Field::check("expired", "Expire the password now", false));
        } else {
            fields.push(Field::text("limit", "Connection limit (blank: unlimited)", &limit_text(&role), ""));
            fields.push(Field::text(
                "valid",
                "Valid until (blank: forever)",
                role.valid_until.as_deref().unwrap_or(""),
                "",
            ));
            fields.push(Field::check("login", "Can log in", role.login));
            fields.push(Field::check("superuser", "Superuser", role.superuser));
            fields.push(Field::check("createdb", "Create databases", role.create_db));
            fields.push(Field::check("createrole", "Create roles", role.create_role));
            fields.push(Field::check("replication", "Replication", role.replication));
            fields.push(Field::check("inherit", "Inherit privileges", role.inherit));
        }
        let was = role.clone();
        let who = role.clone();
        let build: form::Build = Rc::new(move |v: &Values| {
            // Only what differs is sent: `ALTER ROLE` with an unchanged flag would still be a statement.
            let diff = |now: bool, before: bool| (now != before).then_some(now);
            let limit = parse_limit(v.text("limit"))?;
            let mut a = AlterRole::default();
            if limit != was.conn_limit {
                a.conn_limit = Some(limit);
            }
            if mysql {
                a.locked = diff(v.flag("locked"), was.locked);
                a.expire_password = v.flag("expired").then_some(true);
            } else {
                a.login = diff(v.flag("login"), was.login);
                a.superuser = diff(v.flag("superuser"), was.superuser);
                a.create_db = diff(v.flag("createdb"), was.create_db);
                a.create_role = diff(v.flag("createrole"), was.create_role);
                a.replication = diff(v.flag("replication"), was.replication);
                a.inherit = diff(v.flag("inherit"), was.inherit);
                let valid = Some(v.text("valid").trim().to_string()).filter(|s| !s.is_empty());
                if valid != was.valid_until {
                    a.valid_until = Some(valid);
                }
            }
            logic::alter_role(kind, &was, &a)
        });
        let on_run = self.on_run(cx);
        form::open(
            FormSpec {
                title: format!("Alter {}", who.display()),
                intro: Some("Only the attributes you change are sent.".into()),
                fields,
                submit: "Review…".into(),
                build,
                on_run,
            },
            window,
            cx,
        );
    }

    fn form_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let Some(role) = self.selected_role().cloned() else { return };
        let kind = self.kind;
        let who = role.display();
        let fields =
            vec![Field::secret("password", "New password"), Field::secret("again", "Repeat the password")];
        let build: form::Build = Rc::new(move |v: &Values| {
            if v.text("password") != v.text("again") {
                return Err("The two passwords differ".into());
            }
            logic::set_password(kind, &role, v.text("password"))
        });
        let on_run = self.on_run(cx);
        form::open(
            FormSpec {
                title: format!("Set Password for {who}"),
                intro: Some("The password is sent to the server only. It is shown as '***' in the preview, Messages and everywhere else.".into()),
                fields,
                submit: "Review…".into(),
                build,
                on_run,
            },
            window,
            cx,
        );
    }

    fn form_membership(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let Some(role) = self.selected_role().cloned() else { return };
        let others: Vec<Role> =
            self.roles.iter().filter(|r| r.display() != role.display()).cloned().collect();
        if others.is_empty() {
            cx.emit(AdminEvent::Notice(NoticeKind::Info, "There is no other role to add this one to".into()));
            return;
        }
        let kind = self.kind;
        let names: Vec<String> = others.iter().map(Role::display).collect();
        let fields = vec![
            Field::choice(
                "action",
                "Action",
                vec![
                    format!("Make {} a member of…", role.display()),
                    format!("Remove {} from…", role.display()),
                ],
                0,
            ),
            Field::choice("group", "Role", names, 0),
        ];
        let me = role.clone();
        let build: form::Build = Rc::new(move |v: &Values| {
            let group = others
                .iter()
                .find(|r| r.display() == v.choice("group"))
                .ok_or_else(|| "Choose a role".to_string())?;
            if v.choice("action").starts_with("Make") {
                logic::grant_membership(kind, group, &me)
            } else {
                logic::revoke_membership(kind, group, &me)
            }
        });
        let on_run = self.on_run(cx);
        form::open(
            FormSpec {
                title: format!("Membership of {}", role.display()),
                intro: None,
                fields,
                submit: "Review…".into(),
                build,
                on_run,
            },
            window,
            cx,
        );
    }

    fn form_privileges(&mut self, grant: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let Some(role) = self.selected_role().cloned() else { return };
        let kind = self.kind;
        let mysql = kind.family() == Family::MySql;
        let catalog = self.store.read(cx).catalog(&self.conn_id);
        let first_table = catalog.as_ref().and_then(|c| {
            c.schemas.iter().find_map(|s| s.tables.first().map(|t| (s.name.clone(), t.name.clone())))
        });
        let current_db = match self.store.read(cx).state(&self.conn_id) {
            crate::connections::ConnState::Connected { server, .. } => server.current_database.clone(),
            _ => None,
        }
        .unwrap_or_default();
        let targets: Vec<String> = if mysql {
            vec!["Table".into(), "Database (all tables)".into(), "Everything (*.*)".into()]
        } else {
            vec![
                "Table".into(),
                "All tables in schema".into(),
                "Schema".into(),
                "Sequence".into(),
                "Database".into(),
            ]
        };
        let default_obj =
            first_table.map(|(s, t)| if s.is_empty() { t } else { format!("{s}.{t}") }).unwrap_or_default();
        let mut fields = vec![
            Field::choice("target", "On", targets, 0),
            Field::text(
                "object",
                if mysql {
                    "Object (db.table, or a database name)"
                } else {
                    "Object (schema.table, a schema, or a database)"
                },
                &default_obj,
                &current_db,
            ),
            Field::text("privs", "Privileges (comma separated)", "SELECT", "SELECT, INSERT, UPDATE, DELETE"),
        ];
        if grant {
            fields.push(Field::check("option", "With grant option", false));
        }
        let build: form::Build = Rc::new(move |v: &Values| {
            let target = parse_target(mysql, v.choice("target"), v.text("object").trim())?;
            let privs: Vec<String> =
                v.text("privs").split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
            if grant {
                logic::grant_privileges(kind, &role, &privs, &target, v.flag("option"))
            } else {
                logic::revoke_privileges(kind, &role, &privs, &target)
            }
        });
        let title = format!(
            "{} on {}",
            if grant { "Grant Privileges" } else { "Revoke Privileges" },
            self.selected.clone().unwrap_or_default()
        );
        let on_run = self.on_run(cx);
        form::open(
            FormSpec {
                title,
                intro: Some(format!(
                    "Allowed privileges are checked against the product's list (for example {}).",
                    if mysql { "SELECT, INSERT, ALL PRIVILEGES" } else { "SELECT, INSERT, USAGE, ALL" }
                )),
                fields,
                submit: "Review…".into(),
                build,
                on_run,
            },
            window,
            cx,
        );
    }

    fn drop_role(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let Some(role) = self.selected_role().cloned() else { return };
        let change = match logic::drop_role(self.kind, &role) {
            Ok(c) => c,
            Err(e) => {
                cx.emit(AdminEvent::Notice(NoticeKind::Info, e));
                return;
            }
        };
        let Some(count_sql) = logic::owned_objects_query(self.kind, &role) else {
            self.confirm(change, None, window, cx);
            return;
        };
        // PostgreSQL: count what the role owns first, so the warning is part of the confirmation.
        let open = self.side.take_or_open(&self.store, &self.conn_id, cx);
        let task = runtime::spawn(async move {
            let side = open.await?;
            let out = side.execute(&count_sql, &ExecOptions { row_limit: Some(1) }).await;
            Ok::<_, theta_core::Error>((side, out))
        });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await.and_then(|r| r);
            this.update_in(cx, |this, window, cx| {
                let mut warn = None;
                if let Ok((side, out)) = r {
                    this.side.put(side);
                    if let Some(row) = out.ok().and_then(|o| o.result).and_then(|r| r.rows.into_iter().next())
                    {
                        let n = |i: usize| match row.get(i) {
                            Some(Value::Int(v)) => *v,
                            _ => 0,
                        };
                        warn = logic::drop_warning(n(0), n(1));
                    }
                }
                this.confirm(change, warn, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            if ev.keystroke.key == "escape" && self.filter.read(cx).focus_handle(cx).is_focused(window) {
                self.command(Cmd::Clear, window, cx);
                window.focus(&self.focus, cx);
                cx.stop_propagation();
            }
            return;
        }
        let m = ev.keystroke.modifiers;
        let press = keys::Press {
            key: ev.keystroke.key.as_str(),
            shift: m.shift,
            alt: m.alt,
            primary: m.control || m.platform,
        };
        let modal = Settings::get(cx).keymap.is_modal();
        let (cmd, pending) = keys::interpret(press, modal, self.pending);
        self.pending = pending;
        if let Some(c) = cmd {
            self.command(c, window, cx);
        }
        if cmd.is_some() || pending.is_some() {
            cx.stop_propagation();
        }
    }
}

fn limit_text(r: &Role) -> String {
    r.conn_limit.map(|n| n.to_string()).unwrap_or_default()
}

fn parse_limit(s: &str) -> Result<Option<i64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .map(Some)
        .ok_or_else(|| "The connection limit must be a whole number".to_string())
}

/// The form's target choice and object text as a [`Target`].
fn parse_target(mysql: bool, choice: &str, object: &str) -> Result<Target, String> {
    let split = |o: &str| -> (Option<String>, String) {
        match o.split_once('.') {
            Some((s, t)) => (
                Some(s.trim_matches('"').trim_matches('`').to_string()),
                t.trim_matches('"').trim_matches('`').to_string(),
            ),
            None => (None, o.trim_matches('"').trim_matches('`').to_string()),
        }
    };
    if mysql && choice.starts_with("Everything") {
        return Ok(Target::Global);
    }
    if object.is_empty() {
        return Err("Name the object".into());
    }
    let bare = object.trim_matches('"').trim_matches('`').to_string();
    Ok(match choice {
        "Table" => {
            let (schema, name) = split(object);
            Target::Table { schema, name }
        }
        "Sequence" => {
            let (schema, name) = split(object);
            Target::Sequence { schema, name }
        }
        "All tables in schema" => Target::AllTablesInSchema(bare),
        "Schema" => Target::Schema(bare),
        _ => Target::Database(bare),
    })
}

// ---------------------------------------------------------------------------------------------- painting

impl Render for RolesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        self.narrow = window.viewport_size().width < px(1250.);
        let body = match self.kind.unsupported(Feature::Roles) {
            Some(why) => self.render_unavailable(why, cx),
            None => self.render_body(window, cx),
        };
        v_flex()
            .id("roles-tab")
            .size_full()
            .bg(data_surface(cx))
            .text_color(theme.foreground)
            .key_context("AdminRoles")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_header(cx))
            .child(body)
    }
}

impl RolesView {
    fn render_unavailable(&self, why: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_1()
            .child(Icon::new(IconName::Ban).size(px(22.)).text_color(theme.muted_foreground))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Roles are not available here"),
            )
            .child(
                div()
                    .max_w(px(480.))
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(why.to_string()),
            )
            .into_any_element()
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let off = self.can_edit();
        let system = self.show_system;
        h_flex()
            .flex_none()
            .h(px(42.))
            .px_3()
            .gap_2()
            .items_center()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(IconName::Users).size(px(14.)).text_color(theme.muted_foreground))
            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(self.title()))
            .child(
                div()
                    .px_1p5()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{} {}",
                        self.roles.len(),
                        if self.roles.len() == 1 { "role" } else { "roles" }
                    )),
            )
            .when_some(off, |d, why| {
                d.child(div().text_size(px(11.)).text_color(theme.warning).child(format!("Read-only: {why}")))
            })
            .child(div().flex_1())
            .child(div().w(px(230.)).child(Input::new(&self.filter).small().cleanable(true)))
            .when(self.kind.family() == Family::Postgres, |d| {
                d.child(
                    Button::new("roles-system")
                        .small()
                        .label(if system { "Hide system roles" } else { "Show system roles" })
                        .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::ToggleSystem, w, cx))),
                )
            })
            .child(
                Button::new("roles-refresh")
                    .small()
                    .icon(Icon::new(IconName::RefreshCw))
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::Refresh, w, cx))),
            )
            .child(
                Button::new("roles-new")
                    .small()
                    .primary()
                    .icon(Icon::new(IconName::UserPlus))
                    .label("New…")
                    .disabled(off.is_some())
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::New, w, cx))),
            )
            .into_any_element()
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let cols = columns(self.kind, self.narrow);
        let head = h_flex()
            .flex_none()
            .h(px(24.))
            .items_center()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(cols.iter().map(|(c, w)| {
                sized(*w)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child(c.label())
            }));
        let list = if self.shown.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(if self.error.is_some() { theme.danger } else { theme.muted_foreground })
                .child(match (&self.error, self.last) {
                    (Some(e), None) => format!("Could not read the roles: {e}"),
                    (_, None) => "Loading roles…".to_string(),
                    _ => "No role matches".to_string(),
                })
                .into_any_element()
        } else {
            uniform_list(
                "roles-rows",
                self.shown.len(),
                cx.processor(move |this, range: Range<usize>, _w, cx| {
                    let cols = columns(this.kind, this.narrow);
                    range.map(|i| this.render_row(i, &cols, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let detail = self.render_detail(window, cx);
        v_flex()
            .flex_1()
            .min_h_0()
            .child(head)
            .child(list)
            .child(detail)
            .child(self.render_footer(cx))
            .into_any_element()
    }

    fn render_row(&mut self, pos: usize, cols: &[(Col, f32)], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let r = self.roles[self.shown[pos]].clone();
        let id = r.display();
        let selected = self.selected.as_ref() == Some(&id);
        h_flex()
            .id(("roles-row", pos))
            .h(ROW_HEIGHT)
            .w_full()
            .flex_none()
            .items_center()
            .text_size(px(12.))
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.list_hover))
            .text_color(if !r.login && !r.is_role && self.kind.family() != Family::MySql {
                theme.muted_foreground
            } else {
                theme.foreground
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = Some(id.clone());
                this.load_privileges(cx);
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .children(cols.iter().map(|(c, w)| {
                let color = match c {
                    Col::Super if r.superuser => Some(theme.warning),
                    Col::Locked if r.locked => Some(theme.danger),
                    Col::Expiry if r.password_expired => Some(theme.warning),
                    _ => None,
                };
                sized(*w).when_some(color, |d, c| d.text_color(c)).child(SharedString::from(cell(&r, *c)))
            }))
            .into_any_element()
    }

    fn render_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let short = window.viewport_size().height < px(800.);
        let theme = cx.theme().clone();
        let Some(role) = self.selected_role().cloned() else {
            return div().flex_none().h(px(0.)).into_any_element();
        };
        let off = self.can_edit();
        let caption = |t: &'static str| {
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let line = |t: String| div().text_size(px(12.)).child(t);
        let button = |id: &'static str, label: &'static str, icon: IconName| {
            let b = Button::new(id).small().icon(Icon::new(icon)).label(label);
            if off.is_some() { b.disabled(true) } else { b }
        };
        let mysql = self.kind.family() == Family::MySql;
        let mut attrs: Vec<String> = Vec::new();
        if mysql {
            attrs.push(format!("Plugin: {}", role.plugin.clone().unwrap_or_else(|| "—".into())));
            attrs.push(format!("Account: {}", if role.locked { "locked" } else { "open" }));
            attrs.push(format!(
                "Password: {}",
                if role.password_expired {
                    "expired".to_string()
                } else {
                    role.password_lifetime
                        .clone()
                        .map(|d| format!("expires every {d} days"))
                        .unwrap_or_else(|| "server default".into())
                }
            ));
        } else if self.kind.family() == Family::Postgres {
            attrs.push(format!(
                "Valid until: {}",
                role.valid_until.clone().unwrap_or_else(|| "forever".into())
            ));
            attrs.push(format!("Inherits privileges: {}", if role.inherit { "yes" } else { "no" }));
            if role.bypass_rls {
                attrs.push("Bypasses row-level security".into());
            }
            if let Some(c) = &role.comment {
                attrs.push(format!("Comment: {c}"));
            }
        }
        attrs.push(format!(
            "Connection limit: {}",
            role.conn_limit.map(|n| n.to_string()).unwrap_or_else(|| "unlimited".into())
        ));
        let privs = self.privileges.clone();
        let privileges_note = self.privileges_error.clone();
        let supports_privs = self.kind.family() != Family::SqlServer;
        v_flex()
            .flex_none()
            .h(px(if short { 214. } else { 250. }))
            .gap_2()
            .p_3()
            .bg(theme.tab_bar)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(role.display()))
                    .child(div().flex_1())
                    .child(
                        button("roles-alter", "Alter…", IconName::Settings2)
                            .on_click(cx.listener(|t, _, w, cx| t.command(Cmd::Alter, w, cx))),
                    )
                    .child(
                        button("roles-pw", "Set Password…", IconName::KeyRound)
                            .on_click(cx.listener(|t, _, w, cx| t.command(Cmd::Password, w, cx))),
                    )
                    .child(
                        button("roles-member", "Membership…", IconName::Users)
                            .on_click(cx.listener(|t, _, w, cx| t.command(Cmd::Membership, w, cx))),
                    )
                    .child(
                        button("roles-grant", "Grant…", IconName::Plus)
                            .on_click(cx.listener(|t, _, w, cx| t.command(Cmd::Grant, w, cx))),
                    )
                    .child(
                        button("roles-revoke", "Revoke…", IconName::Minus)
                            .on_click(cx.listener(|t, _, w, cx| t.command(Cmd::Revoke, w, cx))),
                    )
                    .child(
                        button("roles-drop", "Drop…", IconName::Trash)
                            .danger()
                            .on_click(cx.listener(|t, _, w, cx| t.command(Cmd::Drop, w, cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .items_stretch()
                    .child(
                        v_flex()
                            .w(px(if short { 250. } else { 320. }))
                            .flex_none()
                            .gap_1()
                            .child(caption("ATTRIBUTES"))
                            .children(attrs.into_iter().map(line))
                            .child(div().h(px(4.)))
                            .child(caption("MEMBER OF"))
                            .child(line(if role.member_of.is_empty() {
                                "—".into()
                            } else {
                                role.member_of.join(", ")
                            }))
                            .child(caption("MEMBERS"))
                            .child(line(if role.members.is_empty() {
                                "—".into()
                            } else {
                                role.members.join(", ")
                            })),
                    )
                    .child(v_flex().flex_1().min_w_0().gap_1().child(caption("PRIVILEGES")).child(
                        div().id("roles-privs").flex_1().min_h_0().overflow_y_scroll().child(
                            if !supports_privs {
                                line("Privileges are not listed for this product.".into())
                                    .text_color(theme.muted_foreground)
                                    .into_any_element()
                            } else if let Some(n) = privileges_note {
                                line(n).text_color(theme.danger).into_any_element()
                            } else if privs.is_empty() {
                                line("No explicit privileges".into())
                                    .text_color(theme.muted_foreground)
                                    .into_any_element()
                            } else {
                                v_flex()
                                    .gap_0p5()
                                    .children(privs.into_iter().map(|p| {
                                        h_flex()
                                            .gap_2()
                                            .text_size(px(12.))
                                            .child(
                                                div()
                                                    .w(px(70.))
                                                    .flex_none()
                                                    .text_color(theme.muted_foreground)
                                                    .child(p.scope.clone()),
                                            )
                                            .child(
                                                div()
                                                    .w(px(if short { 170. } else { 220. }))
                                                    .flex_none()
                                                    .truncate()
                                                    .child(p.object.clone()),
                                            )
                                            .child(div().flex_1().min_w_0().truncate().child(format!(
                                                "{}{}",
                                                p.privileges.join(", "),
                                                if p.grantable { "  (with grant option)" } else { "" }
                                            )))
                                    }))
                                    .into_any_element()
                            },
                        ),
                    )),
            )
            .into_any_element()
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let modal = Settings::get(cx).keymap.is_modal();
        let move_keys = if modal { "j/k" } else { "↑/↓" };
        h_flex()
            .flex_none()
            .h(px(24.))
            .px_3()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.muted_foreground)
            .child(div().truncate().text_color(if self.error.is_some() { theme.warning } else { theme.muted_foreground }).child(
                self.error.clone().unwrap_or_else(|| "Changes are shown as SQL and confirmed before they run".to_string()),
            ))
            .child(div().flex_1())
            .when(!self.narrow, |d| {
                d.child(div().truncate().child(format!(
                    "{move_keys} move · / filter · r refresh · n new · e alter · w password · m membership · a grant · v revoke · Delete drop"
                )))
            })
            .into_any_element()
    }
}

fn sized(width: f32) -> gpui_kit::Div {
    let d = div().px_2().truncate();
    if width > 0. { d.flex_none().w(px(width)) } else { d.flex_1().min_w(px(160.)) }
}

impl Drop for RolesView {
    fn drop(&mut self) {
        self.side.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_parse() {
        assert_eq!(parse_limit(""), Ok(None));
        assert_eq!(parse_limit(" 5 "), Ok(Some(5)));
        assert!(parse_limit("-1").is_err());
        assert!(parse_limit("x").is_err());
    }

    #[test]
    fn targets_from_the_form() {
        assert_eq!(
            parse_target(false, "Table", "public.orders"),
            Ok(Target::Table { schema: Some("public".into()), name: "orders".into() })
        );
        assert_eq!(parse_target(false, "Schema", "\"s\""), Ok(Target::Schema("s".into())));
        assert_eq!(parse_target(true, "Everything (*.*)", ""), Ok(Target::Global));
        assert!(parse_target(false, "Table", "").is_err());
        assert_eq!(parse_target(true, "Database (all tables)", "shop"), Ok(Target::Database("shop".into())));
    }

    #[test]
    fn columns_differ_by_product() {
        let names = |k| columns(k, false).into_iter().map(|c| c.0.label()).collect::<Vec<_>>();
        assert!(names(DatabaseKind::Postgres).contains(&"Create DB"));
        assert!(
            names(DatabaseKind::MySql).contains(&"Host") && names(DatabaseKind::MySql).contains(&"Plugin")
        );
        assert!(!names(DatabaseKind::SqlServer).contains(&"Host"));
    }
}
