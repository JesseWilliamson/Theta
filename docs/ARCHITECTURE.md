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
  theta-drivers      PostgreSQL, MySQL/MariaDB, SQLite, SSH tunnels, demo-data seeder (Tokio)
  theta-sql          statement splitting and classification, formatter, completion, FK joins and
                      inference, plan analysis, lint, SQL generation, export
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

## Status

0.1.0-alpha.5 is the fifth pre-release (0.1.0-alpha.1 to -alpha.4 came before it; the product was called Quarry until -alpha.4). The application is complete in the sense
that every crate in the workspace is wired into it and used:

- All eleven crates exist and are used; the application runs against PostgreSQL, MySQL/MariaDB and SQLite.
- Editor (Helix, Vim and Standard keymaps), completion, formatting, diagnostics, explorer, consoles, results grid with editing,
  export, query plans with insights, map, history, transactions, themes and settings are in.
- Verified by hand on Linux (X11). The macOS and Windows builds compile and pass the unit tests in CI but
  have not been exercised by hand.
- Known gaps are listed in the README under *Known limitations*; the ones that are architectural are:
  a connection is one session shared by its consoles; the editor has no soft wrap or folding; geometry
  decoding stops at the non-curved types.
