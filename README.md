# Quarry

<p align="center"><img src="packaging/icons/hicolor/128x128/apps/quarry.png" alt="Quarry icon" width="96"></p>

Quarry is a keyboard-first database IDE written in Rust on [GPUI](https://www.gpui.rs/) (through the
[`gpui-kit`](https://github.com/longbridge/gpui-kit) component crate). It connects to PostgreSQL,
MySQL/MariaDB and SQLite, gives you a SQL editor with Helix or standard key bindings, and shows results
as an editable grid, as a query plan, or on a map.

It is also a codebase meant to be read: the database drivers, SQL tooling, geometry code and the text
editor are plain Rust libraries with ordinary tests, and the UI crates are thin layers on top. See
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## What works today

Quarry 0.1.0-alpha.1 is an alpha. The application is fully wired together: every feature below is
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
| ![Light theme](docs/screenshots/07-quarry-light.png) **Quarry Light** | ![SSH](docs/screenshots/09-connection-ssh.png) **Connection dialog with SSH tunnel** |

## Features

**Connections**
- PostgreSQL (PostGIS-aware), MySQL / MariaDB and SQLite. TLS modes: Disable, Prefer, Require, Verify full.
- SSH tunnels with a private key file (optionally with a passphrase), a password, or ssh-agent. Host keys
  are checked against `~/.ssh/known_hosts`: an unknown host is recorded on first use, a changed key is
  refused.
- Passwords go to the operating-system keychain and never to `connections.json`. Without a keychain (for
  example a bare Linux session) they are kept in memory for the running session only, and the connection
  dialog says so.
- A connection can be marked read-only and given an accent colour (red, orange, green, blue or purple).
  The colour shows in the title bar and the explorer.
- When a statement fails because the session was lost, Quarry reconnects at once and, for a plain query
  outside a transaction, re-runs it once ("Connection lost — reconnected and re-ran the statement").
  Other statements are not re-run and *Messages* says so. Open transactions are lost with the session.
- Start-up flags open connections from the command line (see [Quick start](#quick-start)).

**Explorer**
- Connections, schemas, tables, views, routines and sequences, with columns, keys, foreign keys and
  indexes under each table, a row estimate on every table, and a filter box.
- Foreign keys that the database does not declare are *suggested* from column names (`customer_id`,
  `customerId`) with a confidence percentage. They appear under *Foreign Keys › Suggested* and in the
  **Related Tables…** picker, which lists both kinds and opens a ready-made `JOIN` query.
- Context menu on a table: *Open Table*, *Copy Name*, *Copy Qualified Name*, *Generate* (`SELECT`, `INSERT`,
  `UPDATE`, `DELETE`), *Show DDL*, *Related Tables…*.

**Editor**
- Two complete key maps: **Helix** (modal, selection-first, multiple selections, registers, macros, text
  objects, surround) and a **Standard** non-modal map that follows Zed's default bindings. Switch with
  *Toggle Keymap* in the command palette. Tables are in [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).
- Tabbed consoles. Tabs can be dragged to reorder. A console keeps its text, cursor, title, connection and
  tab position across restarts.
- Completion for keywords, schemas, tables, columns, aliases, functions and snippets, with dialect-specific
  function lists. After `JOIN` it offers whole `JOIN … ON …` clauses built from declared and suggested
  foreign keys.
- A formatter (indent width and keyword case are settings), a diagnostics pass (syntax problems,
  `UPDATE`/`DELETE` without `WHERE`, unknown tables and columns), and a statement splitter that understands
  comments, quoting and dollar-quoted bodies, so "run statement under the cursor" works in scripts.
- Hand-written SQL highlighter that copes with half-typed statements.
- Zoom (`mod-=`, `mod--`, `mod-0`) changes the editor and results font size only. The rest of the interface
  keeps its size.

**Running SQL**
- Run the statement under the cursor, the selection, or the whole script. A script stops at its first error.
  Each statement that returns rows gets its own result tab.
- One query runs at a time per connection; a second run is refused until the first finishes or is
  cancelled. Cancel asks the server to abort the statement.
- Row limit (default 1,000, a setting): the server stops producing rows at the limit.
- Transaction controls: *Begin Transaction*, *Commit Transaction*, *Roll Back Transaction*. While a
  transaction is open the title bar shows **TXN open** with Commit and Rollback buttons, and closing the
  tab or quitting asks what to do with it.
- A confirmation dialog before `UPDATE` or `DELETE` without `WHERE`, and before *Explain Analyze* of a
  statement that writes.
- Read-only connections refuse statements that write data or change the schema.
- Statement history (up to 5,000 entries), searchable, in the History tab.

**Results**
- Virtualised grid (100,000 rows by 60 columns stays interactive): type-aware sorting, filtering, rectangular
  cell selection, column resizing and auto-fit, an inspector for long text, JSON and geometry values, and
  copy as TSV, JSON or `INSERT`.
- Editing: change cells, add and delete rows, review the staged changes, then submit them. Quarry shows the
  generated `UPDATE`/`INSERT`/`DELETE` statements and applies them in one transaction. A result is editable
  only when it comes from one table whose primary key is in the result; otherwise the grid shows a lock and
  says why.
- Export to CSV, TSV, JSON, Markdown or SQL `INSERT` statements, for the whole result or the selection, to a
  file or the clipboard.
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

**Look and feel**
- 38 bundled themes (Quarry Dark, Quarry Light and 36 from 21 gpui-kit theme files), follow-the-system
  appearance, and import of Zed theme files. Fonts: IBM Plex Sans for the interface, Lilex for code.
- Command palette, table picker, connection picker, a settings dialog, and a which-key box for the Helix
  space menu.

## Install

Releases are published on the [GitHub Releases](https://github.com/JesseWilliamson/Quarry/releases) page.
The release workflow ([`.github/workflows/release.yml`](.github/workflows/release.yml)) builds these files
from a `v*` tag and attaches them to a draft release that the maintainer publishes by hand:

| Platform | File | Install |
| --- | --- | --- |
| Linux x86_64 | `quarry-<version>-linux-x86_64.tar.gz` | extract and run `./install.sh` (installs to `~/.local`) |
| Linux x86_64 | `Quarry-<version>-linux-x86_64.AppImage` | `chmod +x` and run (built only when `appimagetool` could be downloaded) |
| macOS Apple Silicon | `Quarry-<version>-macos-arm64.zip` | unzip, drag `Quarry.app` to Applications |
| macOS Intel | `Quarry-<version>-macos-x86_64.zip` | unzip, drag `Quarry.app` to Applications |
| Windows x86_64 | `quarry-<version>-windows-x86_64.zip` | unzip and run `quarry.exe` |

Every release also has a `SHA256SUMS` file.

- **macOS**: the app is ad-hoc signed but **not notarised**, so Gatekeeper blocks the first launch.
  Right-click the app, choose *Open*, then *Open* again; or run
  `xattr -dr com.apple.quarantine /Applications/Quarry.app`.
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

cargo run --release -p quarry -- --demo
```

`mold` is optional; CI uses it through `-C link-arg=-fuse-ld=mold` to link faster (the release workflow
does not). The libraries (`quarry-core`, `quarry-sql`, `quarry-geo`, `quarry-editor`, `quarry-drivers`) build
and test without any of the GUI packages: `cargo test -p quarry-sql`.

## Quick start

```sh
quarry --demo
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
| `--demo` | create and connect the demo database (`QUARRY_DEMO=1` does the same) |
| `--connect URL` | open a temporary connection; repeatable; `postgres://`, `mysql://`, `mariadb://` or `sqlite://` URLs |
| `--run SQL` | run this SQL (every statement) once the first start-up connection is ready |
| `--script FILE` | the same, from a file |
| `--ssh user@host[:port]` | tunnel the `--connect` connections through SSH; without `--ssh-key` the running ssh-agent authenticates (Quarry prints which) |
| `--ssh-key PATH` | authenticate the `--ssh` tunnel with this private key file |
| `--theme NAME` | start with this theme |
| `--stress-catalog[=N]` | debug: add a synthetic connection with N (default 5,000) empty tables |
| `--version`, `-V` | print the version and exit |
| `--help`, `-h` | list every flag and exit; an unknown flag prints the same text and exits with status 2 |

## Configuration

Quarry keeps its files in one directory (override it with `QUARRY_CONFIG_DIR`):

| Platform | Default location |
| --- | --- |
| Linux | `~/.config/quarry/` (`$XDG_CONFIG_HOME/quarry`) |
| macOS | `~/Library/Application Support/quarry/` |
| Windows | `%APPDATA%\quarry\` |

| File | Contents |
| --- | --- |
| `settings.json` | theme, key map, fonts, row limit, map tile URL, formatter options (hand-editable; a bad key is ignored with a warning) |
| `connections.json` | saved connections, without passwords |
| `themes/*.json` | imported or hand-written themes |
| `history.jsonl` | statement history |
| `consoles/` | one JSON file per open console |
| `workspace.json` | panel sizes and visibility, written when you change them |
| `demo.sqlite` | the `--demo` database |

Map tiles are cached under the platform cache directory (`~/.cache/quarry/tiles` on Linux;
`QUARRY_CACHE_DIR` overrides it). The full settings reference is in the
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

Editor
- No soft wrap and no code folding. Long lines scroll horizontally.
- Diagnostics are heuristic. The linter is written to stay quiet rather than flag valid SQL, so it misses
  some errors, and it understands only the common parts of each dialect. The database remains the judge:
  a failed statement is underlined from the server's error position.
- In the editor the engine's keys win over the application's shortcuts, so a few application chords
  (`ctrl-b`, `ctrl-o`, `ctrl-w`, `ctrl-k`, `F5`) do something else while the editor has focus in the Helix key
  map; the actions stay in the palette and space menu. See
  [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#who-wins-a-key).
- Undo is linear (no undo tree).

Databases
- Read-only connections are enforced by Quarry, which classifies each statement before it runs; it refuses
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
- Export covers the rows that were loaded into the grid, which the row limit caps.
- The grid cannot reorder or hide columns.

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
  quarry-core         contracts: Value, ResultSet, Catalog, Plan, ConnectionConfig, Driver/Connection traits
  quarry-drivers      PostgreSQL, MySQL/MariaDB, SQLite and SSH tunnelling (Tokio)
  quarry-sql          splitting, formatting, completion, FK joins, plan insights, lint, SQL generation, export
  quarry-geo          WKB/EWKB/WKT/GeoJSON, projections, tile maths, measuring
  quarry-editor       text buffer, selections, Helix and Standard keymaps, SQL highlighting (no GPUI)
  quarry-ui           theme, settings, fonts and icons, pickers, small shared widgets
  quarry-editor-view  GPUI widget that paints quarry-editor
  quarry-grid         results grid with editing, filtering, export requests
  quarry-plan-view    EXPLAIN tree, flame graph, raw view, insights
  quarry-map-view     slippy map with tile loader and cache
  quarry              the application: workspace, explorer, connections, dialogs, command palette
```

The rule that shapes everything: whatever can be tested without a window lives outside the GUI crates,
and the UI thread never waits on I/O. Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the
reasoning and [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for how the pieces fit.

## Development

```sh
cargo test -p quarry-core -p quarry-sql -p quarry-geo -p quarry-editor   # pure crates, seconds
cargo test -p quarry-drivers                                             # needs the test databases
cargo test --workspace --exclude quarry-drivers                          # everything else
cargo run -p quarry-grid --example grid_demo                             # each view crate has a demo
script/shot /tmp/shots/main.png -- --demo                                # headless screenshot (Linux)
```

Test databases: PostgreSQL 16 with PostGIS at `postgres://postgres:postgres@127.0.0.1:5432/quarry_demo`
and MariaDB 10.11 at `mysql://quarry:quarry@127.0.0.1:3306/quarry_demo`; the CI workflow starts them as
service containers. [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) covers the details, including the
screenshot harness, adding a database driver, adding a theme, and the CI and release process.
[CONTRIBUTING.md](CONTRIBUTING.md) has the short version.

## Licence and credits

The licence for Quarry's own code is to be decided by the repository owner.

- **Fonts**: IBM Plex Sans (IBM Corp.) and Lilex (The Lilex Project Authors), both under the SIL Open
  Font License 1.1; the licence texts sit next to the fonts in `crates/quarry-ui/assets/fonts/`.
- **Themes**: the bundled theme families were copied from [gpui-kit](https://github.com/longbridge/gpui-kit)
  (Copyright Longbridge, Apache-2.0) and are the work of their original authors, listed in
  [`crates/quarry-ui/NOTICE`](crates/quarry-ui/NOTICE) (Ayu, Catppuccin, Gruvbox, Solarized, Tokyo Night
  and others). The two Quarry themes are original.
- **Map data**: the default basemap is © [OpenStreetMap](https://www.openstreetmap.org/copyright)
  contributors, fetched from `tile.openstreetmap.org` and cached on disk. The public tile servers are
  governed by the [Tile Usage Policy](https://operations.osmfoundation.org/policies/tiles/): light,
  interactive use with a proper User-Agent (Quarry sends `Quarry/<version>`), no bulk downloads. For
  heavy use point Quarry at your own or a commercial tile server by changing `map_tile_url` in
  `settings.json`; the template uses `{z}`, `{x}` and `{y}`. The attribution text on the map follows
  the tile host, so keep the provider's required credit if you change it.
- **Dependencies**: see [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md) (generated by
  `script/third-party.py`).
- The Quarry app icon is original artwork (`packaging/icons/`).
