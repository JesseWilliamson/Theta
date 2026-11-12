# Theta architecture

Theta is a database IDE: connect to databases, explore their structure, write SQL in a modal (Helix or Vim) or
standard editor, run it, and inspect results, plans and geometry. This document explains how the code is
split and why. Each idea points at the file where you can see it (`path:line`; paths are relative to the
repository root; the line numbers of the editor engine are those of the 0.1.0-alpha.2 sources and the others were
written against 0.1.0-alpha.1, so grep for the name if one has drifted). Read it before changing
anything, then see [DEVELOPMENT.md](DEVELOPMENT.md) for commands and workflows.

## Why it is shaped this way

Four ideas drive every boundary in the codebase.

**1. Everything that can be tested without a window lives outside the GPUI crates.**
GPUI needs a GPU surface, so UI code is slow to compile and awkward to test. SQL formatting, completion,
the editor's key maps (Helix, Vim, Standard), geometry decoding and the database drivers are plain Rust libraries with ordinary
unit and integration tests. The GPUI crates turn their outputs into pixels and their inputs into events.
The workspace has five pure crates (`theta-core`, `theta-sql`, `theta-geo`, `theta-editor`,
`theta-drivers`) that build and test without a single GUI package. CI proves it: the `libraries` job in
`.github/workflows/ci.yml:42` never installs a window-system library.

**2. One contract crate, many implementations.** `theta-core` defines `Value`, `ResultSet`, `Catalog`,
`Plan`, `ConnectionConfig` and the `Driver` / `Connection` traits (`crates/theta-core/src/driver.rs:14`
and `:45`). Drivers produce those shapes; the SQL tooling and the view crates consume them. Adding a
database means implementing two traits, not touching the grid or the tree (see "Adding a database driver"
in [DEVELOPMENT.md](DEVELOPMENT.md)). The contract is deliberately free of client libraries and UI code
(`crates/theta-core/src/lib.rs:1`), which is also why changing it is a decision for the repository owner.

**3. The editor engine asks; the host does.** `theta-editor` is a pure state machine:
`Editor::handle_key(&KeyInput)` (`crates/theta-editor/src/editor.rs:685`) changes text and selections and
returns *effects* (`Effect`, `crates/theta-editor/src/editor.rs:63`) such as `RunStatementAtCursor`,
`ExplainStatement`, `OpenCommandPalette` or `SetClipboard`. It cannot open a picker or talk to a database,
and it knows no clock or I/O, so every keymap is tested as "text + selections + keys in → text +
selections out". The application turns effects into actions in one `match`
(`crates/theta/src/workspace/run.rs:252`).

**4. The UI thread never waits, and view crates talk upward with events.** GPUI renders on the main thread.
Database work runs on a small Tokio runtime and the answer comes back through a future that GPUI's executor
polls (`crates/theta/src/runtime.rs:66`). The view crates (`theta-editor-view`, `theta-grid`,
`theta-plan-view`, `theta-map-view`) take plain data in and emit an event enum out
(`EditorEvent` `crates/theta-editor-view/src/view.rs:43`, `GridEvent` `crates/theta-grid/src/types.rs:129`,
`PlanEvent` `crates/theta-plan-view/src/panel.rs:38`, `MapEvent` `crates/theta-map-view/src/model.rs:31`).
None of them holds a connection. That keeps each testable alone (every one has an `examples/*_demo.rs`
that runs it with fake data) and means exactly one place, the workspace, decides what an event does.

## Crates

```text
crates/
  theta-core         contracts: Value, ResultSet, Catalog, Plan, ConnectionConfig, Driver/Connection traits
  theta-drivers      PostgreSQL (+CockroachDB, Redshift, TimescaleDB, YugabyteDB), MySQL/MariaDB (+TiDB), SQLite, DuckDB,
                      SQL Server, ClickHouse, SSH tunnels, demo-data seeder (Tokio)
  theta-sql          statement splitting and classification, formatter, completion, FK joins and
                      inference, plan analysis, lint, SQL generation, export, DDL engine
  theta-import       CSV/TSV parsing, sniffing, type inference, import and whole-table export pipelines
  theta-geo          WKB/EWKB/WKT/GeoJSON, SRID and projections, tile maths, simplify, measure
  theta-editor       rope buffer, selections, undo, SQL lexer, Helix, Vim and Standard keymaps (no GPUI)
  theta-ui           settings, themes, fonts and icons, fuzzy matching, picker, shared widgets (GPUI)
  theta-editor-view  GPUI widget that paints theta-editor and runs the completion popup
  theta-grid         results grid: selection, sorting, filtering, staged edits, inspector, export requests
  theta-plan-view    EXPLAIN tree, flame graph, raw view, insights panel
  theta-map-view     slippy and planar map, tile loader and cache, layers, measure tool
  theta              the application: workspace, explorer, connections, dialogs, command palette
```

Dependencies, as declared in the `Cargo.toml` files. The first block is pure Rust; the second needs GPUI:

```text
 pure crates (no window system)
   theta-core       (nothing)            theta-geo      (nothing)
   theta-editor     (nothing)            theta-sql      ─► theta-core
   theta-drivers    ─► theta-core       (theta-sql in its tests)

 GPUI crates
   theta-ui           (gpui-kit only)
   theta-editor-view  ─► theta-editor, theta-sql, theta-core, theta-ui
   theta-grid         ─► theta-core, theta-geo, theta-ui
   theta-plan-view    ─► theta-core, theta-sql, theta-ui   (theta-drivers in tests and its demo)
   theta-map-view     ─► theta-geo, theta-ui
   theta              ─► every crate above
```

Three consequences of these dependencies are worth knowing:

- `theta-drivers` does not depend on `theta-sql`. A driver executes one statement it is given; splitting
  scripts, classifying statements and reading plans for insights happen in the application. This is why
  the `Connection::execute` contract says "exactly one statement" (`crates/theta-core/src/driver.rs:19`).
- `theta-ui` is the only GPUI crate that the view crates share, and it depends on nothing else in the
  workspace, so a view crate can be built, demoed and tested without the application.
- `theta-editor` and `theta-geo` depend on nothing at all. The editor engine can be reused by anything
  that can feed it keys; the geometry crate has no idea what a database is.

## The editor

GPUI Kit ships a code editor, but it is not modal and has a single cursor. Helix's model is
*selection-first with multiple selections*: every motion produces a selection, and every operator acts on
all selections. That cannot be bolted on, so Theta has its own editor, split in two:

- **`theta-editor` (pure).** A rope buffer, selections, undo history, a mode (`Normal`, `Insert`, `Select`,
  the Vim `Visual`, `VisualLine`, `VisualBlock` and `Replace`, and `Standard`; `crates/theta-editor/src/editor.rs:26`;
  the keymap is `Helix`, `Vim` or `Standard`, `:19`), and `handle_key`. Everything
  is indexed by **char**, not byte. `Editor::apply_with` (`crates/theta-editor/src/editor.rs:413`) is the
  one choke point for text changes: it maps selections, records undo information and bumps the revision,
  so no command can forget one of those steps. The crate's README has the module map.
- **`theta-editor-view` (GPUI).** `EditorView` (`crates/theta-editor-view/src/view.rs`) paints lines,
  selections, the gutter, diagnostics and the completion popup, forwards keyboard, mouse and IME input to
  the engine, and re-emits what the engine asks for as `EditorEvent::Effect`. How the cursor is drawn is the
  engine's call (`Editor::cursor_style`, `crates/theta-editor/src/editor.rs:238`: block, bar or underline), so the
  view holds no per-keymap painting. It does not know SQL: it is
  given a `CompletionProvider` (`crates/theta-editor-view/src/view.rs:59`) and a `DiagnosticsProvider`
  (`:64`) by its host. The popup's pure state lives in `crates/theta-editor-view/src/completion.rs`
  (auto-trigger rules, when Enter means "newline").

### Three keymaps on one machine

Helix, Vim and Standard are three *front ends* to the same back end, which is why adding Vim (about 8,000 lines
in `crates/theta-editor/src/vim/`) touched the rest of the engine very little. `Editor::dispatch`
(`crates/theta-editor/src/editor.rs:759`) routes a key: the open prompt first, then the bindings shared by every
mode (`ctrl-enter`, `F5`), then the keymap. What the keymaps share:

- **Transactions.** Every Vim edit is built as `Change`s and applied through `Editor::vapply`
  (`crates/theta-editor/src/vim/ops.rs:317`), which ends in `apply_with`, so selections, undo, the syntax
  cache and the revision stay consistent without Vim code knowing about them. Undo groups are cut by
  `begin_key` / `end_key` in `editor.rs`: a whole Insert session or one Normal-mode command is one step.
- **Registers and the clipboard.** `reg_get` / `reg_store_named` (`vim/ops.rs:170`, `:202`) are Vim's register
  file over the engine's `Effect::SetClipboard` / `Editor::set_clipboard` pair; `mirror_unnamed` (`:233`) is the
  `clipboard=unnamedplus` option.
- **Motions and regions.** A Vim motion is a value (`Motion`, `vim/mod.rs:83`) that `eval_motion`
  (`vim/nav.rs:47`) turns into a destination and a kind (characterwise inclusive or exclusive, linewise); an
  operator (`Op`, `vim/mod.rs:49`) receives a `Region` built from it (`vim/ops.rs:36`) and `operate`
  (`vim/ops.rs:445`) acts on it. `d3w`, `v3wd`, `dvj` and `` d`a `` differ only in how the region is formed, and
  every text object is just another way to produce one. The word, sentence and paragraph code are ports of Vim's
  (`vim/motion.rs`, `vim/object.rs`) so edge cases agree.
- **Key replay.** `.`, macros (`q`, `@`) and `:normal` do not interpret anything twice: they record the *keys* and
  feed them back through `vim_feed` (`vim/mod.rs:521`), the same pipeline a typed key takes, with a fuel counter that
  bounds runaway recursion. The Helix keymap replays the same way.
- **Prompts and search.** `:` and `/` use the one prompt (`prompt.rs`) that the other keymaps use; Vim adds its
  regex translation (`vim/vregex.rs`) and the ex line (`vim/ex.rs`), and anything it does not know leaves as
  `Effect::HostCommand`.
- **Where Vim differs structurally.** The cursor is a *point* (`Editor::cursor_of` per keymap) rather than a
  one-grapheme selection; `vim_finish_command` (`vim/mod.rs:1058`) re-clamps it after each command, as Normal mode
  may not rest on the line break. The Visual modes are selections plus an anchor and cursor kept in `Visual`
  (`vim/visual.rs:27`); block-wise Visual is one selection per line, and block insert replays what was typed on the
  other lines when Insert mode ends (`vim/insert.rs:130`). Everything else about a command that is half typed lives
  in `Pend` (`vim/mod.rs:201`), which is also what the status bar shows.

**How it is tested.** The behaviour comes from *real Vim*. `crates/theta-editor/tests/vim_oracle/` holds a script
that drives `vim -u NONE -es` with a fixed profile (the options the engine implements), records the resulting
text, cursor and registers for 8,287 cases into `golden.tsv`, and a differential fuzzer. `cargo test` never runs
Vim: `src/tests/vim_oracle.rs` replays the recorded answers, and `gen_dsl_tests.py` writes about 3,500 more cases
as Rust tables (`src/tests/vim_*_cases.rs`, one test per case, named after the keys). The 137 cases where the
engine knowingly differs are listed with a reason in `deviations.tsv`, and a test fails when one of them stops
differing, so the list cannot rot. See the README in that directory for the workflow.

Where the application plugs SQL in: `Workspace::configure_console`
(`crates/theta/src/workspace/run.rs:116`) gives each console the connection's catalog and dialect.
`SqlCompletion` (`crates/theta/src/workspace/console.rs:38`) calls `theta_sql::complete`
(`crates/theta-sql/src/complete.rs:1014`) on the UI thread; the diagnostics closure calls
`theta_sql::lint` (`crates/theta-sql/src/lint.rs:75`) and is `Send + Sync` so the view can run it off the
UI thread. The catalog sits in a shared slot (`CatalogSlot`) that is swapped when the schema refreshes, so
neither provider is rebuilt.

Key precedence: GPUI delivers bound chords before the focused view's key handler, so a plain global
binding would steal the engine's keys (`ctrl-w`, `ctrl-b`, `ctrl-k` ...). The editor therefore publishes
its keymap, mode, prompt and popup state as key context, and `crates/theta/src/keymap/ownership.rs` derives,
by probing the engine, which states use which keystroke; `keymap::bind` appends the matching
`!(ThetaEditor && ...)` predicate to every global binding. The editing engine wins inside the editor, the
application's chord applies elsewhere; see [KEYBINDINGS.md](KEYBINDINGS.md#who-wins-a-key) and the
crate README of `theta-editor-view`. The probe covers every state of all three keymaps, including Vim's Visual
modes, half-typed commands (the `pending` context) and the prompt and popup; two rules sit on top of what the
engine reports (`VIM_RESERVED` in `ownership.rs`: Vim's plain `ctrl` letters are the editor's even where the
engine has nothing bound, so a habitual `ctrl-w` cannot close the tab), and the engine itself ignores `ctrl-shift`
letters so `ctrl-shift-f` (Format Document) is never taken for `ctrl-f`.

## The application

```text
Workspace (root view, crates/theta/src/workspace/mod.rs:146; renders at :820)
├── title bar        connection switcher, Run / Cancel, TXN open + Commit / Roll Back   workspace/title_bar.rs
├── Explorer (left)  connections ▸ schemas ▸ tables ▸ columns, keys, indexes            explorer/
├── tab strip        console tabs, drag to reorder                                      workspace/tabs.rs
├── console area     one EditorView per console, or the welcome screen                  workspace/welcome.rs
├── bottom panel     Results | Messages | Plan | Map | History                          workspace/bottom.rs
├── status bar       mode, cursor, run time, result info, connection                    workspace/status_bar.rs
└── modal            picker (palette, tables, connections, themes) or a dialog          dialogs/, palette.rs
```

Start-up is `theta::app::run` (`crates/theta/src/app.rs:21`): it starts the Tokio runtime, initialises
`theta-ui` (settings, fonts, themes), binds keys (`crates/theta/src/keymap.rs:23`), loads the
`ConnectionStore`, opens the window with a `Workspace`, then applies the command-line flags
(`crates/theta/src/cli.rs`).

**State ownership.** GPUI's rule is that an `Entity<T>` owns state and views observe it. There are three
long-lived owners:

- `ConnectionStore` (`crates/theta/src/connections/mod.rs:80`): saved configs, secrets (keychain or memory,
  `connections/secrets.rs`), one `ConnState` per connection (`:27`: disconnected, connecting, connected
  with an `Arc<dyn Connection>`, the server info and the catalog, or failed), and the catalog loader
  (`:359`). It emits `StoreEvent`s; the workspace and explorer react.
- `Settings` (`crates/theta-ui/src/settings.rs:52`), a GPUI global that is tolerant on read and written
  atomically on every change. Views observe it, so a theme or key map change repaints everything.
- `Workspace`, which owns the tabs, one `Console` per tab (`crates/theta/src/workspace/console.rs`: the
  editor entity, its results, the catalog slot), the five bottom panels, the running-query table and the
  open-transaction set.

The explorer, grid, plan panel and map are *children* of the workspace that it feeds and listens to. The
workspace subscribes to every one of them in `Workspace::with_data_dir`
(`crates/theta/src/workspace/mod.rs:213`).

### Running a statement

This is the path from a key press to rows on screen. Every step names the function that does it.

```text
 ctrl-enter ──► RunStatement action ──► Workspace::run_active ──► run_request            run.rs:400
                (keymap.rs / EditorEffect)                          │ picks statements: exec/pick.rs
                                                                    ▼
                                                      start_run                          run.rs:462
                                      refuse if the connection is busy (one query at a time)
                                      refuse writes on a read-only connection  (pick::read_only_refusal)
                                      confirm UPDATE/DELETE without WHERE      (pick::dangerous_reason)
                                                                    ▼
                                                      execute_picks                      run.rs:519
                       ┌───────────── for each statement ───────────────┐
                       │ runtime::spawn_result(connection.execute(..))  │   Tokio thread, driver does the I/O
                       │ ──► finish_statement (back on the UI thread)   │                run.rs:564
                       └────────────────────────────────────────────────┘
                                                                    ▼
                                    ResultItem pushed on the Console; grid, map, history, messages updated
                                                      finish_run                         run.rs:688
```

1. **Picking statements** (`crates/theta/src/exec/pick.rs`) is pure: `at_cursor`, `selection` and `all` call
   `theta_sql::statement_at` and `split_statements`, then classify each statement with
   `theta_sql::classify` (`crates/theta-sql/src/script.rs:200`). The class drives the read-only refusal
   and the confirmation; `dangerous_reason` (`pick.rs:53`) asks the linter, so the editor underline and the
   dialog never disagree.
2. **Execution** is `runtime::spawn_result(async move { c.execute(&sql, &opts).await })`
   (`run.rs:548`). The future is awaited inside a `cx.spawn_in` task, so the UI thread only resumes
   when the answer is ready. The bridge is `crates/theta/src/runtime.rs:30`: a two-thread Tokio runtime
   whose `JoinHandle` is polled by GPUI's executor (Tokio wakes the registered waker from its worker thread;
   GPUI's waker schedules the continuation back on the main thread). A panic in the task becomes
   `Error::Other` instead of unwinding the UI.
3. **Row limit.** `ExecOptions::row_limit` comes from `Settings` and is honoured by the driver by stopping the
   server, not by truncating afterwards (`crates/theta-core/src/driver.rs:19` and each driver).
4. **Cancel** calls `Connection::cancel` from the UI thread while `execute` is still awaiting
   (`run.rs:705`); the drivers make that `execute` return `Err(Error::Cancelled)`.
5. **One query per connection.** `Workspace::runs` is keyed by connection id; `start_run` refuses a second run.
   All consoles bound to a connection share its one session, which is why `BEGIN` in one console is visible
   in the next, and why the transaction state (`Workspace::txn`) is per connection too.
6. **A lost session.** `finish_statement` treats `Error::Closed` specially (`run.rs:668`): it logs it, skips
   the squiggle, and calls `ConnectionStore::disconnect` then `connect`, so the next statement runs on a
   fresh session.

### How results reach the grid, the map and the plan

- **Grid.** `finish_statement` builds a `GridMeta` with `exec::editable::grid_meta`
  (`crates/theta/src/exec/editable.rs:8`): it decides whether the rows can be edited (one table, a primary
  key in the result, no computed columns, not a view) and which columns hold foreign keys, using the
  catalog. The result is stored in the console, whose own `ResultGrid` shows it while the tab is active
  (`apply_console`; every async completion is keyed by console id, so a result, plan or edit-apply that
  finishes in a background tab never touches the grid on screen).
  `ResultGrid::set_result` (`crates/theta-grid/src/grid.rs:179`) takes `Arc<ResultSet>` plus the meta; the
  grid never sees the connection. What the user does comes back as `GridEvent`s, handled in
  `Workspace::on_grid_event` (`run.rs:951`): `SubmitChanges` becomes SQL in `exec::changes::statements`
  (`crates/theta/src/exec/changes.rs:9`) and is applied by `apply_changes` (`run.rs:1051`) after a
  confirmation dialog; `Export` becomes text in `theta_sql::export`; `FollowReference` opens a new console
  (`follow_reference`, `run.rs:971`); `ShowOnMap` selects a feature.
- **Map.** `detect_geometry` (`run.rs:912`) runs `exec::geo::extract` (`crates/theta/src/exec/geo.rs:66`) on
  the Tokio runtime so a large result does not stall the UI. It finds geometry columns (binary values, and
  text that parses as WKT or GeoJSON through `theta_geo`), builds layers and features, and hands them to
  `MapPanel::set_features` (`crates/theta-map-view/src/panel.rs:89`). Selection is shared both ways: a
  click on a feature raises `MapEvent::FeatureClicked`, handled in `on_map_event` (`run.rs:1189`) by
  `ResultGrid::select_row` (`crates/theta-grid/src/grid.rs:258`); `GridEvent::ShowOnMap` calls
  `MapPanel::select_row` (`crates/theta-map-view/src/panel.rs:111`).
- **Plan.** `explain_console` (`run.rs:1212`) picks the statement, asks for confirmation when an
  `EXPLAIN ANALYZE` would write, and `run_explain` (`run.rs:1240`) calls `Connection::explain`. The driver
  parses the server's output into a `theta_core::Plan`; `theta_sql::analyze_plan`
  (`crates/theta-sql/src/plan.rs:898`) turns it into a summary and insights using the catalog; both go to
  `PlanPanel::set_plan` (`crates/theta-plan-view/src/panel.rs:134`). The panel's events come back
  through `on_plan_event` (`run.rs:1278`): "Open in Console" opens the suggested `CREATE INDEX`, "Explain
  Analyze" reruns.

The *reason* for this arrangement: the three panels are views of data, and the data (`ResultSet`, `Plan`)
is defined once in `theta-core`. The panels have no idea what produced it. That is what lets the grid be
demoed with generated rows and the plan view be tested with captured server output
(`crates/theta-drivers/src/lib.rs:23` exposes the parsers for exactly that).

### Consoles, files and layout

- A console is one JSON file under `<config>/consoles/` (`crates/theta/src/exec/consoles.rs`), written
  600 ms after the last edit (`schedule_save`, `run.rs:188`) and read back by `restore_consoles`
  (`run.rs:161`). Tab order is stored, which is how drag-to-reorder survives a restart. Results are not
  persisted.
- Statement history is `history.jsonl` (`crates/theta/src/exec/history.rs`, capped at 5,000) behind the
  History tab (`crates/theta/src/panels/history.rs`).
- Panel sizes and the selected bottom tab are `workspace.json` (`crates/theta/src/workspace/persist.rs`),
  sanitised on load so a hand-edited file cannot break the layout.

### Actions, keys and the palette

Every user-invokable command is a type in `crates/theta/src/actions.rs:6`. The key map
(`keymap.rs:23`), the macOS menu (`keymap.rs:75`), the command palette (`crates/theta/src/palette.rs:25`)
and the title bar all refer to those types, so a command has one name everywhere. The workspace registers
one handler per action in `Workspace::render` (`crates/theta/src/workspace/mod.rs:820`).

### Added in 0.1.0-alpha.5

- **Query parameters** are pure: `theta_sql::params` finds placeholders with the lexer and substitutes literals
  built by `sql_literal`; `exec/params.rs` applies them to the statements a run picked, and
  `workspace/params.rs` asks (`dialogs/params.rs`) before `start_run` / `explain_picked`, so Run, Run Selection,
  Run All and Explain share one path. History and Messages see the substituted text.
- **Record view and statistics** live in `theta-grid` (`record.rs`, `aggregate.rs`, `agg_ui.rs`). The record is a
  *view of the grid's own selection* (row = cursor row, field = cursor column), so editing, staging and copying
  are the grid's, not a second implementation.
- **Connection folders**: `connections/folders.rs` is the pure model (one level, order within a group is the
  list order); the store persists it in `connections.json` next to each connection (`folder` key), and
  `explorer/tree.rs::flatten_grouped` turns it into rows.
- **Transactions**: `theta_core::txn` holds `TransactionConfig` (saved per connection) and builds the
  statements that start a transaction; `workspace/txn.rs` keeps the session override and `run.rs` issues them
  lazily in `execute_picks`, then asks `Connection::in_transaction` after every statement. No driver change.
- **Live templates**: `theta_editor::snippet` is the engine-level snippet session (tab stops are ranges mapped
  through every transaction; a linked stop is a multi-selection, so mirrors are edited by the engine's own
  multi-cursor editing). `theta_sql::templates` is the pure model (built-ins per dialect, variables, surround);
  `theta::templates` owns `templates.json`; the editor view asks the host to expand an abbreviation
  (`EditorEvent::ExpandSnippet`) because variables such as `${table}` need a picker.

### Added in 0.1.0-alpha.6

- **`theta-import`** (pure; depends on `theta-core` and `theta-sql`) is the bulk-data crate. The parser
  (`CsvReader::read_record`, `crates/theta-import/src/parser.rs:212`) pulls decoded chunks
  (`decode.rs`, UTF-8 / UTF-16 / Latin-1 with BOM detection) and keeps one record in memory; each `Field`
  remembers whether it was quoted, because `,,` (NULL) and `,"",` (empty string) must stay apart. `sniff.rs`
  guesses delimiter, quote, encoding and header from the first 128 KB, `scan.rs` produces the preview and the
  inferred column types (`infer.rs`), `convert.rs:142` turns a field into a `Value` for the target column
  with a row / column / reason error, and `setup.rs:126` (`build_job`) turns the dialog's choices into an
  `ImportJob` after the checks in `setup::check`. `run_import` (`run.rs:274`) is the pipeline and shares
  `RowReader` (`run.rs:30`) with `validate` (`run.rs:133`), so validation promises exactly what the import will
  convert. An import is one transaction (`TxnPlan`: its own, or a savepoint inside the session's), every batch
  is a savepoint, and a rejected batch is replayed row by row to name the offending line. Export is
  `export_table` (`export.rs:90`): keyset paging by primary key (`page_query`, `export.rs:67`) into
  `theta_sql::ExportEncoder` (`crates/theta-sql/src/export.rs:265`), written to a `.part` file that is renamed
  on success. The UI is `dialogs/import.rs` (`ImportView`, `:97`) and `dialogs/export.rs`; the workspace side
  (`workspace/bulk.rs:23`, `begin_bulk`) marks the connection busy so a console cannot interleave statements
  into an import's transaction, and decides the transaction plan (`bulk_txn_plan`, `:53`).
- **The DDL engine** is `theta_sql::ddl`. A `TableDraft` (`ddl.rs:90`, from the catalog with `from_table`,
  `:107`) identifies every column and constraint by a stable `id`, so `diff` (`:606`) pairs the original and the
  edited draft by id and a rename is a `RENAME COLUMN`, not a drop and an add. `create_table` (`:452`) and
  `diff` return `Statement`s tagged with `Flag`s (destructive, table rewrite, implicit commit, lock, outside a
  transaction) and an explanation. SQLite changes that `ALTER TABLE` cannot express go through
  `diff_sqlite` / `sqlite_rebuild` (`:953`, `:1077`), the documented rebuild. `validate` (`:1208`) is the
  inline-validation source for an editor. The live round trip (create, alter step by step, re-introspect) is
  `crates/theta-drivers/tests/ddl_roundtrip.rs`. `workspace/ddl_actions.rs` (`run_ddl`) runs the explorer's
  table actions (Rename, Truncate, Drop, Duplicate Structure), each confirming the exact SQL first. `plan_script`
  (`ddl.rs`) decides how a script is wrapped (one transaction, SQLite's pragmas outside it, joined to the
  session's own transaction, MySQL one by one); `check_changes` and `check_names` report what only the original
  and the edit reveal.
- **The table editor** is a workspace tab, `crates/theta/src/table_editor/`. `model.rs` (`DraftModel`) is pure: the
  original and edited `TableDraft`, undo and redo, the rows each section shows (`Spec`), what a cell edit means
  (a primary key column becomes NOT NULL, deleting a column takes its constraints with it, a removed column stays
  as a struck-through row, new ids never reuse a removed column's) and the `Analysis` (`diff` or `create_table`,
  `validate`, the catalog checks) that every edit re-runs. `keys.rs` is the key map as a table (key and modifiers
  in, `Cmd` out); `highlight.rs` colours the preview; `mod.rs` (`TableEditor`) owns cursor, cell editor, popups
  and run state and emits `TableEditorEvent`s; `render.rs` paints. The editor never touches a database:
  `workspace/table_tabs.rs` hosts the tab (`ConsoleTab::table`), answers the events (confirmation with badges,
  `run_table_script` with per-statement outcomes, catalog refresh and reload, discard prompt), and holds the
  explorer entry points, `:table` and `--modify` / `--new-table`.
- **Catalog extras** (`theta-core/src/schema.rs`): identity, generated columns, checks, unique constraints,
  partial and expression indexes, deferrable foreign keys, view definitions, MySQL table options; filled by the
  three introspection modules and covered by `crates/theta-drivers/tests/catalog_extras.rs`.

### Added in 0.1.0-alpha.7 (part 1): more databases

Theta now has eleven `DatabaseKind`s. Five of them (CockroachDB, Amazon Redshift, TimescaleDB, YugabyteDB,
TiDB) reuse the PostgreSQL and MySQL drivers; three (DuckDB, SQL Server, ClickHouse) have drivers of their
own. What makes that cheap is that a *kind* and a *dialect* are different things:

- `DatabaseKind` (`crates/theta-core/src/config.rs`) is the product the user chose. It owns the
  descriptor (`KindInfo`: label, icon key, port, URL schemes, which form sections apply, how the TLS
  dropdown is worded) and the **capability table** `Features`.
- `Family` is the SQL dialect (`kind.family()`): Postgres, MySql, Sqlite, DuckDb, SqlServer, ClickHouse. The
  lexer, formatter, completion, quoting and SQL generation switch on the family, so CockroachDB gets
  PostgreSQL's lexing for free and TiDB gets MySQL's. Code that must tell two products of one family apart
  (a catalog query, an EXPLAIN format, a DDL restriction) matches on the kind itself.
- `Feature` and `DatabaseKind::unsupported(feature)` answer "can this product do X, and if not, why". The
  explorer menu, the palette, `--modify`, the grid and the EXPLAIN button all ask it and show the reason
  instead of sending SQL the server would reject (`Workspace::refuse_unsupported`,
  `exec::editable::grid_meta`, `explain_console`). The reason text lives once, in the descriptor.
- `DatabaseKind::detect(banner)` and `mismatch(banner)` compare `version()` with the chosen kind on connect
  (`ServerInfo::banner`), and the store warns once when they disagree.

#### How to add a database kind

1. **Core.** Add the variant to `DatabaseKind` and to `DatabaseKind::ALL`; add a `KindInfo` static (scheme(s),
   port, sections, `ssl_labels`, `Features`) and the arm in `info()`. If its SQL is a dialect nobody has
   written yet, add a `Family` variant; if it is a PostgreSQL or MySQL derivative, point `family` at the
   existing one. `config.rs`'s unit tests check that schemes are unique and the sections are consistent.
   Write down in `Features` everything the product cannot do, with the reason as a sentence a user can act on.
2. **Driver.** Implement `Driver` and `Connection` in `crates/theta-drivers/src/<name>.rs` and add it to
   `driver_for`. Reusing a driver means giving it the kind (`PostgresDriver { kind }`) and branching where the
   product differs. The contract that tests check: `execute` honours `ExecOptions::row_limit` on the server,
   `cancel` makes `execute` return `Error::Cancelled`, errors become `Error::Query` with a code and, if the
   server gives one, a byte offset, `introspect` fills the `Catalog`, `explain` returns a `Plan` (or
   `Error::Unsupported` for `analyze`), `server_info().banner` is the server's own version text. Network
   kinds also get an arm in `connect_with_notices` so SSH tunnelling works.
3. **Parsers are pure.** A product's EXPLAIN, constraint text or result format is parsed by a function from
   text to values (`plan_text.rs`, `plan_mssql.rs`, `plan_ch.rs`, `redshift::parse_constraint_def`) with unit
   tests. Say in a comment where each fixture came from: output captured from a server is stronger evidence
   than text copied from documentation.
4. **SQL dialect.** In `theta-sql`: the lexer if quoting or comments differ (`lexer.rs`), reserved words,
   functions and statement starters (`keywords.rs`), literals (`generate.rs`), templates, type suggestions
   for the table editor (`ddl::type_suggestions`) and the statement splitter if the product has a batch
   separator (T-SQL's `GO`, `script.rs`). `tests/dialects.rs` is where these are pinned.
5. **Application.** The connection dialog is driven by the descriptor, so a new kind usually needs nothing;
   an icon goes in `theta-ui` (`DbIcon`, an SVG under `assets/icons`), URL parameters in
   `connections/url.rs`. Decide for the table editor (`Feature::ModifyTable`, `CreateTable`), CSV import
   (`Feature::Import`) and row editing (`Feature::EditRows`) whether they work, and if not, say why in the
   descriptor.
6. **Tests.** A live test per kind (`crates/theta-drivers/tests/kinds_live.rs` for servers that speak an
   existing wire protocol or need the generic run, a file of its own when the product needs different
   checks, as `clickhouse.rs` does). Servers that cannot run everywhere read their URL from an environment
   variable and skip with a printed message; add the service to `.github/workflows/ci.yml`. Never claim a
   product is verified that no test has been run against.

#### Where products differ, and how it is handled

- **CockroachDB**: PostgreSQL wire; its EXPLAIN is a text tree of `• node` lines (`plan_text::parse_cockroach`)
  with no BUFFERS and no JSON; routines come from `information_schema.routines`; `crdb_internal` is hidden.
- **Amazon Redshift**: PostgreSQL 8.0 inside. Introspection is rewritten from `information_schema`,
  `SVV_TABLE_INFO` and `pg_constraint` (`postgres/redshift.rs`); EXPLAIN is plain text; there is no ANALYZE,
  no savepoint and no useful ALTER, so ANALYZE, import and the table editor are disabled with a reason;
  constraints are informational, so the grid is read-only.
- **TimescaleDB**: stock PostgreSQL plus the extension; hypertables are listed and the internal schemas hidden.
- **YugabyteDB**: PostgreSQL wire (YSQL); EXPLAIN without BUFFERS; the table editor is disabled.
- **TiDB**: MySQL wire; EXPLAIN is a table (`plan_text::parse_tidb`); `KILL QUERY` cancels.
- **DuckDB**: embedded (`duck.rs`), one connection on a blocking thread, cancel through the interrupt handle;
  a feature of `theta-drivers` (`duckdb`, default on; `theta` forwards it) because the bundled C++ build dominates a clean build. Measured on the 4-core development machine with the workspace release profile (thin LTO, stripped): a program that only opens an in-memory DuckDB takes 14 min 49 s to build clean (48 min of CPU) and is 56.3 MB; the `parquet` and `json` extensions are linked in, nothing is downloaded at run time. Auto-install and auto-load of extensions are switched off on every connection. The dev profile sets `debug = false` for `libduckdb-sys`, because its C++ objects with line tables took 6.5 GB of disk.
- **SQL Server**: TDS through `tiberius` (`mssql.rs`); no attention packet, so cancel kills the session and
  the next statement reconnects; CLR spatial types need a rewrite (`spatial_retry`); showplan XML in
  `plan_mssql.rs`.
- **ClickHouse**: the HTTP interface through `ureq` (`clickhouse.rs`); results are
  `TabSeparatedWithNamesAndTypes` parsed by type; sessions make `SET` stick; cancel is `KILL QUERY`.

### Added in 0.1.0-alpha.7 (part 2): server administration

Three tools that work on the server instead of the data. Each follows the rule of the rest of the codebase: the
decisions are pure functions with unit tests, the view only paints and asks, and what a product cannot do is a
reason in the capability table.

- **Capabilities.** `Feature::Sessions`, `Roles`, `RoleEdit` and `Dump` (`crates/theta-core/src/config.rs`) with a reason per
  product; `Workspace::refuse_unsupported` shows it. SQL Server has `Roles` but not `RoleEdit` (read-only).
- **`theta_sql::admin`** (pure): `sessions.rs` has one list query per product with the same aliased columns, one parser,
  `apply_blockers` for MySQL's second query, `kill_statement` (the id is validated, never concatenated) and the
  sort / filter used by the view; `roles.rs` has the role and privilege queries, `parse_show_grants`, and the
  statements that change roles as a `Change` whose `sql` goes to the server and whose `shown` goes everywhere else
  (`Change::scrub` cleans a server error; `Debug` never prints the password). Where a MySQL and a MariaDB query
  differ the server banner decides (`MariaDB` in it).
- **`theta_import::dump`**: `plan` is pure (topological order by foreign keys, keys inline or after the data per
  product, sequences, identity counters, views, notes) and `dump` streams it through `export_table` with the
  `SqlInsertsMulti` format. `not_captured` is the list the dialog and the file header print. **`script`**:
  `StatementStream` re-uses `split_statements` on a buffer and holds the last statement back until more text
  arrives, re-splitting only when the buffer doubles; `run_script` is the loop (stop / continue, transaction,
  cancel, progress). **`native`** builds the `pg_dump` / `mysqldump` command (password in the environment).
  `crates/theta-import/tests/dump_roundtrip.rs` dumps the demo data, restores it into a scratch schema and compares
  columns, keys, indexes, row counts and a checksum of every row (PostgreSQL, MariaDB, SQLite; DuckDB with
  `--features duckdb`).
- **The tabs** are `crates/theta/src/admin/`: `keys.rs` (the key table, pure, with the collision test),
  `sessions.rs` and `roles.rs` (views that emit `AdminEvent`s and own a `SideConnection`, a session opened with
  `ConnectionStore::open_side_connection` so a busy console cannot block them), `form.rs` (a generic form with a
  live preview). `workspace/admin_tabs.rs` hosts them as `ConsoleTab::admin`, handles the events (Messages,
  notices, Open in Console), and has the `:` commands. `dialogs/dump.rs` and `dialogs/script.rs` are the two dialogs.
  Because *Run* arrives inside a workspace update, a view must not read the workspace from `refresh`
  (`refresh_with(console_busy)`); `admin_tests.rs` covers it.

### Added in 0.1.0-alpha.8: a syntax tree and the code intelligence built on it

Until alpha.7 everything that "understood" SQL (completion, the linter, FK inference, the formatter) worked from
tokens and heuristics. alpha.8 adds a real parser, `theta_sql::syntax` (`crates/theta-sql/src/syntax/`), and, on
top of it, name resolution and the editor features that need to know what an identifier *means*.

#### Why a hand-written parser, not a crate

`sqlparser` (sqlparser-rs, 0.64 at the time of writing) is the obvious candidate and was considered first. It is
built for the opposite job: it turns *valid* SQL into an AST and returns the first error, it drops whitespace and
comments (so the tree cannot cover the input), spans exist on tokens but not on every AST node, it has no error
recovery, and a construct outside its grammar is an error rather than an opaque region. An IDE needs the reverse of
each of those: a tree for text that is half typed, a span on every node so a cursor can be mapped to one, trivia
kept so edits can be expressed as text ranges, and silence about syntax it does not know. I did not benchmark or
prototype it; the decision rests on that design. It is not a dependency.

#### Tree shape

`syntax::parse(sql, kind) -> Parse { root, errors }`. The tree is lossless and uniform: `Node { kind, span,
children }`, where a leaf is a token (`Kind::Token(TokenKind)`, trivia included) and everything else is a syntax
node. Two invariants are tested on every parse of the corpus and of every fuzz input
(`Node::check_tiling`): the root spans `0..len`, and the children of each node tile its span exactly.
The leaves therefore concatenate back to the input and `Node::path_at(offset)` is plain interval arithmetic.
The doc comment on each `Kind` variant lists the child nodes a consumer may rely on; everything else (keywords,
punctuation) is in between only so the tiling holds. Two shapes worth knowing: a chain of the same operator
(`a + b + c`, `a AND b AND c`), of set operations and of joins is one *flat* node with many operands, never a
left-leaning tower (`Parser::wrap_merging`), so tree depth follows real nesting and never statement length; and a
trailing-trivia rule: trivia belongs to the parent of the node it follows, so every node starts and ends on a
significant token.

Queries and DML (SELECT with CTEs, set operations, every join kind, LATERAL, window definitions, INSERT with ON
CONFLICT / ON DUPLICATE KEY / RETURNING / OUTPUT, UPDATE ... FROM / joins, DELETE ... USING, MERGE) are parsed in
depth; expressions use precedence climbing (`expr.rs`, the table is in the module docs). DDL and every other
statement are *shallow*: the parser knows the kind and the names a statement defines or refers to (`Kind::Ddl`
with `Name` children, `ColumnDef`s of a CREATE TABLE) and keeps the rest as tokens. Queries that appear inside
such statements (`CREATE VIEW ... AS`, `PREPARE ... AS`, `COPY (query)`, `DECLARE ... CURSOR FOR`) are parsed too;
routine bodies (`CREATE FUNCTION/PROCEDURE/TRIGGER`) are deliberately opaque, because `NEW`/`OLD` and procedural
variables would look like unknown columns.

#### Error recovery, and the rule that matters: no false errors

A statement is parsed on its own: the script splitter (`script::segments`, which already knows `;`, `GO`, routine
bodies and dollar quotes) decides where it ends, so a broken statement cannot swallow the next. Inside a statement
the parser never fails: where it does not understand something it consumes tokens up to the next place the grammar
can resume (a `,`, a `)`, a clause keyword) into a `Kind::Unknown` node and carries on, so the intact clauses
around the problem still parse (`one_error_does_not_hide_the_rest` pins this).

`Parse::errors` is a different thing and much smaller than "where the parser got confused". Every `error(...)`
call is placed where the text cannot be valid in the dialect it is emitted in, and the call site says why (the
list is in the module docs of `parser.rs`). Where dialects disagree the rule is per family: `SELECT FROM t` and
`SELECT 1 AND` are fine in PostgreSQL, `SELECT a, FROM t` is fine in DuckDB, `a JOIN b` without a condition is
an error only in PostgreSQL and T-SQL, an unterminated `/* ...` is fine in SQLite. Every one of those exceptions
was found by the differential test, not by reading manuals. When in doubt a rule is dropped for that family.

#### How it is tested

* `crates/theta-drivers/tests/sql_differential.rs` runs the corpus in `crates/theta-sql/tests/corpus/` against
  real engines (PostgreSQL 16, MariaDB 10.11, bundled SQLite, bundled DuckDB) and asserts that the parser reports
  no error for any statement the engine accepts *or rejects only for a semantic reason*. It prints how many
  engine-rejected statements the parser also flags, and asserts nothing about that number. The README of the
  corpus directory says how each engine is asked. ClickHouse and T-SQL have only documentation examples and are
  weaker evidence; the ClickHouse server available locally is version 18.16 and rejects much of the corpus.
* `crates/theta-sql/tests/parser_robustness.rs`: the whole corpus in all six dialects, every truncation at every
  token boundary, random deletions and insertions, random token soup, 20,000-deep nesting on a 1 MB stack and a
  linear-time bound. These found a real hang (a `GO` separator) and a stack overflow (deep left-leaning chains).
* Unit tests next to the code (`syntax/tests.rs`): tree shapes, exact spans, each certain error, dialect
  differences, recovery.

#### How to add a statement or a construct

1. Add the `Kind` (and its doc line in `node.rs`) if the tree needs a new node.
2. Parse it in `dml.rs` / `query.rs` / `expr.rs`; consume with `bump`, group with `start(kind)` / `finish()`, wrap
   an already-parsed operand with `mark()` / `wrap()`. Every loop must consume at least one token.
3. Never call `error` unless the text is invalid in every dialect the rule is active for. If a server accepts a
   statement the test rejects, fix the *rule*, not the test.
4. Add a statement to `crates/theta-sql/tests/corpus/*.sql` (one per line), run the differential test, add a unit
   test for the tree shape.

#### Resolution (`theta_sql::analysis`)

`analysis::analyze(sql, kind, catalog) -> Analysis` runs the parser, then walks the tree once building *scopes*
and resolving every identifier. The result holds the parse, the symbol table (aliases, CTEs, CTE and derived
columns, select aliases), a `Resolution` per reference, the expanded `*`s and the diagnostics.

* A reference resolves to a catalog table, catalog column, symbol, function, pseudo source (`excluded`,
  `inserted`, `deleted`) or to `Unresolved(Problem)`; and, the important case, to `Unknown`.
  **Unresolved is only produced when every source in scope is fully known.** One unloaded schema, a system table,
  a script-created object of unknown shape, a statement containing an opaque `Unknown` node, or an empty catalog
  ("not loaded") makes the answer `Unknown` and no diagnostic is emitted. This is the same rule as the parser's:
  a missing diagnostic is cheap, a false one is not.
* Dialect rules live in `analysis/names.rs`: identifier folding (unquoted names fold to lower case in PostgreSQL and
  DuckDB; MySQL, SQLite and T-SQL compare case-insensitively, as their servers do for columns), quoting, and a strict-then-loose match (an exact match
  wins; otherwise a case-insensitive match is accepted for dialects that fold).
* Scopes: CTE scopes (recursive or not), derived tables, `USING` / `NATURAL` (the merged column is not
  ambiguous), `LATERAL` / `APPLY` (see the preceding FROM items), correlated subqueries (outer scopes are
  searched after inner), `RETURNING` / `OUTPUT`. A *passthrough* output column (`select a from t`) reuses the
  source's symbol, so renaming and ORDER BY see one thing.
* Diagnostics emitted: unknown table and column (with the tables it was looked for in), ambiguous column,
  undefined and duplicate alias, column-count mismatch (INSERT, set operations), ungrouped column (PostgreSQL,
  DuckDB, SQL Server only; it skips anything inside a call, columns determined by a primary key, and
  positional references).
* `lint::lint` merges parse errors (Critical), analysis diagnostics (Warning) and the meaning-level checks that are
  not about names (UPDATE/DELETE without WHERE, `= NULL`, implicit cross join, a near-miss statement keyword).

#### Editor assistance (`theta_sql::assist`)

Pure functions from `(sql, analysis, cursor)` to byte-range `Edit`s; `apply_edits` applies them. Nothing here
touches the UI.

* `code_actions` offers fixes for the problem at the cursor, and the statement-wide ones from anywhere in the
  statement. Every fix has a before/after unit test in `assist/tests.rs`.
* `definition_at`, `usages_of`, `highlights_at`, `target_at` answer navigation. A catalog object is a *target*
  (table or column) that the app resolves against the explorer and, for usages, against every open tab.
* `rename_prepare` / `rename_at` rename a symbol or refuse with a reason: empty, unchanged, not an identifier,
  alias collision, an inner scope that would capture the new name, a CTE collision or one that would hide a
  table the statement uses, a duplicated output column, an ORDER BY / GROUP BY that would bind differently, a
  passthrough column (rename the source), a catalog object (the message points to the table editor).

#### How the editor uses it

`theta/src/exec/intel.rs` analyses the *statement under the cursor* (the splitter's segment), so a 5,000-line
script costs one statement per keystroke; only catalog-object usages analyse the whole script. Diagnostics run
on the background executor after a debounce and are cancelled by the next edit; the view holds them back while
typing (`TYPING_IDLE`) and emits `DiagnosticsChanged` when the pause ends. The engine crates only know *keys*:
`gd`, `gr`, `F12`, ... become `Effect`s (`GoToDefinition`, `FindUsages`, `RenameSymbol`, `CodeActions`,
`NextProblem`, `ShowProblems`) and the workspace (`workspace/code_intel.rs`) answers them with pickers, the name
dialog and `Explorer::reveal_table`. A multi-range edit goes through `Editor::replace_ranges`, one undo step.

#### How to add a quick fix

1. Add a `FixKind` variant only if it is a new *section*; otherwise return an `Action { title, kind, edits }` from
   `code_actions` (`assist/fixes.rs`). Compute edits from the tree and the analysis, never from regexes on the
   text, and make them non-overlapping byte ranges.
2. Offer it only when it is certainly correct for the dialect; return nothing otherwise.
3. Add a before/after test in `assist/tests.rs` (`apply_edits` the action and compare the text) and a test that it
   is *not* offered where it would be wrong.

#### Numbers

Differential test, run on this machine (`cargo test -p theta-drivers --test sql_differential`; set
`THETA_DIFF_VERBOSE=1` for the missed lists). (a) is asserted, (b) and (c) are reported.

| engine | statements | false parser errors | false name diagnostics | (c) name errors also flagged | (b) syntax errors also flagged |
| --- | --- | --- | --- | --- | --- |
| PostgreSQL 16 | 2346 | 0 of 1451 | 0 of 905 | 363 of 435 (83%) | 560 of 864 (65%) |
| MariaDB 10.11 | 2356 | 0 of 1310 | 0 of 949 | 283 of 320 (88%) | 548 of 1040 (53%) |
| SQLite (bundled) | 2191 | 0 of 1177 | 0 of 714 | 321 of 412 (78%) | 561 of 1007 (56%) |
| DuckDB 1.2 (2 not asked) | 2632 | 0 of 1637 | 0 of 1007 | 339 of 461 (74%) | 516 of 993 (52%) |
| ClickHouse (docs, weak) | 138 | 0 | 0 | n/a | n/a |
| T-SQL (docs, weak) | 234 | 0 | 0 | n/a | n/a |

`corpus/typos.sql` is a separate, asserted list: every statement in it that an engine rejects as a syntax
error must reach the editor (parser error, or the linter's near-miss keyword check for `selct`); it holds the 18
typos reported from real use and their near neighbours, and `corpus/neighbours.sql` holds the valid side.
(b) is low on purpose: the parser flags a syntax error only where every dialect it applies to agrees, and the
engines reject far more (type errors, bad option values, missing keywords the parser tolerates). The corpus was
written by me, so these are catch rates on *my* statements, not a prediction for real-world SQL. Analysing a
5,000-statement file (one statement per line) takes about 0.23 s in the optimised test profile; parse time is
linear (500 units 0.04 s, 4,000 units 0.32 s).

### Added in 0.1.0-alpha.9 (part A)

#### Cell viewer

The grid already had an inspector (`theta-grid/src/inspect.rs`: what to show for a value, pure;
`inspector.rs`: the pane). alpha.9 extends it instead of adding a second panel, so "follows the cursor" is the
inspector's existing dirty flag (`inspector_dirty`, set on every cursor move, cleared by `sync_inspector`).
Three pure additions: `imagekind.rs` recognises PNG, JPEG, GIF and WebP from the signature and reads the
dimensions from the header by hand (no decoding dependency; GPUI decodes at paint time), `fold.rs` turns the
pretty JSON and its highlight spans into lines, bracket pairs (from the `Punct` spans, so a bracket inside a
string is never a fold) and the rows that remain under a set of folded lines, and `inspect_bytes` decides
between a picture, a hex dump and a note. The derived, per-cursor-move data (decoded `Image`, `JsonDoc`, hex
lines) is `ViewerState`, built once in `sync_inspector`, so painting only slices vectors. JSON and hex are
`uniform_list`s: a 200,000-character document is thousands of rows and only the visible ones are built.
Bounds are constants in `inspect.rs` (`MAX_BODY_CHARS`, `MAX_HEX_BYTES`, `MAX_IMAGE_BYTES`); every cut states
the real size. The workspace reaches the grid of the active console through `ResultGrid::toggle_cell_viewer`
and `copy_cell_value` for the palette actions, the global chords and the `:cell` host command; the Helix and Vim
space menus use the new `FxKind::Host`, a generic "send this `:` command to the application", so a menu entry for
an application feature needs no engine change beyond one line in each menu.

#### Copy table

`theta_sql::copy` (pure) decides, `theta_import::copy_table` does, `dialogs/copy_table.rs` shows. The type mapping
never goes product to product: `canon_of` reads a source column into a canonical `Canon` (integer width, decimal
with precision, text with or without a length, timestamp with or without a zone, array of ..., or `Other(name)`) by
one table per source family (SQLite's free-text declared types go through its five affinity rules), and `render`
writes a `Canon` in a target family's words, returning the reason when the result is not an exact equivalent;
`plan_copy` collects those into `Substitution`s for the preview. Copies between two tables of the same product skip
the mapping and copy the type text verbatim. `insert_statements` converts values the target column needs in another
form (MySQL's `tinyint(1)` 0/1 into a real boolean for PostgreSQL, a geometry into bytes for a byte column, an
offset timestamp into UTC wall time for MySQL, whose DATETIME has no zone and which MariaDB will not parse with
one); the live tests found the first of these. The runner reuses the export's keyset pager (`export::page_query`)
for the read side and wraps everything, `CREATE TABLE` included, in one transaction where the target has
transactional DDL (`transactional_ddl`); where it has not (MySQL) a failed or cancelled copy `DROP`s the table it
made, and `Cleanup` says which happened, including when the clean-up itself failed. Live tests in
`crates/theta-import/tests/copy_live.rs` copy between PostgreSQL, MariaDB, SQLite and DuckDB, compare the rows,
and prove the clean target after a cancel and after a row the target rejects.

#### Charts

`theta-chart` is a pure crate (it depends on `theta-core` only) holding everything about a chart that is not
painting, so each rule has a unit test: `spec::suggest` (defaults from column types; a column on a very
different scale is not charted with the first), `shape::build` (rows to series: category slots, grouping by a
series column, aggregation, bucketing of crowded continuous x into equal intervals, the bar / pie category
limits, and a note for every row or category that was left out), `ticks` (Heckbert nice numbers, calendar-aware
time ticks on month starts), `time` (civil-date arithmetic, no calendar dependency), `scale`, `hit` and
`layout`. `Layout::new(data, width, height)` is the whole geometry: plot rectangle, scales, tick positions,
bar rectangles, pie slices, `hit` (which mark is under a pixel) and `readout` (the text), all in plain `f64`.
`crates/theta/src/panels/chart.rs` is only the shell: the dropdowns and buttons, `canvas` painting of a `Layout`
(`paint_path` for lines, areas and slices, `paint_quad` for bars and points, shaped text for labels), the
pointer and key handlers, and the colours, which are the theme's own hues so the chart follows a theme change.
The workspace feeds it from `apply_console`, the one place every result passes through (a result that is the
same `Arc` as the one shown keeps the user's choices). PNG export is not offered: `Window::render_to_image`
exists only under GPUI's `test-support` feature.

#### Search across tables

Three layers, so that the dialect logic and the concurrency are each testable without a window:

- `theta_sql::search` (pure) plans the search: which tables a scope means (`Scope`; system schemas only when
  the whole connection is searched), which columns are searched (`ColumnClass` from `TypeKind`; binary and
  geometry never), and the one statement per table. The statement counts, over `LIMIT row_limit` rows, how many
  rows it saw, how many matched and how many each column matched (`COUNT(CASE WHEN … THEN 1 END)`, not `SUM`,
  which is NUMERIC on MySQL and NULL on an empty table). The match is `strpos` / `LOCATE` / `instr` and not
  `LIKE`, because a pattern needs its own escaping; case folding is explicit (`lower`) and MySQL's
  case-sensitive form casts to `BINARY`, since its collations ignore case. The needle goes through `sql_literal`
  and identifiers through `quote_ident`; `predicate`'s tests pin every dialect's text. `filter_statement`
  builds the "open the matching rows" query from the same predicates, so a count and the rows it opens agree.
- `theta_import::search::run_search` is the runner. A `Connection` runs one statement at a time, so concurrency
  *is* the number of sessions in the pool: one worker task per session pulls the next table from a shared queue,
  and no more than `pool.len()` statements are ever in flight. Each statement races a timer and the
  `CancelToken`; the loser's side effect is a `Connection::cancel`, then the aborted `execute` is awaited for up to
  five seconds so the session is idle for the next table. Every table ends as `Matched`, `NoMatch`, `TimedOut`,
  `Failed` or `Cancelled`, or is counted in `not_run`. It lives in `theta-import` because its live tests need the
  databases and that crate is tested in CI's libraries job (`tests/search_live.rs` seeds a scratch
  schema/database/file per engine, checks counts that a `LIKE` implementation would get wrong, and proves the
  concurrency bound, the timeout and the cancel with bounded `pg_sleep` / `SLEEP` statements).
- `dialogs/search.rs` is the view. `build_rows` (pure, unit-tested) turns the results into the list rows: hits
  in arrival order with their columns, then a "Not searched, timed out or failed" section, so nothing is hidden.
  Server products open up to four sessions with `ConnectionStore::open_side_connection`; products whose database
  cannot be opened twice (in-memory SQLite, DuckDB) run through the console's own session under `begin_bulk`.
  Workers push results into a mutex; a 100 ms tick copies them into the view (the same pattern as the dump).
  `Feature::TableSearch` in `theta-core` disables SQL Server and ClickHouse with the reason.

## Status

0.1.0-alpha.8 is the eighth pre-release (0.1.0-alpha.1 to -alpha.7 came before it; the product was called Quarry until -alpha.4). The application is complete in the sense
that every crate in the workspace is wired into it and used:

- All twelve crates exist and are used; the application runs against PostgreSQL, MySQL/MariaDB and SQLite, and (since alpha.7) CockroachDB, Redshift, TimescaleDB, YugabyteDB, TiDB, DuckDB, SQL Server and ClickHouse. DuckDB runs locally; ClickHouse, CockroachDB, TimescaleDB, TiDB and SQL Server pass live tests in CI; Redshift and YugabyteDB are unverified. See the README's table.
- Editor (Helix, Vim and Standard keymaps), completion, formatting, diagnostics, explorer, consoles, results grid with editing,
  export, query plans with insights, map, history, transactions, themes and settings are in.
- Verified by hand on Linux (X11). The macOS and Windows builds compile and pass the unit tests in CI but
  have not been exercised by hand.
- Known gaps are listed in the README under *Known limitations*; the ones that are architectural are:
  a connection is one session shared by its consoles; the editor has no soft wrap or folding; geometry
  decoding stops at the non-curved types.
