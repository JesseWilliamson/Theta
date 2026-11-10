# Quarry architecture

Quarry is a database IDE: connect to databases, explore their structure, write SQL in
a modal (Helix) or standard editor, run it, and inspect results, plans and geometry.

## Why it is shaped this way

Three ideas drive every boundary in the codebase.

**1. Everything that can be tested without a window lives outside the UI crate.**
GPUI needs a GPU surface, so UI code is slow to compile and awkward to test. SQL
formatting, completion, the Helix state machine, geometry decoding and the database
drivers are all pure Rust libraries with ordinary unit/integration tests. The UI
crate is a thin layer that turns their outputs into pixels and their inputs into
keystrokes.

**2. One contract crate, many implementations.** `quarry-core` defines `Value`,
`ResultSet`, `Catalog`, `Plan`, `ConnectionConfig` and the `Driver`/`Connection`
traits. Drivers produce those shapes; the UI and SQL tooling consume them. Adding
a database means implementing two traits, not touching the grid or the tree.

**3. The UI thread never waits.** GPUI renders on the main thread. Database I/O runs
on a Tokio runtime on background threads; results come back through channels and
are applied in `cx.spawn` tasks. A slow query can never freeze typing or scrolling.

## Crates

```text
crates/
  quarry-core      contracts: Value, ResultSet, Catalog, Plan, ConnectionConfig, traits
  quarry-drivers   PostgreSQL, MySQL/MariaDB, SQLite + SSH tunnelling (Tokio)
  quarry-sql       statement splitting, formatter, completion, FK join suggestions,
                   plan analysis (insights from an EXPLAIN tree)
  quarry-geo       WKB/EWKB/WKT/GeoJSON, Web-Mercator projection, tile maths
  quarry-editor    text buffer + selections + Helix/standard keymaps + SQL highlighting
                   (no GPUI: takes key events in, exposes text/selections/spans out)
  quarry           the GPUI application
```

Dependency arrows only ever point "down" this list towards `quarry-core`.
`quarry-editor` and `quarry-geo` depend on nothing in the workspace.

## The editor

GPUI Kit ships a code editor, but it is not modal and has a single cursor. Helix's
model is *selection-first with multiple selections*: every motion produces a
selection, and every operator acts on all selections. That cannot be bolted on, so
Quarry has its own editor split in two:

- `quarry-editor` (pure): a rope buffer, a set of selections, undo history, a mode
  (`Normal`/`Insert`/`Select`, or the non-modal `Standard` keymap), and a function
  `handle_key(KeyInput) -> Vec<EditorEffect>`. Fully unit-tested as
  "text + selections + keys in → text + selections out".
- `quarry/src/editor_view` (GPUI): a custom element that paints lines, selections,
  cursors, the gutter and diagnostics, and forwards key events to the core.

Effects (`EditorEffect::RunStatement`, `RequestCompletion`, `OpenCommandPalette`, …)
are how the pure core asks the application to do something it cannot do itself.

## The application

```text
Workspace (root view)
├── TitleBar            connection switcher, run/cancel, transaction state
├── Explorer (left)     connections ▸ schemas ▸ tables ▸ columns/keys/indexes
├── Center              tabbed SQL consoles (EditorView)
├── Bottom panel        Results grid | Messages | Plan | Map | History
└── StatusBar           mode, cursor, connection, row count, timings
```

State ownership follows GPUI's rule that an `Entity<T>` owns state and views observe
it: `ConnectionStore` (configs + live sessions + catalogs) and `Settings` (theme,
keymap, fonts) are app-level entities; each console tab owns its editor and its
results.

## Roadmap

Each milestone ends with a tagged release and screenshots reviewed by a human-like
pass (alignment, spacing, contrast, empty/error/loading states).

| Milestone | Contents |
| --- | --- |
| M0 Foundations | workspace, contracts, headless screenshot harness, CI |
| M1 Libraries | drivers (+SSH), SQL tooling, geometry, editor core — all tested headlessly |
| M2 Prototype | app shell, themes, explorer, editor view, run query → results grid |
| M3 IDE features | completion, formatting, FK joins, EXPLAIN visualiser, map viewer, history, export, editable grid |
| M4 Polish & release | settings UI, keymap/theme pickers, packaging, docs, cross-platform CI builds |
