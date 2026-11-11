# Changelog

All notable changes to Quarry are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow
[Semantic Versioning](https://semver.org/) once releases begin.

## [Unreleased]

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

### Known limitations

- No soft wrap or folding in the editor; the linter is heuristic.
- Curved geometry types are not decoded.
- MySQL 8 plan parsing is tested from fixtures only; live testing was against MariaDB 10.11.
- Without a keychain (a bare Linux session) passwords are not persisted.
- The macOS app is not notarised and the Windows build is unsigned. Neither platform has been exercised by
  hand.
- The AppImage does not bundle the Vulkan or Wayland libraries.
- The map basemap needs network access (cached tiles still draw offline).

[Unreleased]: https://github.com/JesseWilliamson/Quarry/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/JesseWilliamson/Quarry/releases/tag/v0.1.0-alpha.1
