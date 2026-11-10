# Changelog

All notable changes to Quarry are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow
[Semantic Versioning](https://semver.org/) once releases begin.

## [Unreleased]

Nothing has been released yet. This section summarises what exists in the repository.

### Added

- **quarry-core**: the shared contract: `Value`, `ResultSet`, `Catalog`, `Plan`, `ConnectionConfig`,
  errors with source positions, and the `Driver` / `Connection` traits.
- **quarry-drivers**: PostgreSQL (with PostGIS geometry as EWKB), MySQL / MariaDB and SQLite drivers;
  row limits that stop the server early; cancellation; introspection of schemas, tables, columns,
  keys, indexes and routines; `EXPLAIN` / `EXPLAIN ANALYZE` parsers for all three databases; SSH tunnels
  (password, key file, ssh-agent) with known-hosts checking; a demo-data seeder.
- **quarry-sql**: statement splitting and classification; formatter; context-aware completion; foreign-key
  join suggestions including inferred keys; plan analysis with insights and index suggestions; linter;
  `INSERT`/`UPDATE`/`DELETE`/DDL generation; result export (CSV, TSV, JSON, Markdown, SQL inserts).
- **quarry-geo**: WKB/EWKB, WKT and GeoJSON decoders and encoders, SRID handling (WGS 84, Web-Mercator, UTM),
  slippy-map and planar cameras, simplification, hit testing, distance and area measurement.
- **quarry-editor**: rope buffer, multiple selections, undo history, SQL highlighter, the Helix keymap
  (counts, registers, macros, text objects, surround, regex selections, command line) and a Standard keymap
  with Zed's defaults.
- **quarry-ui**: theme registry with Quarry Dark / Light and 21 bundled theme files from gpui-kit, Zed theme
  import, tolerant `settings.json`, bundled IBM Plex Sans and Lilex fonts, picker, shared widgets.
- **quarry-editor-view**: GPUI editor widget with gutter, diagnostics, completion popup, which-key box and
  prompt bar.
- **quarry-grid**: virtualised results grid with selection, sorting, filtering, inspector, staged cell
  editing, export requests and foreign-key navigation.
- **quarry-plan-view**: plan tree, flame graph, raw view and insights panel.
- **quarry-map-view**: slippy / planar map with tile cache, layers, measure tool and GeoJSON export.
- **quarry** (application, in progress): workspace shell, connection store with OS keychain secrets,
  connection dialog with SSH settings, explorer, command palette, consoles that persist across restarts,
  statement history, `--demo`, `--connect`, `--theme`.
- Packaging: application icon, Linux tarball with `install.sh` and AppImage, macOS `.app` bundle (ad-hoc
  signed), Windows zip with embedded icon, `SHA256SUMS` and a draft-release workflow with a dry-run mode.
- Documentation: README, user guide, key bindings, development guide, contributing guide and a
  generated third-party licence list.

### Changed

- CI runs formatting and `clippy` as separate non-blocking jobs, builds and tests the whole workspace on
  Linux, macOS and Windows, and runs the database and SSH integration tests against service containers.
- Wall-clock assertions in tests widen their bounds under `CI`.

[Unreleased]: https://github.com/JesseWilliamson/Quarry/commits/main
