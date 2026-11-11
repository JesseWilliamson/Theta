# Quarry

<p align="center"><img src="packaging/icons/hicolor/128x128/apps/quarry.png" alt="Quarry icon" width="96"></p>

Quarry is a keyboard-first database IDE written in Rust on [GPUI](https://www.gpui.rs/) (through the
[`gpui-kit`](https://github.com/longbridge/gpui-kit) component crate). It connects to PostgreSQL,
MySQL/MariaDB and SQLite, gives you a SQL editor with Helix or standard key bindings, and shows results
as an editable grid, as a query plan, or on a map.

It is also a codebase meant to be read: the database drivers, SQL tooling, geometry code and the text
editor are plain Rust libraries with ordinary tests, and the UI crates are thin layers on top. See
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

> **Status.** The libraries and the view crates (editor, grid, plan panel, map) are implemented and
> tested. The application crate (`crates/quarry`) that wires them together is under active
> development; the feature list below says what each crate provides, and the
> [changelog](CHANGELOG.md) says what has been integrated. No release has been published yet.

<!-- screenshots -->
| | |
| --- | --- |
| ![Editor and results](docs/screenshots/01-editor-results.png) **Helix editor and results grid** | ![Completion](docs/screenshots/02-completion-fk-joins.png) **Completion with foreign-key join suggestions** |
| ![Map](docs/screenshots/03-map-stores-zones.png) **Geometry on a map** | ![Plan](docs/screenshots/04-plan-insights.png) **EXPLAIN ANALYZE with insights** |
| ![Editable grid](docs/screenshots/05-editable-grid.png) **Staged edits in the grid** | ![Explorer](docs/screenshots/06-explorer-suggested-fks-related.png) **Suggested foreign keys and related tables** |
| ![Light theme](docs/screenshots/07-quarry-light.png) **Quarry Light** | ![SSH](docs/screenshots/09-connection-ssh.png) **Connection dialog with SSH tunnel** |

## Features

**Connections**
- PostgreSQL (PostGIS-aware), MySQL / MariaDB and SQLite, with TLS modes `disable`, `prefer`,
  `require` and `verify-full`.
- SSH tunnels with password, private-key (optionally passphrase-protected) or ssh-agent
  authentication. Host keys are checked against `~/.ssh/known_hosts` (trust on first use; a changed key
  is refused).
- Passwords live in the operating-system keychain, never in `connections.json`. Without a keychain
  (for example a bare Linux session) they are kept in memory for the running session only.
- Connections can be marked read-only or given an accent colour; `--connect URL` opens one from the
  command line.

**Editor**
- Two complete key maps: **Helix** (modal, selection-first, multiple selections, registers, macros,
  text objects, surround) and a **Standard** non-modal map with Zed's default bindings. See the
  [cheat sheet](#key-bindings) and [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).
- Context-aware completion: keywords, schemas, tables, columns, aliases, functions, snippets and ready
  made `JOIN ... ON` suggestions, with dialect-specific function lists.
- Foreign-key join suggestions follow declared keys in both directions and also *inferred* keys
  (column names such as `customer_id` or `customerId` that point at a table, ranked by confidence).
- A SQL formatter (indent width, keyword case and maximum line width are configurable), a linter
  (syntax problems, dangerous statements such as `DELETE` without `WHERE`, unknown names) and a
  statement splitter so "run statement under the cursor" works in scripts with comments, dollar
  quoting and `$$` bodies.
- Hand-written SQL highlighter that copes with half-typed statements; tested on 20,000-line buffers.

**Results**
- Virtualised grid (100,000 rows by 60 columns stays interactive): type-aware sorting, filtering,
  rectangular cell selection, column resizing and auto-fit, an inspector for long text, JSON and
  geometry values, and copy as TSV.
- Editing: change cells, add and delete rows, review the staged changes, then submit them as one
  transaction (`UPDATE`/`INSERT`/`DELETE` are generated from the primary key). Results from more than
  one table are read-only, and the grid says why.
- Export to CSV, TSV, JSON, Markdown or SQL `INSERT` statements, for the whole result or the
  selection, to a file or the clipboard. Foreign-key columns carry a link icon and can be followed to the referenced row.

**Query plans**
- `EXPLAIN` / `EXPLAIN ANALYZE` for PostgreSQL, MySQL/MariaDB and SQLite, shown as a tree table, a
  flame graph, or the raw server output.
- Insights written for the specific plan: sequential scans with selective filters, row estimates that
  are badly off, nested-loop rescans, sorts and hashes that spill to disk, cartesian products, MySQL
  `filesort`/temporary tables/full scans, SQLite full scans and temporary B-trees. Where it can,
  an insight offers a `CREATE INDEX` statement.

**Map viewer**
- Draws the geometry columns of a result on a slippy map (OpenStreetMap tiles by default) or on a plain
  planar grid when the coordinate system is unknown. Decodes WKB, EWKB, WKT and GeoJSON; understands
  Web-Mercator, WGS 84 and UTM SRIDs.
- Layers, feature picking that shows the row's other columns, a measure tool (distance and
  area), copy coordinates / WKT, and GeoJSON export.

**Look and feel**
- Bundled themes (Quarry Dark and Light plus 21 theme files from gpui-kit, most with a light and a dark variant), follow-the-system
  appearance, and **import of Zed theme files**. Fonts: IBM Plex Sans for the UI, Lilex for code.
- Command palette, table picker, connection picker, a which-key box for the Helix space menu,
  statement history, and consoles that survive restarts.

## Install

No release has been published yet. When one is, the
[Releases](https://github.com/JesseWilliamson/Quarry/releases) page will carry:

| Platform | File | Install |
| --- | --- | --- |
| Linux x86_64 | `quarry-<version>-linux-x86_64.tar.gz` | extract and run `./install.sh` (installs to `~/.local`) |
| Linux x86_64 | `Quarry-<version>-linux-x86_64.AppImage` | `chmod +x` and run |
| macOS (Apple Silicon / Intel) | `Quarry-<version>-macos-arm64.zip` / `-x86_64.zip` | unzip, drag `Quarry.app` to Applications |
| Windows x86_64 | `quarry-<version>-windows-x86_64.zip` | unzip and run `quarry.exe` |

Every release also has a `SHA256SUMS` file.

- **macOS**: the app is ad-hoc signed but **not notarised**, so Gatekeeper blocks the first launch.
  Right-click the app, choose *Open*, then *Open* again; or run
  `xattr -dr com.apple.quarantine /Applications/Quarry.app`.
- **Linux** needs a working Vulkan driver (Mesa `libvulkan_radeon`/`intel`/`lvp`, or the proprietary
  one) plus `libxkbcommon`, `libwayland-client`, `libxcb` and `fontconfig`, which every desktop
  distribution ships. See the [troubleshooting](docs/USER_GUIDE.md#troubleshooting) section.

## Build from source

You need a recent stable Rust toolchain (1.99 or newer; see [`rust-toolchain.toml`](rust-toolchain.toml)) and a
C compiler. Building the GPUI application takes several minutes and a few GB of disk.

```sh
# Linux (Debian/Ubuntu package names, as used by CI)
sudo apt-get install clang mold libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb1-dev \
  libxcb-xkb-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig-dev \
  libasound2-dev libssl-dev libzstd-dev libvulkan1 mesa-vulkan-drivers

# macOS: full Xcode (GPUI compiles Metal shaders with `xcrun metal`), plus its command line tools
xcode-select --install

# Windows: Visual Studio Build Tools with the "Desktop development with C++" workload
# (MSVC + Windows 10/11 SDK)

cargo run --release -p quarry -- --demo
```

`mold` is optional; CI uses it through `-C link-arg=-fuse-ld=mold` to link faster. The libraries
(`quarry-core`, `quarry-sql`, `quarry-geo`, `quarry-editor`, `quarry-drivers`) build and test without
any of the GUI packages: `cargo test -p quarry-sql`.

## Quick start

```sh
quarry --demo
```

`--demo` creates a SQLite database with sample customers, orders, products, stores, delivery zones and routes in the
configuration directory and connects to it, so there is something to query, plan and map immediately.
Other flags: `--connect URL` (repeatable; `postgres://`, `mysql://`, `mariadb://` or `sqlite://` URLs),
`--theme NAME`. Setting `QUARRY_DEMO=1` is the same as `--demo`.

Then, in the editor (Helix keymap by default): press `i`, type `select * from customers`, press `Esc`,
then `space r` (or `ctrl-enter` in either keymap) to run the statement under the cursor. `space ?`
opens the command palette. The [user guide](docs/USER_GUIDE.md) walks through the rest.

## Configuration

Quarry keeps its files in one directory (override it with `QUARRY_CONFIG_DIR`):

| Platform | Default location |
| --- | --- |
| Linux | `~/.config/quarry/` (`$XDG_CONFIG_HOME/quarry`) |
| macOS | `~/Library/Application Support/quarry/` |
| Windows | `%APPDATA%\quarry\` |

| File | Contents |
| --- | --- |
| `settings.json` | theme, keymap, fonts, row limit, map tile URL, formatter options (hand-editable; bad keys are ignored with a warning) |
| `connections.json` | saved connections, without passwords |
| `themes/*.json` | imported or hand-written themes |
| `history.jsonl` | statement history |
| `consoles/` | one JSON file per open console |
| `workspace.json` | panel sizes and visibility |
| `demo.sqlite` | the `--demo` database |

Map tiles are cached under the platform cache directory (`~/.cache/quarry/tiles` on Linux;
`QUARRY_CACHE_DIR` overrides it). The full settings reference is in the
[user guide](docs/USER_GUIDE.md#settings).

## Key bindings

`mod` is `ctrl` (Linux, Windows) or `cmd` (macOS). The complete tables are in
[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).

**Anywhere**

| Keys | Action |
| --- | --- |
| `mod-shift-p`, `F1` | command palette |
| `mod-p` | go to table |
| `mod-enter` / `mod-shift-enter` | run statement / run all (`F5` also runs all inside the editor) |
| `mod-.` | cancel the running query |
| `mod-e` | explain (`mod-shift-alt-e` explain analyze) |
| `mod-shift-f` | format the document |
| `mod-b` / `mod-j` | toggle explorer / bottom panel |
| `mod-n` / `mod-w` | new console / close tab |

**Helix keymap (default)**

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

**Standard keymap**

| Keys | Action |
| --- | --- |
| `ctrl-z`, `ctrl-shift-z` | undo, redo |
| `ctrl-d`, `ctrl-shift-l` | add next occurrence, select all occurrences |
| `ctrl-/` | toggle `--` comment |
| `alt-up/down` | move lines |
| `ctrl-shift-k` | delete line |
| `ctrl-f`, `ctrl-g` | find, go to line |
| `ctrl-space` | completion |

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
