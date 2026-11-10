# Development guide

How Quarry is put together, how to change it, and how it is tested, built and released. Read
[ARCHITECTURE.md](ARCHITECTURE.md) first for the reasoning; this document is the practical companion.
[AGENTS.md](../AGENTS.md) holds the working rules used by the automated contributors on this repository
(build one crate at a time, commit only your own paths, never invent GPUI APIs).

## How the pieces fit

```text
                       ┌────────────────────────────── quarry (application) ─────────────────────────────┐
                       │ workspace, explorer, connections, dialogs, palette, run/edit/export orchestration│
                       └───┬───────────┬───────────┬────────────┬───────────────┬──────────────┬──────────┘
                           │           │           │            │               │              │
                    quarry-editor-view  quarry-grid  quarry-plan-view  quarry-map-view     quarry-drivers
                           │           │           │            │               │              │
                    quarry-editor   quarry-geo   quarry-sql ─────┘      quarry-geo          quarry-core
                           │                                                 (all view crates use quarry-ui
                           └ no workspace dependencies                        for theme, settings, widgets)
```

Layers, from the bottom:

1. **Pure libraries**, no window system and mostly no I/O: `quarry-core` (data model and the
   `Driver`/`Connection` traits), `quarry-sql`, `quarry-geo`, `quarry-editor`. Their tests run in seconds
   with plain `cargo test`.
2. **`quarry-drivers`**: the only crate that talks to servers (Tokio; `tokio-postgres`, `mysql_async`,
   `rusqlite`, `russh`). Integration tests need real databases.
3. **`quarry-ui`**: settings, themes, bundled fonts and icons, fuzzy matching, a picker, small shared widgets.
4. **View crates** (`quarry-editor-view`, `quarry-grid`, `quarry-plan-view`, `quarry-map-view`): each is a GPUI
   entity that takes plain data in and emits an event enum out. They never touch a connection; the
   application reacts to events (`GridEvent::SubmitChanges`, `PlanEvent::Rerun`, `MapEvent::FeatureClicked`,
   `EditorEvent` ...). Each has an `examples/*_demo.rs` that runs it alone with fake data.
5. **`quarry`**: owns connections, runs statements on a Tokio runtime and applies results back on the
   UI thread, and wires the view crates together.

Two conventions run through all of it. Everything in the editor core is indexed by **char**, not byte.
And no UI code waits: database work is spawned on the Tokio runtime (`crates/quarry/src/runtime.rs`) and
the answer comes back through `cx.spawn`.

## Building and testing

```sh
cargo test -p quarry-core -p quarry-sql -p quarry-geo -p quarry-editor      # seconds
cargo test -p quarry-drivers                                                # needs PostgreSQL + MariaDB
cargo test -p quarry-grid -p quarry-editor-view -p quarry-plan-view -p quarry-map-view -p quarry-ui
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

GPUI tests use `gpui-kit`'s `test-support` feature (`TestAppContext`), which runs without a window but
still links the platform libraries; on Linux run them under `xvfb-run` if the plain run complains about a
display. `QUARRY_FUZZ_SEED` / `QUARRY_FUZZ_ITERS` lengthen the editor fuzz test.

Wall-clock assertions in tests (`quarry-editor/src/tests/perf.rs`, completion and FK inference bounds in
`quarry-sql`, highlighting in `quarry-editor`) widen their limits tenfold when the `CI` environment variable
is set, because shared runners are slow and noisy. They still catch an accidental O(n²).

### Test databases

| Server | URL | Used by |
| --- | --- | --- |
| PostgreSQL 16 + PostGIS | `postgres://postgres:postgres@127.0.0.1:5432/quarry_demo` | `quarry-drivers`, `quarry-geo` (`tests/postgis`) |
| MariaDB 10.11 | `mysql://quarry:quarry@127.0.0.1:3306/quarry_demo` | `quarry-drivers` |

The URLs are hard-coded in `crates/quarry-drivers/tests/common/mod.rs`. Tests create uniquely named
schemas and databases, so they can run concurrently, and drop them afterwards; the MariaDB user therefore
needs global privileges (`GRANT ALL ON *.* TO 'quarry'@'%'`). One way to run both locally:

```sh
docker run -d --name quarry-pg -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=quarry_demo -p 5432:5432 postgis/postgis:16-3.4
docker run -d --name quarry-maria -e MARIADB_ROOT_PASSWORD=root -e MARIADB_DATABASE=quarry_demo \
  -e MARIADB_USER=quarry -e MARIADB_PASSWORD=quarry -p 3306:3306 mariadb:10.11
docker exec quarry-maria mariadb -uroot -proot -e "GRANT ALL ON *.* TO 'quarry'@'%'; FLUSH PRIVILEGES;"
```

SSH tunnel tests need a real `sshd` and are skipped unless `QUARRY_TEST_SSH=1`:
`crates/quarry-drivers/tests/ssh/setup.sh` (as root) starts one on `127.0.0.1:2222`. CI does this too.
`cargo run -p quarry-drivers --example seed -- postgres|mysql|sqlite ...` fills a database with the demo
schema (`customers`, `orders`, `stores`, `routes` ...).

## The headless screenshot harness

`script/shot` renders the **real** application under Xvfb with software Vulkan (llvmpipe) and writes a
PNG, so a UI change can be looked at without a display:

```sh
cargo build -p quarry
script/shot /tmp/shots/main.png -- --demo
script/shot /tmp/shots/palette.png steps.sh -- --demo      # drive the UI first
```

`steps.sh` is a bash snippet sourced after the window appears. It can call `xdotool key ctrl+p`,
`xdotool type 'select'`, `xdotool mousemove 400 300 click 1`, `sleep 0.5`, and `snap NAME` to write extra
PNGs next to the output. Environment: `SHOT_SIZE` (default `1600x1000`), `SHOT_WAIT` (seconds before the
final capture), `QUARRY_BIN` (default `target/debug/quarry`). Each run gets a throw-away
`QUARRY_CONFIG_DIR`, so shots are reproducible, and the app's stderr goes to `OUT.log`. Needs `xvfb`,
`xdotool`, ImageMagick (`import`) and a Vulkan driver (`mesa-vulkan-drivers`). The view crates' examples
can be photographed the same way with `QUARRY_BIN=target/debug/examples/grid_demo`.

## Environment variables

| Variable | Effect |
| --- | --- |
| `QUARRY_CONFIG_DIR` | where settings, connections, themes, history and consoles live |
| `QUARRY_CACHE_DIR` | base of the map tile cache (`<dir>/quarry/tiles`) |
| `QUARRY_DEMO=1` | same as `--demo` |
| `QUARRY_WINDOW_SIZE=WxH` | initial window size (used by `script/shot`) |
| `QUARRY_GRID_TIMING=1` | print grid frame timings |
| `QUARRY_TZ_OFFSET_MIN` | fixed time-zone offset for message timestamps |
| `QUARRY_TEST_SSH`, `QUARRY_TEST_SSH_DIR`, `QUARRY_TEST_SSH_PORT` | SSH integration tests |
| `QUARRY_FUZZ_SEED`, `QUARRY_FUZZ_ITERS` | editor fuzz test |
| `QUARRY_DUMP_PLANS` | `plan_insights` test: print captured plans |

## Adding a database driver

A driver turns one database's wire protocol into `quarry-core`'s shapes. Everything downstream (grid,
tree, plan panel, completion) consumes those shapes, so you do not touch the UI. Using a hypothetical
"DuckDB" as the example:

1. **Teach the contract about the new kind** (`crates/quarry-core/src/config.rs`). `DatabaseKind` is a closed
   enum: add a variant, then fix `ALL`, `label`, `default_port`, `is_file_based` and `quote_ident`. The
   compiler will point at every `match`; the other places to visit are `quarry-sql` (`keywords.rs`,
   `lexer.rs`, `complete.rs`, `generate.rs`, `lint.rs`, `plan.rs` for dialect behaviour such as quoting,
   function lists and string escaping), `quarry-plan-view/src/model.rs`, and in the application
   `connections/url.rs` (URL scheme), the connection dialog and the explorer. Contract changes belong to
   the repository owner (AGENTS.md): propose them rather than working around them.
2. **Create the module** `crates/quarry-drivers/src/duckdb/{mod.rs,introspect.rs,types.rs}` and add the client
   crate to `crates/quarry-drivers/Cargo.toml`. Mirror `sqlite.rs` for an embedded database and
   `postgres/` or `mysql/` for a server.
3. **Implement `Connection`** (`quarry-core/src/driver.rs`). The contract is stricter than it looks:
   - `execute` runs **exactly one statement**, one at a time per connection, in call order; callers split
     scripts with `quarry_sql::split_statements`. Honour `ExecOptions::row_limit` by *stopping the server*
     producing rows, not by truncating afterwards.
   - Map every column to a `Value` and a `TypeKind` (the grid sorts, formats and edits by `TypeKind`).
     Fill `Column::origin` (table and column) when the server reports it: the editable grid depends on
     it. Geometry must come back as `Value::Geometry(EWKB)`.
   - `cancel` may be called from the UI thread while `execute` is awaiting and must make that call return
     `Err(Error::Cancelled)`; it is `Ok` when nothing runs.
   - `introspect` returns a `Catalog`: schemas, tables and views, columns, primary and foreign keys,
     indexes, routines, with row estimates where cheap (completion and plan insights use them).
   - `explain(sql, analyze)` returns a `Plan` tree of `PlanNode`s plus the raw server text. With
     `analyze = true` the statement really runs, so wrap writes in a transaction that is rolled back.
     Keep the raw-output parser a public function (see `parse_postgres_plan_json`) so the plan view can test
     against captured plans without a server.
   - `in_transaction` must report the server's view; `close` must release any resources, including the SSH
     tunnel if the connection owns one.
4. **Implement `Driver`** (`kind`, `connect`) and register it in `driver_for` and, if it can be reached through
   SSH, in `connect()` in `crates/quarry-drivers/src/lib.rs` (the tunnel hands you a local port).
5. **Test it** the way the others are: `tests/duckdb.rs` using helpers from `tests/common/mod.rs`
   (unique schema names, `all()` for unlimited rows), covering types, NULLs, row limits, cancellation, errors
   with positions, introspection, `explain`, and transactions. Add the engine to the service containers in
   `.github/workflows/ci.yml` if it needs a server. Extend `src/demo.rs` so `--demo`-style seeding works.
6. **Docs**: the feature list in `README.md`, `crates/quarry-drivers/README.md`, and `CHANGELOG.md`.

## Adding a theme

Themes are gpui-kit `ThemeSet` JSON files (one file holds a family, usually a light and a dark variant).

1. Drop the file into `crates/quarry-ui/assets/themes/` and add an
   `include_str!` entry for it to the `BUNDLED` table in `crates/quarry-ui/src/themes.rs`; that is all the
   registration there is, and the theme then appears in the theme picker.
2. Credit the author and licence in `crates/quarry-ui/NOTICE`. Only add themes you may redistribute.
3. Check it in the gallery: `cargo run -p quarry-ui --example gallery`, or with `quarry --theme "<name>"`.

A user does not need to touch the source: *Import Zed Theme…* (command palette) converts a Zed theme
family (`zed.dev/schema/themes/v0.2.0.json`) and saves it as `<config>/themes/<slug>.json`;
dropping a ready-made gpui-kit theme file there works too. The converter
(`crates/quarry-ui/src/zed_import.rs`) starts from the bundled Quarry theme of the same appearance and
overrides only what the Zed file defines, so a sparse theme still produces a complete one.

## Adding a view feature, a rule of thumb

Put the decision in a pure function with a unit test (a sort comparator, a flame-graph layout, a plan
insight rule, a key translation), then call it from the GPUI layer, which should contain little more than
layout and event plumbing. The view crates follow this closely (`quarry-grid` lists its pure modules in its
README). When you change a UI, build it and look at a `script/shot` image before calling it done.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and on pull requests:

| Job | What | Blocking |
| --- | --- | --- |
| `fmt` | `cargo fmt --all -- --check` | no (until every crate is formatted) |
| `libraries` | clippy and tests for the five pure crates, against PostGIS and MariaDB service containers, plus the SSH tests against a throw-away `sshd` | yes |
| `app` (Linux, macOS, Windows) | `cargo build --workspace --all-targets`, then `cargo test --workspace --exclude quarry-drivers` (Linux under `xvfb-run` with software Vulkan) | yes |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | no (until clean) |

All jobs use `Swatinem/rust-cache`, `CARGO_INCREMENTAL=0` and `RUSTFLAGS=-C debuginfo=0` to keep build time and
the target directory small; Linux additionally links with `mold`. The toolchain version is pinned in
`rust-toolchain.toml` and mirrored by `RUST_TOOLCHAIN` in the workflows; bump both together. Watch a
run with `gh run watch` and read failures with `gh run view <id> --log-failed`.

## Releasing

Releases are cut from tags; `.github/workflows/release.yml` does the work.

1. Update `CHANGELOG.md` (move *Unreleased* under the new version) and `version` in the root
   `Cargo.toml` (`[workspace.package]`); run `cargo check` so `Cargo.lock` follows.
2. Tag with an annotation, which becomes the top of the release notes: `git tag -a v0.2.0 -m "Headline"` and
   `git push origin v0.2.0`.
3. The workflow builds four targets: Linux x86_64 (on Ubuntu 22.04 for an older glibc), macOS arm64 and x86_64,
   Windows x86_64. It produces:
   - `quarry-<v>-linux-x86_64.tar.gz` (binary, `quarry.desktop`, icons, `install.sh`, docs and notices) and
     `Quarry-<v>-linux-x86_64.AppImage` (built with `appimagetool`; it does not bundle libraries, the host
     supplies Vulkan, xkbcommon, Wayland and fontconfig, exactly as for the tarball);
   - `Quarry-<v>-macos-<arch>.zip` containing `Quarry.app` (`packaging/macos/bundle.sh`: `Info.plist` with
     bundle id `dev.quarry.Quarry`, `.icns` from the icon via `sips` and `iconutil`, ad-hoc `codesign`);
   - `quarry-<v>-windows-x86_64.zip` with `quarry.exe` linked for the GUI subsystem and the icon and version
     resource embedded with `rcedit`;
   - `SHA256SUMS` and release notes (`packaging/release-notes.sh`: the tag annotation, then the commit log
     since the previous tag).
4. A **draft** release is created with everything attached. Review it, then publish by hand.

To test the pipeline without releasing anything: *Actions → Release → Run workflow* (or
`gh workflow run release.yml --ref main -f ref=main -f dry_run=true`). A dry run builds and packages
everything and uploads the files as workflow artifacts (`dist-*`, `release-files`) and creates no release.

Packaging assets live in `packaging/`: `icons/` (the SVG, generated by `gen_icon.py`, and rasters produced by
`build.sh`), `linux/` (desktop file, `install.sh`, `package.sh`), `macos/` (`Info.plist.in`, `bundle.sh`),
`windows/quarry.ico`, `release-notes.sh`.

Not notarised: macOS notarisation and Windows code signing need paid certificates. Until the owner adds
them, users see Gatekeeper and SmartScreen warnings (see the user guide).

## Third-party licences

`script/third-party.py` regenerates [THIRD_PARTY.md](THIRD_PARTY.md) from `cargo metadata`; rerun it after
changing dependencies (`--check` exits non-zero if the file is stale).
