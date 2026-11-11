//! The Sessions tab: a live table of the server's sessions, with Cancel Query and Terminate Session.
//!
//! The tab owns a session of its own (`ConnectionStore::open_side_connection`), so it keeps refreshing, and can
//! cancel a query, while a console is busy on the shared one. Everything about a product (the query, the row
//! mapping, the kill statement) is `theta_sql::admin::sessions`; this file is the view: refresh loop, sorting
//! and filtering state, painting, and the two confirmations. It never touches the workspace directly: it
//! emits [`AdminEvent`]s.

use super::keys::{self, Cmd};
use super::{AdminEvent, INTERVALS, SideConnection};
use crate::connections::ConnectionStore;
use crate::dialogs::confirm::{self, Choice, Item, Tone};
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::workspace::Workspace;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, UniformListScrollHandle, WeakEntity,
    Window, div, px, uniform_list,
};
use std::ops::Range;
use std::time::{Duration, Instant};
use theta_core::{DatabaseKind, ExecOptions, Value};
use theta_sql::admin::sessions::{self as logic, Action, Queries, Session, SessionColumn};
use theta_ui::{ROW_HEIGHT, Settings, data_surface};

/// (column, width in px; 0 = takes the rest).
const COLUMNS: [(SessionColumn, f32); 10] = [
    (SessionColumn::Id, 64.),
    (SessionColumn::User, 104.),
    (SessionColumn::Database, 104.),
    (SessionColumn::Client, 140.),
    (SessionColumn::Application, 96.),
    (SessionColumn::State, 130.),
    (SessionColumn::Wait, 190.),
    (SessionColumn::TxnAge, 84.),
    (SessionColumn::QueryAge, 84.),
    (SessionColumn::Query, 0.),
];

pub struct SessionsView {
    conn_id: String,
    name: String,
    kind: DatabaseKind,
    store: Entity<ConnectionStore>,
    ws: WeakEntity<Workspace>,
    side: SideConnection,
    queries: Result<Queries, &'static str>,
    rows: Vec<Session>,
    /// Indices into `rows` after filtering and sorting.
    shown: Vec<usize>,
    selected: Option<String>,
    filter: Entity<InputState>,
    sort: (SessionColumn, bool),
    interval: usize,
    interval_select: Entity<SelectState<Vec<String>>>,
    paused: bool,
    loading: bool,
    last: Option<Instant>,
    error: Option<String>,
    /// Lock information could not be read (MySQL), said once in the footer.
    lock_note: Option<String>,
    own_ids: Vec<String>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    pending: keys::Pending,
    _timer: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AdminEvent> for SessionsView {}

impl Focusable for SessionsView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SessionsView {
    pub fn new(
        conn_id: String,
        store: Entity<ConnectionStore>,
        ws: WeakEntity<Workspace>,
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
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter sessions  (/)"));
        let default_interval = 2;
        let interval_select = cx.new(|cx| {
            SelectState::new(
                INTERVALS.iter().map(|(l, _)| l.to_string()).collect::<Vec<_>>(),
                Some(IndexPath::new(default_interval)),
                window,
                cx,
            )
        });
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&filter, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => this.reshape(cx),
            InputEvent::PressEnter { .. } => window.focus(&this.focus, cx),
            _ => {}
        }));
        subs.push(cx.subscribe_in(
            &interval_select,
            window,
            |this, _, ev: &SelectEvent<Vec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(label)) = ev else { return };
                if let Some(i) = INTERVALS.iter().position(|(l, _)| l == label) {
                    this.interval = i;
                }
                window.focus(&this.focus, cx);
                cx.notify();
            },
        ));
        let queries = logic::queries(kind, &banner);
        let mut this = SessionsView {
            conn_id,
            name,
            kind,
            store,
            ws,
            side: SideConnection::default(),
            queries,
            rows: Vec::new(),
            shown: Vec::new(),
            selected: None,
            filter,
            sort: (SessionColumn::Id, false),
            interval: default_interval,
            interval_select,
            paused: false,
            loading: false,
            last: None,
            error: None,
            lock_note: None,
            own_ids: Vec::new(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            pending: None,
            _timer: None,
            _subs: subs,
        };
        if this.queries.is_ok() {
            // Not directly: `refresh` asks the workspace whether a console is busy, and the workspace is
            // the entity that is creating this view right now.
            cx.spawn(async move |this, cx| {
                this.update(cx, |this, cx| this.refresh(cx)).ok();
            })
            .detach();
            this.start_timer(window, cx);
        }
        this
    }

    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    pub fn title(&self) -> String {
        format!("Sessions · {}", self.name)
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        window.focus(&self.focus, cx);
    }

    pub fn session_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn rows(&self) -> &[Session] {
        &self.rows
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn selected_session(&self) -> Option<&Session> {
        let id = self.selected.as_ref()?;
        self.rows.iter().find(|s| &s.id == id)
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    // ------------------------------------------------------------ refreshing

    fn start_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._timer = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let wait = match this.update(cx, |t, _| INTERVALS[t.interval].1) {
                    Ok(ms) => ms,
                    Err(_) => return,
                };
                cx.background_executor().timer(Duration::from_millis(wait)).await;
                let ok = this.update(cx, |t, cx| {
                    if !t.paused && !t.loading {
                        t.refresh(cx);
                    }
                });
                if ok.is_err() {
                    return;
                }
            }
        }));
    }

    /// Fetch the sessions now (also what Run does in this tab).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Ok(q) = self.queries.clone() else { return };
        if self.loading {
            return;
        }
        self.loading = true;
        let open = self.side.take_or_open(&self.store, &self.conn_id, cx);
        // The console's own session, to mark it as Theta's: only asked when the console is not mid-query,
        // because the shared connection would make the question wait behind it.
        let console = (!self.ws.upgrade().is_some_and(|w| w.read(cx).is_busy(&self.conn_id)))
            .then(|| self.store.read(cx).connection(&self.conn_id))
            .flatten();
        let kind = self.kind;
        let task = runtime::spawn(async move {
            let side = open.await?;
            let opts = ExecOptions { row_limit: None };
            let sessions = side.execute(&q.sessions, &opts).await?;
            let mut rows = logic::parse_sessions(&sessions.result.unwrap_or_default());
            let mut note = None;
            if let Some(b) = &q.blockers {
                match side.execute(b, &opts).await {
                    Ok(o) => logic::apply_blockers(&mut rows, &o.result.unwrap_or_default()),
                    Err(e) => note = Some(format!("Lock information is not available: {e}")),
                }
            }
            let mut own = Vec::new();
            if let (Some(sql), Some(c)) = (q.own_id, console)
                && let Ok(o) = c.execute(sql, &opts).await
                && let Some(r) = o.result
                && let Some(v) = r.rows.first().and_then(|r| r.first())
            {
                own.push(v.display());
            }
            Ok::<_, theta_core::Error>((side, rows, note, own))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.and_then(|r| r);
            this.update(cx, |this, cx| this.loaded(kind, result, cx)).ok();
        })
        .detach();
    }

    #[allow(clippy::type_complexity)]
    fn loaded(
        &mut self,
        kind: DatabaseKind,
        result: Result<
            (std::sync::Arc<dyn theta_core::Connection>, Vec<Session>, Option<String>, Vec<String>),
            theta_core::Error,
        >,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        match result {
            Ok((side, mut rows, note, own)) => {
                self.side.put(side);
                self.own_ids = own;
                logic::mark_own(&mut rows, &self.own_ids);
                if kind == DatabaseKind::Postgres || kind == DatabaseKind::TimescaleDb {
                    // The driver names every Theta connection `Theta`: a console's session shows up as ours.
                    for r in &mut rows {
                        r.is_self |= r.application == "Theta";
                    }
                }
                self.rows = rows;
                self.error = None;
                self.lock_note = note;
                self.last = Some(Instant::now());
            }
            Err(e) => {
                // Drop the connection: the next refresh opens a fresh one (the server may have restarted).
                self.side.discard();
                self.error = Some(e.to_string());
            }
        }
        self.reshape(cx);
    }

    /// Recompute what is shown after the rows, filter or sort changed. The selection follows the session
    /// (by id), not the row position, so a refresh does not move it.
    fn reshape(&mut self, cx: &mut Context<Self>) {
        let text = self.filter.read(cx).value().to_string();
        let mut rows: Vec<Session> = self.rows.clone();
        logic::sort(&mut rows, self.sort.0, self.sort.1);
        self.rows = rows;
        self.shown = (0..self.rows.len()).filter(|i| logic::matches_filter(&self.rows[*i], &text)).collect();
        if self.selected.as_ref().is_none_or(|id| !self.shown.iter().any(|i| &self.rows[*i].id == id)) {
            self.selected = self.shown.first().map(|i| self.rows[*i].id.clone());
        }
        cx.emit(AdminEvent::StateChanged);
        cx.notify();
    }

    fn cursor(&self) -> usize {
        self.selected
            .as_ref()
            .and_then(|id| self.shown.iter().position(|i| &self.rows[*i].id == id))
            .unwrap_or(0)
    }

    fn select_at(&mut self, pos: usize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let pos = pos.min(self.shown.len() - 1);
        self.selected = Some(self.rows[self.shown[pos]].id.clone());
        self.scroll.scroll_to_item(pos, gpui_kit::ScrollStrategy::Nearest);
        cx.notify();
    }

    // ------------------------------------------------------------ commands

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
                if self.filter.read(cx).value().is_empty() {
                    return;
                }
                self.filter.update(cx, |s, cx| s.set_value("", window, cx));
            }
            Cmd::Refresh => self.refresh(cx),
            Cmd::Pause => {
                self.paused = !self.paused;
                cx.notify();
            }
            Cmd::Interval => {
                self.interval = (self.interval + 1) % INTERVALS.len();
                let i = self.interval;
                self.interval_select
                    .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(i)), window, cx));
                cx.notify();
            }
            Cmd::SortNext => {
                let at = SessionColumn::ALL.iter().position(|c| *c == self.sort.0).unwrap_or(0);
                self.sort = (SessionColumn::ALL[(at + 1) % SessionColumn::ALL.len()], false);
                self.reshape(cx);
            }
            Cmd::SortReverse => {
                self.sort.1 = !self.sort.1;
                self.reshape(cx);
            }
            Cmd::CancelQuery => self.request(Action::Cancel, window, cx),
            Cmd::Terminate => self.request(Action::Terminate, window, cx),
            Cmd::Copy => self.copy_query(cx),
            Cmd::OpenInConsole => self.open_in_console(cx),
            Cmd::Enter | Cmd::PaneNext => {}
            // The Roles tab's commands.
            _ => {}
        }
    }

    pub fn sort_by(&mut self, col: SessionColumn, cx: &mut Context<Self>) {
        self.sort = if self.sort.0 == col { (col, !self.sort.1) } else { (col, false) };
        self.reshape(cx);
    }

    fn copy_query(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.selected_session() {
            let text = s.query.clone();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.emit(AdminEvent::Notice(crate::connections::NoticeKind::Success, "Query copied".into()));
        }
    }

    fn open_in_console(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.selected_session() {
            let text = s.query.clone();
            if !text.trim().is_empty() {
                cx.emit(AdminEvent::OpenInConsole { connection_id: self.conn_id.clone(), sql: text });
            }
        }
    }

    /// Cancel or terminate the selected session, after showing the exact statement.
    pub fn request(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.selected_session().cloned() else { return };
        let sql = match logic::kill_statement(self.kind, action, &s.id) {
            Ok(sql) => sql,
            Err(why) => {
                cx.emit(AdminEvent::Notice(crate::connections::NoticeKind::Info, why));
                return;
            }
        };
        let who = format!(
            "session {} ({}{})",
            s.id,
            if s.user.is_empty() { "no user".to_string() } else { format!("user {}", s.user) },
            if s.database.is_empty() { String::new() } else { format!(", database {}", s.database) }
        );
        let mut intro = format!("{} on {who}?\n{}", action.label(), logic::effect_note(self.kind, action));
        if s.is_self {
            intro.push_str("\n\nThis is one of Theta's own connections");
            intro.push_str(if s.application == "Theta" || !self.own_ids.contains(&s.id) {
                " (this tab's session, or a console's): "
            } else {
                " (the session your consoles use for this connection): "
            });
            intro.push_str(match action {
                Action::Cancel => "cancelling it stops whatever it is running for you.",
                Action::Terminate => "terminating it makes the next statement reconnect, and any open transaction in it is lost.",
            });
        }
        let view = cx.entity().downgrade();
        let label = action.label().to_string();
        let sql_run = sql.clone();
        let id = s.id.clone();
        confirm::open_items(
            window,
            cx,
            action.label(),
            intro,
            vec![Item {
                sql: sql.clone(),
                badges: vec![("affects another session".to_string(), true)],
                note: None,
            }],
            vec![Choice::new(label, Tone::Danger, move |_, cx| {
                view.update(cx, |v, cx| v.execute_kill(action, id.clone(), sql_run.clone(), cx)).ok();
            })],
        );
    }

    fn execute_kill(&mut self, action: Action, id: String, sql: String, cx: &mut Context<Self>) {
        let open = self.side.take_or_open(&self.store, &self.conn_id, cx);
        let q = sql.clone();
        let task = runtime::spawn(async move {
            let side = open.await?;
            let out = side.execute(&q, &ExecOptions { row_limit: Some(1) }).await;
            Ok::<_, theta_core::Error>((side, out))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await.and_then(|r| r);
            this.update(cx, |this, cx| {
                let (kind, lines, ok) = match res {
                    Ok((side, Ok(out))) => {
                        this.side.put(side);
                        // `pg_cancel_backend` answers false when it could not signal the backend.
                        let refused = out
                            .result
                            .as_ref()
                            .and_then(|r| r.rows.first())
                            .and_then(|r| r.first())
                            .is_some_and(|v| matches!(v, Value::Bool(false)));
                        if refused {
                            (MessageKind::Error, vec![format!("The server did not signal session {id}: it may have finished already, or you may lack the privilege.")], false)
                        } else {
                            (MessageKind::Ok, vec![format!("{} sent to session {id}.", action.label())], true)
                        }
                    }
                    Ok((side, Err(e))) => {
                        this.side.put(side);
                        (MessageKind::Error, vec![e.to_string()], false)
                    }
                    Err(e) => (MessageKind::Error, vec![e.to_string()], false),
                };
                cx.emit(AdminEvent::Log { connection_id: this.conn_id.clone(), title: sql.clone(), kind, lines: lines.clone() });
                cx.emit(AdminEvent::Notice(
                    if ok { crate::connections::NoticeKind::Success } else { crate::connections::NoticeKind::Error },
                    lines.join(" "),
                ));
                // Give the server a moment to act, then show the result.
                this.loading = false;
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            // Escape in the filter box clears it and gives the list the keyboard back.
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

// ---------------------------------------------------------------------------------------------- painting

fn cell_text(s: &Session, c: SessionColumn) -> String {
    match c {
        SessionColumn::Id => s.id.clone(),
        SessionColumn::User => s.user.clone(),
        SessionColumn::Database => s.database.clone(),
        SessionColumn::Client => s.client.clone(),
        SessionColumn::Application => s.application.clone(),
        SessionColumn::State => s.state.clone(),
        SessionColumn::Wait => s.wait.clone(),
        SessionColumn::TxnAge => s.txn_age.map(logic::format_age).unwrap_or_default(),
        SessionColumn::QueryAge => s.query_age.map(logic::format_age).unwrap_or_default(),
        // One line: a statement's own line breaks would make rows uneven.
        SessionColumn::Query => s.query.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

fn is_number(c: SessionColumn) -> bool {
    matches!(c, SessionColumn::TxnAge | SessionColumn::QueryAge | SessionColumn::Id)
}

fn sized(width: f32) -> gpui_kit::Div {
    let d = div().px_2().truncate();
    if width > 0. { d.flex_none().w(px(width)) } else { d.flex_1().min_w(px(160.)) }
}

impl Render for SessionsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let surface = data_surface(cx);
        let body = match &self.queries {
            Err(why) => self.render_unavailable(why, cx),
            Ok(_) => self.render_body(window, cx),
        };
        v_flex()
            .id("sessions-tab")
            .size_full()
            .bg(surface)
            .text_color(theme.foreground)
            .key_context("AdminSessions")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_header(cx))
            .child(body)
    }
}

impl SessionsView {
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
                    .child("Sessions are not available here"),
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
        let blocked = self.rows.iter().filter(|s| logic::is_blocked(s)).count();
        let chip = |text: String, danger: bool| {
            let color = if danger { theme.warning } else { theme.muted_foreground };
            div()
                .px_1p5()
                .h(px(18.))
                .flex()
                .items_center()
                .rounded(px(3.))
                .border_1()
                .border_color(if danger { color.opacity(0.6) } else { theme.border })
                .text_size(px(11.))
                .text_color(color)
                .child(text)
        };
        let paused = self.paused;
        h_flex()
            .flex_none()
            .h(px(42.))
            .px_3()
            .gap_2()
            .items_center()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(IconName::Activity).size(px(14.)).text_color(theme.muted_foreground))
            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(self.title()))
            .child(chip(
                format!("{} {}", self.rows.len(), if self.rows.len() == 1 { "session" } else { "sessions" }),
                false,
            ))
            .when(blocked > 0, |d| d.child(chip(format!("{blocked} blocked"), true)))
            .child(div().flex_1())
            .child(div().w(px(230.)).child(Input::new(&self.filter).small().cleanable(true)))
            .child(div().w(px(84.)).child(Select::new(&self.interval_select).small()))
            .child(
                Button::new("sessions-pause")
                    .small()
                    .icon(Icon::new(if paused { IconName::Play } else { IconName::Pause }))
                    .label(if paused { "Resume" } else { "Pause" })
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::Pause, w, cx))),
            )
            .child(
                Button::new("sessions-refresh")
                    .small()
                    .icon(Icon::new(IconName::RefreshCw))
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::Refresh, w, cx))),
            )
            .into_any_element()
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let head = h_flex()
            .flex_none()
            .h(px(24.))
            .items_center()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(COLUMNS.iter().map(|(c, w)| {
                let col = *c;
                let active = self.sort.0 == col;
                sized(*w)
                    .id(("sessions-col", col as usize))
                    .cursor_pointer()
                    .when(is_number(col), |d| d.text_right())
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active { theme.foreground } else { theme.muted_foreground })
                    .on_click(cx.listener(move |this, _, _, cx| this.sort_by(col, cx)))
                    .child(format!(
                        "{}{}",
                        col.label(),
                        if active { if self.sort.1 { " ↓" } else { " ↑" } } else { "" }
                    ))
            }));
        let list = if self.shown.is_empty() {
            let text = if self.rows.is_empty() && self.last.is_none() && self.error.is_none() {
                "Loading sessions…"
            } else if self.rows.is_empty() {
                "No sessions"
            } else {
                "No session matches the filter"
            };
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        } else {
            uniform_list(
                "sessions-rows",
                self.shown.len(),
                cx.processor(|this, range: Range<usize>, _w, cx| {
                    range.map(|i| this.render_row(i, cx)).collect::<Vec<_>>()
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

    fn render_row(&mut self, pos: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.rows[self.shown[pos]].clone();
        let selected = self.selected.as_ref() == Some(&s.id);
        let blocked = logic::is_blocked(&s);
        let blocker = logic::blocks_others(&self.rows, &s);
        let idle = s.state.starts_with("idle") && !blocked;
        let stripe = if blocked {
            Some(theme.warning)
        } else if blocker {
            Some(theme.danger)
        } else {
            None
        };
        let id = s.id.clone();
        h_flex()
            .id(("sessions-row", pos))
            .relative()
            .h(ROW_HEIGHT)
            .w_full()
            .flex_none()
            .items_center()
            .text_size(px(12.))
            .cursor_pointer()
            .when(blocked, |d| d.bg(theme.warning.opacity(0.10)))
            .when(blocker && !blocked, |d| d.bg(theme.danger.opacity(0.08)))
            .when(selected, |d| d.bg(theme.list_hover))
            .text_color(if idle { theme.muted_foreground } else { theme.foreground })
            .when_some(stripe, |d, c| d.child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(c)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = Some(id.clone());
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .children(COLUMNS.iter().map(|(c, w)| {
                let mut text = cell_text(&s, *c);
                if *c == SessionColumn::Id && s.is_self {
                    text.push_str(" ●");
                }
                if *c == SessionColumn::Wait && blocked {
                    text = format!("blocked by {}", s.blocked_by.join(", "));
                }
                if *c == SessionColumn::State && blocker && !blocked {
                    text = format!("{} · blocking", s.state);
                }
                let color = match c {
                    SessionColumn::Wait if blocked => Some(theme.warning),
                    SessionColumn::State if blocker => Some(theme.danger),
                    _ => None,
                };
                sized(*w)
                    .when_some(color, |d, c| d.text_color(c))
                    .when(is_number(*c), |d| d.text_right())
                    .child(SharedString::from(text))
            }))
            .into_any_element()
    }

    fn render_detail(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = Settings::get(cx).buffer_font_family.clone();
        let Some(s) = self.selected_session().cloned() else {
            return div().flex_none().h(px(0.)).into_any_element();
        };
        let kind = self.kind;
        let cancel_off = logic::action_unavailable(kind, Action::Cancel);
        let term_off = logic::action_unavailable(kind, Action::Terminate);
        let blocked_by: Vec<String> = s
            .blocked_by
            .iter()
            .map(|id| match self.rows.iter().find(|o| &o.id == id) {
                Some(o) => format!(
                    "{id} ({}{})",
                    if o.user.is_empty() { "?" } else { &o.user },
                    if o.is_self { ", Theta" } else { "" }
                ),
                None => id.clone(),
            })
            .collect();
        let blocking: Vec<String> =
            self.rows.iter().filter(|o| o.blocked_by.contains(&s.id)).map(|o| o.id.clone()).collect();
        let field = |label: &'static str, value: String| {
            v_flex()
                .gap_0p5()
                .min_w(px(110.))
                .child(div().text_size(px(10.5)).text_color(theme.muted_foreground).child(label))
                .child(div().text_size(px(12.)).child(if value.is_empty() {
                    "—".to_string()
                } else {
                    value
                }))
        };
        let button = |id: &'static str, label: &'static str, icon: IconName, off: Option<String>| {
            let b = Button::new(id).small().icon(Icon::new(icon)).label(label);
            match off {
                Some(_) => b.disabled(true),
                None => b,
            }
        };
        v_flex()
            .flex_none()
            .h(px(238.))
            .gap_2()
            .p_3()
            .bg(theme.tab_bar)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .items_start()
                    .child(field("Session", format!("{}{}", s.id, if s.is_self { "  (Theta)" } else { "" })))
                    .child(field("User", s.user.clone()))
                    .child(field("Database", s.database.clone()))
                    .child(field("Client", s.client.clone()).min_w(px(180.)))
                    .child(field("State", s.state.clone()))
                    .child(field("Transaction", s.txn_age.map(logic::format_age).unwrap_or_default()))
                    .child(field("Query age", s.query_age.map(logic::format_age).unwrap_or_default()))
                    .when(!blocked_by.is_empty(), |d| d.child(field("Blocked by", blocked_by.join(", "))))
                    .when(!blocking.is_empty(), |d| d.child(field("Blocking", blocking.join(", ")))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(cancel_off.clone().or(term_off.clone()).unwrap_or_default()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                button("sess-copy", "Copy", IconName::Copy, None)
                                    .on_click(cx.listener(|this, _, w, cx| this.command(Cmd::Copy, w, cx))),
                            )
                            .child(
                                button("sess-console", "Open in Console", IconName::SquareTerminal, None)
                                    .on_click(
                                        cx.listener(|this, _, w, cx| this.command(Cmd::OpenInConsole, w, cx)),
                                    ),
                            )
                            .child(
                                button("sess-cancel", "Cancel Query", IconName::Square, cancel_off.clone())
                                    .on_click(
                                        cx.listener(|this, _, w, cx| this.request(Action::Cancel, w, cx)),
                                    ),
                            )
                            .child(
                                button("sess-term", "Terminate Session", IconName::Skull, term_off.clone())
                                    .danger()
                                    .on_click(
                                        cx.listener(|this, _, w, cx| this.request(Action::Terminate, w, cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .id("sessions-query")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_md()
                    .bg(theme.muted)
                    .border_1()
                    .border_color(theme.border)
                    .font_family(mono)
                    .text_size(px(12.))
                    .child(if s.query.trim().is_empty() {
                        "(no statement)".to_string()
                    } else {
                        s.query.clone()
                    }),
            )
            .into_any_element()
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let secs = INTERVALS[self.interval].1 as f64 / 1000.0;
        let status = match (&self.error, self.last) {
            (Some(e), _) => format!("Could not read the sessions: {e}"),
            (None, Some(t)) => format!(
                "Updated {} s ago · {}",
                t.elapsed().as_secs(),
                if self.paused { "paused".to_string() } else { format!("refreshing every {secs:.0} s") }
            ),
            (None, None) => "Connecting…".to_string(),
        };
        let modal = Settings::get(cx).keymap.is_modal();
        let keys = if modal {
            "● Theta's own · j/k move · / filter · r refresh · p pause · i interval · s sort · c cancel · x terminate · y copy · o console"
        } else {
            "● Theta's own · ↑/↓ move · / filter · r refresh · p pause · i interval · s sort · c cancel · x terminate · y copy · o console"
        };
        h_flex()
            .flex_none()
            .h(px(24.))
            .px_3()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(if self.error.is_some() { theme.danger } else { theme.muted_foreground })
            .child(div().truncate().child(status))
            .when_some(self.lock_note.clone(), |d, n| {
                d.child(div().truncate().text_color(theme.warning).child(n))
            })
            .child(div().flex_1())
            .child(div().truncate().text_color(theme.muted_foreground).child(keys))
            .into_any_element()
    }
}

impl Drop for SessionsView {
    fn drop(&mut self) {
        self.side.close();
    }
}
