# Changelog

All notable changes to Theta (called Quarry before 0.1.0-alpha.4) are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow
[Semantic Versioning](https://semver.org/) once releases begin.

## [Unreleased]

### Fixed

- A console run (or `--run`) that contained DDL left the explorer showing the old schema on every product. The
  catalog is now refreshed once after such a run.
- `CREATE SPATIAL INDEX` / `FULLTEXT` on MySQL and MariaDB: the DDL engine wrote `USING SPATIAL`, a syntax error
  (found by the dump round trip). DuckDB views are dumped from the whole `CREATE VIEW` its catalog stores.

### Added

- **Server administration**: three tools, each reachable from the explorer's connection menu, the command palette
  and a `:` command, and keyboard-driven in the Helix, Vim and Standard key maps (see the user guide chapter
  *Administer a server*). Products that cannot do one say why, from the same capability table as everything else
  (`Feature::Sessions`, `Roles`, `RoleEdit`, `Dump`).
  - **Sessions** tab (`:sessions`): a live table of server sessions with a filter, sorting, a refresh interval and
    a pause toggle; sessions that are blocked are highlighted with the session that blocks them, and Cancel Query
    and Terminate Session confirm with the exact SQL first. PostgreSQL (and TimescaleDB), MySQL / MariaDB, SQL
    Server and ClickHouse. The tab uses a session of its own, so it keeps working while a console is busy, and it
    marks Theta's own sessions. Verified live on PostgreSQL 16, MariaDB 10.11 and ClickHouse 18.16.
  - **Roles** tab (`:roles`): roles and users with their attributes, memberships and privileges grouped by object;
    create, alter, set password, add to / remove from a role, grant, revoke and drop, each as a form with a live
    preview and a confirmation. The password is never written to Messages, history or any file (shown as `'***'`),
    and server errors that echo the statement are cleaned before they are logged. PostgreSQL and MySQL / MariaDB
    edit; SQL Server lists logins and server roles read-only (unverified).
  - **Export Database…** (`:dump`) and **Run SQL Script…** (`:script FILE`). The dump is Theta's own, streaming
    and cancellable: DDL from the DDL engine in dependency order (foreign keys after the data on PostgreSQL and
    MySQL), data as multi-row `INSERT`s, sequences and identity counters, views. The dialog lists what it does not
    capture, and offers `pg_dump` / `mysqldump` when installed with the exact command (password in the
    environment, never in the arguments). Run SQL Script splits the file as it reads it (never loaded whole), stops
    at the first error or collects them, can run in one transaction, and cancels. Round trips of the demo data are
    tested on PostgreSQL, MariaDB, SQLite and DuckDB.
  - `theta_sql::admin` holds the per-product SQL and row mapping, `theta_import::dump`, `script` and `native` the
    pipelines.
- **New Table suggests foreign keys** before the table exists: inference runs on the draft
  (`theta_sql::infer_foreign_keys_for`), with the same heuristics as the catalog-wide inference.

- **Eight more databases.** CockroachDB, Amazon Redshift, TimescaleDB, YugabyteDB and TiDB (they reuse the
  PostgreSQL and MySQL drivers, each with its own descriptor, URL schemes, icon and the handling its
  documentation calls for), plus DuckDB, SQL Server and ClickHouse with drivers of their own.
  - `DatabaseKind` gained `family()` (the SQL dialect), `unsupported(Feature)` (a stated reason for what a product
    cannot do; menus, palette, `--modify`, the grid and Explain show it instead of sending failing SQL) and
    `detect()` / `mismatch()`, which compare `version()` with the chosen product on connect and warn once.
  - **CockroachDB**: text-plan EXPLAIN (with ANALYZE) parser, routines from `information_schema`.
    **Redshift**: introspection from `information_schema`, `SVV_TABLE_INFO` and `pg_constraint`, plain-text
    EXPLAIN, no ANALYZE, import and table editor disabled with reasons. **TimescaleDB**: hypertables listed,
    internal schemas hidden. **YugabyteDB**: EXPLAIN without BUFFERS. **TiDB**: tabular EXPLAIN parser, `KILL QUERY`.
  - **DuckDB**: file or `:memory:`, introspection through the `duckdb_*()` functions, EXPLAIN / EXPLAIN ANALYZE as a
    plan, cancel through the interrupt handle, DECIMAL, HUGEINT, temporal types and LIST / STRUCT / MAP (as JSON)
    values, Parquet and CSV through plain SQL; extensions are never downloaded. Behind the default-on cargo
    feature `duckdb` because it adds about 55 MB to the binary and about 15 minutes to a clean build.
  - **SQL Server** (TDS via `tiberius` with rustls): SQL authentication, Encrypt off / on / trust certificate,
    `sys.*` introspection (identity and computed columns, keys, checks, indexes, routines), `SET ROWCOUNT` row
    limit, cancel by `KILL`, showplan XML for EXPLAIN (estimated) and STATISTICS XML (actual), `geometry` /
    `geography` as EWKB, `[bracket]` quoting, `TOP`, `GO` batches and T-SQL keywords. Not yet run against a server.
  - **ClickHouse** over HTTP: engine, sorting key, primary key and partition key in the explorer (*Storage*),
    `KILL QUERY` cancel, `EXPLAIN json = 1`, Nullable / Array / Map / Tuple / LowCardinality / DateTime64 /
    Decimal / UUID / IPv4 / IPv6 values, backtick quoting, ClickHouse functions; the grid is read-only with the
    reason shown.
  - The explorer shows a table's storage facts (engine, distribution style, hypertable) in a *Storage* section;
    SQL Server's `TOP` in generated SELECTs; the connection dialog words TLS per product and follows ClickHouse's
    HTTPS port; `--connect` and the URL field take `cockroachdb://`, `redshift://`, `timescaledb://`,
    `yugabytedb://`, `tidb://`, `duckdb://`, `sqlserver://` (`encrypt`, `trustServerCertificate`), `clickhouse://`
    and `clickhouses://`.
  - `docs/ARCHITECTURE.md` has a walkthrough of how to add a database kind; the README lists, per product,
    whether it has been run against a real server.

### Changed

- **New app icon and logo**: the theta is now the lowercase theta of the GFS Didot typeface (a tall, high-contrast
  serif oval with a thin crossbar joined to both sides), white on the same dark tile. Its outline is embedded in
  `packaging/icons/gen_icon.py`, the single source of the icon, and GFS Didot's licence (SIL OFL 1.1) is credited in
  `docs/THIRD_PARTY.md`, the NOTICE file and the website footer. The icon has separate, heavier drawings of the same
  letter for 16, 24 and 32/48 px, so it stays legible in a taskbar; the in-app mark and the website use the same design.
- The statement splitter, formatter and parameter finder know T-SQL's `GO`, `BEGIN TRANSACTION` and `DECLARE`;
  the formatter keeps `GO` on its own line.

## [0.1.0-alpha.6] - 2026-11-11

### Added

- **Modify Table / New Table editor.** A workspace tab with a header (table name, connection, database kind, a
  summary such as `3 statements · 1 destructive`, **Revert**, **Copy SQL**, **Open in Console**, **Execute**), the
  sections **Columns · Keys · Foreign Keys · Indexes · Checks · Options** (counts and error / warning dots) and a
  persistent **DDL Preview** (beside the grid, below it in a narrow window, resizable, collapsible): one block per
  statement with badges (destructive, table rewrite, lock, implicit commit, outside transaction, rebuild), the
  engine's explanation, SQL highlighting and a copy button, validation problems with **Fix** buttons, and a
  "No changes" state. *Columns* is a virtualised, typed, keyboard-driven grid: name, type (dialect-aware
  suggestions, free text allowed), nullable, default, primary key (with its position), identity /
  auto-increment, comment; a removed column stays, struck through, until Execute. *Keys* edits the primary and
  unique keys, *Foreign Keys* picks the referenced table from the catalog and offers the inferred keys with one
  click, *Indexes* edit columns in order, unique, method and a partial predicate, *Checks* and the table comment
  are editable, and the MySQL engine / charset / collation are shown. Undo and redo within the tab. **Execute**
  confirms the exact SQL with its badges and runs it through the same path as the explorer's table actions: one
  transaction on PostgreSQL and SQLite (SQLite's rebuild keeps its pragmas outside it and checks foreign keys
  before committing; the preview shows which statement is running), statement by statement on MySQL / MariaDB (stated, and a failure names what was already
  applied), inside the open transaction in manual mode, refused on read-only connections. On failure the failing
  statement is marked with the server's error and every edit is kept; on success the tab reloads as *Modify
  <name>* and the table is selected in the explorer. Closing a tab with unapplied changes asks.
  Entry points: *New Table…* (Tables group, palette, `:table new [SCHEMA]`, `theta --new-table [SCHEMA]`),
  *Modify Table…* (table menu, palette, `:table modify NAME`, `theta --modify TABLE`), *Modify Column…*, *Rename
  Column…*, *Drop Column…*, *New Index…*, *Drop Index…*, *New Foreign Key…*, *Drop Foreign Key…* and *Add as
  Foreign Key…* on a suggested key. Fully keyboard operable in the Helix, Vim and Standard key maps (see
  [KEYBINDINGS](docs/KEYBINDINGS.md#table-editor)); no new global chord (Execute is `mod-enter`).
- **DDL engine additions** (`theta_sql::ddl`): per-dialect type, index-method, foreign-key-action and identity
  vocabulary; `plan_script` (how a script is wrapped and run); validation issues carry their section;
  `check_names` (table, constraint and index name clashes) and `check_changes` (MySQL refuses to change the type
  of a column that a foreign key uses, on either side).
- **Import Data (CSV / TSV).** *Import Data…* on a table, *Import as New Table…* on the Tables group, the palette
  (*Import Data from CSV/TSV…*, *Import CSV/TSV as New Table…*), `:import [FILE]` in Helix and Vim, and
  `theta --import FILE [--into [SCHEMA.]TABLE]`. The dialog has a typed path field (Browse… is optional), a
  detected format (delimiter, quote, encoding, header row; each can be overridden), a live preview of the first
  100 rows, a *Columns* tab that maps file columns onto an existing table (auto-mapped by normalised name, per
  column skip, target type shown, mismatches flagged) or lays out a new table (editable names and types with the
  generated `CREATE TABLE` shown), and options (empty fields are NULL, a NULL token, skip bad rows instead of
  stopping, empty the table first, rows per INSERT). **Validate** parses the whole file off the UI thread and
  lists every problem with its line and column without writing anything; **Import** shows a progress bar with
  rows read / inserted / skipped and rows per second, and Cancel rolls everything back. An import is one
  transaction (in a manual-commit session it joins the open transaction through a savepoint and leaves it open),
  each batch runs in a savepoint so a rejected row is named exactly (or skipped and listed), statements are kept
  under 1 MB and under half of MySQL's `max_allowed_packet`, and a table created by a failed import is gone again.
  Emptying the table first shows the exact statement and asks twice.
- **Export Data.** *Export Data…* on a table and *Export Table Data…* in the palette (`:export [TABLE]`) write a
  whole table as CSV, TSV, JSON or SQL INSERTs, paged by primary key (constant memory, stable while the table
  changes; `OFFSET` paging without one), with progress and Cancel. A cancelled or failed export leaves no file.
- **Table actions.** *Rename…*, *Duplicate Structure…*, *Truncate…* and *Drop…* on a table (and in the palette).
  Each shows the exact SQL and the engine's warnings in a confirmation dialog; Drop lists the foreign keys that
  reference the table; read-only connections refuse; a manual-commit session runs the statement inside its open
  transaction; MySQL's implicit commit is stated plainly.
- **DDL engine** (`theta_sql::ddl`): `TableDraft` (columns, primary key, unique constraints, foreign keys, indexes,
  checks, comments, identity and generated columns) built from the catalog, `create_table`, and an id-based `diff`
  that produces the ordered, minimal `ALTER` script per dialect with each statement tagged (destructive, table
  rewrite, implicit commit, lock, outside a transaction) and explained. Renames are renames, never drop-and-add.
  SQLite changes that `ALTER TABLE` cannot do use the documented table rebuild. Includes validation (duplicate
  names, missing types, primary key on a nullable column with an auto-fix, foreign-key type checks).
- **Catalog**: identity / auto-increment and generated columns, check constraints, unique constraints (distinct from
  unique indexes), partial and expression indexes, deferrable foreign keys, view definitions and MySQL table options,
  for PostgreSQL, MySQL / MariaDB and SQLite.
- New crate **`theta-import`**: streaming RFC 4180 parser (quoted newlines, configurable delimiter, quote and escape,
  ragged rows, huge fields, BOM, UTF-8 / UTF-16 / Latin-1), format sniffing, type inference, typed conversion with
  row / column / reason errors, and the import and export pipelines. Measured on 1M rows (77 MB, 5 columns,
  dev profile): raw parse 4.5M rows/s, parse and convert 1.1M rows/s, end to end 69k rows/s into PostgreSQL,
  104k into MariaDB and 232k into SQLite.

### Changed

- Table context menu: *Modify Table…*, *Rename…*, *Duplicate Structure…*, *Truncate…*, *Drop…*, *Import Data…*,
  *Export Data…*; column menu: *Modify Column…*, *Rename Column…*, *Drop Column…*.
- Confirmation dialogs take keyboard focus when they open, so `tab` reaches their buttons even when the SQL editor
  had it (a destructive confirmation has no default button: `tab` to it and `space`).
- `ExportEncoder` in `theta_sql::export` streams CSV, TSV, JSON and SQL INSERTs page by page.
- Theta is now released under the MIT licence (`LICENSE`).
- The repository moved to <https://github.com/JesseWilliamson/Theta>; links, the map-tile User-Agent and the
  website point there.
- Website at <https://jessewilliamson.github.io/Theta/>, deployed from `site/` by GitHub Pages.
- Website redesign: it opens in the dark theme for every visitor (the system colour scheme is no longer consulted;
  light is available from the toggle), headlines are set in IBM Plex Serif Light, the hero is centred under a faint
  square grid with a three-column summary strip, and `D` jumps to the download section. Fonts are self-hosted WOFF2.
  The editor illustration now uses only tables and keys that exist in the demo database, and the hero screenshot shows
  a connection called "demo".
- The θ mark has a shorter bar: 55% of the ring's inner width, centred a hair above the middle and a little lighter than
  the ring, with a clear gap on both sides. The 16, 24 and 32 px drawings were redrawn on their pixel grids, and the
  website now derives its copy of the mark from `packaging/icons/theta-mark.svg` so the two cannot drift apart.

### Contract changes

- `theta-core`: `ColumnInfo::{identity, generated}`, `Table::{checks, unique_constraints, view_definition, options}`,
  `ForeignKey::{deferrable, initially_deferred}`, `Index::{predicate, has_expressions}`; new types `Identity`,
  `GeneratedColumn`, `CheckConstraint`, `UniqueConstraint`, `TableOption`; `Default` for `ColumnInfo`, `Table`,
  `ForeignKey`, `Index`, `TableKind` and `TypeKind`. All new fields are serde-default. No `Connection` / `Driver`
  trait change.

### Not in this release

- The modify-table editor tab (column grid, DDL preview pane, Execute / Revert) and the *Modify…* / *New Table…* /
  column and key menu entries: the engine and catalog they need are in, the tab is not.
- PostgreSQL `COPY ... FROM STDIN` for faster imports, and unescaping of backslash-escaped TSV on import.

## [0.1.0-alpha.5] - 2026-11-11

### Added

- **Query parameters.** A statement with `?`, `$1`, `:name` or `@name` asks for values before it runs (Run, Run
  Selection, Run All, Explain): a *Parameters* dialog with a type per value (Auto, Text, Number, Boolean, NULL,
  Raw SQL), values remembered per console, Enter to run. The values are substituted client-side as escaped
  literals; History records the substituted statement and Messages lists the values. Placeholders inside strings,
  comments and dollar-quoted bodies, Postgres `::casts` and jsonb `?` / `?|` / `?&`, and MySQL user variables
  (`@x := 1`, `SET @x`, `@@system`) are not parameters. Setting `editor.parameters`, palette *Toggle Query
  Parameters*. New module `theta_sql::params`.
- **Record view** in the results grid (toolbar button, `F4`, `r` in Helix and Vim): the current row as a list
  of fields with wrapped values, pretty-printed JSON, NULL styling and a geometry summary; `up`/`down` (`k`/`j`)
  change rows, `tab` and `left`/`right` change fields, fields are editable through the grid's usual staging, and
  *Value* / *Row* copy buttons. The list is virtualised, so a 500-column row is cheap.
- **Selection statistics** popover (click the footer summary, `F3`, `a` in Helix and Vim): count, distinct,
  NULLs, min, max, sum, average and text lengths, computed on a background thread for large selections, with a
  copy button.
- **Connection folders and ordering.** Folders (one level) in the explorer: create, rename, delete, move a
  connection into one, collapse state remembered; *Move Up / Down* (`alt-up` / `alt-down`) within a folder; the
  connection picker shows and matches `Folder / Name`. Saved in `connections.json`; existing files load unchanged.
- **Transaction mode and isolation level.** Per connection (dialog) and per session (title-bar chip, palette):
  auto-commit or manual, with PostgreSQL / MySQL isolation levels and SQLite `BEGIN` modes; manual mode starts
  the transaction lazily, starts it read-only on read-only connections, and shows `Manual · TXN open ·
  Repeatable Read`. MySQL's implicit commit on DDL is detected and reported.
- **Live templates.** Built-in and user templates (`templates.json`) with tab stops, linked placeholders and
  variables (`${table}`, `${columns}`, `${selection}`, `${clipboard}`, `${date}`), expanded with Tab or from
  the completion list (kind Snippet); engine-level snippet sessions in all three key maps; Settings ›
  Templates; *Surround Selection with…*; *Edit Live Templates…*.

### Changed

- The fixed completion snippets are now the built-in live templates (their bodies carry tab stops).
- Accepting a *Snippet* completion goes through the template expansion instead of inserting plain text.

### Contract changes

- `theta-core`: new `txn` module (`TransactionConfig`, `Isolation`, `SqliteBegin`, `begin_statements`,
  `isolation_levels`, `starts_transaction`, `implicit_commit_note`) and a new field
  `ConnectionConfig::transaction` (serde-default, omitted when default, so existing files are unaffected).
  No `Connection` / `Driver` trait or driver change; new integration tests cover the three live databases.
- `theta-editor`: `Editor::insert_snippet`, `snippet_active`, `snippet_navigate`, `end_snippet`; `Snippet` type.
- `theta-editor-view`: `CompletionProvider::snippet_for`, `EditorEvent::ExpandSnippet`, `EditorView::insert_snippet`.

### Not in this release

- CSV / TSV import, whole-table CSV export and the create / alter table editor were planned and are not started.


## [0.1.0-alpha.4] - 2026-11-11

### Renamed from Quarry to Theta

The project is now **Theta** and its icon is the Greek letter θ. The name comes from the theta join (θ-join) of
relational algebra, the general form of JOIN. Entries below this one keep the name that was current when they were
written. The GitHub repository keeps its old address (`JesseWilliamson/Quarry`) for now; GitHub redirects it once it is
renamed.

Your data carries over:

- **Configuration, history, consoles and the demo database** move from `<config dir>/quarry/` to `<config dir>/theta/`
  the first time Theta starts, if the new directory does not exist yet (renamed in place, copied if the two are on
  different filesystems; a notice is printed to stderr). Nothing is overwritten and nothing moves while
  `THETA_CONFIG_DIR` is set. The map tile cache moves from `<cache dir>/quarry/` to `<cache dir>/theta/` the same way.
- **Themes**: "Quarry Dark" and "Quarry Light" are now **Theta Dark** and **Theta Light**; a `settings.json` that
  names the old ones is read as the new ones, silently (the file is rewritten with the new names the next time
  settings are saved). The bundled theme file `quarry.json` is now `theta.json`.
- **Saved passwords**: the keychain service is now `theta`. A password stored under the old `quarry` service is read as
  a fallback and copied to the new one the first time it is needed; the old entry is left in place until the
  connection is deleted.

What you have to change by hand:

- **Binary and package**: `quarry` is now `theta` (`cargo run -p theta`, `theta --demo`, `theta --version` prints
  `theta 0.1.0-alpha.4`); library crates are `theta-core`, `theta-drivers`, `theta-sql`, `theta-geo`, `theta-editor`,
  `theta-editor-view`, `theta-grid`, `theta-map-view`, `theta-plan-view` and `theta-ui`.
- **Environment variables**: every `QUARRY_*` variable is now `THETA_*` (`THETA_CONFIG_DIR`, `THETA_CACHE_DIR`,
  `THETA_WINDOW_SIZE`, `THETA_DEMO`, `THETA_TILE_URL_OVERRIDE`, and the test and debug switches). The old names are not
  read any more.
- **Release artifacts**: `theta-<version>-linux-x86_64.tar.gz`, `Theta-<version>-linux-x86_64.AppImage`,
  `Theta-<version>-macos-<arch>.zip` (containing `Theta.app`) and `theta-<version>-windows-x86_64.zip` (`theta.exe`).
  The Linux install uses `theta.desktop`, the `theta` hicolor icon name and `~/.local/bin/theta`; uninstall the old
  `quarry` files separately.
- **macOS bundle id**: `dev.quarry.Quarry` is now `dev.theta.Theta`, so macOS treats it as a different app (its
  preferences and permissions do not carry over).
- The map tile `User-Agent` is now `Theta/<version> (+<repository url>)`; the demo stores are "Theta Coffee <City>"
  (`--demo` databases created earlier keep the old names).
- The local test databases keep the legacy name `quarry_demo` (and the MariaDB user `quarry`): renaming a live test
  database would only add churn. Key contexts and element ids moved from `Quarry…` to `Theta…`, which matters only to
  custom keymaps written against the previous alpha.

### Changed

- **App icon: the lowercase Greek letter theta**, drawn as geometry (a ring with a horizontal bar that overshoots it a
  little) in thin white lines on the same dark grey gradient tile. The ring is 0.77 as wide as it is tall, the glyph is
  54% of the tile height and the stroke 3% of it. The 32/48 px, 24 px and 16 px icons are separate drawings with every
  edge on a whole pixel (bar on a pixel row, counters at least 2 px at 16 px), and the in-app mark has a heavier stroke.
  `theta-light.svg` is the dark-on-light version; everything is regenerated by `packaging/icons/build.sh`, which now
  also writes `theta-24.svg`.
- The About page and the README explain the name once.

## [0.1.0-alpha.3] - 2026-11-11

A visual release: a new app icon and a darker text area in Quarry Dark. No behaviour changes.

### Changed

- **App icon: three stacked isometric slabs drawn as thin white lines on a dark grey gradient** (`#3b3d42` to
  `#18191b`, top-left to bottom-right, with a faint highlight along the top edge). It replaces the "Q" hexagon of
  0.1.0-alpha.2 and brings back the stacked blocks of the first icon, now as line art. Three detail levels keep the
  strokes at least one pixel wide: the full icon for 64 px and up, a simplified one with a heavier stroke and open-topped
  lower slabs for 32 and 48 px, and a minimal one (top rhombus and two chevrons) for 16 and 24 px. `quarry-light.svg`
  is the same drawing in dark lines on a light grey tile, and the glyph on the welcome view is redrawn with a wider
  gap between slabs so it stays open at about 40 px. Everything is regenerated by `packaging/icons/build.sh`.
- **Quarry Dark: the text area is now the darkest surface**, as in Zed. The editor, its gutter, the results grid
  rows, the plan view, the map panel and the active tab use `#21252b` (was `#282c33`); explorer, tab strips,
  toolbars, column headers and popups stay on the lighter panel colour `#2f343e`, and the title and status bars one
  step above that. The active line is a softer lift, dialogs (settings, connection, confirmations) sit on the popup
  colour so they stand out from the darker editor, and the grid toolbar and footer, the plan toolbar and the
  messages and history toolbars are now chrome rather than data.
- Data surfaces share one colour through `quarry_ui::data_surface()` (the theme's editor background, or its window
  background when it has none). Themes that define no separate panel colours look as before; Catppuccin Latte's
  grid and plan now use its lighter editor background.
- Comments, hints and line numbers are lighter in both Quarry themes: comments and hints reach 4:1 contrast and
  line numbers 3:1 (they were about 2.3:1 and 2:1 in Quarry Dark). Quarry Light already had the editor as its
  lightest surface and keeps its surface colours.

### Tests

- A test locks the Quarry Dark surface ladder (editor darker than panels, panels darker than the title and
  status bars) and the contrast minimums for text, muted text, line numbers and comments; a second one covers
  Quarry Light and a third checks that every bundled theme has a readable data surface.

## [0.1.0-alpha.2] - 2026-11-11

The second pre-release. It adds a Vim keymap to the editor and lets the map show all of a world-wide result.
It was run by hand on Linux (X11) like the first one; the macOS and Windows builds are still only compiled and
unit-tested in CI.

### Added

- **Vim keymap**, the third editor key map next to Helix (still the default) and Standard. A modal editor in the
  Vim tradition (Neovim where they differ): operators (`d c y < > = g~ gu gU g? gc`), counts, motions, text
  objects, registers (named, numbered, small delete, `"+` `"*`, `".` `":` `"/`), marks and the jumplist, `.` repeat,
  macros (`q`, `@`), Visual, Visual-line and Visual-block (with block `I` / `A`), Replace mode, `/` `?` search with
  Vim regex and `hlsearch`, and the ex line (`:s` `:g` `:normal` `:sort` `:m` `:t` `:d` `:y` and ranges). `space` is
  the leader and opens the same which-key menu as in Helix. About 3,900 engine tests, most of them generated from
  real Vim 9.1; 137 recorded cases that differ on purpose are listed with their reasons. Tables:
  [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#vim-key-map); scope and deviations (windows, folds, `:map`,
  Vimscript, digraphs, `gq`, tag objects and more are out of scope; options are fixed): the
  [engine README](crates/quarry-editor/README.md#vim-keymap) and the user guide.
  - Status bar chip per mode (`NORMAL`, `INSERT`, `VISUAL`, `V-LINE`, `V-BLOCK`, `REPLACE`, each in its own
    colour), half-typed commands next to it and `recording @a` while a macro is recorded.
  - The cursor style follows the mode for every key map: block (Normal, Visual, Helix Select), bar (Insert), underline
    (Vim Replace). Visual-block selections are outlined as one rectangle.
  - Insert mode shares the completion popup rules with Helix (`ctrl-n` / `ctrl-p` open it and move in it; the first
    `esc` closes the popup and the second leaves Insert mode). `=` formats the lines it covers; `:help` opens
    Settings › Keymap; an unknown `:set` option says so. `H` `M` `L` and the scroll commands follow what the
    window shows.
  - The Vim `ctrl-` letters belong to the editor while it has focus, so `mod-w` and friends do not reach the
    application there; see [Who wins a key](docs/KEYBINDINGS.md#who-wins-a-key). The ownership test now covers
    every Vim mode, half-typed commands, the prompt and the popup.
  - The grid, map, plan panel and explorer take their `h j k l` navigation in the Vim key map too.
- **`vim.clipboard_unnamed`** setting (Settings › Editor › *Vim: share the clipboard*, default on): yanks and deletes
  also go to the system clipboard and `p` pastes what another application copied, like `set clipboard=unnamedplus`.
  Off, only `"+` and `"*` use it. Engine option: `Editor::set_vim_clipboard_unnamed`.
- **Keymap picker.** *Select Keymap…* in the command palette, the gear menu and the View menu replaces *Toggle
  Keymap*; *Use Helix Keymap*, *Use Vim Keymap* and *Use Standard Keymap* switch directly. The settings dialog has
  three buttons, and the keymap reference there follows the active key map as soon as it changes.
- **Welcome view** offers the key map as a choice (Helix, Vim, Standard) and shows the key cheat sheet of the one
  selected.

### Changed

- **Map: *Fit* and zooming out show all the data.** The zoom floor was one world width, so in a short, wide panel
  (1320 × 240, say) world-wide points in the far north and south fell off the top and bottom. The floor is now
  where the world is 80% as tall as the panel (in a tall, narrow panel the world's width still limits it, so the
  whole longitude range stays reachable). Where the world is narrower than the panel the basemap repeats sideways
  with the repeats dimmed, and every feature is drawn exactly once on the copy centred on the camera; hover, click,
  selection, *Show on Map* and the measure tool all use that copy.
- New monochrome app icon: a white "Q" built from an isometric cube outline on a near-black tile, with a
  heavier variant for 16 and 24 px. The same glyph replaces the stand-in database icon on the welcome view.
  Every size, the macOS master and `quarry.ico` are regenerated by `packaging/icons/build.sh`.

### Fixed

- In the Vim key map `ctrl-shift-f`, `ctrl-shift-e`, `ctrl-shift-n` and `ctrl-shift-t` were taken for `ctrl-f` etc.
  (Vim ignores the case of a ctrl chord), which swallowed *Format Document*, *Focus Explorer*, *New Connection* and
  *Reopen Closed Tab*; they now reach the application.
- Vim `=` on some lines formatted the whole document; it formats the lines it covers.
- The welcome view no longer shifts the key map buttons when a shorter key sheet is chosen.

## [0.1.0-alpha.1] - 2026-11-11

The first release. It is an alpha: the application is complete and wired together, and was run by hand on
Linux (X11) against PostgreSQL 16 with PostGIS, MariaDB 10.11 and SQLite. The macOS and Windows builds are
compiled and unit-tested in CI but have not been exercised by hand. See *Known limitations* in the README.

### Added

Connections
- PostgreSQL (PostGIS-aware), MySQL / MariaDB and SQLite connections with TLS modes Disable, Prefer,
  Require and Verify full.
- SSH tunnels with key file (optional passphrase), password or ssh-agent authentication; host keys are
  checked against `~/.ssh/known_hosts` (trust on first use, a changed key is refused).
- Connection dialog with URL paste, *Test Connection*, *Save* and *Save & Connect*; read-only connections;
  an accent colour per connection; edit, duplicate and delete from the explorer.
- Passwords in the operating-system keychain, never in `connections.json`; in memory only when there is no
  keychain.
- Automatic reconnect when a statement fails because the session was lost (PostgreSQL class 08 and
  57P01/57P02/57P03, MySQL "server has gone away" / "lost connection", broken pipe, EOF): the first
  failing statement reconnects at once and a plain query outside a transaction is re-run once; anything
  else is reported as not re-run.
- Run and Explain on a console whose connection is down connect, wait and then run, and show the connection
  error in *Messages* when that fails.
- *Reopen Closed Tab* (`mod-shift-t`, palette, `:reopen`): the last 20 closed consoles (text, connection,
  cursor) are kept across restarts, so closing a tab never discards work.
- `--help`, `--version`; `--ssh` without `--ssh-key` authenticates with the ssh-agent; unknown flags print
  the usage and exit with status 2. `--demo` reuses an existing demo connection.
- Panics are logged to `<config dir>/logs/panic-*.log` and shown as a notice with the log path.
- Command-line flags: `--demo`, `--connect URL` (repeatable), `--run SQL`, `--script FILE`,
  `--ssh user@host[:port]` with `--ssh-key PATH`, `--theme NAME`, `--stress-catalog[=N]`, `--version`,
  `--help`; `QUARRY_DEMO=1`.
- A demo database (`--demo`): customers, orders, products, stores, delivery zones and routes in SQLite.

Key handling
- While the SQL editor has focus its engine's keys win over application shortcuts, derived from the engine
  itself and tested for both key maps and every mode: Helix Insert `ctrl-w` deletes a word instead of
  closing the tab, `ctrl-k` kills to the end of the line, `ctrl-b` pages up, `ctrl-o` is the jumplist,
  `ctrl-n` / `ctrl-p` move the completion selection, and `F5` runs all. *Refresh Schema* is `F5` in the
  explorer and `mod-alt-r` everywhere. The editor publishes `keymap`, `mode`, `prompt` and `completion` as
  key context.
- The results grid keeps its own chords (`mod-0` set NULL, `mod-enter`, ...), the explorer and pickers keep
  `ctrl-n` / `ctrl-p`.
- The editor scrolls horizontally to the caret after every edit, including typing at the end of a long
  line and text from the input method.
- Messages hide a meaningless error code (SQLite's "Unknown"); *Cancel Query*, *Commit Transaction* and
  *Roll Back Transaction* are named the same in menus, palette, tooltips and docs.

Connection dialog
- The database type is a dropdown (icon and label per type, keyboard navigable) driven by one descriptor per
  kind in `quarry-core`: label, icon, default port, URL schemes and which form sections apply. Adding a
  database kind needs no dialog change.

Map
- Basemap registry and picker (toolbar menu, Settings › Map): OpenStreetMap Standard (default),
  OpenTopoMap, None (graticule) and user-defined basemaps with `{s}` and `{r}` support, per-basemap
  attribution, max zoom (overzoom beyond it), dark flag and tile cache. The old `map_tile_url` setting is
  migrated. CARTO basemaps are not built in: their 2026 terms require an API key (see the user guide).
- A quieter scale bar (1px rule, end ticks, haloed label) in the bottom-right corner above the attribution.
- Hover cards, the context menu and the layers popover, and the plan flame-graph card, are drawn on the
  window overlay layer and clamped to the window, so a short bottom panel no longer clips them. Focus rings of
  switches in the connection and settings dialogs are no longer cut off at the edge of the scroll area.

Explorer
- Connections, schemas, tables, views, routines, sequences, columns, keys, foreign keys and indexes, with
  row estimates, a filter box and keyboard navigation.
- Foreign keys inferred from column names, shown as *Suggested* with a confidence, and a **Related
  Tables…** picker that lists declared and suggested keys and opens a `JOIN` query.
- Table context menu: *Open Table*, *Copy Name*, *Copy Qualified Name*, *Generate* `SELECT` / `INSERT` /
  `UPDATE` / `DELETE`, *Show DDL*.

Editor
- A modal Helix key map (counts, registers, macros, text objects, surround, multiple selections, regex
  selections, `:` command line, a which-key box for the space menu) and a Standard key map with Zed's
  default bindings.
- Context-aware completion for keywords, schemas, tables, columns, aliases, functions and snippets, with
  `JOIN … ON …` suggestions from declared and inferred foreign keys; auto-trigger after `FROM`, `JOIN`,
  `ON`, `INTO`, `UPDATE`, `TABLE`; Enter inserts a newline unless a suggestion was chosen.
- SQL formatter (indent width and keyword case are settings) and a diagnostics pass for syntax problems,
  `UPDATE`/`DELETE` without `WHERE` and unknown names. A failed statement is underlined at the server's
  error position.
- Hand-written SQL highlighter that copes with half-typed statements.
- Tabbed consoles that persist across restarts (text, cursor, title, connection, tab order), drag to
  reorder, *Open SQL File…* and *Save Console As…*.
- Editor and results font zoom (`mod-=`, `mod--`, `mod-0`).

Running SQL
- Run the statement under the cursor, the selection or the whole script; cancel a running query; one query
  at a time per connection; each result gets its own tab; a configurable row limit that stops the server.
- Transactions: *Begin Transaction*, *Commit Transaction* and *Roll Back Transaction*, a **TXN open** indicator with Commit and Roll Back
  buttons, and a prompt when closing a console or quitting with a transaction open.
- Confirmation before `UPDATE` / `DELETE` without `WHERE` and before *Explain Analyze* of a write.
- Messages tab with per-statement outcomes, notices and errors (code, detail, hint); History tab with
  search, up to 5,000 entries.

Results
- Virtualised results grid: type-aware sorting, filtering, rectangular selection, column resize and
  auto-fit, a value inspector, copy as TSV / JSON / `INSERT`.
- Editable results: stage cell edits, new rows and deletions, review the generated `UPDATE` / `INSERT` /
  `DELETE` statements, and apply them in one transaction.
- Export to CSV, TSV, JSON, Markdown or SQL `INSERT` statements for the whole result or the selection, to a
  file or the clipboard.
- Foreign-key columns are marked and can be followed to the referenced row (ctrl-click).

Plans and map
- *Explain* and *Explain Analyze* for PostgreSQL, MySQL/MariaDB and SQLite as a tree table, a flame graph
  and the raw output, with insights (sequential scans, bad row estimates, nested-loop rescans, spills,
  cartesian products, MySQL `filesort` and temporary tables, SQLite full scans) and suggested
  `CREATE INDEX` statements that open in a console.
- Map for geometry columns (WKB, EWKB, WKT, GeoJSON; WGS 84, Web-Mercator, UTM) on an OpenStreetMap
  basemap with an on-disk tile cache, or on a planar grid; layers, feature picking, measure tool, copy
  coordinates, GeoJSON export, and selection shared with the grid.

Look and feel
- 38 bundled themes (Quarry Dark, Quarry Light and 36 from 21 gpui-kit theme files), follow-the-system
  appearance, import of Zed theme files, IBM Plex Sans and Lilex fonts.
- Command palette, table, connection and theme pickers, a settings dialog (Appearance, Editor, Data, Map,
  Keymap, About) and a tolerant hand-editable `settings.json`.

Libraries
- `quarry-core` (the shared contract), `quarry-drivers`, `quarry-sql`, `quarry-geo`, `quarry-editor` as
  pure Rust crates; `quarry-ui`, `quarry-editor-view`, `quarry-grid`, `quarry-plan-view` and
  `quarry-map-view` as GPUI crates, each with a stand-alone demo.

Packaging and docs
- Application icon; Linux tarball with `install.sh` and an AppImage; macOS `.app` bundle (ad-hoc signed,
  not notarised); Windows zip with an embedded icon (unsigned); `SHA256SUMS`; a release workflow that
  creates a draft GitHub release, with a dry-run mode.
- Pre-release versions: `0.1.0-alpha.1` is used verbatim for artifact names, `--version` and the User-Agent;
  macOS bundle versions and Windows version resources use the numeric `0.1.0`. Re-running the release
  workflow for a tag replaces an existing draft release.
- README, user guide, key binding reference, architecture and development guides, contributing guide and a
  generated third-party licence list.

### Fixed

- Map: the world is never drawn more than once. Zoom-out stops where one copy of the world covers the
  viewport width (`log2(width / 256)`), "fit" centres world-spanning data at that zoom, and data on both
  sides of the antimeridian (lon 179 and -179) is framed along the short arc instead of across the globe.
- The database-type dropdown and the font selects filter as you type (their list delegate was a plain
  `Vec`, which ignores the query); typing "sq" lists only SQLite.
- Results, plans, map features, status and grid edits belong to the console that produced them. Each
  console has its own results grid (staged edits, sort and scroll survive switching tabs), an EXPLAIN or
  an edit-apply that finishes while another tab is active is stored with its console, and a console with
  a running statement shows "Running…" in its own panel.
- Helix: `ctrl-Home` / `ctrl-End` go to the start / last line in Normal and Select mode (and the document
  ends in Insert mode).
- Enter in a confirmation dialog (for example "Apply changes?") confirms its primary choice instead of
  silently closing it.
- Secondary (muted) text is pulled toward the foreground when a theme leaves it below 4:1 contrast.

### Known limitations

- No soft wrap or folding in the editor; the linter is heuristic.
- Curved geometry types are not decoded.
- MySQL 8 plan parsing is tested from fixtures only; live testing was against MariaDB 10.11.
- Without a keychain (a bare Linux session) passwords are not persisted.
- The macOS app is not notarised and the Windows build is unsigned. Neither platform has been exercised by
  hand.
- The AppImage does not bundle the Vulkan or Wayland libraries.
- The map basemap needs network access (cached tiles still draw offline).

[Unreleased]: https://github.com/JesseWilliamson/Theta/compare/v0.1.0-alpha.4...HEAD
[0.1.0-alpha.4]: https://github.com/JesseWilliamson/Theta/compare/v0.1.0-alpha.3...v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/JesseWilliamson/Theta/compare/v0.1.0-alpha.2...v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/JesseWilliamson/Theta/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/JesseWilliamson/Theta/releases/tag/v0.1.0-alpha.1
