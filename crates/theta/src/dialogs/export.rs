//! Export Data: stream a whole table to a file, in pages, with progress and Cancel.
//!
//! Unlike exporting a result grid this is not limited by the result row limit and never holds the table in
//! memory (`theta_import::export_table` pages through it). A cancelled or failed export leaves no file.

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
    App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_import::{ExportJob, ExportOutcome, ExportProgress, ExportReport, Paging, export_table_to_path};
use theta_sql::ExportFormat;

const FORMATS: [(&str, &str); 4] =
    [("CSV", "csv"), ("TSV", "tsv"), ("JSON", "json"), ("SQL INSERT statements", "sql")];

pub struct ExportView {
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    schema: String,
    table: String,
    path: Entity<InputState>,
    format: Entity<SelectState<Vec<String>>>,
    page: Entity<InputState>,
    running: bool,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<ExportProgress>>,
    status: Option<(bool, String)>,
    estimate: Option<u64>,
    keyed: bool,
    _subs: Vec<Subscription>,
}

impl Drop for ExportView {
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
    let view = cx.new(|cx| ExportView::new(ws, store, conn_id, schema, table, window, cx));
    let for_cancel = view.clone();
    let focus = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let for_cancel = for_cancel.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Export Data")
            .w(px(620.))
            .overlay_closable(false)
            .child(view.clone())
            // Enter belongs to the fields (it starts the export); it must not close the dialog.
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| for_cancel.update(cx, |v, cx| v.escape(cx)))
    });
    window.defer(cx, move |window, cx| {
        let input = focus.read(cx).path.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
}

fn extension_for(label: &str) -> &'static str {
    FORMATS.iter().find(|f| f.0 == label).map_or("csv", |f| f.1)
}

impl ExportView {
    fn new(
        ws: WeakEntity<Workspace>,
        store: Entity<ConnectionStore>,
        conn_id: String,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (estimate, keyed) = store
            .read(cx)
            .catalog(&conn_id)
            .and_then(|c| {
                c.find_table(Some(&schema), &table).map(|(_, t)| (t.row_estimate, !t.primary_key.is_empty()))
            })
            .unwrap_or((None, false));
        let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let default = dir.join(format!("{table}.csv")).display().to_string();
        let path = cx.new(|cx| {
            InputState::new(window, cx).default_value(default).placeholder("Where to write the file")
        });
        let format = cx.new(|cx| {
            SelectState::new(
                FORMATS.iter().map(|f| f.0.to_string()).collect::<Vec<_>>(),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let page = cx.new(|cx| InputState::new(window, cx).default_value("5000"));
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&format, window, |this, sel, _: &SelectEvent<Vec<String>>, window, cx| {
            // Keep the file name's extension in step with the format.
            let ext = sel.read(cx).selected_value().map_or("csv", |l| extension_for(l));
            let current = this.path.read(cx).value().to_string();
            if let Some((stem, old)) = current.rsplit_once('.')
                && FORMATS.iter().any(|f| f.1 == old)
            {
                this.path.update(cx, |s, cx| s.set_value(format!("{stem}.{ext}"), window, cx));
            }
            cx.notify();
        }));
        subs.push(cx.subscribe_in(&path, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.start(window, cx);
            }
        }));
        Self {
            ws,
            store,
            conn_id,
            schema,
            table,
            path,
            format,
            page,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(ExportProgress::default())),
            status: None,
            estimate,
            keyed,
            _subs: subs,
        }
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
        let page = self.page.read(cx).value().trim().parse::<usize>().unwrap_or(0);
        if page == 0 {
            self.status = Some((false, "Rows per page must be a positive number.".into()));
            cx.notify();
            return;
        }
        let Some(connection) = self.store.read(cx).connection(&self.conn_id) else { return };
        let (kind, key) = {
            let s = self.store.read(cx);
            let kind = s.config(&self.conn_id).map(|c| c.kind);
            let key = s
                .catalog(&self.conn_id)
                .and_then(|c| {
                    c.find_table(Some(&self.schema), &self.table).map(|(_, t)| t.primary_key.clone())
                })
                .unwrap_or_default();
            (kind, key)
        };
        let Some(kind) = kind else { return };
        let label = self.format.read(cx).selected_value().cloned().unwrap_or_default();
        let format = match extension_for(&label) {
            "tsv" => ExportFormat::Tsv,
            "json" => ExportFormat::Json,
            "sql" => ExportFormat::SqlInserts { table: String::new() },
            _ => ExportFormat::Csv,
        };
        let job = ExportJob {
            kind,
            schema: Some(self.schema.clone()).filter(|s| !s.is_empty()),
            table: self.table.clone(),
            key,
            columns: None,
            format,
            page_rows: page,
        };
        let Some(ws) = self.ws.upgrade() else { return };
        if !ws.update(cx, |w, cx| w.begin_bulk(&self.conn_id, window, cx)) {
            return;
        }
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.progress.lock().unwrap() =
            ExportProgress { rows_estimate: self.estimate, ..Default::default() };
        self.status = Some((true, "Exporting…".into()));
        cx.notify();
        let (cancel, progress, estimate) = (self.cancel.clone(), self.progress.clone(), self.estimate);
        let target = path.clone();
        let task = runtime::spawn(async move {
            let cb = move |p: &ExportProgress| *progress.lock().unwrap() = p.clone();
            export_table_to_path(connection.as_ref(), &target, &job, estimate, &cancel, &cb).await
        });
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
        cx.spawn_in(window, async move |this, cx| {
            let rep = task.await;
            this.update_in(cx, |this, _, cx| this.finish(path, rep.map_err(|e| e.to_string()), cx)).ok();
        })
        .detach();
    }

    fn finish(&mut self, path: PathBuf, rep: Result<ExportReport, String>, cx: &mut Context<Self>) {
        self.running = false;
        let conn = self.conn_id.clone();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.end_bulk(&conn, cx));
        }
        let (ok, text) = match &rep {
            Ok(r) => match &r.outcome {
                ExportOutcome::Completed => (
                    true,
                    format!(
                        "Exported {} rows ({}) to {} in {:.1}s.{}",
                        r.rows,
                        human(r.bytes),
                        path.display(),
                        r.elapsed.as_secs_f64(),
                        if r.paging == Paging::Offset {
                            " The table has no primary key, so it was read with OFFSET paging: rows changing during the export may repeat or be missed."
                        } else {
                            ""
                        }
                    ),
                ),
                ExportOutcome::Cancelled => (false, "Cancelled. No file was written.".to_string()),
                ExportOutcome::Failed(e) => (false, format!("Export failed, no file was written: {e}")),
            },
            Err(e) => (false, e.clone()),
        };
        if let Some(ws) = self.ws.upgrade() {
            let kind = if ok { MessageKind::Ok } else { MessageKind::Error };
            let title = format!("EXPORT {} TO {}", self.table, path.display());
            ws.update(cx, |w, cx| w.log_bulk(&conn, &title, kind, vec![text.clone()], cx));
        }
        self.status = Some((ok, text));
        cx.notify();
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

impl Render for ExportView {
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
        let fraction = live.fraction().map(|f| f * 100.).unwrap_or(0.);
        v_flex()
            .gap_3()
            .child(div().text_size(px(13.)).child(format!(
                "{}{} · {}",
                if self.schema.is_empty() { String::new() } else { format!("{}.", self.schema) },
                self.table,
                match self.estimate {
                    Some(n) => format!("about {} rows", crate::dialogs::import::fmt_count(n)),
                    None => "row count unknown".to_string(),
                }
            )))
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(caption("File"))
                            .child(Input::new(&self.path)),
                    )
                    .child(
                        Button::new("export-browse")
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(220.))
                            .child(caption("Format"))
                            .child(Select::new(&self.format)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(120.))
                            .child(caption("Rows per page"))
                            .child(Input::new(&self.page)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pt_5()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(if self.keyed {
                                "Paged by primary key: constant memory, stable while the table changes."
                            } else {
                                "No primary key: paged with OFFSET (slower on large tables)."
                            }),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_h(px(40.))
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
                                        Progress::new("export-progress")
                                            .value(fraction)
                                            .loading(live.rows_estimate.is_none()),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{} rows · {}", live.rows, human(live.bytes))),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("export-go")
                            .primary()
                            .label("Export")
                            .disabled(self.running)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    )
                    .child(
                        Button::new("export-close")
                            .label(if self.running { "Cancel Run" } else { "Close" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.running {
                                    this.escape(cx);
                                } else {
                                    window.close_dialog(cx);
                                }
                            })),
                    ),
            )
    }
}
