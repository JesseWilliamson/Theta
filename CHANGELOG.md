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
- Automatic reconnect when a statement fails because the session was lost.
- Command-line flags: `--demo`, `--connect URL` (repeatable), `--run SQL`, `--script FILE`,
  `--ssh user@host[:port]` with `--ssh-key PATH`, `--theme NAME`, `--stress-catalog[=N]`; `QUARRY_DEMO=1`.
- A demo database (`--demo`): customers, orders, products, stores, delivery zones and routes in SQLite.

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
- Transactions: *Begin*, *Commit* and *Roll Back*, a **TXN open** indicator with Commit and Rollback
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
- README, user guide, key binding reference, architecture and development guides, contributing guide and a
  generated third-party licence list.

### Known limitations

- No soft wrap or folding in the editor; the linter is heuristic.
- A few editor keys are shadowed by the application's global key bindings (`F5`, and in the Helix key map
  `ctrl-b`, `ctrl-o`, `ctrl-w`, `ctrl-k` on Linux and Windows). The grid's `mod-0` (set NULL) is shadowed by
  *Reset Editor Font Size*.
- Curved geometry types are not decoded.
- MySQL 8 plan parsing is tested from fixtures only; live testing was against MariaDB 10.11.
- Without a keychain (a bare Linux session) passwords are not persisted.
- The macOS app is not notarised and the Windows build is unsigned. Neither platform has been exercised by
  hand.
- The AppImage does not bundle the Vulkan or Wayland libraries.
- The map basemap needs network access (cached tiles still draw offline).

[Unreleased]: https://github.com/JesseWilliamson/Quarry/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/JesseWilliamson/Quarry/releases/tag/v0.1.0-alpha.1
