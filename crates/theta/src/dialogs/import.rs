//! The Import Data dialog: pick a file, check what Theta made of it, map its columns, validate, import.
//!
//! The dialog is a thin view. Format guessing, scanning, type inference, mapping rules, job building and the
//! pipeline itself live in `theta-import` with their own tests; this file holds the controls, runs the work
//! off the UI thread, and shows progress. Every file-choosing step has a typed path field, so a keyboard
//! user (and the screenshot harness, which cannot drive a native file dialog) never needs Browse….

use crate::connections::ConnectionStore;
use crate::panels::messages::MessageKind;
use crate::runtime;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, px,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theta_core::Family;
use theta_core::{Catalog, ColumnInfo, DatabaseKind, TableKind};
use theta_grid::{GridMeta, ResultGrid};
use theta_import::plan::auto_map;
use theta_import::scan::{Scan, scan};
use theta_import::setup::{Compat, Destination, ImportSetup, NewColumnSpec, build_job, compatibility};
use theta_import::sniff::{SAMPLE_BYTES, sniff};
use theta_import::{
    ConvertOptions, CsvOptions, Encoding, ErrorMode, ImportReport, Outcome, Progress as RunProgress, Target,
    run_import, validate,
};
use theta_ui::{CHROME_TEXT_SIZE, data_surface};

/// What opens the dialog: the connection, optionally the table to import into and a file to pre-fill.
#[derive(Debug, Clone)]
pub struct Request {
    pub conn_id: String,
    /// `(schema, table)` of an existing table to import into.
    pub table: Option<(String, String)>,
    pub path: Option<String>,
    /// Start in "new table" mode.
    pub new_table: bool,
}

const DELIMITERS: [(&str, u8); 4] =
    [("Comma ,", b','), ("Semicolon ;", b';'), ("Tab", b'\t'), ("Pipe |", b'|')];
const QUOTES: [(&str, Option<u8>); 3] =
    [("Double quote \"", Some(b'"')), ("Single quote '", Some(b'\'')), ("None", None)];
const SKIP: &str = "(skip)";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Preview,
    Columns,
    Options,
    Report,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Existing,
    New,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Scanning,
    Validating,
    Importing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReportKind {
    Validate,
    Import,
}

/// The controls for one file column.
struct ColRow {
    /// New table: the column's name.
    name: Entity<InputState>,
    /// New table: the column's type, free text.
    ty: Entity<InputState>,
    /// Existing table: which table column receives it.
    dest: Entity<SelectState<Vec<String>>>,
    skip: bool,
}

pub struct ImportView {
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    conn_id: String,
    kind: DatabaseKind,
    read_only: bool,
    catalog: Option<Arc<Catalog>>,

    path: Entity<InputState>,
    delimiter: Entity<SelectState<Vec<String>>>,
    quote: Entity<SelectState<Vec<String>>>,
    encoding: Entity<SelectState<Vec<String>>>,
    has_header: bool,
    mode: Mode,
    table_pick: Entity<SelectState<SearchableVec<String>>>,
    new_name: Entity<InputState>,
    cols: Vec<ColRow>,

    empty_null: bool,
    null_token: Entity<InputState>,
    truncate: bool,
    skip_bad: bool,
    batch: Entity<InputState>,

    scan: Option<Scan>,
    scan_error: Option<String>,
    file_size: Option<u64>,
    detected_encoding: Option<Encoding>,
    scan_gen: u64,
    grid: Entity<ResultGrid>,
    tab: Tab,

    phase: Phase,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<RunProgress>>,
    report: Option<(ReportKind, ImportReport)>,
    status: Option<(bool, String)>,
    /// Truncate was requested and the user has been shown the statement; the next Import confirms it.
    armed: bool,
    last_table: Option<(String, String)>,
    _subs: Vec<Subscription>,
}

impl Drop for ImportView {
    fn drop(&mut self) {
        // Closing the dialog while it works must not leave an import running unseen.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn open(
    ws: WeakEntity<Workspace>,
    store: Entity<ConnectionStore>,
    request: Request,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| ImportView::new(ws, store, request, window, cx));
    let for_cancel = view.clone();
    let focus = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let for_cancel = for_cancel.clone();
        dialog
            .bg(cx.theme().popover)
            .title("Import Data")
            .w(px(940.))
            .margin_top(px(20.))
            .overlay_closable(false)
            .child(view.clone())
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| {
                // Escape while a run is in flight cancels the run; the dialog stays so the result is seen.
                for_cancel.update(cx, |v, cx| v.escape(cx))
            })
    });
    window.defer(cx, move |window, cx| {
        let input = focus.read(cx).path.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
}

fn select_of(
    items: Vec<String>,
    ix: Option<usize>,
    window: &mut Window,
    cx: &mut Context<ImportView>,
) -> Entity<SelectState<Vec<String>>> {
    cx.new(|cx| SelectState::new(items, ix.map(IndexPath::new), window, cx))
}

pub(crate) fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

impl ImportView {
    fn new(
        ws: WeakEntity<Workspace>,
        store: Entity<ConnectionStore>,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (kind, read_only) = {
            let s = store.read(cx);
            let c = s.config(&request.conn_id);
            (c.map_or(DatabaseKind::Postgres, |c| c.kind), c.is_some_and(|c| c.read_only))
        };
        let catalog = store.read(cx).catalog(&request.conn_id);
        let tables: Vec<String> = catalog
            .as_ref()
            .map(|c| {
                c.schemas
                    .iter()
                    .flat_map(|s| {
                        s.tables
                            .iter()
                            .filter(|t| t.kind == TableKind::Table)
                            .map(move |t| qualified(&s.name, &t.name))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let preselect = request
            .table
            .as_ref()
            .and_then(|(s, t)| tables.iter().position(|x| *x == qualified(s, t)))
            .map(IndexPath::new);
        let mode = if request.new_table || tables.is_empty() { Mode::New } else { Mode::Existing };

        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Path to a .csv, .tsv or .txt file (type or paste it, then press Enter)")
                .default_value(request.path.clone().unwrap_or_default())
        });
        let delimiter = select_of(DELIMITERS.iter().map(|d| d.0.to_string()).collect(), Some(0), window, cx);
        let quote = select_of(QUOTES.iter().map(|d| d.0.to_string()).collect(), Some(0), window, cx);
        let encoding =
            select_of(Encoding::ALL.iter().map(|e| e.label().to_string()).collect(), Some(0), window, cx);
        let table_pick =
            cx.new(|cx| SelectState::new(SearchableVec::new(tables), preselect, window, cx).searchable(true));
        let default_schema = catalog.as_ref().and_then(|c| c.default_schema.clone()).unwrap_or_default();
        let new_name = cx.new(|cx| {
            InputState::new(window, cx).placeholder("new_table  (or schema.new_table)").default_value(
                request.path.as_deref().map(theta_import::plan::table_name_from_file).unwrap_or_default(),
            )
        });
        let null_token = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. NULL or \\N"));
        let batch = cx.new(|cx| InputState::new(window, cx).default_value("1000"));
        let grid = cx.new(|cx| ResultGrid::new(window, cx));
        let _ = default_schema;

        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&path, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.load_file(window, cx);
            }
        }));
        for sel in [&delimiter, &quote, &encoding] {
            subs.push(cx.subscribe_in(sel, window, |this, _, _: &SelectEvent<Vec<String>>, window, cx| {
                this.rescan(window, cx)
            }));
        }
        subs.push(cx.subscribe_in(
            &table_pick,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, window, cx| {
                if matches!(ev, SelectEvent::Confirm(Some(_))) {
                    this.armed = false;
                    this.rebuild_rows(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(&new_name, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.armed = false;
                cx.notify();
            }
        }));

        let mut view = Self {
            ws,
            store,
            conn_id: request.conn_id,
            kind,
            read_only,
            catalog,
            path,
            delimiter,
            quote,
            encoding,
            has_header: true,
            mode,
            table_pick,
            new_name,
            cols: Vec::new(),
            empty_null: true,
            null_token,
            truncate: false,
            skip_bad: false,
            batch,
            scan: None,
            scan_error: None,
            file_size: None,
            detected_encoding: None,
            scan_gen: 0,
            grid,
            tab: Tab::Preview,
            phase: Phase::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(RunProgress::default())),
            report: None,
            status: None,
            armed: false,
            last_table: None,
            _subs: subs,
        };
        if view.read_only {
            view.status = Some((false, "This connection is read-only, so nothing can be imported.".into()));
        }
        if request.path.is_some() {
            view.load_file(window, cx);
        }
        view
    }

    fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        if matches!(self.phase, Phase::Validating | Phase::Importing) {
            self.cancel.store(true, Ordering::Relaxed);
            self.status = Some((false, "Cancelling…".into()));
            cx.notify();
            return false;
        }
        true
    }

    fn path_text(&self, cx: &App) -> String {
        self.path.read(cx).value().trim().to_string()
    }

    fn csv_options(&self, cx: &App) -> CsvOptions {
        let d = self.delimiter.read(cx).selected_value().cloned().unwrap_or_default();
        let q = self.quote.read(cx).selected_value().cloned().unwrap_or_default();
        let e = self.encoding.read(cx).selected_value().cloned().unwrap_or_default();
        CsvOptions {
            delimiter: DELIMITERS.iter().find(|x| x.0 == d).map_or(b',', |x| x.1),
            quote: QUOTES.iter().find(|x| x.0 == q).map_or(Some(b'"'), |x| x.1),
            encoding: Encoding::ALL.iter().copied().find(|x| x.label() == e).unwrap_or_default(),
            ..Default::default()
        }
    }

    fn convert_options(&self, cx: &App) -> ConvertOptions {
        let token = self.null_token.read(cx).value().to_string();
        ConvertOptions {
            empty_as_null: self.empty_null,
            null_token: (!token.is_empty()).then_some(token),
            trim_text: false,
        }
    }

    // ---------------------------------------------------------------- file

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Data".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(p) = paths.into_iter().next() else { return };
            this.update_in(cx, |this, window, cx| {
                this.path.update(cx, |s, cx| s.set_value(p.display().to_string(), window, cx));
                this.load_file(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// A new file: guess its format, put the guesses into the controls, then scan with them.
    fn load_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path_text(cx);
        if path.is_empty() {
            return;
        }
        self.scan_gen += 1;
        let generation = self.scan_gen;
        self.phase = Phase::Scanning;
        self.report = None;
        self.armed = false;
        self.status = None;
        cx.notify();
        let task = cx.background_spawn(async move {
            use std::io::Read;
            let mut f = std::fs::File::open(&path).map_err(|e| format!("Cannot open {path}: {e}"))?;
            let size = f.metadata().map(|m| m.len()).ok();
            let mut head = Vec::new();
            f.by_ref()
                .take(SAMPLE_BYTES as u64 + 1)
                .read_to_end(&mut head)
                .map_err(|e| format!("Cannot read {path}: {e}"))?;
            Ok::<_, String>((sniff(&head), size))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.scan_gen != generation {
                    return;
                }
                match result {
                    Ok((s, size)) => {
                        this.file_size = size;
                        this.has_header = s.has_header;
                        let di = DELIMITERS.iter().position(|d| d.1 == s.delimiter).unwrap_or(0);
                        let qi = QUOTES.iter().position(|q| q.1 == s.quote).unwrap_or(0);
                        let ei = Encoding::ALL.iter().position(|e| *e == Encoding::Auto).unwrap_or(0);
                        this.delimiter
                            .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(di)), window, cx));
                        this.quote
                            .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(qi)), window, cx));
                        this.encoding
                            .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(ei)), window, cx));
                        this.detected_encoding = Some(s.encoding);
                        let name = theta_import::plan::table_name_from_file(&this.path_text(cx));
                        this.new_name.update(cx, |s, cx| {
                            if s.value().is_empty() {
                                s.set_value(name, window, cx)
                            }
                        });
                        this.rescan(window, cx);
                    }
                    Err(e) => {
                        this.phase = Phase::Idle;
                        this.scan = None;
                        this.scan_error = Some(e);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Re-read the sample with the controls as they are now.
    fn rescan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path_text(cx);
        if path.is_empty() {
            return;
        }
        self.scan_gen += 1;
        let generation = self.scan_gen;
        let (csv, conv, header) = (self.csv_options(cx), self.convert_options(cx), self.has_header);
        self.phase = Phase::Scanning;
        cx.notify();
        let task = cx.background_spawn(async move {
            let f = std::fs::File::open(&path).map_err(|e| format!("Cannot open {path}: {e}"))?;
            scan(f, &csv, header, &conv, theta_import::scan::SAMPLE_ROWS).map_err(|e| e.to_string())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.scan_gen != generation {
                    return;
                }
                this.phase = Phase::Idle;
                match result {
                    Ok(s) => {
                        this.scan_error = None;
                        let preview = Arc::new(s.preview.clone());
                        this.grid.update(cx, |g, cx| {
                            g.set_result(
                                preview,
                                GridMeta::new().with_read_only_reason("Preview"),
                                window,
                                cx,
                            )
                        });
                        this.scan = Some(s);
                        this.rebuild_rows(window, cx);
                    }
                    Err(e) => {
                        this.scan = None;
                        this.scan_error = Some(e);
                        this.cols.clear();
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------- mapping

    fn existing_table(&self, cx: &App) -> Option<(String, String, Vec<ColumnInfo>)> {
        let label = self.table_pick.read(cx).selected_value()?.clone();
        let (s, t) = split_qualified(&label);
        let catalog = self.catalog.as_ref()?;
        let (schema, table) = catalog.find_table(Some(&s), &t)?;
        Some((schema.name.clone(), table.name.clone(), table.columns.clone()))
    }

    fn rebuild_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cols.clear();
        let Some(sc) = self.scan.clone() else {
            cx.notify();
            return;
        };
        let suggested = sc.suggested_column_names();
        let existing = self.existing_table(cx);
        let mapping = existing.as_ref().map(|(_, _, cols)| auto_map(&sc.names, cols));
        let dest_items: Vec<String> = std::iter::once(SKIP.to_string())
            .chain(existing.iter().flat_map(|(_, _, c)| c.iter().map(|c| c.name.clone())))
            .collect();
        for i in 0..sc.width {
            let name = suggested.get(i).cloned().unwrap_or_default();
            let ty = sc.types.get(i).map(|t| t.sql_type(self.kind)).unwrap_or_else(|| "text".into());
            let sel = mapping.as_ref().and_then(|m| m[i]).map_or(0, |d| d + 1);
            self.cols.push(ColRow {
                name: cx.new(|cx| InputState::new(window, cx).default_value(name)),
                ty: cx.new(|cx| InputState::new(window, cx).default_value(ty)),
                dest: select_of(dest_items.clone(), Some(sel), window, cx),
                skip: false,
            });
        }
        cx.notify();
    }

    /// `(schema, table)` of the new table; a bare name goes into the default schema.
    fn new_table_parts(&self, cx: &App) -> (Option<String>, String) {
        let raw = self.new_name.read(cx).value().trim().to_string();
        match raw.split_once('.') {
            Some((s, t)) => (Some(s.to_string()), t.to_string()),
            None => (
                self.catalog
                    .as_ref()
                    .and_then(|c| c.default_schema.clone())
                    .filter(|_| self.kind.family() != Family::Sqlite),
                raw,
            ),
        }
    }

    fn setup(&self, plan: theta_import::TxnPlan, cx: &App) -> Result<ImportSetup, Vec<String>> {
        let scan = self.scan.as_ref().ok_or_else(|| vec!["Choose a file first.".to_string()])?;
        let dest = match self.mode {
            Mode::Existing => {
                let (schema, table, columns) = self
                    .existing_table(cx)
                    .ok_or_else(|| vec!["Choose the table to import into.".to_string()])?;
                let mapping = self
                    .cols
                    .iter()
                    .map(|r| {
                        let sel = r.dest.read(cx).selected_value().cloned().unwrap_or_default();
                        columns.iter().position(|c| c.name == sel)
                    })
                    .collect();
                Destination::Existing {
                    schema: Some(schema).filter(|s| !s.is_empty()),
                    table,
                    columns,
                    mapping,
                }
            }
            Mode::New => {
                let (schema, table) = self.new_table_parts(cx);
                Destination::New {
                    schema,
                    table,
                    columns: self
                        .cols
                        .iter()
                        .map(|r| NewColumnSpec {
                            name: r.name.read(cx).value().to_string(),
                            sql_type: r.ty.read(cx).value().to_string(),
                            nullable: true,
                            skip: r.skip,
                        })
                        .collect(),
                }
            }
        };
        let _ = scan;
        let batch = self.batch.read(cx).value().trim().parse::<usize>().unwrap_or(0);
        Ok(ImportSetup {
            kind: self.kind,
            csv: self.csv_options(cx),
            has_header: self.has_header,
            convert: self.convert_options(cx),
            on_error: if self.skip_bad { ErrorMode::Skip } else { ErrorMode::Abort },
            batch_rows: batch,
            max_statement_bytes: theta_import::DEFAULT_STATEMENT_BYTES,
            truncate_first: self.truncate,
            txn: plan,
            dest,
        })
    }

    // ----------------------------------------------------------------- runs

    fn start(&mut self, importing: bool, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Validating | Phase::Importing) {
            return;
        }
        if importing && self.read_only {
            return;
        }
        let Some(ws) = self.ws.upgrade() else { return };
        let plan = ws.read(cx).bulk_txn_plan(&self.conn_id, cx);
        let Some(plan) = plan else { return };
        let setup = match self.setup(plan.clone(), cx) {
            Ok(s) => s,
            Err(problems) => {
                self.status = Some((false, problems.join(" ")));
                cx.notify();
                return;
            }
        };
        let job = match build_job(&setup) {
            Ok(j) => j,
            Err(problems) => {
                self.status = Some((false, problems.join(" ")));
                self.tab = Tab::Columns;
                cx.notify();
                return;
            }
        };
        let path = self.path_text(cx);
        let size = self.file_size;
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.progress.lock().unwrap() = RunProgress { bytes_total: size, ..Default::default() };
        self.report = None;

        if !importing {
            self.phase = Phase::Validating;
            self.status = Some((true, "Validating the whole file; nothing is written…".into()));
            self.tab = Tab::Report;
            cx.notify();
            let (cancel, progress) = (self.cancel.clone(), self.progress.clone());
            let task = cx.background_spawn(async move {
                let f = std::fs::File::open(&path).map_err(|e| format!("Cannot open {path}: {e}"))?;
                let cb = move |p: &RunProgress| *progress.lock().unwrap() = p.clone();
                Ok::<_, String>(validate(f, &job, size, &cancel, &cb))
            });
            self.watch(window, cx);
            cx.spawn_in(window, async move |this, cx| {
                let r = task.await;
                this.update_in(cx, |this, _, cx| this.finish(ReportKind::Validate, r, cx)).ok();
            })
            .detach();
            return;
        }

        // Import. Emptying a table is the one destructive part: show the exact statement and make the
        // user press Import a second time.
        if let Some(sql) = &job.truncate_sql
            && !self.armed
        {
            self.armed = true;
            self.status = Some((
                false,
                format!(
                    "This first empties the table: {sql}. Press Import again to confirm, or change any option to back out."
                ),
            ));
            cx.notify();
            return;
        }
        if matches!(self.mode, Mode::New)
            && self.kind.family() == Family::MySql
            && matches!(&plan, theta_import::TxnPlan::Session { open: true, .. })
        {
            self.status = Some((
                false,
                "MySQL commits the open transaction when a table is created. Commit or roll back first, or import into an existing table.".into(),
            ));
            cx.notify();
            return;
        }
        let Some(connection) = self.store.read(cx).connection(&self.conn_id) else {
            self.status = Some((false, "The connection is not available.".into()));
            cx.notify();
            return;
        };
        let began = ws.update(cx, |w, cx| w.begin_bulk(&self.conn_id, window, cx));
        if !began {
            return;
        }
        self.armed = false;
        self.phase = Phase::Importing;
        self.status = Some((true, "Importing…".into()));
        self.tab = Tab::Report;
        let target = (job.schema.clone().unwrap_or_default(), job.table.clone());
        self.last_table = Some(target);
        cx.notify();
        let (cancel, progress) = (self.cancel.clone(), self.progress.clone());
        let task = runtime::spawn(async move {
            let f = std::fs::File::open(&path).map_err(|e| format!("Cannot open {path}: {e}"))?;
            let cb = move |p: &RunProgress| *progress.lock().unwrap() = p.clone();
            Ok::<_, String>(run_import(connection.as_ref(), f, &job, size, &cancel, &cb).await)
        });
        self.watch(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let r = match task.await {
                Ok(r) => r,
                Err(e) => Err(e.to_string()),
            };
            this.update_in(cx, |this, _, cx| this.finish(ReportKind::Import, r, cx)).ok();
        })
        .detach();
    }

    /// Repaint every 100 ms while a run is in flight so the counters and the bar move.
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let alive = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.phase, Phase::Validating | Phase::Importing)
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish(&mut self, kind: ReportKind, result: Result<ImportReport, String>, cx: &mut Context<Self>) {
        self.phase = Phase::Idle;
        let importing = kind == ReportKind::Import;
        let rep = match result {
            Ok(r) => r,
            Err(e) => {
                self.status = Some((false, e.clone()));
                if importing {
                    self.end_bulk(cx);
                }
                cx.notify();
                return;
            }
        };
        let rate = if rep.elapsed.as_secs_f64() > 0.0 {
            rep.rows_read as f64 / rep.elapsed.as_secs_f64()
        } else {
            0.0
        };
        let (ok, text) = match (&rep.outcome, kind) {
            (Outcome::Completed, ReportKind::Validate) => (
                rep.issue_count == 0,
                if rep.issue_count == 0 {
                    format!("Valid: all {} rows convert cleanly.", fmt_count(rep.rows_read))
                } else {
                    format!(
                        "{} of {} rows have problems (see the report). Nothing was written.",
                        fmt_count(rep.issue_count),
                        fmt_count(rep.rows_read)
                    )
                },
            ),
            (Outcome::Completed, ReportKind::Import) => (
                true,
                format!(
                    "Imported {} rows in {:.1}s ({} rows/s){}{}.",
                    fmt_count(rep.rows_inserted),
                    rep.elapsed.as_secs_f64(),
                    fmt_count(rate as u64),
                    if rep.rows_skipped > 0 {
                        format!(", skipped {}", fmt_count(rep.rows_skipped))
                    } else {
                        String::new()
                    },
                    if rep.committed {
                        ""
                    } else {
                        ". The transaction is still open: commit or roll back from the title bar"
                    },
                ),
            ),
            (Outcome::Cancelled, _) => {
                (false, "Cancelled. Nothing was written (the transaction was rolled back).".into())
            }
            (Outcome::Failed(e), ReportKind::Import) => {
                (false, format!("Import failed and was rolled back: {e}"))
            }
            (Outcome::Failed(e), ReportKind::Validate) => {
                (false, format!("Could not read the whole file: {e}"))
            }
        };
        self.status = Some((ok, text.clone()));
        if importing {
            let msg_kind =
                if matches!(rep.outcome, Outcome::Completed) { MessageKind::Ok } else { MessageKind::Error };
            let mut lines = vec![text];
            lines.extend(rep.issues.iter().take(20).map(|i| i.to_string()));
            if rep.issue_count > 20 {
                lines.push(format!("… and {} more", rep.issue_count - 20));
            }
            let title = format!(
                "IMPORT {} INTO {}",
                self.path_text(cx),
                self.last_table.as_ref().map(|(s, t)| qualified(s, t)).unwrap_or_default()
            );
            let conn = self.conn_id.clone();
            let changed = rep.created_table || rep.rows_inserted > 0;
            if let Some(ws) = self.ws.upgrade() {
                ws.update(cx, |w, cx| w.log_bulk(&conn, &title, msg_kind, lines, cx));
            }
            if changed {
                self.store.update(cx, |s, cx| s.refresh_catalog(&conn, cx));
            }
            self.end_bulk(cx);
        }
        self.report = Some((kind, rep));
        cx.notify();
    }

    fn end_bulk(&mut self, cx: &mut Context<Self>) {
        let conn = self.conn_id.clone();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.end_bulk(&conn, cx));
        }
    }

    fn open_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((schema, table)) = self.last_table.clone() else { return };
        let conn = self.conn_id.clone();
        window.close_dialog(cx);
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| w.open_table(conn, schema, table, window, cx));
        }
    }
}

fn qualified(schema: &str, table: &str) -> String {
    if schema.is_empty() { table.to_string() } else { format!("{schema}.{table}") }
}

fn split_qualified(label: &str) -> (String, String) {
    match label.split_once('.') {
        Some((s, t)) => (s.to_string(), t.to_string()),
        None => (String::new(), label.to_string()),
    }
}

// -------------------------------------------------------------------- render

fn caption(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn labelled(label: &'static str, control: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex().gap_1().child(caption(label, cx)).child(control)
}

impl ImportView {
    fn tab_button(&self, tab: Tab, label: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let active = self.tab == tab;
        let theme = cx.theme().clone();
        div()
            .id(label)
            .px_3()
            .h(px(26.))
            .flex()
            .items_center()
            .text_size(CHROME_TEXT_SIZE)
            .cursor_pointer()
            .border_b_2()
            .when(active, |d| {
                d.border_color(theme.primary).text_color(theme.foreground).font_weight(FontWeight::MEDIUM)
            })
            .when(!active, |d| {
                d.border_color(gpui_kit::transparent_black())
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.list_hover))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
            .into_any_element()
    }

    fn summary_line(&self, cx: &App) -> String {
        let Some(sc) = &self.scan else { return String::new() };
        let size = self.file_size.map(|b| format!(" · {}", human_bytes(b))).unwrap_or_default();
        format!(
            "{} columns · {} sampled rows{}{}{}",
            sc.width,
            fmt_count(sc.sampled as u64),
            size,
            if sc.ragged > 0 {
                format!(" · {} ragged rows in the sample", sc.ragged)
            } else {
                String::new()
            },
            if sc.unterminated_quote {
                " · a quoted field is never closed (wrong quote setting?)"
            } else {
                ""
            },
        ) + &{
            let _ = cx;
            String::new()
        }
    }

    fn render_columns(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(sc) = &self.scan else {
            return empty_note("Choose a file to see its columns.", cx);
        };
        let existing = self.existing_table(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        let head = |t: &'static str, w: Option<f32>| {
            let d = div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t);
            match w {
                Some(w) => d.w(px(w)).flex_shrink_0(),
                None => d.flex_1().min_w_0(),
            }
        };
        rows.push(
            h_flex()
                .gap_2()
                .px_2()
                .h(px(24.))
                .items_center()
                .child(head("#", Some(24.)))
                .child(head("File column", Some(130.)))
                .child(head("Sample", Some(130.)))
                .child(head(
                    if self.mode == Mode::New { "New column name" } else { "Table column" },
                    Some(190.),
                ))
                .child(head(if self.mode == Mode::New { "Type" } else { "Target type" }, Some(150.)))
                .child(head("Check", None))
                .child(head(if self.mode == Mode::New { "Include" } else { "" }, Some(44.)))
                .into_any_element(),
        );
        for (i, row) in self.cols.iter().enumerate() {
            let inferred = sc.types.get(i).cloned();
            let name = sc.names.get(i).cloned().unwrap_or_default();
            // The first value that is not NULL among the first rows, single-lined.
            let sample = sc
                .preview
                .rows
                .iter()
                .take(5)
                .filter_map(|r| r.get(i))
                .find(|v| !v.is_null())
                .map(|v| v.display().replace(['\n', '\r', '\t'], " "))
                .unwrap_or_default();
            let (control, ty_cell, flag): (AnyElement, AnyElement, (Option<bool>, String)) = match self.mode {
                Mode::New => (
                    Input::new(&row.name).small().disabled(row.skip).into_any_element(),
                    Input::new(&row.ty).small().disabled(row.skip).into_any_element(),
                    (None, inferred.map(|t| format!("looks like {}", t.label())).unwrap_or_default()),
                ),
                Mode::Existing => {
                    let sel = row.dest.read(cx).selected_value().cloned().unwrap_or_default();
                    let col = existing.as_ref().and_then(|(_, _, cols)| cols.iter().find(|c| c.name == sel));
                    let ty = col.map(|c| c.data_type.clone()).unwrap_or_else(|| "—".into());
                    let flag = match (col, &inferred) {
                        (Some(c), Some(t)) => match compatibility(t, &Target::from_column(c, self.kind)) {
                            Compat::Ok => (None, String::new()),
                            Compat::Check(m) => (Some(false), m),
                            Compat::Mismatch(m) => (Some(true), m),
                        },
                        _ => (None, String::new()),
                    };
                    (
                        Select::new(&row.dest).small().into_any_element(),
                        div()
                            .text_size(CHROME_TEXT_SIZE)
                            .text_color(theme.muted_foreground)
                            .child(ty)
                            .into_any_element(),
                        flag,
                    )
                }
            };
            let flag_color = match flag.0 {
                Some(true) => theme.danger,
                Some(false) => theme.warning,
                None => theme.muted_foreground,
            };
            let skip_switch = (self.mode == Mode::New).then(|| {
                Switch::new(("skip", i))
                    .checked(!row.skip)
                    .tooltip(if row.skip { "Skipped: click to include" } else { "Included: click to skip" })
                    .on_change(cx.listener(move |this, v: &bool, _, cx| {
                        if let Some(r) = this.cols.get_mut(i) {
                            r.skip = !*v;
                        }
                        cx.notify();
                    }))
            });
            rows.push(
                h_flex()
                    .gap_2()
                    .px_2()
                    .h(px(28.))
                    .items_center()
                    .child(
                        div()
                            .w(px(24.))
                            .flex_shrink_0()
                            .text_size(CHROME_TEXT_SIZE)
                            .text_color(theme.muted_foreground)
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .w(px(130.))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(CHROME_TEXT_SIZE)
                            .child(if name.is_empty() { format!("column {}", i + 1) } else { name }),
                    )
                    .child(
                        div()
                            .w(px(130.))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(CHROME_TEXT_SIZE)
                            .text_color(theme.muted_foreground)
                            .child(sample),
                    )
                    .child(div().w(px(190.)).flex_shrink_0().child(control))
                    .child(div().w(px(150.)).flex_shrink_0().child(ty_cell))
                    .child(
                        h_flex().flex_1().min_w_0().gap_2().items_center().child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(11.))
                                .text_color(flag_color)
                                .child(flag.1),
                        ),
                    )
                    .child(div().w(px(44.)).flex_shrink_0().children(skip_switch))
                    .into_any_element(),
            );
        }
        if self.mode == Mode::New {
            let preview = self
                .setup(theta_import::TxnPlan::Own { begin: Vec::new() }, cx)
                .and_then(|s| build_job(&s))
                .map(|j| j.create_sql.unwrap_or_default())
                .unwrap_or_else(|p| p.join(" "));
            let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
            rows.push(
                div()
                    .m_2()
                    .p_2()
                    .rounded_md()
                    .bg(theme.muted)
                    .font_family(mono)
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("{preview};")))
                    .into_any_element(),
            );
        }
        v_flex().id("import-columns").size_full().overflow_y_scroll().children(rows).into_any_element()
    }

    fn render_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let row = |label: &'static str, hint: &'static str, control: AnyElement| {
            h_flex()
                .gap_3()
                .items_center()
                .justify_between()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_size(px(13.)).child(label))
                        .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(hint)),
                )
                .child(div().flex_shrink_0().child(control))
        };
        let sw = |id: &'static str, on: bool, f: fn(&mut ImportView, bool)| {
            Switch::new(id)
                .checked(on)
                .on_change(cx.listener(move |this, v: &bool, window, cx| {
                    f(this, *v);
                    this.armed = false;
                    if id == "o-empty" {
                        this.rescan(window, cx);
                    }
                    cx.notify();
                }))
                .into_any_element()
        };
        v_flex()
            .id("import-options")
            .size_full()
            .overflow_y_scroll()
            .gap_3()
            .px_2()
            .py_1()
            .child(row("Empty fields are NULL", "An unquoted empty field is NULL; a quoted \"\" stays an empty string", sw("o-empty", self.empty_null, |t, v| t.empty_null = v)).into_any_element())
            .child(row("NULL token", "Text that also means NULL (unquoted), such as NULL or \\N", div().w(px(200.)).child(Input::new(&self.null_token).small()).into_any_element()).into_any_element())
            .child(row("Skip bad rows instead of stopping", "Off: the first bad row aborts and rolls everything back. On: bad rows are listed and the rest imported", sw("o-skip", self.skip_bad, |t, v| t.skip_bad = v)).into_any_element())
            .child(row("Empty the table first", "Existing table only. Shows the exact statement and asks again before it runs", sw("o-trunc", self.truncate, |t, v| t.truncate = v)).into_any_element())
            .child(row("Rows per INSERT", "Statements are also kept under 1 MB (and under half of MySQL's max_allowed_packet)", div().w(px(100.)).child(Input::new(&self.batch).small()).into_any_element()).into_any_element())
            .into_any_element()
    }

    fn render_report(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        let live = self.progress.lock().unwrap().clone();
        let Some((kind, rep)) = &self.report else {
            let running = matches!(self.phase, Phase::Validating | Phase::Importing);
            if running {
                let rate = live.rows_per_second();
                return v_flex()
                    .gap_2()
                    .p_3()
                    .child(div().text_size(px(13.)).child(format!(
                        "{} rows read · {} inserted · {} skipped · {} rows/s",
                        fmt_count(live.rows_read),
                        fmt_count(live.rows_inserted),
                        fmt_count(live.rows_skipped),
                        fmt_count(rate as u64)
                    )))
                    .into_any_element();
            }
            return empty_note("Validate checks the whole file without writing. Import writes it.", cx);
        };
        let mut lines: Vec<AnyElement> = Vec::new();
        let stat = |l: &str, v: String| {
            h_flex()
                .gap_2()
                .child(div().w(px(140.)).text_color(theme.muted_foreground).child(l.to_string()))
                .child(div().child(v))
        };
        let what = if *kind == ReportKind::Validate { "Rows that would import" } else { "Rows inserted" };
        lines.push(stat("Rows read", fmt_count(rep.rows_read)).into_any_element());
        lines.push(stat(what, fmt_count(rep.rows_inserted)).into_any_element());
        lines.push(stat("Rows with problems", fmt_count(rep.issue_count)).into_any_element());
        lines.push(stat("Time", format!("{:.2}s", rep.elapsed.as_secs_f64())).into_any_element());
        if rep.encoding_warning {
            lines.push(div().text_color(theme.warning).child("Some bytes were not valid in the chosen encoding and were replaced; check the Encoding option.").into_any_element());
        }
        if !rep.issues.is_empty() {
            lines.push(
                div()
                    .mt_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child(format!("Problems (first {} of {})", rep.issues.len(), fmt_count(rep.issue_count)))
                    .into_any_element(),
            );
            for issue in rep.issues.iter().take(300) {
                lines.push(
                    div()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .text_color(theme.danger)
                        .child(issue.to_string())
                        .into_any_element(),
                );
            }
        }
        v_flex()
            .id("import-report")
            .size_full()
            .overflow_y_scroll()
            .gap_1()
            .p_2()
            .text_size(px(13.))
            .children(lines)
            .into_any_element()
    }
}

fn empty_note(text: &'static str, cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn human_bytes(b: u64) -> String {
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

impl Render for ImportView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let busy = matches!(self.phase, Phase::Validating | Phase::Importing);
        let body_h = (f32::from(window.viewport_size().height) - 400.).clamp(200., 430.);
        let live = self.progress.lock().unwrap().clone();
        let body = match self.tab {
            Tab::Preview => {
                if self.scan.is_some() {
                    div().size_full().bg(data_surface(cx)).child(self.grid.clone()).into_any_element()
                } else {
                    empty_note(
                        if self.phase == Phase::Scanning {
                            "Reading the file…"
                        } else {
                            "Enter a file path above and press Enter."
                        },
                        cx,
                    )
                }
            }
            Tab::Columns => self.render_columns(cx),
            Tab::Options => self.render_options(cx),
            Tab::Report => self.render_report(cx),
        };
        let mode_btn = |label: &'static str, mode: Mode, cx: &mut Context<Self>| {
            Button::new(label).small().label(label).when(self.mode == mode, |b| b.primary()).on_click(
                cx.listener(move |this, _, window, cx| {
                    this.mode = mode;
                    this.armed = false;
                    this.rebuild_rows(window, cx);
                }),
            )
        };
        let target_control: AnyElement = match self.mode {
            Mode::Existing => Select::new(&self.table_pick)
                .placeholder("Choose a table")
                .search_placeholder("Filter tables")
                .into_any_element(),
            Mode::New => Input::new(&self.new_name).into_any_element(),
        };
        let can_import = !busy && !self.read_only && self.scan.is_some();
        let fraction = live.fraction().map(|f| f * 100.).unwrap_or(0.);
        let can_open = !busy
            && self.last_table.is_some()
            && matches!(&self.report, Some((ReportKind::Import, r)) if matches!(r.outcome, Outcome::Completed) && r.rows_inserted > 0);
        let import_label = if self.armed { "Confirm Import" } else { "Import" };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(div().flex_1().min_w_0().child(labelled("File", Input::new(&self.path), cx)))
                    .child(
                        Button::new("browse")
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(div().w(px(150.)).child(labelled("Delimiter", Select::new(&self.delimiter), cx)))
                    .child(div().w(px(150.)).child(labelled("Quote", Select::new(&self.quote), cx)))
                    .child(div().w(px(150.)).child(labelled("Encoding", Select::new(&self.encoding), cx)))
                    .child(div().h(px(32.)).flex().items_center().child(
                        Switch::new("header").label("Header row").checked(self.has_header).on_change(
                            cx.listener(|this, v: &bool, window, cx| {
                                this.has_header = *v;
                                this.rescan(window, cx);
                            }),
                        ),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(self.summary_line(cx)),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex().gap_1().child(caption("Import into", cx)).child(
                            h_flex()
                                .gap_1()
                                .child(mode_btn("Existing table", Mode::Existing, cx))
                                .child(mode_btn("New table", Mode::New, cx)),
                        ),
                    )
                    .child(div().flex_1().min_w_0().child(labelled(
                        match self.mode {
                            Mode::Existing => "Table",
                            Mode::New => "New table name",
                        },
                        target_control,
                        cx,
                    ))),
            )
            .child(
                h_flex()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.tab_button(Tab::Preview, "Preview", cx))
                    .child(self.tab_button(Tab::Columns, "Columns", cx))
                    .child(self.tab_button(Tab::Options, "Options", cx))
                    .child(self.tab_button(Tab::Report, "Report", cx)),
            )
            .child(
                div()
                    .h(px(body_h))
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(body),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_h(px(38.))
                    .when_some(self.scan_error.clone(), |d, e| {
                        d.child(div().text_size(px(12.)).text_color(theme.danger).child(e))
                    })
                    .when_some(self.status.clone(), |d, (ok, text)| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(if ok { theme.foreground } else { theme.danger })
                                .child(text),
                        )
                    })
                    .when(busy, |d| {
                        d.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div().flex_1().child(
                                        Progress::new("import-progress")
                                            .value(fraction)
                                            .loading(live.bytes_total.is_none()),
                                    ),
                                )
                                .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(
                                    format!(
                                        "{} read · {} inserted · {} skipped · {} rows/s",
                                        fmt_count(live.rows_read),
                                        fmt_count(live.rows_inserted),
                                        fmt_count(live.rows_skipped),
                                        fmt_count(live.rows_per_second() as u64)
                                    ),
                                )),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .when(can_open, |d| {
                        d.child(
                            Button::new("open-table")
                                .label("Open Table")
                                .on_click(cx.listener(|this, _, window, cx| this.open_table(window, cx))),
                        )
                    })
                    .child(
                        Button::new("validate")
                            .label("Validate")
                            .disabled(busy || self.scan.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.start(false, window, cx))),
                    )
                    .child(
                        Button::new("import")
                            .primary()
                            .label(import_label)
                            .disabled(!can_import)
                            .on_click(cx.listener(|this, _, window, cx| this.start(true, window, cx))),
                    )
                    .child(
                        Button::new("import-close")
                            .label(if busy { "Cancel Run" } else { "Close" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if busy_now(this.phase) {
                                    this.escape(cx);
                                } else {
                                    window.close_dialog(cx);
                                }
                            })),
                    ),
            )
    }
}

fn busy_now(p: Phase) -> bool {
    matches!(p, Phase::Validating | Phase::Importing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::secrets::SecretStore;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use theta_core::{ConnectionConfig, Value};

    fn wait_for(
        vcx: &mut VisualTestContext,
        what: &str,
        mut done: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        vcx.executor().allow_parking();
        for _ in 0..800 {
            vcx.run_until_parked();
            if done(vcx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    struct Fx {
        ws: Entity<Workspace>,
        store: Entity<ConnectionStore>,
        conn: String,
        dir: tempfile::TempDir,
    }

    fn setup(cx: &mut TestAppContext) -> (Fx, &mut VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.executor().allow_parking();
        cx.update(|cx| {
            theta_ui::init(cx);
            crate::keymap::bind(cx);
        });
        let (store, _) = ConnectionStore::load(None, Arc::new(SecretStore::memory_only()));
        let store = cx.new(|_| store);
        let mut config = ConnectionConfig::new("t", "t", DatabaseKind::Sqlite);
        config.path = ":memory:".into();
        let conn = store.update(cx, |s, cx| {
            let id = s.add_temporary(config, cx);
            s.connect(&id, cx);
            id
        });
        let (ws, vcx) = cx.add_window_view(|window, cx| {
            let mut ws = Workspace::with_data_dir(store.clone(), dir.path().to_path_buf(), window, cx);
            ws.quiet = true;
            ws
        });
        wait_for(vcx, "the connection", |vcx| store.read_with(vcx, |s, _| s.state(&conn).is_connected()));
        (Fx { ws, store, conn, dir }, vcx)
    }

    fn count(fx: &Fx, vcx: &mut VisualTestContext, table: &str) -> Option<i64> {
        let conn = fx.store.read_with(vcx, |s, _| s.connection(&fx.conn))?;
        let sql = format!("SELECT count(*) FROM {table}");
        let slot = Arc::new(Mutex::new(None));
        let writer = slot.clone();
        runtime::runtime().spawn(async move {
            let r = conn.execute(&sql, &theta_core::ExecOptions { row_limit: None }).await;
            *writer.lock().unwrap() = Some(r);
        });
        wait_for(vcx, "the count", |_| slot.lock().unwrap().is_some());
        let result = slot.lock().unwrap().take()?.ok()?;
        match result.result?.rows.first()?.first()? {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    #[gpui_kit::test]
    fn the_dialog_scans_a_file_then_imports_it_into_a_new_table(cx: &mut TestAppContext) {
        let (fx, vcx) = setup(cx);
        let path = fx.dir.path().join("people.csv");
        std::fs::write(&path, "id,Full Name,born\n1,Ann,2000-01-02\n2,\"Bo\nb\",\n3,Cy,2002-03-04\n")
            .unwrap();
        let (ws, store, conn) = (fx.ws.downgrade(), fx.store.clone(), fx.conn.clone());
        let view = vcx.update(|window, cx| {
            let request = Request {
                conn_id: conn,
                table: None,
                path: Some(path.display().to_string()),
                new_table: true,
            };
            cx.new(|cx| ImportView::new(ws, store, request, window, cx))
        });
        wait_for(vcx, "the scan", |vcx| {
            view.read_with(vcx, |v, _| v.scan.is_some() && v.phase == Phase::Idle)
        });
        view.read_with(vcx, |v, cx| {
            let s = v.scan.as_ref().unwrap();
            assert_eq!(s.names, vec!["id", "Full Name", "born"]);
            assert!(v.has_header);
            assert_eq!(v.cols.len(), 3);
            assert_eq!(v.cols[1].name.read(cx).value().as_ref(), "full_name");
            assert_eq!(v.cols[0].ty.read(cx).value().as_ref(), "INTEGER");
            assert_eq!(v.cols[2].ty.read(cx).value().as_ref(), "DATE");
        });
        // Validate first: nothing may be written.
        view.update_in(vcx, |v, window, cx| v.start(false, window, cx));
        wait_for(vcx, "validation", |vcx| view.read_with(vcx, |v, _| v.report.is_some()));
        view.read_with(vcx, |v, _| {
            let (kind, rep) = v.report.as_ref().unwrap();
            assert!(*kind == ReportKind::Validate);
            assert_eq!((rep.rows_read, rep.issue_count), (3, 0));
        });
        assert_eq!(count(&fx, vcx, "people"), None, "validation must not create the table");
        view.update_in(vcx, |v, window, cx| v.start(true, window, cx));
        wait_for(vcx, "the import", |vcx| {
            view.read_with(vcx, |v, _| matches!(&v.report, Some((ReportKind::Import, _))))
        });
        view.read_with(vcx, |v, _| {
            let (_, rep) = v.report.as_ref().unwrap();
            assert_eq!(rep.outcome, Outcome::Completed, "{:?}", v.status);
            assert_eq!(rep.rows_inserted, 3);
        });
        assert_eq!(count(&fx, vcx, "people"), Some(3));
        // The connection is released for consoles again.
        assert!(fx.ws.read_with(vcx, |ws, _| ws.runs.is_empty()));
    }

    #[gpui_kit::test]
    fn a_read_only_connection_refuses_to_import(cx: &mut TestAppContext) {
        let (fx, vcx) = setup(cx);
        fx.store.update(vcx, |s, cx| {
            let mut c = s.config(&fx.conn).cloned().unwrap();
            c.read_only = true;
            s.update(c, cx);
        });
        let ws = fx.ws.downgrade();
        let (store, conn) = (fx.store.clone(), fx.conn.clone());
        let view = vcx.update(|window, cx| {
            cx.new(|cx| {
                ImportView::new(
                    ws,
                    store,
                    Request { conn_id: conn, table: None, path: None, new_table: true },
                    window,
                    cx,
                )
            })
        });
        view.read_with(vcx, |v, _| {
            assert!(v.read_only);
            assert!(v.status.as_ref().is_some_and(|(ok, t)| !ok && t.contains("read-only")));
        });
    }
}
