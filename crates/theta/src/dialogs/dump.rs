//! Export Database: Theta's own streaming SQL dump, or `pg_dump` / `mysqldump` when one is installed.
//!
//! The built-in dump (`theta_import::dump`) needs nothing installed and works for every product with a
//! verified DDL generator, but it is not a full backup: the dialog lists what it leaves out for this product
//! before anything is written. The native tool is the complete alternative; its exact command line is shown,
//! and the password reaches it through the environment, never an argument.

use crate::connections::ConnectionStore;
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::{Catalog, DatabaseKind, Feature, TableKind};
use theta_import::dump::{DumpOptions, DumpOutcome, DumpProgress, dump_to_path, not_captured};
use theta_import::native::{self, NativeRequest};

/// `(schema, name)`.
type QualName = (String, String);

const CONTENT: [&str; 3] = ["Structure and data", "Structure only", "Data only"];

struct Item {
    schema: String,
    name: String,
    view: bool,
    checked: bool,
    rows: Option<u64>,
}

pub struct DumpView {
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    kind: DatabaseKind,
    catalog: Arc<Catalog>,
    items: Vec<Item>,
    content: Entity<SelectState<Vec<String>>>,
    qualify: bool,
    path: Entity<InputState>,
    per_insert: Entity<InputState>,
    native: Option<PathBuf>,
    native_reason: Option<String>,
    use_native: bool,
    running: bool,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<DumpProgress>>,
    status: Option<(bool, String)>,
    notes: Vec<String>,
    scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

impl Drop for DumpView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn open(
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(view) = DumpView::build(ws, store, conn_id, window, cx) else { return };
    let for_cancel = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let for_cancel = for_cancel.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Export Database")
            .w(px(680.))
            .overlay_closable(false)
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| for_cancel.update(cx, |v, cx| v.escape(cx)))
            .child(view.clone())
    });
}

impl DumpView {
    fn build(
        ws: WeakEntity<Workspace>,
        store: Entity<ConnectionStore>,
        conn_id: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<Self>> {
        let (config, catalog) = {
            let s = store.read(cx);
            (s.config(&conn_id).cloned()?, s.catalog(&conn_id)?)
        };
        let kind = config.kind;
        let mut items = Vec::new();
        for s in &catalog.schemas {
            // PostGIS and similar extensions own tables of their own; they are listed but start unchecked.
            for t in &s.tables {
                let extension_owned = t.name == "spatial_ref_sys";
                items.push(Item {
                    schema: s.name.clone(),
                    name: t.name.clone(),
                    view: matches!(t.kind, TableKind::View),
                    checked: matches!(t.kind, TableKind::Table | TableKind::View) && !extension_owned,
                    rows: t.row_estimate,
                });
            }
        }
        items.retain(|i| {
            i.view
                || catalog
                    .find_table(Some(&i.schema), &i.name)
                    .is_some_and(|(_, t)| t.kind == TableKind::Table)
        });
        let single_schema = catalog.schemas.iter().filter(|s| !s.tables.is_empty()).count() <= 1;
        let (native, native_reason) = native_tool(kind, &config);
        let stem = if !config.database.is_empty() {
            config.database.clone()
        } else {
            PathBuf::from(&config.path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "database".into())
        };
        let default = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(format!("{stem}.sql"))
            .display()
            .to_string();
        Some(cx.new(|cx| {
            let content = cx.new(|cx| {
                SelectState::new(
                    CONTENT.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    Some(IndexPath::new(0)),
                    window,
                    cx,
                )
            });
            let path = cx.new(|cx| {
                InputState::new(window, cx).default_value(default).placeholder("Where to write the file")
            });
            let per_insert = cx.new(|cx| InputState::new(window, cx).default_value("500"));
            let sub =
                cx.subscribe_in(&content, window, |_, _, _: &SelectEvent<Vec<String>>, _, cx| cx.notify());
            DumpView {
                ws,
                store,
                conn_id,
                kind,
                catalog,
                items,
                content,
                qualify: !single_schema,
                path,
                per_insert,
                native,
                native_reason,
                use_native: false,
                running: false,
                cancel: Arc::new(AtomicBool::new(false)),
                progress: Arc::new(Mutex::new(DumpProgress::default())),
                status: None,
                notes: Vec::new(),
                scroll: UniformListScrollHandle::new(),
                _subs: vec![sub],
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

    fn structure_data(&self, cx: &App) -> (bool, bool) {
        match self.content.read(cx).selected_value().map(String::as_str) {
            Some("Structure only") => (true, false),
            Some("Data only") => (false, true),
            _ => (true, true),
        }
    }

    fn chosen(&self) -> (Vec<QualName>, Vec<QualName>) {
        let pick = |view: bool| {
            self.items
                .iter()
                .filter(|i| i.checked && i.view == view)
                .map(|i| (i.schema.clone(), i.name.clone()))
                .collect::<Vec<_>>()
        };
        (pick(false), pick(true))
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = PathBuf::from(self.path.read(cx).value().to_string());
        let dir = current.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let name = current.file_name().map(|n| n.to_string_lossy().to_string());
        let rx = cx.prompt_for_new_path(&dir, name.as_deref());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(p))) = rx.await else { return };
            this.update_in(cx, |this, window, cx| {
                this.path.update(cx, |s, cx| s.set_value(p.display().to_string(), window, cx))
            })
            .ok();
        })
        .detach();
    }

    /// The command that would run, for the preview and for starting it.
    fn native_command(&self, cx: &App) -> Result<(PathBuf, native::NativeCommand), String> {
        let program = self.native.clone().ok_or_else(|| "no dump tool found".to_string())?;
        let config = self.store.read(cx).config(&self.conn_id).cloned().ok_or("the connection is gone")?;
        let (structure, data) = self.structure_data(cx);
        let (tables, _) = self.chosen();
        let all = self.items.iter().filter(|i| !i.view).all(|i| i.checked);
        let out = PathBuf::from(self.path.read(cx).value().trim());
        let has_password = self
            .store
            .read(cx)
            .secrets()
            .with_secrets(&config)
            .password
            .as_deref()
            .is_some_and(|p| !p.is_empty());
        let req = NativeRequest {
            kind: self.kind,
            host: &config.host,
            port: config.port,
            user: &config.user,
            database: &config.database,
            has_password,
            tables: if all { Vec::new() } else { tables },
            structure,
            data,
            output: &out,
        };
        let cmd = native::command(&program.display().to_string(), &req)?;
        Ok((program, cmd))
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let path = PathBuf::from(self.path.read(cx).value().trim());
        if path.as_os_str().is_empty() {
            self.status = Some((false, "Enter the file to write.".into()));
            cx.notify();
            return;
        }
        let (tables, views) = self.chosen();
        if tables.is_empty() && views.is_empty() {
            self.status = Some((false, "Choose at least one table or view.".into()));
            cx.notify();
            return;
        }
        let (structure, data) = self.structure_data(cx);
        let schemas: std::collections::HashSet<&str> =
            tables.iter().chain(views.iter()).map(|(s, _)| s.as_str()).collect();
        if !self.qualify && schemas.len() > 1 && !self.use_native {
            self.status = Some((
                false,
                "The chosen tables are in several schemas; turn on “Write schema names” so they do not collide.".into(),
            ));
            cx.notify();
            return;
        }
        if self.use_native {
            self.start_native(path, window, cx);
            return;
        }
        let per = self.per_insert.read(cx).value().trim().parse::<usize>().unwrap_or(0);
        if per == 0 {
            self.status = Some((false, "Rows per INSERT must be a positive number.".into()));
            cx.notify();
            return;
        }
        let Some(connection) = self.store.read(cx).connection(&self.conn_id) else {
            self.status = Some((false, "The connection is not open.".into()));
            cx.notify();
            return;
        };
        let Some(ws) = self.ws.upgrade() else { return };
        if !ws.update(cx, |w, cx| w.begin_bulk(&self.conn_id, window, cx)) {
            return;
        }
        let mut opts = DumpOptions::new(self.kind);
        opts.tables = tables;
        opts.views = views;
        opts.structure = structure;
        opts.data = data;
        opts.qualify = self.qualify;
        opts.rows_per_insert = per;
        self.running = true;
        self.notes.clear();
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.progress.lock().unwrap() = DumpProgress::default();
        self.status = Some((true, "Writing the dump…".into()));
        cx.notify();
        let (cancel, progress, catalog) = (self.cancel.clone(), self.progress.clone(), self.catalog.clone());
        let target = path.clone();
        let task = runtime::spawn(async move {
            let cb = move |p: &DumpProgress| *progress.lock().unwrap() = p.clone();
            dump_to_path(connection.as_ref(), &catalog, &opts, &target, &cancel, &cb).await
        });
        self.tick(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let rep = task.await;
            this.update_in(cx, |this, _, cx| this.finish(path, rep.map_err(|e| e.to_string()), cx)).ok();
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
        path: PathBuf,
        rep: Result<theta_import::DumpReport, String>,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        let conn = self.conn_id.clone();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.end_bulk(&conn, cx));
        }
        let (ok, text) = match rep {
            Ok(r) => {
                self.notes = r.notes.clone();
                match r.outcome {
                    DumpOutcome::Completed => (
                        true,
                        format!(
                            "Dumped {} table(s), {} rows ({}) to {} in {:.1}s.",
                            r.tables,
                            r.rows,
                            human(r.bytes),
                            path.display(),
                            r.elapsed.as_secs_f64()
                        ),
                    ),
                    DumpOutcome::Cancelled => (false, "Cancelled. No file was written.".to_string()),
                    DumpOutcome::Failed(e) => (false, format!("The dump failed, no file was written: {e}")),
                }
            }
            Err(e) => (false, e),
        };
        self.log(&path, ok, &text, cx);
        self.status = Some((ok, text));
        cx.notify();
    }

    fn log(&self, path: &std::path::Path, ok: bool, text: &str, cx: &mut Context<Self>) {
        if let Some(ws) = self.ws.upgrade() {
            let kind = if ok { MessageKind::Ok } else { MessageKind::Error };
            let title = format!("DUMP TO {}", path.display());
            let mut lines = vec![text.to_string()];
            lines.extend(self.notes.iter().cloned());
            let conn = self.conn_id.clone();
            ws.update(cx, |w, cx| w.log_bulk(&conn, &title, kind, lines, cx));
        }
    }

    fn start_native(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let (program, cmd) = match self.native_command(cx) {
            Ok(c) => c,
            Err(e) => {
                self.status = Some((false, e));
                cx.notify();
                return;
            }
        };
        let config = self.store.read(cx).config(&self.conn_id).cloned();
        let password = config.and_then(|c| self.store.read(cx).secrets().with_secrets(&c).password);
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.status = Some((true, format!("Running {}…", cmd.display())));
        cx.notify();
        let cancel = self.cancel.clone();
        let args = cmd.args.clone();
        let env = cmd.password_env.zip(password);
        let task = runtime::spawn(async move {
            let mut c = std::process::Command::new(&program);
            c.args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if let Some((name, value)) = env {
                c.env(name, value);
            }
            let mut child = c.spawn().map_err(|e| format!("could not start {}: {e}", program.display()))?;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Cancelled.".to_string());
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let mut err = String::new();
                        if let Some(mut e) = child.stderr.take() {
                            use std::io::Read;
                            let _ = e.read_to_string(&mut err);
                        }
                        return if status.success() {
                            Ok(())
                        } else {
                            Err(format!("{status}: {}", err.trim()))
                        };
                    }
                    Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                    Err(e) => return Err(e.to_string()),
                }
            }
        });
        self.tick(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            this.update_in(cx, |this, _, cx| {
                this.running = false;
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let (ok, text) = match r {
                    Ok(()) => (true, format!("Wrote {} ({}).", path.display(), human(size))),
                    Err(e) => (false, format!("The dump tool failed: {e}")),
                };
                this.log(&path, ok, &text, cx);
                this.status = Some((ok, text));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The native tool for this product, or why there is none to offer.
fn native_tool(
    kind: DatabaseKind,
    config: &theta_core::ConnectionConfig,
) -> (Option<PathBuf>, Option<String>) {
    let names = native::program_names(kind);
    if names.is_empty() {
        return (None, Some("this product has no native dump tool Theta can drive".into()));
    }
    if config.ssh.is_some() {
        return (
            None,
            Some(
                "the connection goes through an SSH tunnel, which the command line tool would not use".into(),
            ),
        );
    }
    let path = std::env::var("PATH").unwrap_or_default();
    match native::find_in_path(names, &path) {
        Some(p) => (Some(p), None),
        None => (None, Some(format!("{} was not found on PATH", names.join(" or ")))),
    }
}

fn human(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}

impl Render for DumpView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let live = self.progress.lock().unwrap().clone();
        let caption = |t: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let chosen = self.items.iter().filter(|i| i.checked).count();
        let unsupported = self.kind.unsupported(Feature::Dump);
        let native_cmd = if self.use_native { Some(self.native_command(cx)) } else { None };
        let fraction = if live.tables_total > 0 {
            live.tables_done as f32 / live.tables_total as f32 * 100.
        } else {
            0.
        };
        let missing: Vec<String> = not_captured(self.kind).into_iter().map(|l| format!("• {l}")).collect();
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(caption("Tables and views"))
                    .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!("{chosen} of {} chosen", self.items.len())))
                    .child(div().flex_1())
                    .child(Button::new("dump-all").label("All").on_click(cx.listener(|this, _, _, cx| {
                        this.items.iter_mut().for_each(|i| i.checked = true);
                        cx.notify();
                    })))
                    .child(Button::new("dump-none").label("None").on_click(cx.listener(|this, _, _, cx| {
                        this.items.iter_mut().for_each(|i| i.checked = false);
                        cx.notify();
                    }))),
            )
            .child(
                div().h(px(170.)).border_1().border_color(theme.border).rounded_md().child(
                    uniform_list(
                        "dump-items",
                        self.items.len(),
                        cx.processor(|this, range: Range<usize>, _w, cx| {
                            let theme = cx.theme().clone();
                            range
                                .map(|i| {
                                    let it = &this.items[i];
                                    h_flex()
                                        .id(("dump-item", i))
                                        .h(px(24.))
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
                                        .child(div().flex_1().truncate().child(if it.schema.is_empty() { it.name.clone() } else { format!("{}.{}", it.schema, it.name) }))
                                        .child(div().text_color(theme.muted_foreground).child(if it.view { "view".to_string() } else { it.rows.map(|n| format!("~{n} rows")).unwrap_or_default() }))
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(v_flex().gap_1().w(px(200.)).child(caption("Content")).child(Select::new(&self.content)))
                    .child(v_flex().gap_1().w(px(120.)).child(caption("Rows per INSERT")).child(Input::new(&self.per_insert)))
                    .child(div().flex_1().pb_1().child(Switch::new("dump-qualify").label("Write schema names").checked(self.qualify).on_change(cx.listener(|this, v, _, cx| {
                        this.qualify = *v;
                        cx.notify();
                    })))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(v_flex().gap_1().flex_1().min_w_0().child(caption("File")).child(Input::new(&self.path)))
                    .child(Button::new("dump-browse").label("Browse…").on_click(cx.listener(|this, _, w, cx| this.browse(w, cx)))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(theme.muted)
                    .border_1()
                    .border_color(theme.border)
                    .child(div().text_size(px(11.)).font_weight(FontWeight::MEDIUM).child("This dump does not contain:"))
                    .child(div().id("dump-missing").max_h(px(84.)).overflow_y_scroll().text_size(px(11.)).text_color(theme.muted_foreground).child(v_flex().gap_0p5().children(missing.into_iter().map(|l| div().child(l))))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Switch::new("dump-native")
                            .label(format!("Use {} instead (the complete, native format)", native::program_names(self.kind).first().copied().unwrap_or("the native tool")))
                            .checked(self.use_native)
                            .disabled(self.native.is_none())
                            .on_change(cx.listener(|this, v, _, cx| {
                                this.use_native = *v;
                                cx.notify();
                            })),
                    )
                    .when_some(self.native_reason.clone(), |d, why| {
                        d.child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!("Not available: {why}")))
                    })
                    .when_some(native_cmd, |d, cmd| {
                        d.child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(match cmd {
                            Ok((_, c)) => format!("Command: {}   (the password goes in the environment variable named, not in the arguments)", c.display()),
                            Err(e) => e,
                        }))
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_h(px(36.))
                    .when_some(self.status.clone(), |d, (ok, t)| {
                        d.child(div().text_size(px(12.)).text_color(if ok { theme.foreground } else { theme.danger }).child(t))
                    })
                    .children(self.notes.iter().map(|n| div().text_size(px(11.)).text_color(theme.warning).child(n.clone())))
                    .when(self.running && !self.use_native, |d| {
                        d.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().flex_1().child(Progress::new("dump-progress").value(fraction)))
                                .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!(
                                    "{}{}/{} · {} rows · {}",
                                    live.table.as_deref().map(|t| format!("{t} · ")).unwrap_or_default(),
                                    live.tables_done,
                                    live.tables_total,
                                    live.rows,
                                    human(live.bytes)
                                ))),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("dump-go")
                            .primary()
                            .label("Export")
                            .disabled(self.running || (unsupported.is_some() && !self.use_native))
                            .on_click(cx.listener(|this, _, w, cx| this.start(w, cx))),
                    )
                    .child(Button::new("dump-close").label(if self.running { "Cancel Run" } else { "Close" }).on_click(cx.listener(|this, _, w, cx| {
                        if this.running {
                            this.escape(cx);
                        } else {
                            w.close_dialog(cx);
                        }
                    }))),
            )
            .when_some(unsupported.filter(|_| !self.use_native), |d, why| {
                d.child(div().text_size(px(11.)).text_color(theme.warning).child(format!("The built-in dump is off for this product: {why}")))
            })
    }
}
