//! Run SQL Script: execute a `.sql` file statement by statement, streaming, with progress and Cancel.
//!
//! `theta_import::run_script` reads the file in chunks and never holds it in memory; this dialog picks the
//! file and the error policy, shows the statement and byte position, and lists what failed. It runs on the
//! connection's shared session, so a `SET search_path` or `USE` typed in a console before opening it applies
//! (that is how a dump is restored into a scratch schema).

use crate::connections::ConnectionStore;
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::text::plural;
use theta_core::{DatabaseKind, Feature};
use theta_import::script::{ScriptOptions, ScriptOutcome, ScriptProgress, ScriptReport, run_script};

pub struct ScriptView {
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    kind: DatabaseKind,
    path: Entity<InputState>,
    stop_on_error: bool,
    single_txn: bool,
    running: bool,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<ScriptProgress>>,
    status: Option<(bool, String)>,
    errors: Vec<String>,
    _subs: Vec<Subscription>,
}

impl Drop for ScriptView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn open(
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    initial: Option<PathBuf>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(kind) = store.read(cx).config(&conn_id).map(|c| c.kind) else { return };
    let view = cx.new(|cx| {
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial.map(|p| p.display().to_string()).unwrap_or_default())
                .placeholder("The .sql file to run")
        });
        // Enter in the file field runs the script, as it does in the Import, Export and Copy Table dialogs.
        let enter = cx.subscribe_in(&path, window, |this: &mut ScriptView, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.start(window, cx);
            }
        });
        ScriptView {
            ws,
            store,
            conn_id,
            kind,
            path,
            stop_on_error: true,
            single_txn: false,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(ScriptProgress::default())),
            status: None,
            errors: Vec::new(),
            _subs: vec![enter],
        }
    });
    let for_cancel = view.clone();
    let shown = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let for_cancel = for_cancel.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Run SQL Script")
            .w(px(640.))
            .overlay_closable(false)
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| for_cancel.update(cx, |v, cx| v.escape(cx)))
            .child(shown.clone())
    });
    window.defer(cx, move |window, cx| {
        let input = view.read(cx).path.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
}

impl ScriptView {
    fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            // The statement that is running should stop too, not just the loop that would send the next.
            if let Some(c) = self.store.read(cx).connection(&self.conn_id) {
                runtime::spawn(async move {
                    let _ = c.cancel().await;
                });
            }
            self.status = Some((false, "Cancelling…".into()));
            cx.notify();
            return false;
        }
        true
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Run SQL Script".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(p) = paths.into_iter().next() else { return };
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
            self.status = Some((false, "Choose the .sql file to run.".into()));
            cx.notify();
            return;
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                self.status = Some((false, theta_core::friendly::io_message(&e, &path, theta_core::friendly::IoOp::Read)));
                cx.notify();
                return;
            }
        };
        let total = file.metadata().map(|m| m.len()).ok();
        let Some(connection) = self.store.read(cx).connection(&self.conn_id) else {
            self.status = Some((false, "The connection is not open.".into()));
            cx.notify();
            return;
        };
        let Some(ws) = self.ws.upgrade() else { return };
        if !ws.update(cx, |w, cx| w.begin_bulk(&self.conn_id, window, cx)) {
            return;
        }
        let mut opts = ScriptOptions::new(self.kind);
        opts.stop_on_error = self.stop_on_error;
        opts.single_transaction = self.single_txn;
        self.running = true;
        self.errors.clear();
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.progress.lock().unwrap() = ScriptProgress { total_bytes: total, ..Default::default() };
        self.status = Some((true, "Running…".into()));
        cx.notify();
        let (cancel, progress) = (self.cancel.clone(), self.progress.clone());
        let task = runtime::spawn(async move {
            let cb = move |p: &ScriptProgress| *progress.lock().unwrap() = p.clone();
            run_script(connection.as_ref(), std::io::BufReader::new(file), total, &opts, &cancel, &cb).await
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

    fn finish(&mut self, path: PathBuf, rep: Result<ScriptReport, String>, cx: &mut Context<Self>) {
        self.running = false;
        let conn = self.conn_id.clone();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.end_bulk(&conn, cx));
        }
        let (ok, text) = match rep {
            Ok(r) => {
                self.errors = r
                    .errors
                    .iter()
                    .take(8)
                    .map(|e| format!("line {} · {} · {}", e.line, e.head, e.message))
                    .collect();
                let more = if r.error_count as usize > self.errors.len() {
                    format!(" ({} more not shown)", r.error_count as usize - self.errors.len())
                } else {
                    String::new()
                };
                let base = format!(
                    "{}, {} affected in {}, {}{more}.",
                    plural(r.statements, "statement", "statements"),
                    plural(r.rows_affected, "row", "rows"),
                    theta_core::text::duration(r.elapsed),
                    plural(r.error_count, "error", "errors")
                );
                match r.outcome {
                    ScriptOutcome::Completed if r.error_count == 0 => (true, format!("Done: {base}")),
                    ScriptOutcome::Completed => (false, format!("Finished with errors: {base}")),
                    ScriptOutcome::Stopped => (
                        false,
                        format!(
                            "Stopped at the first error: {base}{}",
                            if r.rolled_back { " Everything was rolled back." } else { "" }
                        ),
                    ),
                    ScriptOutcome::Cancelled => (
                        false,
                        format!(
                            "Cancelled: {base}{}",
                            if r.rolled_back { " Everything was rolled back." } else { "" }
                        ),
                    ),
                    ScriptOutcome::Failed(e) => (false, format!("The script failed: {e}. Before that: {base}")),
                }
            }
            Err(e) => (false, e),
        };
        if let Some(ws) = self.ws.upgrade() {
            let kind = if ok { MessageKind::Ok } else { MessageKind::Error };
            let title = format!("RUN SCRIPT {}", path.display());
            let mut lines = vec![text.clone()];
            lines.extend(self.errors.iter().cloned());
            ws.update(cx, |w, cx| w.log_bulk(&conn, &title, kind, lines, cx));
        }
        // The script probably changed the schema.
        self.store.update(cx, |s, cx| s.refresh_catalog(&self.conn_id, cx));
        self.status = Some((ok, text));
        cx.notify();
    }
}

impl Render for ScriptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let live = self.progress.lock().unwrap().clone();
        let txn_off = self.kind.unsupported(Feature::Transactions);
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
                    .child(v_flex().gap_1().flex_1().min_w_0().child(caption("File")).child(Input::new(&self.path)))
                    .child(Button::new("script-browse").label("Browse…").on_click(cx.listener(|this, _, w, cx| this.browse(w, cx)))),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Switch::new("script-stop")
                            .label("Stop at the first error (off: keep going and list the errors)")
                            .checked(self.stop_on_error || self.single_txn)
                            .disabled(self.single_txn)
                            .on_change(cx.listener(|this, v, _, cx| {
                                this.stop_on_error = *v;
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("script-txn")
                            .label("Run in one transaction (roll back if anything fails)")
                            .checked(self.single_txn)
                            .disabled(txn_off.is_some())
                            .on_change(cx.listener(|this, v, _, cx| {
                                this.single_txn = *v;
                                cx.notify();
                            })),
                    )
                    .when_some(txn_off, |d, why| {
                        d.child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!("Not available: {why}")))
                    })
                    .when(self.kind.family() == theta_core::Family::MySql, |d| {
                        d.child(div().text_size(px(11.)).text_color(theme.muted_foreground).child("MySQL and MariaDB commit each DDL statement on their own, so a transaction only covers the data statements."))
                    }),
            )
            .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child("The file is read in pieces, never loaded whole. COPY … FROM stdin (pg_dump's default format), DELIMITER blocks and /*! … */ executable comments are not supported; use pg_dump --inserts."))
            .child(
                v_flex()
                    .gap_1()
                    .min_h(px(60.))
                    .when_some(self.status.clone(), |d, (ok, t)| {
                        d.child(div().text_size(px(12.)).text_color(if ok { theme.foreground } else { theme.danger }).child(t))
                    })
                    .when(self.running, |d| {
                        d.child(
                            v_flex()
                                .gap_1()
                                .child(Progress::new("script-progress").value(live.fraction().map(|f| f * 100.).unwrap_or(0.)).loading(live.total_bytes.is_none()))
                                .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(format!(
                                    "statement {} · {} / {} read · {}",
                                    live.statements + 1,
                                    human(live.bytes_read),
                                    live.total_bytes.map(human).unwrap_or_else(|| "?".into()),
                                    plural(live.errors, "error", "errors")
                                )))
                                .child(div().text_size(px(11.)).truncate().child(live.current.clone())),
                        )
                    })
                    .child(div().id("script-errors").max_h(px(110.)).overflow_y_scroll().child(v_flex().gap_0p5().children(self.errors.iter().map(|e| div().text_size(px(11.)).text_color(theme.danger).child(e.clone()))))),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("script-go").primary().label("Run").disabled(self.running).on_click(cx.listener(|this, _, w, cx| this.start(w, cx))))
                    .child(Button::new("script-close").label(if self.running { "Cancel Run" } else { "Close" }).on_click(cx.listener(|this, _, w, cx| {
                        if this.running {
                            this.escape(cx);
                        } else {
                            w.close_dialog(cx);
                        }
                    }))),
            )
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
