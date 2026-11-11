# Quarry architecture

Quarry is a database IDE: connect to databases, explore their structure, write SQL in a modal (Helix) or
standard editor, run it, and inspect results, plans and geometry. This document explains how the code is
split and why. Each idea points at the file where you can see it (`path:line`; paths are relative to the
repository root, and line numbers are those of the 0.1.0-alpha.1 sources). Read it before changing
anything, then see [DEVELOPMENT.md](DEVELOPMENT.md) for commands and workflows.

## Why it is shaped this way

Four ideas drive every boundary in the codebase.

**1. Everything that can be tested without a window lives outside the GPUI crates.**
GPUI needs a GPU surface, so UI code is slow to compile and awkward to test. SQL formatting, completion,
the Helix state machine, geometry decoding and the database drivers are plain Rust libraries with ordinary
unit and integration tests. The GPUI crates turn their outputs into pixels and their inputs into events.
The workspace has five pure crates (`quarry-core`, `quarry-sql`, `quarry-geo`, `quarry-editor`,
`quarry-drivers`) that build and test without a single GUI package. CI proves it: the `libraries` job in
`.github/workflows/ci.yml:42` never installs a window-system library.

**2. One contract crate, many implementations.** `quarry-core` defines `Value`, `ResultSet`, `Catalog`,
`Plan`, `ConnectionConfig` and the `Driver` / `Connection` traits (`crates/quarry-core/src/driver.rs:14`
and `:45`). Drivers produce those shapes; the SQL tooling and the view crates consume them. Adding a
database means implementing two traits, not touching the grid or the tree (see "Adding a database driver"
in [DEVELOPMENT.md](DEVELOPMENT.md)). The contract is deliberately free of client libraries and UI code
(`crates/quarry-core/src/lib.rs:1`), which is also why changing it is a decision for the repository owner.

**3. The editor engine asks; the host does.** `quarry-editor` is a pure state machine:
`Editor::handle_key(&KeyInput)` (`crates/quarry-editor/src/editor.rs:559`) changes text and selections and
returns *effects* (`Effect`, `crates/quarry-editor/src/editor.rs:43`) such as `RunStatementAtCursor`,
`ExplainStatement`, `OpenCommandPalette` or `SetClipboard`. It cannot open a picker or talk to a database,
and it knows no clock or I/O, so the whole Helix keymap is tested as "text + selections + keys in → text +
selections out". The application turns effects into actions in one `match`
(`crates/quarry/src/workspace/run.rs:252`).

**4. The UI thread never waits, and view crates talk upward with events.** GPUI renders on the main thread.
Database work runs on a small Tokio runtime and the answer comes back through a future that GPUI's executor
polls (`crates/quarry/src/runtime.rs:66`). The view crates (`quarry-editor-view`, `quarry-grid`,
`quarry-plan-view`, `quarry-map-view`) take plain data in and emit an event enum out
(`EditorEvent` `crates/quarry-editor-view/src/view.rs:43`, `GridEvent` `crates/quarry-grid/src/types.rs:129`,
`PlanEvent` `crates/quarry-plan-view/src/panel.rs:38`, `MapEvent` `crates/quarry-map-view/src/model.rs:31`).
None of them holds a connection. That keeps each testable alone (every one has an `examples/*_demo.rs`
that runs it with fake data) and means exactly one place, the workspace, decides what an event does.

## Crates

```text
crates/
  quarry-core         contracts: Value, ResultSet, Catalog, Plan, ConnectionConfig, Driver/Connection traits
  quarry-drivers      PostgreSQL, MySQL/MariaDB, SQLite, SSH tunnels, demo-data seeder (Tokio)
  quarry-sql          statement splitting and classification, formatter, completion, FK joins and
                      inference, plan analysis, lint, SQL generation, export
  quarry-geo          WKB/EWKB/WKT/GeoJSON, SRID and projections, tile maths, simplify, measure
  quarry-editor       rope buffer, selections, undo, SQL lexer, Helix and Standard keymaps (no GPUI)
  quarry-ui           settings, themes, fonts and icons, fuzzy matching, picker, shared widgets (GPUI)
  quarry-editor-view  GPUI widget that paints quarry-editor and runs the completion popup
  quarry-grid         results grid: selection, sorting, filtering, staged edits, inspector, export requests
  quarry-plan-view    EXPLAIN tree, flame graph, raw view, insights panel
  quarry-map-view     slippy and planar map, tile loader and cache, layers, measure tool
  quarry              the application: workspace, explorer, connections, dialogs, command palette
```

Dependencies, as declared in the `Cargo.toml` files. The first block is pure Rust; the second needs GPUI:

```text
 pure crates (no window system)
   quarry-core       (nothing)            quarry-geo      (nothing)
   quarry-editor     (nothing)            quarry-sql      ─► quarry-core
   quarry-drivers    ─► quarry-core       (quarry-sql in its tests)

 GPUI crates
   quarry-ui           (gpui-kit only)
   quarry-editor-view  ─► quarry-editor, quarry-sql, quarry-core, quarry-ui
   quarry-grid         ─► quarry-core, quarry-geo, quarry-ui
   quarry-plan-view    ─► quarry-core, quarry-sql, quarry-ui   (quarry-drivers in tests and its demo)
   quarry-map-view     ─► quarry-geo, quarry-ui
   quarry              ─► every crate above
```

Three consequences of these dependencies are worth knowing:

- `quarry-drivers` does not depend on `quarry-sql`. A driver executes one statement it is given; splitting
  scripts, classifying statements and reading plans for insights happen in the application. This is why
  the `Connection::execute` contract says "exactly one statement" (`crates/quarry-core/src/driver.rs:19`).
- `quarry-ui` is the only GPUI crate that the view crates share, and it depends on nothing else in the
  workspace, so a view crate can be built, demoed and tested without the application.
- `quarry-editor` and `quarry-geo` depend on nothing at all. The editor engine can be reused by anything
  that can feed it keys; the geometry crate has no idea what a database is.

## The editor

GPUI Kit ships a code editor, but it is not modal and has a single cursor. Helix's model is
*selection-first with multiple selections*: every motion produces a selection, and every operator acts on
all selections. That cannot be bolted on, so Quarry has its own editor, split in two:

- **`quarry-editor` (pure).** A rope buffer, selections, undo history, a mode (`Normal`, `Insert`, `Select`, `crates/quarry-editor/src/editor.rs:25`; the keymap is `Helix` or
  `Standard`, `:19`), and `handle_key`. Everything
  is indexed by **char**, not byte. `Editor::apply_with` (`crates/quarry-editor/src/editor.rs:345`) is the
  one choke point for text changes: it maps selections, records undo information and bumps the revision,
  so no command can forget one of those steps. The crate's README has the module map.
- **`quarry-editor-view` (GPUI).** `EditorView` (`crates/quarry-editor-view/src/view.rs:116`) paints lines,
  selections, the gutter, diagnostics and the completion popup, forwards keyboard, mouse and IME input to
  the engine, and re-emits what the engine asks for as `EditorEvent::Effect`. It does not know SQL: it is
  given a `CompletionProvider` (`crates/quarry-editor-view/src/view.rs:59`) and a `DiagnosticsProvider`
  (`:64`) by its host. The popup's pure state lives in `crates/quarry-editor-view/src/completion.rs`
  (auto-trigger rules, when Enter means "newline").

Where the application plugs SQL in: `Workspace::configure_console`
(`crates/quarry/src/workspace/run.rs:116`) gives each console the connection's catalog and dialect.
`SqlCompletion` (`crates/quarry/src/workspace/console.rs:38`) calls `quarry_sql::complete`
(`crates/quarry-sql/src/complete.rs:1014`) on the UI thread; the diagnostics closure calls
`quarry_sql::lint` (`crates/quarry-sql/src/lint.rs:75`) and is `Send + Sync` so the view can run it off the
UI thread. The catalog sits in a shared slot (`CatalogSlot`) that is swapped when the schema refreshes, so
neither provider is rebuilt.

Key precedence: GPUI delivers bound chords before the focused view's key handler, so a plain global
binding would steal the engine's keys (`ctrl-w`, `ctrl-b`, `ctrl-k` ...). The editor therefore publishes
its keymap, mode, prompt and popup state as key context, and `crates/quarry/src/keymap/ownership.rs` derives,
by probing the engine, which states use which keystroke; `keymap::bind` appends the matching
`!(QuarryEditor && ...)` predicate to every global binding. The editing engine wins inside the editor, the
application's chord applies elsewhere; see [KEYBINDINGS.md](KEYBINDINGS.md#who-wins-a-key) and the
crate README of `quarry-editor-view`.

## The application

```text
Workspace (root view, crates/quarry/src/workspace/mod.rs:146; renders at :820)
├── title bar        connection switcher, Run / Cancel, TXN open + Commit / Rollback   workspace/title_bar.rs
├── Explorer (left)  connections ▸ schemas ▸ tables ▸ columns, keys, indexes            explorer/
├── tab strip        console tabs, drag to reorder                                      workspace/tabs.rs
├── console area     one EditorView per console, or the welcome screen                  workspace/welcome.rs
├── bottom panel     Results | Messages | Plan | Map | History                          workspace/bottom.rs
├── status bar       mode, cursor, run time, result info, connection                    workspace/status_bar.rs
└── modal            picker (palette, tables, connections, themes) or a dialog          dialogs/, palette.rs
```

Start-up is `quarry::app::run` (`crates/quarry/src/app.rs:21`): it starts the Tokio runtime, initialises
`quarry-ui` (settings, fonts, themes), binds keys (`crates/quarry/src/keymap.rs:23`), loads the
`ConnectionStore`, opens the window with a `Workspace`, then applies the command-line flags
(`crates/quarry/src/cli.rs`).

**State ownership.** GPUI's rule is that an `Entity<T>` owns state and views observe it. There are three
long-lived owners:

- `ConnectionStore` (`crates/quarry/src/connections/mod.rs:80`): saved configs, secrets (keychain or memory,
  `connections/secrets.rs`), one `ConnState` per connection (`:27`: disconnected, connecting, connected
  with an `Arc<dyn Connection>`, the server info and the catalog, or failed), and the catalog loader
  (`:359`). It emits `StoreEvent`s; the workspace and explorer react.
- `Settings` (`crates/quarry-ui/src/settings.rs:52`), a GPUI global that is tolerant on read and written
  atomically on every change. Views observe it, so a theme or key map change repaints everything.
- `Workspace`, which owns the tabs, one `Console` per tab (`crates/quarry/src/workspace/console.rs`: the
  editor entity, its results, the catalog slot), the five bottom panels, the running-query table and the
  open-transaction set.

The explorer, grid, plan panel and map are *children* of the workspace that it feeds and listens to. The
workspace subscribes to every one of them in `Workspace::with_data_dir`
(`crates/quarry/src/workspace/mod.rs:213`).

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

1. **Picking statements** (`crates/quarry/src/exec/pick.rs`) is pure: `at_cursor`, `selection` and `all` call
   `quarry_sql::statement_at` and `split_statements`, then classify each statement with
   `quarry_sql::classify` (`crates/quarry-sql/src/script.rs:200`). The class drives the read-only refusal
   and the confirmation; `dangerous_reason` (`pick.rs:53`) asks the linter, so the editor underline and the
   dialog never disagree.
2. **Execution** is `runtime::spawn_result(async move { c.execute(&sql, &opts).await })`
   (`run.rs:548`). The future is awaited inside a `cx.spawn_in` task, so the UI thread only resumes
   when the answer is ready. The bridge is `crates/quarry/src/runtime.rs:30`: a two-thread Tokio runtime
   whose `JoinHandle` is polled by GPUI's executor (Tokio wakes the registered waker from its worker thread;
   GPUI's waker schedules the continuation back on the main thread). A panic in the task becomes
   `Error::Other` instead of unwinding the UI.
3. **Row limit.** `ExecOptions::row_limit` comes from `Settings` and is honoured by the driver by stopping the
   server, not by truncating afterwards (`crates/quarry-core/src/driver.rs:19` and each driver).
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
  (`crates/quarry/src/exec/editable.rs:8`): it decides whether the rows can be edited (one table, a primary
  key in the result, no computed columns, not a view) and which columns hold foreign keys, using the
  catalog. The result is stored in the console and shown when its tab is active (`apply_console`).
  `ResultGrid::set_result` (`crates/quarry-grid/src/grid.rs:179`) takes `Arc<ResultSet>` plus the meta; the
  grid never sees the connection. What the user does comes back as `GridEvent`s, handled in
  `Workspace::on_grid_event` (`run.rs:951`): `SubmitChanges` becomes SQL in `exec::changes::statements`
  (`crates/quarry/src/exec/changes.rs:9`) and is applied by `apply_changes` (`run.rs:1051`) after a
  confirmation dialog; `Export` becomes text in `quarry_sql::export`; `FollowReference` opens a new console
  (`follow_reference`, `run.rs:971`); `ShowOnMap` selects a feature.
- **Map.** `detect_geometry` (`run.rs:912`) runs `exec::geo::extract` (`crates/quarry/src/exec/geo.rs:66`) on
  the Tokio runtime so a large result does not stall the UI. It finds geometry columns (binary values, and
  text that parses as WKT or GeoJSON through `quarry_geo`), builds layers and features, and hands them to
  `MapPanel::set_features` (`crates/quarry-map-view/src/panel.rs:89`). Selection is shared both ways: a
  click on a feature raises `MapEvent::FeatureClicked`, handled in `on_map_event` (`run.rs:1189`) by
  `ResultGrid::select_row` (`crates/quarry-grid/src/grid.rs:258`); `GridEvent::ShowOnMap` calls
  `MapPanel::select_row` (`crates/quarry-map-view/src/panel.rs:111`).
- **Plan.** `explain_console` (`run.rs:1212`) picks the statement, asks for confirmation when an
  `EXPLAIN ANALYZE` would write, and `run_explain` (`run.rs:1240`) calls `Connection::explain`. The driver
  parses the server's output into a `quarry_core::Plan`; `quarry_sql::analyze_plan`
  (`crates/quarry-sql/src/plan.rs:898`) turns it into a summary and insights using the catalog; both go to
  `PlanPanel::set_plan` (`crates/quarry-plan-view/src/panel.rs:134`). The panel's events come back
  through `on_plan_event` (`run.rs:1278`): "Open in Console" opens the suggested `CREATE INDEX`, "Explain
  Analyze" reruns.

The *reason* for this arrangement: the three panels are views of data, and the data (`ResultSet`, `Plan`)
is defined once in `quarry-core`. The panels have no idea what produced it. That is what lets the grid be
demoed with generated rows and the plan view be tested with captured server output
(`crates/quarry-drivers/src/lib.rs:23` exposes the parsers for exactly that).

### Consoles, files and layout

- A console is one JSON file under `<config>/consoles/` (`crates/quarry/src/exec/consoles.rs`), written
  600 ms after the last edit (`schedule_save`, `run.rs:188`) and read back by `restore_consoles`
  (`run.rs:161`). Tab order is stored, which is how drag-to-reorder survives a restart. Results are not
  persisted.
- Statement history is `history.jsonl` (`crates/quarry/src/exec/history.rs`, capped at 5,000) behind the
  History tab (`crates/quarry/src/panels/history.rs`).
- Panel sizes and the selected bottom tab are `workspace.json` (`crates/quarry/src/workspace/persist.rs`),
  sanitised on load so a hand-edited file cannot break the layout.

### Actions, keys and the palette

Every user-invokable command is a type in `crates/quarry/src/actions.rs:6`. The key map
(`keymap.rs:23`), the macOS menu (`keymap.rs:75`), the command palette (`crates/quarry/src/palette.rs:25`)
and the title bar all refer to those types, so a command has one name everywhere. The workspace registers
one handler per action in `Workspace::render` (`crates/quarry/src/workspace/mod.rs:820`).

## Status

0.1.0-alpha.1 is the first release. The application is complete in the sense that every crate in the
workspace is wired into it and used:

- All eleven crates exist and are used; the application runs against PostgreSQL, MySQL/MariaDB and SQLite.
- Editor (both keymaps), completion, formatting, diagnostics, explorer, consoles, results grid with editing,
  export, query plans with insights, map, history, transactions, themes and settings are in.
- Verified by hand on Linux (X11). The macOS and Windows builds compile and pass the unit tests in CI but
  have not been exercised by hand.
- Known gaps are listed in the README under *Known limitations*; the ones that are architectural are:
  a connection is one session shared by its consoles; the editor has no soft wrap or folding; geometry
  decoding stops at the non-curved types.
