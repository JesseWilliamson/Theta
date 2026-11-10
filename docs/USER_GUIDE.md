# Quarry user guide

Task-oriented notes for using Quarry. Key bindings are summarised where they matter and listed in full in
[KEYBINDINGS.md](KEYBINDINGS.md). `mod` means `cmd` on macOS and `ctrl` elsewhere.

> **Integration status.** Quarry is built from independent view components (editor, grid, plan panel, map)
> that the application crate wires together. This guide describes what those components and the
> application implement in the source tree; the [changelog](../CHANGELOG.md) records what has been
> integrated in each release. If a screen here does not behave as described in your build, the
> component exists but may not be connected yet.

Contents: [Connect](#connect-to-a-database) · [SSH tunnel](#connect-through-an-ssh-tunnel) ·
[Write and run SQL](#write-and-run-sql) · [Helix essentials](#helix-essentials) ·
[Results grid](#the-results-grid) · [Edit data safely](#edit-data-safely) · [Explain](#explain-and-query-plans) ·
[Map](#the-map) · [Themes](#themes) · [Settings](#settings) · [Troubleshooting](#troubleshooting)

## Connect to a database

Try it without a server first: `quarry --demo` creates `demo.sqlite` in the configuration directory and
connects to it.

For your own database open *New Connection…* (`mod-shift-n`, or from the command palette). Pick the kind,
fill in the fields, then *Test Connection*, *Save* or *Save & Connect*.

| Field | Notes |
| --- | --- |
| Connection URL | paste `postgres://user:pass@host:5432/db?sslmode=require`, `mysql://...`, `mariadb://...` or `sqlite:///path/to.db` to fill the fields (the same parser serves `--connect`) |
| Name, Host, Port, User, Password, Database | the port defaults to 5432 / 3306 |
| SSL mode | Disable, Prefer (try TLS, fall back), Require (TLS, certificate not verified), Verify full (TLS, chain and host name verified) |
| Database file (SQLite) | *Browse…*, or *Create if missing* |
| Read-only | the application refuses statements that write |
| Colour | an accent colour, handy for marking production |

Passwords go to the operating-system keychain, not to `connections.json`. Switch connections with
`mod-alt-c` or the title-bar switcher. From the command line: `quarry --connect postgres://me@localhost/app`
(repeatable; these connections are temporary).

The explorer on the left shows connections, then schemas, tables and views, and their columns, keys and
indexes. `mod-shift-e` focuses it; in it `/` filters, arrows (or `h j k l` in Helix mode) navigate and
`enter` opens or connects. `mod-p` jumps to any table by fuzzy name; `F5` refreshes the catalog after
you change the schema.

## Connect through an SSH tunnel

Tick the SSH tunnel section of the connection dialog and give it the SSH host, port (22), user and an
authentication method:

- **Key file**: choose the private key; a passphrase is optional and is kept in the keychain.
- **Password**: the SSH password, kept in the keychain.
- **SSH agent**: uses the agent named by `SSH_AUTH_SOCK` (on Windows, the OpenSSH agent).

The *Host* and *Port* fields of the main form are then interpreted **from the SSH server's point of
view**: for a database on the SSH machine itself use `localhost`. Quarry opens one SSH session, a loopback
listener and a `direct-tcpip` channel per database connection, and tears it down when the connection
closes. Host keys are checked against `~/.ssh/known_hosts`: an unknown host is accepted and recorded
(trust on first use), a *changed* key is refused. SQLite connections have no tunnel (they are files).

## Write and run SQL

Open a console with `mod-n`. Consoles are tabs, each bound to the connection that was active; their
text survives restarts.

- **Run the statement under the cursor**: `mod-enter` (Helix: also `space r`). With a selection, only the
  selection runs. Statements are found by a lexer that understands comments, quotes and dollar-quoted
  bodies, so a script can hold many statements.
- **Run everything**: `mod-shift-enter`, `F5` inside the editor (Helix: `space R`). **Cancel**: `mod-.`
  (Helix: `space x`); the server is asked to abort the query.
- **Completion** appears as you type, or on `ctrl-space`. It knows the tables and columns of the active
  connection and what is in scope (aliases, CTEs), offers dialect-specific functions and snippets, and,
  after `JOIN`, whole `JOIN other ON ...` clauses derived from foreign keys. Keys declared in the database
  rank first; relationships Quarry *infers* from names (`orders.customer_id` to `customers.id`) are offered
  too and are labelled as inferred. `tab` or `enter` accepts, `esc` dismisses.
- **Format** with `mod-shift-f` (Helix: `:format`). Indent width (default 4), keyword upper-casing (default
  on) and the maximum line width (100) come from the `format` settings.
- **Diagnostics** underline syntax problems, `UPDATE`/`DELETE` without `WHERE`, suspicious predicates and,
  when the catalog is known, unknown tables and columns. The linter is deliberately conservative: it would
  rather stay quiet than flag valid SQL.
- **Row limit**: results are capped at 1000 rows by default (`row_limit` in settings). The server stops
  producing rows at the limit rather than sending everything.
- **Transactions**: *Begin*, *Commit* and *Roll Back Transaction* are in the command palette. Each console
  is one server session, so session settings and open transactions behave like a normal `psql` or `mysql`
  prompt; the title bar shows the transaction state.
- **Files**: *Open SQL File…* and *Save Console As…* in the palette.
- **History**: every executed statement is recorded in `history.jsonl` and listed in the History tab of the
  bottom panel.

The command palette (`mod-shift-p`, `F1`, Helix `space ?`) lists every action with its shortcut.

## Helix essentials

The default keymap is [Helix](https://helix-editor.com/)'s: *select first, then act*. Everything below
works in the SQL editor; `mod-enter` runs from any mode.

| Do this | Keys |
| --- | --- |
| Start typing | `i` (before), `a` (after), `o` / `O` (new line below / above); `esc` returns to Normal mode |
| Move | `h j k l`; `w` / `b` / `e` by word; `gg` first line, `G` last, `5G` line 5; `gh` / `gl` line start / end |
| Select | motions select (`w` selects the next word); `x` the current line (again to extend); `%` everything; `v` toggles Select mode so motions extend |
| Act on the selection | `d` delete, `c` change, `y` yank, `p` paste after, `u` undo, `U` redo, `.` repeat |
| Multiple cursors | `C` copies the selection to the next line; `s` selects all matches of a regex *inside* the selection; `,` keeps only the primary |
| Surround and objects | `mi(` inside parentheses, `ma"` around a string, `ms)` surround, `md(` delete the surround |
| Search | `/` and `?`, `n` / `N`, `*` searches for the word under the cursor |
| Comment | `ctrl-c` |
| Command line | `:w` save, `:q` close tab, `:format`, `:run`, `:explain`, `:goto 120`, `:theme NAME` |
| Space menu | `space` shows what follows (a which-key box): `r` run, `e` explain, `f` tables, `b` connections, `k` completion |

Three recipes: delete a line with `x` `d`; change a word with `w` `c`, type the replacement, `esc`; rename
every occurrence of `name` in the statement by selecting the statement (`%` for everything), `s`, typing
`name`, `enter` (one cursor per match), then `c`, the new name, `esc`.
Prefer the Standard keymap if modal editing is unfamiliar: switch it with *Toggle Keymap* in the
command palette or `"keymap": "standard"` in `settings.json`. It follows Zed's defaults: `ctrl-z`,
`ctrl-d` (next occurrence), `ctrl-/` (comment), `alt-up/down` (move line), `ctrl-f` find, `ctrl-g` go to line.

## The results grid

Results appear in the bottom panel (`mod-j` toggles it). The footer shows the row count, the time and
the summary of the selected cells.

- **Select**: click a cell, shift-click or drag for a rectangle, click row numbers or column headers to
  select whole rows and columns, `mod-a` for all.
- **Sort**: the arrow at the right of a header cycles ascending, descending, off (shift-click for a secondary
  key). NULLs sort last in both directions; numbers, text and dates sort by type, with natural ordering
  for strings with numbers in them.
- **Filter**: `mod-f` or `/` opens a filter box that matches any cell; `esc` closes it.
- **Columns**: drag a header edge to resize; double-click it to fit; right-click a header for sort,
  auto-fit and copy name.
- **Inspector**: `space` opens a pane showing the full value of the current cell: long text, pretty-printed
  JSON and geometry as WKT.
- **Copy**: `mod-c` copies the selection as tab-separated text, `mod-shift-c` with the column headers.
- **Export**: choose CSV, TSV, JSON, Markdown or SQL `INSERT` statements, for the whole result or only the
  selection, to a file or the clipboard. Exports contain every byte (no truncation of long values).
- **Foreign keys**: columns that reference another table show a link icon; on a read-only result,
  `enter` or ctrl-click follows the reference.
- **Geometry**: cells with geometry are shown as text with a pin icon, and *Show on map* opens them on
  the [map](#the-map).

The grid is built for large results: only the visible window is drawn, and sorting or filtering more
than 20,000 rows happens in the background.

## Edit data safely

A result is editable only when every column comes from one table and that table has a primary key; otherwise
the grid shows a lock and says why (for instance "Results from more than one table"). Connections marked
*Read-only* never write.

1. Double-click a cell, or press `enter` / `F2` (in the Standard keymap you can also just start typing;
   in Helix, `i`). Edits are **staged**, not sent: changed cells are tinted with a corner marker, new
   rows are added at the bottom, and rows marked with `delete` are struck through.
2. `mod-0` sets a cell to NULL; `mod-z` undoes the last staged edit; `tab` / `shift-tab` move between
   cells while editing, `esc` cancels the cell edit.
3. Review the staged changes, then submit with `mod-enter` or the *Submit* button. The application
   generates `UPDATE`, `INSERT` and `DELETE` statements keyed by the primary key values the rows had
   **before** your edit, runs them in a single transaction and reports success or failure. If anything
   fails, nothing is applied and the staged edits stay so you can fix them.

Values you type are parsed according to the column type (integers, decimals, booleans, dates, JSON ...);
text that does not parse is rejected at the cell instead of reaching the database. Use *Run* with a
hand-written statement for anything the grid cannot express.

## Explain and query plans

Press `mod-e` (Helix `space e`) to explain the statement under the cursor, `mod-shift-alt-e`
(`space E`) to explain **analyze**, which really executes it. Writes are analysed inside a transaction that
is rolled back, so nothing is changed.

The plan panel shows three views of the same plan, always next to the list of insights:

- **Tree**: the plan as an indented table of operations with exclusive time, rows (estimated, or actual when
  analysed), cost, loops and buffers (columns drop out when the panel is narrow); `enter` opens a node's
  full property list.
- **Flame**: a flame graph sized by time (or cost when the plan was not analysed).
- **Raw**: the server's own output (PostgreSQL JSON, MariaDB/MySQL JSON, SQLite `EXPLAIN QUERY PLAN`).

**Insights** are specific statements about *this* plan, ranked by severity: a sequential scan that reads
many rows to return few; row estimates that are off by orders of magnitude (stale statistics); a nested loop
rescanning a table; sorts and hashes that spill to disk (with a `work_mem` hint); cartesian products and
joins without a condition; the node that takes most of the runtime; for MySQL/MariaDB `filesort`, temporary
tables and full scans; for SQLite full-table scans and temporary B-trees. Where a missing index is the
likely cure, the insight carries a `CREATE INDEX` statement that you can open in a console. Plain `EXPLAIN`
shows estimates only; *Explain Analyze* adds the real timings and row counts.

## The map

Result columns that hold geometry (PostGIS and MySQL geometry, WKB/EWKB bytes, WKT or GeoJSON text) can be
drawn on a map. Use *Show on map* in the grid (one cell, or every geometry of the column).

- Pan by dragging or with the arrow keys, zoom with the wheel or `+` / `-`, `0` fits everything, and
  double-click zooms in.
- Coordinates in WGS 84, Web-Mercator and UTM SRIDs are placed on an OpenStreetMap basemap. Data with an
  unknown coordinate system is drawn on a plain planar grid instead, without tiles.
- Click a feature to see the other columns of its row; each geometry column is a layer you can show or hide.
- `m` starts the **measure tool**: click points for a distance, double-click to close the polygon and get the
  area. `esc` leaves it.
- Right-click for *Copy coordinates*, *Copy as WKT point* and *Copy feature WKT*; the toolbar exports the
  visible layers as GeoJSON.
- Tiles are cached on disk, so panning back is instant and previously seen areas still draw offline;
  "Basemap unavailable" with a *Retry* button appears when nothing can be fetched.
- Change the tile server with `map_tile_url` in `settings.json` (see below). The default is the public
  OpenStreetMap server, which asks for light, interactive use; the map shows the required attribution.

## Themes

Open the theme picker with `mod-k mod-t` (Helix: `space t`, `:theme`). *Quarry Dark* and *Quarry Light*
are the defaults; 21 more theme files from gpui-kit are bundled (Catppuccin, Gruvbox, Solarized, Tokyo Night,
Ayu, Everforest and others), and the picker previews as you move.

To follow the operating system's light/dark setting set `follow_system_appearance` to `true`; then
`light_theme` and `dark_theme` decide which two themes alternate.

**Zed themes**: choose *Import Zed Theme…* in the command palette and select a Zed theme JSON file (the
format of `~/.config/zed/themes/*.json`). Quarry converts it and saves it to `<config>/themes/`, where it
shows up in the picker. Colours Zed does not define fall back to the Quarry theme of the same
appearance. You can also drop a gpui-kit theme file into that folder yourself.

## Settings

Everything is in `settings.json` in the configuration directory (command palette: *Open Settings File*). The
file is hand-editable: unknown keys are ignored and a key with a wrong type is ignored with a warning, so
one typo never resets the rest. The app saves changes made in its own UI (theme, zoom, keymap) to the
same file.

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `"Quarry Dark"` | current theme |
| `follow_system_appearance` | `false` | switch between `light_theme` and `dark_theme` with the OS |
| `light_theme`, `dark_theme` | `"Quarry Light"`, `"Quarry Dark"` | used when following the system |
| `keymap` | `"helix"` | `"helix"` or `"standard"` |
| `ui_font_family`, `ui_font_size` | `"IBM Plex Sans"`, `14` | interface font (size is limited to 8-40) |
| `buffer_font_family`, `buffer_font_size` | `"Lilex"`, `14` | editor and grid font |
| `buffer_line_height` | `1.5` | multiplier of the buffer font size |
| `row_limit` | `1000` | maximum rows fetched per query |
| `map_tile_url` | `https://tile.openstreetmap.org/{z}/{x}/{y}.png` | XYZ tile template for the map |
| `format.indent` | `4` | formatter indent width |
| `format.uppercase_keywords` | `true` | formatter keyword case |

Zoom (`mod-=`, `mod--`, `mod-0`) changes the font sizes.

## Troubleshooting

**The window does not open on Linux, or says no Vulkan device was found.** Quarry draws through Vulkan.
Install a driver: `mesa-vulkan-drivers` (Debian/Ubuntu), `vulkan-radeon` / `vulkan-intel` (Arch, Fedora's
`mesa-vulkan-drivers`), or your GPU vendor's driver. Check with `vulkaninfo --summary`. In a virtual
machine or container with no GPU, Mesa's software renderer (`lavapipe`, part of `mesa-vulkan-drivers`) works,
slowly.

**Passwords are not remembered on Linux.** Quarry stores secrets through the freedesktop Secret Service (GNOME
Keyring, KDE Wallet, KeePassXC) over D-Bus. In a minimal window manager session, over plain SSH, or in a
container there is none, so Quarry keeps secrets in memory for that run only, and the connection dialog tells
you ("Password will not be saved"). Start a keyring daemon (`gnome-keyring-daemon --start --components=secrets`)
and unlock it, or type the password at each start.

**macOS says the app "is damaged" or "cannot be opened because the developer cannot be verified".** Releases are
ad-hoc signed but not notarised. Right-click `Quarry.app`, choose *Open* and confirm once; or, after copying the
app to Applications, run `xattr -dr com.apple.quarantine /Applications/Quarry.app`. Building from source
avoids the quarantine flag altogether.

**Windows SmartScreen says "Windows protected your PC".** The executable is not code-signed. Choose *More info*,
then *Run anyway*. Verify the download against `SHA256SUMS` if you want to be careful.

**The map is grey or says "Basemap unavailable".** The tile server could not be reached (offline, proxy, firewall)
or rejected the request. Quarry trusts the operating system's certificate store, so corporate proxies with their
own CA normally work. Click *Retry*, or point `map_tile_url` at a server you can reach. Geometries are
still drawn without a basemap.

**A connection to PostgreSQL or MySQL fails with a TLS error.** Try SSL mode *Prefer* or *Disable* for servers
without TLS, *Require* for self-signed certificates, and *Verify full* only when the server certificate
chains to a root your system trusts and matches the host name.

**A query seems stuck.** Press `mod-.` to cancel. Quarry asks the server to abort the statement
(a cancel request for PostgreSQL, `KILL QUERY` for MySQL/MariaDB, `sqlite3_interrupt` for SQLite) and the console
stays usable.

**Settings or connections seem lost.** A corrupt `connections.json` is renamed to `connections.json.broken` and a
fresh file started. A settings file that is not valid JSON falls back to the defaults with a warning. Both
files can be restored from the backup you keep of the configuration directory.
