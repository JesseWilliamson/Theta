//! Copy Table: copy a table, with or without its rows, to another schema of the same connection or to
//! another open connection, possibly of a different product.
//!
//! The planning (type mapping, the `CREATE TABLE`, the substitutions) is `theta_sql::copy`, the work is
//! `theta_import::copy_table`; this view collects the choices, shows the exact DDL and every column whose type
//! had to be substituted *before* anything runs, and shows progress and the outcome afterwards, including
//! what was done to leave the target clean.
//!
//! **Sessions.** A copy needs a session that is not a console's, because it holds a transaction open. Server
//! products get sessions of their own (`ConnectionStore::open_side_connection`); SQLite and DuckDB files
//! cannot reliably be opened twice, so the copy runs on the connection's own session and marks the connection
//! busy, as an import does.

use crate::connections::ConnectionStore;
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, div, px,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::{Catalog, Connection, DatabaseKind, Family, Feature, Table};
use theta_import::copy::{Cleanup, CopyJob, CopyOutcome, CopyProgress, CopyReport, copy_table};
use theta_sql::copy::{CopyPlan, plan_copy};

const CONTENT: [&str; 2] = ["Structure and data", "Structure only"];
/// Rows per `INSERT` and per page read from the source.
const PAGE_ROWS: usize = 500;

type Opening = Pin<Box<dyn Future<Output = Result<Arc<dyn Connection>, theta_core::Error>> + Send>>;

struct Target {
    id: String,
    name: String,
    kind: DatabaseKind,
    catalog: Arc<Catalog>,
}

pub struct CopyView {
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    source: Source,
    targets: Vec<Target>,
    target: Entity<SelectState<Vec<String>>>,
    schema: Entity<SelectState<Vec<String>>>,
    name: Entity<InputState>,
    content: Entity<SelectState<Vec<String>>>,
    running: bool,
    /// Connections marked busy for the copy (those whose own session it uses).
    bulk_held: Vec<String>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<CopyProgress>>,
    status: Option<(bool, String)>,
    _subs: Vec<Subscription>,
}

struct Source {
    id: String,
    kind: DatabaseKind,
    schema: String,
    table: Table,
}

impl Drop for CopyView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn open(
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    schema: String,
    table: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(view) = CopyView::build(ws, store, conn_id, schema, table, window, cx) else { return };
    let (for_cancel, focus) = (view.clone(), view.clone());
    window.open_dialog(cx, move |dialog, _, cx| {
        let for_cancel = for_cancel.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Copy Table")
            .w(px(720.))
            .overlay_closable(false)
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| for_cancel.update(cx, |v, cx| v.escape(cx)))
            .child(view.clone())
    });
    // Opened from the palette, the picker hands the keyboard back to the editor after this dialog has
    // opened; without a field in the dialog holding it, Escape and Tab would act on the editor behind.
    window.defer(cx, move |window, cx| {
        let input = focus.read(cx).name.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
}

impl CopyView {
    fn build(
        ws: WeakEntity<Workspace>,
        store: Entity<ConnectionStore>,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<Self>> {
        let (src_kind, src_table, targets) = {
            let s = store.read(cx);
            let kind = s.config(&conn_id)?.kind;
            let catalog = s.catalog(&conn_id)?;
            let (_, t) = catalog.find_table(Some(&schema), &table)?;
            let targets: Vec<Target> = s
                .configs()
                .iter()
                .filter(|c| s.state(&c.id).is_connected())
                .filter_map(|c| {
                    Some(Target {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        kind: c.kind,
                        catalog: s.catalog(&c.id)?,
                    })
                })
                .collect();
            (kind, t.clone(), targets)
        };
        let at = targets.iter().position(|t| t.id == conn_id).unwrap_or(0);
        Some(cx.new(|cx| {
            let target = cx.new(|cx| {
                SelectState::new(
                    targets.iter().map(|t| format!("{} ({})", t.name, t.kind.label())).collect::<Vec<_>>(),
                    Some(IndexPath::new(at)),
                    window,
                    cx,
                )
            });
            let schemas = schema_names(targets.get(at));
            let at_schema = schemas.iter().position(|s| s == &schema).unwrap_or(0);
            let schema_sel =
                cx.new(|cx| SelectState::new(schemas, Some(IndexPath::new(at_schema)), window, cx));
            let name = cx.new(|cx| InputState::new(window, cx).default_value(format!("{table}_copy")));
            let content = cx.new(|cx| {
                SelectState::new(
                    CONTENT.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    Some(IndexPath::new(0)),
                    window,
                    cx,
                )
            });
            let subs = vec![
                cx.subscribe_in(
                    &target,
                    window,
                    |this: &mut Self, _, _: &SelectEvent<Vec<String>>, window, cx| {
                        this.target_changed(window, cx)
                    },
                ),
                cx.subscribe_in(
                    &schema_sel,
                    window,
                    |_: &mut Self, _, _: &SelectEvent<Vec<String>>, _, cx| cx.notify(),
                ),
                cx.subscribe_in(&content, window, |_: &mut Self, _, _: &SelectEvent<Vec<String>>, _, cx| {
                    cx.notify()
                }),
                cx.subscribe_in(&name, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.start(window, cx),
                    _ => {}
                }),
            ];
            CopyView {
                ws,
                store,
                source: Source { id: conn_id, kind: src_kind, schema, table: src_table },
                targets,
                target,
                schema: schema_sel,
                name,
                content,
                running: false,
                bulk_held: Vec::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                progress: Arc::new(Mutex::new(CopyProgress::default())),
                status: None,
                _subs: subs,
            }
        }))
    }

    fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.status = Some((false, "Cancelling…".into()));
            cx.notify();
            return false;
        }
        true
    }

    fn target_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let schemas = schema_names(self.current_target(cx));
        self.schema.update(cx, |s, cx| {
            s.set_items(schemas, window, cx);
            s.set_selected_index(Some(IndexPath::new(0)), window, cx);
        });
        cx.notify();
    }

    fn current_target(&self, cx: &App) -> Option<&Target> {
        self.target.read(cx).selected_index(cx).and_then(|i| self.targets.get(i.row))
    }

    fn chosen(&self, cx: &App) -> Option<(&Target, String, String)> {
        let t = self.current_target(cx)?;
        let schema = self.schema.read(cx).selected_value().cloned().unwrap_or_default();
        Some((t, schema, self.name.read(cx).value().trim().to_string()))
    }

    fn with_data(&self, cx: &App) -> bool {
        self.content.read(cx).selected_value().map(String::as_str) != Some("Structure only")
    }

    /// The plan for the current choices, or why there is none yet.
    fn plan(&self, cx: &App) -> Result<CopyPlan, String> {
        let (t, schema, name) = self.chosen(cx).ok_or("Connect a target first.")?;
        if let Some(why) = t.kind.unsupported(Feature::CopyTable) {
            return Err(format!("{}: {why}", t.kind.label()));
        }
        let plan = plan_copy(self.source.kind, &self.source.table, t.kind, &schema, &name)?;
        if t.catalog.find_table(Some(&schema), &name).is_some() {
            return Err(format!(
                "{} already exists in the target; choose another name.",
                qualified(&schema, &name)
            ));
        }
        Ok(plan)
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let plan = match self.plan(cx) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some((false, e));
                cx.notify();
                return;
            }
        };
        let Some((target, dst_schema, dst_table)) = self.chosen(cx).map(|(t, s, n)| (t.id.clone(), s, n))
        else {
            return;
        };
        let dst_kind = self.current_target(cx).map(|t| t.kind).unwrap_or(self.source.kind);
        let (src_id, dst_id) = (self.source.id.clone(), target);
        let with_data = self.with_data(cx);

        // Sessions: servers get their own; a file database is used through the console's session.
        let own = |kind: DatabaseKind| matches!(kind.family(), Family::Postgres | Family::MySql);
        let store = self.store.read(cx);
        let mut bulk: Vec<String> = Vec::new();
        let src_main = (!own(self.source.kind)).then(|| store.connection(&src_id)).flatten();
        let dst_main = (!own(dst_kind)).then(|| store.connection(&dst_id)).flatten();
        if (!own(self.source.kind) && src_main.is_none()) || (!own(dst_kind) && dst_main.is_none()) {
            self.status = Some((false, "A connection is not open.".into()));
            cx.notify();
            return;
        }
        let open_side =
            |id: &str| -> Option<Opening> { store.open_side_connection(id).map(|f| Box::pin(f) as Opening) };
        let (src_open, dst_open) = (
            own(self.source.kind).then(|| open_side(&src_id)).flatten(),
            own(dst_kind).then(|| open_side(&dst_id)).flatten(),
        );
        if src_main.is_some() {
            bulk.push(src_id.clone());
        }
        if dst_main.is_some() && !bulk.contains(&dst_id) {
            bulk.push(dst_id.clone());
        }
        let rows_estimate = self.source.table.row_estimate;
        let Some(ws) = self.ws.upgrade() else { return };
        for id in &bulk {
            if !ws.update(cx, |w, cx| w.begin_bulk(id, window, cx)) {
                // Release what was reserved before the refusal.
                for done in bulk.iter().take_while(|d| *d != id) {
                    ws.update(cx, |w, cx| w.end_bulk(done, cx));
                }
                return;
            }
        }
        self.bulk_held = bulk.clone();

        let job = CopyJob {
            src_kind: self.source.kind,
            src_schema: (!self.source.schema.is_empty()).then(|| self.source.schema.clone()),
            src_table: self.source.table.name.clone(),
            key: self.source.table.primary_key.clone(),
            columns: plan.columns.clone(),
            dst_kind,
            dst_schema: dst_schema.clone(),
            dst_table: dst_table.clone(),
            create_sql: plan.create.clone(),
            with_data,
            page_rows: PAGE_ROWS,
            rows_estimate,
        };
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.progress.lock().unwrap() = CopyProgress::default();
        self.status = Some((true, "Copying…".into()));
        cx.notify();
        let (cancel, progress) = (self.cancel.clone(), self.progress.clone());
        let task = runtime::spawn(async move {
            let mut closers: Vec<Arc<dyn Connection>> = Vec::new();
            let src: Arc<dyn Connection> = match (src_main, src_open) {
                (Some(c), _) => c,
                (None, Some(f)) => match f.await {
                    Ok(c) => {
                        closers.push(c.clone());
                        c
                    }
                    Err(e) => return Err(format!("could not open a session on the source: {e}")),
                },
                _ => return Err("the source connection no longer exists".into()),
            };
            let dst: Arc<dyn Connection> = match (dst_main, dst_open) {
                (Some(c), _) => c,
                (None, Some(f)) => match f.await {
                    Ok(c) => {
                        closers.push(c.clone());
                        c
                    }
                    Err(e) => return Err(format!("could not open a session on the target: {e}")),
                },
                _ => return Err("the target connection no longer exists".into()),
            };
            let cb = move |p: &CopyProgress| *progress.lock().unwrap() = p.clone();
            let report = copy_table(src.as_ref(), dst.as_ref(), &job, &cancel, &cb).await;
            for c in closers {
                let _ = c.close().await;
            }
            Ok(report)
        });
        self.tick(window, cx);
        let target_label = qualified(&dst_schema, &dst_table);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            this.update_in(cx, |this, _, cx| this.finish(outcome, target_label, dst_id, cx)).ok();
        })
        .detach();
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let alive = this.update(cx, |this, cx| {
                    cx.notify();
                    this.running
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish(
        &mut self,
        outcome: Result<CopyReport, String>,
        label: String,
        dst_id: String,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        if let Some(ws) = self.ws.upgrade() {
            for id in std::mem::take(&mut self.bulk_held) {
                ws.update(cx, |w, cx| w.end_bulk(&id, cx));
            }
        }
        let (ok, text) = match outcome {
            Ok(r) => match &r.outcome {
                CopyOutcome::Completed => {
                    // The explorer must show the new table.
                    self.store.update(cx, |s, cx| s.refresh_catalog(&dst_id, cx));
                    (true, format!("Copied {} rows to {label} in {:.1} s.", r.rows, r.elapsed.as_secs_f64()))
                }
                CopyOutcome::Cancelled => (
                    false,
                    format!("Cancelled after {} rows. {}", r.rows, describe_cleanup(&r.cleanup, &label)),
                ),
                CopyOutcome::Failed(e) => {
                    (false, format!("The copy failed: {e}. {}", describe_cleanup(&r.cleanup, &label)))
                }
            },
            Err(e) => (false, e),
        };
        if let Some(ws) = self.ws.upgrade() {
            let kind = if ok { MessageKind::Ok } else { MessageKind::Error };
            let title =
                format!("COPY TABLE {} TO {label}", qualified(&self.source.schema, &self.source.table.name));
            let src = self.source.id.clone();
            let text = text.clone();
            ws.update(cx, |w, cx| w.log_bulk(&src, &title, kind, vec![text], cx));
        }
        self.status = Some((ok, text));
        cx.notify();
    }
}

fn describe_cleanup(c: &Cleanup, label: &str) -> String {
    match c {
        Cleanup::NotNeeded => String::new(),
        Cleanup::RolledBack => format!("The transaction was rolled back; {label} does not exist."),
        Cleanup::Dropped => format!("The half-made table {label} was dropped."),
        Cleanup::Failed(e) => format!("Clean-up failed: {e}"),
    }
}

fn qualified(schema: &str, table: &str) -> String {
    if schema.is_empty() { table.to_string() } else { format!("{schema}.{table}") }
}

fn schema_names(t: Option<&Target>) -> Vec<String> {
    let Some(t) = t else { return Vec::new() };
    let mut names: Vec<String> = t.catalog.schemas.iter().map(|s| s.name.clone()).collect();
    if names.is_empty() {
        names.push(t.catalog.default_schema.clone().unwrap_or_else(|| "main".into()));
    }
    names
}

impl Render for CopyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let caption = |t: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let plan = self.plan(cx);
        let live = self.progress.lock().unwrap().clone();
        let src_label = qualified(&self.source.schema, &self.source.table.name);
        let can_run = plan.is_ok() && !self.running;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(caption("Copy"))
                    .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(src_label))
                    .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!(
                            "{} · {} columns{}",
                            self.source.kind.label(),
                            self.source.table.columns.len(),
                            self.source
                                .table
                                .row_estimate
                                .map(|n| format!(" · about {n} rows"))
                                .unwrap_or_default()
                        ))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(caption("To connection"))
                            .child(Select::new(&self.target)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(170.))
                            .child(caption("Schema"))
                            .child(Select::new(&self.schema)),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(caption("New table name"))
                            .child(Input::new(&self.name)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(210.))
                            .child(caption("Content"))
                            .child(Select::new(&self.content)),
                    ),
            )
            .child(match &plan {
                Ok(p) => v_flex()
                    .gap_2()
                    .child(caption("This statement will run first"))
                    .child(
                        div()
                            .id("copy-ddl")
                            .max_h(px(176.))
                            .overflow_y_scroll()
                            .p_2()
                            .rounded_md()
                            .bg(theme.muted)
                            .border_1()
                            .border_color(theme.border)
                            .font_family("monospace")
                            .text_size(px(11.))
                            .child(p.create.clone()),
                    )
                    .child(if p.substitutions.is_empty() {
                        // A same-product copy says so in the notes below; the other case needs saying here.
                        div()
                            .when(!p.verbatim, |d| {
                                d.text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child("Every column type has an exact equivalent in the target.")
                            })
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_1()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.warning.opacity(0.6))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.warning)
                                    .child(format!(
                                        "{} column{} will not keep an exact type",
                                        p.substitutions.len(),
                                        if p.substitutions.len() == 1 { "" } else { "s" }
                                    )),
                            )
                            .child(div().id("copy-subs").max_h(px(120.)).overflow_y_scroll().child(
                                v_flex().gap_0p5().children(p.substitutions.iter().map(|s| {
                                    h_flex()
                                        .gap_2()
                                        .text_size(px(11.))
                                        .child(
                                            div()
                                                .w(px(150.))
                                                .flex_shrink_0()
                                                .truncate()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(s.column.clone()),
                                        )
                                        .child(
                                            div()
                                                .w(px(190.))
                                                .flex_shrink_0()
                                                .truncate()
                                                .font_family("monospace")
                                                .child(format!("{} → {}", s.from, s.to)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_color(theme.muted_foreground)
                                                .child(s.reason.clone()),
                                        )
                                })),
                            ))
                            .into_any_element()
                    })
                    .children(p.notes.iter().map(|n| {
                        div().text_size(px(11.)).text_color(theme.muted_foreground).child(n.clone())
                    }))
                    .into_any_element(),
                Err(e) => {
                    div().text_size(px(12.)).text_color(theme.danger).child(e.clone()).into_any_element()
                }
            })
            .child(
                v_flex()
                    .gap_1()
                    .min_h(px(36.))
                    .when_some(self.status.clone(), |d, (ok, t)| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(if ok { theme.foreground } else { theme.danger })
                                .child(t),
                        )
                    })
                    .when(self.running, |d| {
                        d.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div().flex_1().child(
                                        Progress::new("copy-progress")
                                            .value(live.fraction().map_or(0., |f| f * 100.)),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{} rows", live.rows)),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("copy-go")
                            .primary()
                            .label("Copy")
                            .disabled(!can_run)
                            .on_click(cx.listener(|this, _, w, cx| this.start(w, cx))),
                    )
                    .child(
                        Button::new("copy-close")
                            .label(if self.running { "Cancel Copy" } else { "Close" })
                            .on_click(cx.listener(|this, _, w, cx| {
                                if this.running {
                                    this.escape(cx);
                                } else {
                                    w.close_dialog(cx);
                                }
                            })),
                    ),
            )
    }
}
