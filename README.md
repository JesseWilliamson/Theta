<h1 align="center"><img src="packaging/icons/theta.svg" alt="" width="64" align="absmiddle"> Theta</h1>

Theta, a keyboard-first database IDE, is written in Rust on [GPUI](https://www.gpui.rs/) (through the
[`gpui-kit`](https://github.com/longbridge/gpui-kit) component crate). It connects to PostgreSQL,
MySQL/MariaDB, SQLite and (see [Supported databases](#supported-databases) for how far each is tested)
CockroachDB, Amazon Redshift, TimescaleDB, YugabyteDB, TiDB, DuckDB, SQL Server and ClickHouse, gives you a SQL editor with Helix, Vim or standard key bindings, and shows results
as an editable grid, as a query plan, or on a map. The name comes from the theta join (θ-join) of relational
algebra, the general form of JOIN: suggesting joins is what Theta is best at.

It is also a codebase meant to be read: the database drivers, SQL tooling, geometry code and the text
editor are plain Rust libraries with ordinary tests, and the UI crates are thin layers on top. See
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## What works today

Theta 0.1.0-alpha.8 is an alpha. The application is fully wired together: every feature below is
implemented and was run by hand on Linux (X11) against PostgreSQL 16 with PostGIS, MariaDB 10.11 and
SQLite. SSH tunnelling is covered by the driver integration tests only. The macOS and Windows builds are
compiled and unit-tested in CI but nobody has used them by hand yet. See
[Known limitations](#known-limitations).

<!-- screenshots -->
| | |
| --- | --- |
| ![Editor and results](docs/screenshots/01-editor-results.png) **Helix editor and results grid** | ![Completion](docs/screenshots/02-completion-fk-joins.png) **Completion with foreign-key join suggestions** |
| ![Map](docs/screenshots/03-map-stores-zones.png) **Geometry on a map** | ![Plan](docs/screenshots/04-plan-insights.png) **EXPLAIN ANALYZE with insights** |
| ![Editable grid](docs/screenshots/05-editable-grid.png) **Staged edits in the grid** | ![Explorer](docs/screenshots/06-explorer-suggested-fks-related.png) **Suggested foreign keys and related tables** |
| ![Light theme](docs/screenshots/07-theta-light.png) **Theta Light** | ![SSH](docs/screenshots/09-connection-ssh.png) **Connection dialog with SSH tunnel** |
| ![Vim visual block](docs/screenshots/10-vim-visual-block.png) **Vim keymap: a `V-BLOCK` selection** | ![Keymap choice](docs/screenshots/11-keymap-choice-welcome.png) **Choose Helix, Vim or Standard on the welcome view** |
| ![Record view](docs/screenshots/12-record-view.png) **Record view: one row as a list, JSON pretty-printed** | ![Query parameters](docs/screenshots/13-query-parameters.png) **Query parameters: values asked for before the run** |
| ![Import](docs/screenshots/14-import-csv.png) **CSV import: detected format, column mapping, sample values and type checks** | ![Modify table](docs/screenshots/15-modify-table.png) **Modify Table: a rename, an added column and a dropped one, with the exact SQL and its warnings** |
| ![New table](docs/screenshots/16-new-table.png) **New Table: columns, a foreign key and the `CREATE TABLE` it makes** | ![Sessions](docs/screenshots/17-sessions.png) **Sessions: a blocked session, its blocker and Cancel Query** |
| ![Roles](docs/screenshots/18-roles.png) **Roles: attributes, memberships and privileges by object** | |

## Supported databases

How each product is connected, and **what has actually been run against a real server**. "Live locally" means
the driver's integration tests and the application were run against that server on the development machine;
"live in CI" means the driver's live test (connect, typed values, introspection, EXPLAIN, row limit, cancel,
errors) passes against a service container in `.github/workflows/ci.yml`, but nobody has driven the app by hand
against that product; "unverified" means no server of that product has been used at all (the parsers are unit-tested
on text from the vendor's documentation, which is weaker evidence than captured output).

| Database | Driver | Verified | Notes |
| --- | --- | --- | --- |
| PostgreSQL (PostGIS) | `tokio-postgres` | live locally (16 with PostGIS 3.4) and in CI | the reference implementation |
| MySQL / MariaDB | `mysql_async` | live locally (MariaDB 10.11) and in CI; MySQL 8 plans from fixtures only | |
| SQLite | `rusqlite` (bundled) | live locally | |
| DuckDB | `duckdb` (bundled, cargo feature `duckdb`, default on) | live locally (DuckDB 2.0.0 via duckdb-rs 1.20000.1), in memory and file | no extension downloads, so no spatial functions; adds about 55 MB to the binary and 15 minutes to a clean build |
| ClickHouse | HTTP (`ureq`, rustls) | live locally against ClickHouse 18.16.1 (the version Ubuntu packages): connect, typed values, introspection, row limit, cancel, errors, sessions; modern types and `EXPLAIN` not run locally; also passes in CI against ClickHouse 24.8 (typed values including DateTime64, IPv4/IPv6 and Map) | read-only grid |
| CockroachDB | PostgreSQL driver | live in CI against CockroachDB v24.3.4 (not run by hand in the app) | its own EXPLAIN parser |
| TimescaleDB | PostgreSQL driver | live in CI against the TimescaleDB image (not run by hand in the app) | hypertables listed |
| TiDB | MySQL driver | live in CI against TiDB v8.1.1 (not run by hand in the app) | its own EXPLAIN parser |
| SQL Server | `tiberius` (rustls) | live in CI against SQL Server 2022 (not run by hand in the app; sessions, roles and spatial values are not covered by that test) | SQL authentication only; showplan XML parser unit-tested on a hand-written document |
| Amazon Redshift | PostgreSQL driver | unverified | own introspection queries from the documented views; no CI service exists |
| YugabyteDB | PostgreSQL driver | unverified | no CI service exists |

Per-product behaviour (what is disabled and why) is in the [user guide](docs/USER_GUIDE.md#other-databases).

## Features

**Connections**
- PostgreSQL (PostGIS-aware), MySQL / MariaDB, SQLite, CockroachDB, Amazon Redshift, TimescaleDB, YugabyteDB,
  TiDB, DuckDB (a file or in memory), SQL Server and ClickHouse (over HTTP). TLS modes: Disable, Prefer, Require,
  Verify full (SQL Server and ClickHouse word them as Encryption and Protocol). A feature a product cannot do
  (EXPLAIN ANALYZE on Redshift, row editing on ClickHouse...) says so instead of sending SQL that fails, and a
  connection set up as the wrong product is warned about on connect.
- SSH tunnels with a private key file (optionally with a passphrase), a password, or ssh-agent. Host keys
  are checked against `~/.ssh/known_hosts`: an unknown host is recorded on first use, a changed key is
  refused.
- Passwords go to the operating-system keychain and never to `connections.json`. Without a keychain (for
  example a bare Linux session) they are kept in memory for the running session only, and the connection
  dialog says so.
- A connection can be marked read-only and given an accent colour (red, orange, green, blue or purple).
  The colour shows in the title bar and the explorer.
- When a statement fails because the session was lost, Theta reconnects at once and, for a plain query
  outside a transaction, re-runs it once ("Connection lost — reconnected and re-ran the statement").
  Other statements are not re-run and *Messages* says so. Open transactions are lost with the session.
- Start-up flags open connections from the command line (see [Quick start](#quick-start)).

**Explorer**
- Connections, schemas, tables, views, routines and sequences, with columns, keys, foreign keys and
  indexes under each table, a row estimate on every table, and a filter box.
- Foreign keys that the database does not declare are *suggested* from column names (`customer_id`,
  `customerId`) with a confidence percentage. They appear under *Foreign Keys › Suggested* and in the
  **Related Tables…** picker, which lists both kinds and opens a ready-made `JOIN` query.
- Connection folders (one level deep) and manual ordering: *New Folder*, *Move to Folder…*, *Move Up / Down*;
  collapsed state and order are saved, and the connection picker shows `Folder / Name`.
- Context menu on a table: *Open Table*, *Copy Name*, *Copy Qualified Name*, *Generate* (`SELECT`, `INSERT`,
  `UPDATE`, `DELETE`), *Show DDL*, *Related Tables…*, *Modify Table…*, *Rename…*, *Duplicate Structure…*,
  *Truncate…*, *Drop…*, *Import Data…*, *Export Data…*; on the Tables group *New Table…*; on a column *Modify
  Column…*, *Rename Column…*, *Drop Column…*.

**Data in and out**
- *Import Data* from CSV / TSV into an existing table (columns mapped by name, per-column skip, type checks) or as
  a new table (types inferred, `CREATE TABLE` shown): streaming parser (quoted newlines, any delimiter, UTF-8 /
  UTF-16 / Latin-1), a live preview, **Validate** with line numbers, a progress bar, Cancel that rolls back, one
  transaction (or a savepoint in your manual-commit transaction), skip-bad-rows or abort. About 70k rows/s into
  PostgreSQL, 100k into MariaDB and 230k into SQLite on a developer machine (see the changelog).
- *Export Data*: a whole table as CSV, TSV, JSON or SQL INSERTs, paged by primary key with constant memory.
- **Modify Table / New Table editor** (a tab): columns (name, type with the dialect's suggestions, nullable, default,
  primary key with its order, identity / auto-increment, comment), primary and unique keys, foreign keys (referenced
  table and columns from the catalog, `ON DELETE` / `ON UPDATE`, deferrable) with one-click *suggested* keys, indexes
  (columns in order, unique, method, partial `WHERE`), checks, table comment, and the MySQL engine / charset /
  collation read-only. Every edit re-runs the DDL engine and shows the script beside the grid: one block per
  statement with badges (destructive, table rewrite, lock, implicit commit, outside transaction, rebuild), the
  engine's explanation, SQL highlighting, and validation problems with *Fix* buttons. A removed column stays in the
  grid, struck through, until Execute. Execute confirms the exact SQL, runs it in one transaction (PostgreSQL,
  SQLite), one statement at a time on MySQL / MariaDB (said plainly), or inside your open transaction in manual
  mode, and marks the failing statement with the server's error while keeping your edits. Fully keyboard
  operable in the Helix, Vim and Standard key maps; open it from the explorer (*New Table…*, *Modify Table…*,
  *Modify Column…*, *New Index…*, *New Foreign Key…*, *Add as Foreign Key…*), the palette, `:table new` /
  `:table modify NAME`, or `theta --modify TABLE` / `--new-table`.
- **Server administration** (PostgreSQL, MySQL / MariaDB, SQL Server, ClickHouse as stated): a *Sessions* tab with a
  live, filterable table of server sessions, blocked sessions shown with their blocker, and Cancel Query /
  Terminate Session with the exact SQL confirmed first; a *Roles* tab with attributes, memberships and privileges
  by object, and create / alter / set password / grant / revoke / drop through preview-and-confirm forms (the
  password is never logged); *Export Database…*, Theta's own streaming SQL dump (and `pg_dump` / `mysqldump`
  when installed, with the exact command), and *Run SQL Script…* for large files with progress, error policy,
  single-transaction and cancel. Keyboard-driven in all three keymaps; what each product cannot do says why.
  What the built-in dump leaves out is listed in the dialog and the user guide.
- *Rename Table*, *Rename Column*, *Drop Column*, *Drop Index*, *Drop Foreign Key*, *Duplicate Structure*,
  *Truncate* and *Drop* from the explorer, each confirmed with the exact SQL; Drop lists the foreign keys that
  point at the table.

**Editor**
- Three complete key maps: **Helix** (modal, selection-first, multiple selections, registers, macros, text
  objects, surround; the default), **Vim** (modal, verb-then-motion: operators, text objects, registers, marks,
  macros, `.` repeat, Visual / Visual-line / Visual-block, `:s` `:g` `:normal` and the rest of the ex line,
  verified against real Vim by about 3,900 tests) and a **Standard** non-modal map that follows Zed's default
  bindings. Choose on the welcome view, with *Select Keymap…* in the command palette or in Settings. Vim
  yanks and deletes share the system clipboard (setting `vim.clipboard_unnamed`). Tables are in
  [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).
- Tabbed consoles. Tabs can be dragged to reorder. A console keeps its text, cursor, title, connection and
  tab position across restarts.
- Completion for keywords, schemas, tables, columns, aliases, functions and snippets, with dialect-specific
  function lists. After `JOIN` it offers whole `JOIN … ON …` clauses built from declared and suggested
  foreign keys.
- A formatter (indent width and keyword case are settings) and a statement splitter that understands
  comments, quoting and dollar-quoted bodies, so "run statement under the cursor" works in scripts.
- A real SQL parser (hand-written, error-tolerant, all six dialects) and code intelligence built on it:
  diagnostics (syntax errors, unknown table or column, ambiguous column, undefined or duplicate alias,
  column-count mismatch, ungrouped column) inline and in a Problems tab; quick fixes and refactorings (did you
  mean, qualify, expand `*`, add alias, add `GROUP BY`, comma join to `JOIN`, flip `JOIN`, extract / inline CTE,
  keyword case); go to definition, find usages and occurrence highlighting; rename of aliases, CTEs and
  select aliases. All keyboard-driven in the Helix, Vim and Standard key maps.
- Hand-written SQL highlighter that copes with half-typed statements.
- **Live templates**: `sel`, `join`, `cte`, `crt` and a dozen more expand with Tab into snippets with tab stops,
  linked placeholders (typing in one edits the mirrors), and variables (`${table}` asks which table, `${columns}`,
  `${selection}`, `${clipboard}`, `${date}`). Add your own in `templates.json`; Settings › Templates lists them all.
  *Surround Selection with…* wraps SQL in `SELECT count(*) FROM (…) t`, a CTE, `EXISTS (…)` and so on.
- Zoom (`mod-=`, `mod--`, `mod-0`) changes the editor and results font size only. The rest of the interface
  keeps its size.

**Running SQL**
- Run the statement under the cursor, the selection, or the whole script. A script stops at its first error.
  Each statement that returns rows gets its own result tab.
- One query runs at a time per connection; a second run is refused until the first finishes or is
  cancelled. Cancel asks the server to abort the statement.
- Row limit (default 1,000, a setting): the server stops producing rows at the limit.
- Transaction controls: *Begin Transaction*, *Commit Transaction*, *Roll Back Transaction*. While a
  transaction is open the title bar shows **TXN open** with Commit and Roll Back buttons, and closing the
  tab or quitting asks what to do with it.
- A confirmation dialog before `UPDATE` or `DELETE` without `WHERE`, and before *Explain Analyze* of a
  statement that writes.
- Read-only connections refuse statements that write data or change the schema.
- Statement history (up to 5,000 entries), searchable, in the History tab.
- **Query parameters**: `?`, `$1`, `:name` and `@name` ask for values (typed: Auto, Text, Number, Boolean, NULL,
  Raw SQL) before Run, Run All and Explain; values are remembered per console and substituted client-side as
  escaped literals. Strings, comments, `::casts`, jsonb `?` operators and MySQL user variables are left alone.
- **Transaction mode and isolation level** per connection and per session: auto-commit, or manual (Theta
  starts the transaction lazily and keeps it open) with Read Committed / Repeatable Read / Serializable
  (PostgreSQL, MySQL; Read Uncommitted too on MySQL) or SQLite's `BEGIN` modes. The title-bar chip reads
  `Manual · TXN open · Repeatable Read` and reflects what the server says, including MySQL's implicit commits.

**Results**
- Virtualised grid (100,000 rows by 60 columns stays interactive): type-aware sorting, filtering, rectangular
  cell selection, column resizing and auto-fit, a cell viewer that follows the cursor (pretty JSON with folding, wrapped text, hex dumps, PNG / JPEG / GIF / WebP pictures, geometry), and
  copy as TSV, JSON or `INSERT`.
- Editing: change cells, add and delete rows, review the staged changes, then submit them. Theta shows the
  generated `UPDATE`/`INSERT`/`DELETE` statements and applies them in one transaction. A result is editable
  only when it comes from one table whose primary key is in the result; otherwise the grid shows a lock and
  says why.
- Export to CSV, TSV, JSON, Markdown or SQL `INSERT` statements, for the whole result or the selection, to a
  file or the clipboard.
- **Record view** (one row as a list of fields: wrapped values, pretty JSON, editable in place, row navigation,
  virtualised for 500 columns) and a **selection statistics** popover (count, distinct, nulls, min, max, sum,
  average, text lengths; computed off the UI thread for large selections).
- Foreign-key columns carry a link icon; ctrl-click (cmd-click on macOS) opens the referenced row in a new
  console.

**Query plans**
- *Explain* and *Explain Analyze* for PostgreSQL, MySQL/MariaDB and SQLite, shown as a tree table, a flame
  graph, or the raw server output.
- Insights written for the specific plan: sequential scans with selective filters, row estimates that are
  badly off, nested-loop rescans, sorts and hashes that spill to disk, cartesian products, MySQL
  `filesort`/temporary tables/full scans, SQLite full scans and temporary B-trees. When an index would help,
  the insight carries a `CREATE INDEX` statement and an **Open in Console** button.

**Map**
- Draws the geometry columns of a result on an OpenStreetMap basemap, or on a plain planar grid when the
  coordinate system is unknown. Decodes WKB, EWKB, WKT and GeoJSON; understands WGS 84, Web-Mercator and
  UTM SRIDs.
- Layers, feature picking that shows the row's other columns, a measure tool (distance and area), copy
  coordinates / WKT, GeoJSON export.
- Selection is shared with the grid: clicking a feature selects its row, and *Show on Map* in the grid
  selects the feature.
- *Fit* and zooming out show all of the data, also world-wide points in a short, wide panel: the map zooms
  out until the world is 80% as tall as the panel, the basemap repeats sideways with the repeats dimmed, and each
  feature is drawn exactly once (hover, click and measure use that one copy).

**Look and feel**
- 38 bundled themes (Theta Dark, Theta Light and 36 from 21 gpui-kit theme files), follow-the-system
  appearance, and import of Zed theme files. Fonts: IBM Plex Sans for the interface, Lilex for code.
- Command palette, table picker, connection picker, a keymap picker, a settings dialog, and a which-key box for the
  Helix and Vim space menu.

## Install

Releases are published on the [GitHub Releases](https://github.com/JesseWilliamson/Theta/releases) page.
The release workflow ([`.github/workflows/release.yml`](.github/workflows/release.yml)) builds these files
from a `v*` tag and attaches them to a draft release that the maintainer publishes by hand:

| Platform | File | Install |
| --- | --- | --- |
| Linux x86_64 | `theta-<version>-linux-x86_64.tar.gz` | extract and run `./install.sh` (installs to `~/.local`) |
| Linux x86_64 | `Theta-<version>-linux-x86_64.AppImage` | `chmod +x` and run (built only when `appimagetool` could be downloaded) |
| macOS Apple Silicon | `Theta-<version>-macos-arm64.zip` | unzip, drag `Theta.app` to Applications |
| macOS Intel | `Theta-<version>-macos-x86_64.zip` | unzip, drag `Theta.app` to Applications |
| Windows x86_64 | `theta-<version>-windows-x86_64.zip` | unzip and run `theta.exe` |

Every release also has a `SHA256SUMS` file.

- **macOS**: the app is ad-hoc signed but **not notarised**, so Gatekeeper blocks the first launch.
  Right-click the app, choose *Open*, then *Open* again; or run
  `xattr -dr com.apple.quarantine /Applications/Theta.app`.
- **Windows**: the executable is not code-signed, so SmartScreen warns on first launch (*More info*, then
  *Run anyway*).
- **Linux** needs a working Vulkan driver (Mesa `libvulkan_radeon`/`intel`/`lvp`, or the proprietary one)
  plus `libxkbcommon`, `libwayland-client`, `libxcb` and `fontconfig`. The AppImage does not bundle them;
  it uses the ones on the host, exactly like the tarball. See the
  [troubleshooting](docs/USER_GUIDE.md#troubleshooting) section.

## Build from source

You need a recent stable Rust toolchain and a C compiler. [`rust-toolchain.toml`](rust-toolchain.toml) says
`stable`; the CI and release workflows pin **1.99.1** (`RUST_TOOLCHAIN` in `.github/workflows/*.yml`), and
GPUI's macOS backend needs 1.99 or newer, so on macOS use at least that. Building the GPUI application takes
several minutes and a few GB of disk.

```sh
# Linux (Debian/Ubuntu package names, as used by CI)
sudo apt-get install clang mold libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb1-dev \
  libxcb-xkb-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig-dev \
  libasound2-dev libssl-dev libzstd-dev libvulkan1 mesa-vulkan-drivers

# macOS: full Xcode (GPUI's build script compiles Metal shaders with `xcrun metal`), plus its command line tools
xcode-select --install

# Windows: Visual Studio Build Tools with the "Desktop development with C++" workload
# (MSVC + Windows 10/11 SDK)

cargo run --release -p theta -- --demo
```

`mold` is optional; CI uses it through `-C link-arg=-fuse-ld=mold` to link faster (the release workflow
does not). The libraries (`theta-core`, `theta-sql`, `theta-geo`, `theta-editor`, `theta-drivers`) build
and test without any of the GUI packages: `cargo test -p theta-sql`.

## Quick start

```sh
theta --demo
```

`--demo` creates a SQLite database with sample customers, orders, products, stores, delivery zones and
routes in the configuration directory, saves it as the connection *Demo (SQLite)* and connects to it.

In the editor (Helix key map by default): press `i`, type `select * from customers`, then press
`ctrl-enter` (`cmd-enter` on macOS). `ctrl-enter` runs the statement under the cursor from any mode. In
Normal mode `space r` does the same. `space ?` opens the command palette.

A few queries to try on the demo database:

```sql
select c.first_name, count(*) as orders
from customers c join orders o on o.customer_id = c.id
group by c.id order by orders desc;

select name, city, location_wkt from stores;   -- then open the Map tab
```

Run the second one and open the **Map** tab: the WKT text column is drawn as 25 points. With the first one,
press `ctrl-e` to explain it. The [user guide](docs/USER_GUIDE.md) walks through the rest.

Command-line flags:

| Flag | Meaning |
| --- | --- |
| `--demo` | create and connect the demo database (`THETA_DEMO=1` does the same) |
| `--connect URL` | open a temporary connection; repeatable; `postgres://`, `mysql://`, `mariadb://` or `sqlite://` URLs |
| `--run SQL` | run this SQL (every statement) once the first start-up connection is ready |
| `--script FILE` | the same, from a file |
| `--ssh user@host[:port]` | tunnel the `--connect` connections through SSH; without `--ssh-key` the running ssh-agent authenticates (Theta prints which) |
| `--ssh-key PATH` | authenticate the `--ssh` tunnel with this private key file |
| `--theme NAME` | start with this theme |
| `--stress-catalog[=N]` | debug: add a synthetic connection with N (default 5,000) empty tables |
| `--version`, `-V` | print the version and exit |
| `--help`, `-h` | list every flag and exit; an unknown flag prints the same text and exits with status 2 |

## Configuration

Theta keeps its files in one directory (override it with `THETA_CONFIG_DIR`):

| Platform | Default location |
| --- | --- |
| Linux | `~/.config/theta/` (`$XDG_CONFIG_HOME/theta`) |
| macOS | `~/Library/Application Support/theta/` |
| Windows | `%APPDATA%\theta\` |

| File | Contents |
| --- | --- |
| `settings.json` | theme, key map (Helix, Vim or Standard) and `vim.clipboard_unnamed`, fonts, row limit, map tile URL, formatter options (hand-editable; a bad key is ignored with a warning) |
| `connections.json` | saved connections, without passwords |
| `themes/*.json` | imported or hand-written themes |
| `history.jsonl` | statement history |
| `consoles/` | one JSON file per open console |
| `workspace.json` | panel sizes and visibility, written when you change them |
| `demo.sqlite` | the `--demo` database |

Map tiles are cached under the platform cache directory (`~/.cache/theta/tiles` on Linux;
`THETA_CACHE_DIR` overrides it). The full settings reference is in the
[user guide](docs/USER_GUIDE.md#settings).

## Key bindings

`mod` is `ctrl` (Linux, Windows) or `cmd` (macOS). The complete tables are in
[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md), including the keys that the application binds globally and
that therefore win over the same keys in the editor.

**Anywhere**

| Keys | Action |
| --- | --- |
| `mod-shift-p`, `F1` | command palette |
| `mod-p` | go to table |
| `mod-alt-c` | switch connection |
| `mod-enter` / `mod-shift-enter` | run statement / run all |
| `mod-.` | cancel the running query |
| `mod-e` / `mod-shift-alt-e` | explain / explain analyze |
| `mod-shift-f` | format the document |
| `mod-b` / `mod-j` | toggle explorer / bottom panel |
| `mod-1` / `mod-2` | focus editor / results |
| `mod-n` / `mod-w` | new console / close tab |
| `mod-,` | settings |
| `F5` | refresh the schema |

**Vim key map**

The complete tables (and the `space` leader menu that reaches every application action) are in
[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#vim-key-map).

| Keys | Action |
| --- | --- |
| `h j k l`, `w b e`, `0 ^ $`, `gg G`, `{ }` | motions |
| `d c y` + motion or text object | `dw`, `ciw`, `di(`, `yap`, `>ip`, `gUiw`, `gcc` |
| `v` `V` `ctrl-v` | Visual, Visual-line, Visual-block (`I` / `A` insert on every line) |
| `p P`, `"a` `"+` | put, registers and the system clipboard |
| `u` `ctrl-r`, `.`, `qa` `@a` | undo / redo, repeat, macros |
| `/ ? n N * #`, `:s/a/b/g`, `:g/pat/d` | search and ex commands |
| `space r`, `space e`, `space f`, `space ?` | run, explain, tables, command palette |
| `:w :q :run :format :goto N :help` | application commands |

**Helix key map (default)**

| Keys | Action |
| --- | --- |
| `h j k l`, `w b e` | move by character, line, word |
| `x`, `%`, `v` | select line, select all, Select mode |
| `i a o` / `I A O` | insert, append, open line / at line start, end, above |
| `d c y p` | delete, change, yank, paste |
| `u` / `U`, `.` | undo / redo, repeat last change |
| `s`, `S`, `K` | select / split / keep matches of a regex |
| `/ ? n N *` | search |
| `mi( ma( ms) md( mr(` | text objects and surround |
| `space r`, `space R` | run statement, run all |
| `space e`, `space E` | explain, explain analyze |
| `space f`, `space b`, `space ?` | tables, connections, command palette |
| `:w :q :format :goto N` | command line |

**Standard key map**

| Keys | Action |
| --- | --- |
| `ctrl-z`, `ctrl-shift-z` | undo, redo |
| `ctrl-d`, `ctrl-shift-l` | add next occurrence, select all occurrences |
| `ctrl-/` | toggle `--` comment |
| `alt-up/down` | move lines |
| `ctrl-shift-k` | delete line |
| `ctrl-f`, `ctrl-g` | find, go to line |
| `ctrl-space` | completion |

## Known limitations

Databases added in 0.1.0-alpha.7
- Only DuckDB and (against a 2018 server) ClickHouse have been run. SQL Server, CockroachDB, TiDB and TimescaleDB
  are covered by live tests that exist but have not been run yet; Redshift and YugabyteDB have no test server at
  all. Read the table under [Supported databases](#supported-databases) before trusting a product.
- SQL Server: SQL authentication only; cancelling a statement ends the session (temp tables and an open
  transaction are lost); a result with several result sets shows the first; the table editor and CSV import are
  disabled. ClickHouse: read-only grid, no transactions, no *Explain Analyze*. DuckDB: no row editing in the grid,
  no table editor, no CSV import, no extensions. Amazon Redshift: no *Explain Analyze*, table editor or import.
  A TLS-verified ClickHouse connection through an SSH tunnel checks the certificate against the tunnel's local
  address (127.0.0.1) and fails; use *Require* there.
- A DuckDB build adds about 55 MB to the binary and about 15 minutes to a clean build; `cargo build
  --no-default-features` leaves it out.

Server administration (Sessions, Roles, Export Database, Run SQL Script)
- Verified live on PostgreSQL 16 (sessions with a blocked session and Cancel Query, every role action, dump and
  restore), MariaDB 10.11 (sessions with InnoDB lock waits and Cancel Query, create / alter / password / drop of a
  user, dump and restore) and ClickHouse 18.16 (sessions and Cancel Query). On MariaDB, *grant* and *revoke*
  privileges could not be run end to end because the test account has no `GRANT OPTION`: the statements are
  unit-tested and the server's refusal is shown as it arrives. SQL Server sessions and logins (and everything
  with `sys.dm_*` / `sys.server_*`), MySQL 8's lock-wait and role queries, TimescaleDB, and `pg_dump` /
  `mysqldump` runs through the dialog are unverified: unit tests only (the `pg_dump` / `mysqldump` command
  builder is unit-tested; the dialog has not been run with the tools).
- Sessions and Roles are off, with a reason, on SQLite, DuckDB, CockroachDB, Redshift, YugabyteDB and TiDB;
  ClickHouse has no Roles. The built-in dump is off on SQL Server, ClickHouse, CockroachDB, Redshift, YugabyteDB and
  TiDB. SQL Server cannot cancel a query without ending its session.
- The built-in dump is not a backup: no routines, triggers, extensions, custom types, ownership or grants, and
  its view definitions are as the server prints them (a MariaDB view body names its database). Restoring does
  not support `COPY … FROM stdin`, `DELIMITER` or `/*! … */`. Roles lists explicit grants only. The Sessions list
  has no horizontal scroll: below 1250 px wide the Database, Client and Application columns move to the detail pane.

Editor
- No soft wrap and no code folding. Long lines scroll horizontally.
- Diagnostics stay quiet rather than flag valid SQL, so they miss errors: against real servers the parser flags
  52-65% of the statements the engine rejects for syntax and the resolver 74-88% of the unknown-name errors
  (see [ARCHITECTURE](docs/ARCHITECTURE.md)). Name checks need a loaded catalog and are silent for schemas that
  are not loaded. DDL and anything outside queries and DML is parsed shallowly (only the names it defines or
  uses); routine bodies are opaque. T-SQL and ClickHouse were checked against documentation examples only.
  The database remains the judge: a failed statement is underlined from the server's error position.
- Rename covers aliases, CTEs and select aliases in one statement. Renaming a table or column of the database is
  not offered. Usages of a catalog object are searched in open consoles only, not in saved files that are closed.
- In the editor the engine's keys win over the application's shortcuts, so a few application chords
  (`ctrl-b`, `ctrl-o`, `ctrl-w`, `ctrl-k`, `F5`) do something else while the editor has focus in the Helix key
  map, and in the Vim key map the `ctrl-` letters Vim uses (`ctrl-w`, `ctrl-b`, `ctrl-e`, `ctrl-n`, `ctrl-o`,
  `ctrl-p` ...) are the editor's as well; the actions stay in the palette and space menu. See
  [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#who-wins-a-key).
- The Vim key map is not Vim. Out of scope: Vimscript, `:map` and friends, windows and tabs (`ctrl-w`, `:split`),
  folds, digraphs and literal insert, spell checking, tags, `gq` / `gw`, the change list (`g;`), `gn`, the
  tag objects, `:!` shell commands, and back-references in `:s` patterns. `space` is the leader (it no longer
  moves right), the options are fixed (`shiftwidth=4`, `expandtab`, `ignorecase` with `smartcase`), and
  typing `(` inserts `()`. The full list is in the
  [engine README](crates/theta-editor/README.md#deviations-from-vim).
- Undo is linear (no undo tree).

Import, export and DDL
- The table editor changes tables, not views, routines or sequences. It cannot reorder the columns of an existing
  table (no dialect's `ALTER TABLE` does; reordering is available while a table is new), edit generated columns
  (they are shown and kept) or author an expression index (one is shown with its definition and can be kept or
  dropped; write new ones in a console). Foreign keys on MySQL / MariaDB cannot change the type of a column they
  use: the editor says so before Execute instead of letting the server refuse. SQLite changes that need a table
  rebuild copy every row (and the confirmation says so); the table's triggers are read first and recreated
  afterwards. The tab's edits are not saved across a restart.
- Import does not use PostgreSQL `COPY`; it sends batched multi-row `INSERT`s (about 70k rows/s). TSV files whose
  values use backslash escapes (`\t`, `\n`) are read literally. Browse… and Open use the system file dialog, which
  has not been tried on macOS or Windows; the path field always works.
- MySQL commits DDL implicitly, so *Rename / Truncate / Drop* and a multi-statement table edit there cannot be rolled
  back (the editor names the statements that were already applied if one fails), and importing into a new
  table commits the `CREATE TABLE` (a failed import drops it again).

Databases
- Read-only connections are enforced by Theta, which classifies each statement before it runs; it refuses
  data changes and schema changes. It is not a server-side guarantee: a `SELECT` that calls a function with
  side effects, or a `CALL`, runs. Use a read-only database account when that matters.
- Statements on one connection run one at a time, and all consoles bound to a connection share its single
  server session (and its transaction).
- MySQL 8 `EXPLAIN` parsing is tested from captured fixtures only. Live testing was against MariaDB 10.11.
- MySQL/MariaDB: `execute` refuses text that holds a second statement; scripts are split before they are sent.

Results and map
- Curved geometry types (`CIRCULARSTRING`, `COMPOUNDCURVE`, `CURVEPOLYGON`) are not decoded and are not
  drawn. Z and M ordinates are read but dropped.
- The basemap needs network access to the tile server. Tiles that were fetched before are cached on disk;
  without network the geometry is still drawn, on a grey background.
- Results are not saved across restarts (console text is).
- Exporting a result grid covers the rows that were loaded into it, which the row limit caps; *Export Data…* on a
  table is not limited.
- The grid cannot reorder or hide columns.

Added in 0.1.0-alpha.5
- Query parameters are substituted by Theta, not bound by the server: they stand where a literal can, and a
  parameter for an identifier needs the *Raw SQL* type.
- Connection folders are one level deep and have no drag and drop (the tree list has no drop target); use the
  menus, `alt-up` / `alt-down` and the palette.
- In manual transaction mode a plain `SELECT` also starts a transaction (as JDBC does); an open transaction on
  a busy production database holds locks and a snapshot until you end it.
- CSV / TSV import, whole-table export and the Modify Table / New Table editor arrived in 0.1.0-alpha.6 (see
  Import, export and DDL above for what the editor does not do).

Platform and packaging
- Without a keychain (a bare Linux session) passwords are not persisted. Saved connections open without
  their password, and the connection dialog says "Password will not be saved".
- Saved connections are not opened at start-up; open one from the explorer or run a statement in its console.
- The macOS app is ad-hoc signed and not notarised. The Windows build is unsigned.
- The macOS and Windows builds are compiled and unit-tested in CI, but nobody has exercised them by hand.
  Only X11 has been tested on Linux; Wayland has not.
- The AppImage does not bundle the Vulkan, Wayland, xkbcommon and fontconfig libraries.

## Architecture

```text
crates/
  theta-core         contracts: Value, ResultSet, Catalog, Plan, ConnectionConfig, Driver/Connection traits
  theta-drivers      PostgreSQL, MySQL/MariaDB, SQLite and SSH tunnelling (Tokio)
  theta-sql          splitting, formatting, completion, FK joins, plan insights, lint, SQL generation, export
  theta-geo          WKB/EWKB/WKT/GeoJSON, projections, tile maths, measuring
  theta-editor       text buffer, selections, Helix and Standard keymaps, SQL highlighting (no GPUI)
  theta-ui           theme, settings, fonts and icons, pickers, small shared widgets
  theta-editor-view  GPUI widget that paints theta-editor
  theta-grid         results grid with editing, filtering, export requests
  theta-plan-view    EXPLAIN tree, flame graph, raw view, insights
  theta-map-view     slippy map with tile loader and cache
  theta              the application: workspace, explorer, connections, dialogs, command palette
```

The rule that shapes everything: whatever can be tested without a window lives outside the GUI crates,
and the UI thread never waits on I/O. Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the
reasoning and [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for how the pieces fit.

## Development

```sh
cargo test -p theta-core -p theta-sql -p theta-geo -p theta-editor   # pure crates, seconds
cargo test -p theta-drivers                                             # needs the test databases
cargo test --workspace --exclude theta-drivers                          # everything else
cargo run -p theta-grid --example grid_demo                             # each view crate has a demo
script/shot /tmp/shots/main.png -- --demo                                # headless screenshot (Linux)
```

Test databases: PostgreSQL 16 with PostGIS at `postgres://postgres:postgres@127.0.0.1:5432/quarry_demo`
and MariaDB 10.11 at `mysql://quarry:quarry@127.0.0.1:3306/quarry_demo` (the name predates the rename to Theta and
is kept so the existing databases keep working); the CI workflow starts them as
service containers. [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) covers the details, including the
screenshot harness, adding a database driver, adding a theme, and the CI and release process.
[CONTRIBUTING.md](CONTRIBUTING.md) has the short version.

## Licence and credits

Theta's own code is released under the [MIT licence](LICENSE).

- **Fonts**: IBM Plex Sans (IBM Corp.) and Lilex (The Lilex Project Authors), both under the SIL Open
  Font License 1.1; the licence texts sit next to the fonts in `crates/theta-ui/assets/fonts/`.
- **Themes**: the bundled theme families were copied from [gpui-kit](https://github.com/longbridge/gpui-kit)
  (Copyright Longbridge, Apache-2.0) and are the work of their original authors, listed in
  [`crates/theta-ui/NOTICE`](crates/theta-ui/NOTICE) (Ayu, Catppuccin, Gruvbox, Solarized, Tokyo Night
  and others). The two Theta themes are original.
- **Map data**: the default basemap is © [OpenStreetMap](https://www.openstreetmap.org/copyright)
  contributors, fetched from `tile.openstreetmap.org` and cached on disk. The public tile servers are
  governed by the [Tile Usage Policy](https://operations.osmfoundation.org/policies/tiles/): light,
  interactive use with a proper User-Agent (Theta sends `Theta/<version>`), no bulk downloads. For
  heavy use point Theta at your own or a commercial tile server by changing `map_tile_url` in
  `settings.json`; the template uses `{z}`, `{x}` and `{y}`. The attribution text on the map follows
  the tile host, so keep the provider's required credit if you change it.
- **Dependencies**: see [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md) (generated by
  `script/third-party.py`).
- The Theta app icon is original artwork (`packaging/icons/`).
