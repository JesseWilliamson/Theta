# Development guide

How Theta is put together, how to change it, and how it is tested, built and released. Read
[ARCHITECTURE.md](ARCHITECTURE.md) first for the reasoning; this document is the practical companion.
[AGENTS.md](../AGENTS.md) holds the working rules used by the automated contributors on this repository
(build one crate at a time, commit only your own paths, never invent GPUI APIs).

## How the pieces fit

```text
 theta (application: workspace, explorer, connections, dialogs, palette, run / edit / export orchestration)
   │
   ├─► theta-editor-view ─► theta-editor, theta-sql, theta-core, theta-ui
   ├─► theta-grid        ─► theta-core, theta-geo, theta-ui
   ├─► theta-plan-view   ─► theta-core, theta-sql, theta-ui
   ├─► theta-map-view    ─► theta-geo, theta-ui
   ├─► theta-drivers     ─► theta-core
   └─► theta-sql, theta-geo, theta-editor, theta-ui, theta-core
```

Layers, from the bottom:

1. **Pure libraries**, no window system and mostly no I/O: `theta-core` (data model and the
   `Driver`/`Connection` traits), `theta-sql`, `theta-geo`, `theta-editor`. Their tests run in seconds
   with plain `cargo test`.
2. **`theta-drivers`**: the only crate that talks to servers (Tokio; `tokio-postgres`, `mysql_async`,
   `rusqlite`, `russh`). Integration tests need real databases.
3. **`theta-ui`**: settings, themes, bundled fonts and icons, fuzzy matching, a picker, small shared widgets. It
   needs GPUI but depends on nothing else in the workspace.
4. **View crates** (`theta-editor-view`, `theta-grid`, `theta-plan-view`, `theta-map-view`): each is a GPUI
   entity that takes plain data in and emits an event enum out. They never touch a connection; the
   application reacts to events (`GridEvent::SubmitChanges`, `PlanEvent::Rerun`, `MapEvent::FeatureClicked`,
   `EditorEvent::Effect` ...). Each has an `examples/*_demo.rs` that runs it alone with fake data.
5. **`theta`**: owns connections, runs statements on a Tokio runtime and applies results back on the
   UI thread, and wires the view crates together.

Two conventions run through all of it. Everything in the editor core is indexed by **char**, not byte.
And no UI code waits: database work is spawned on the Tokio runtime (`crates/theta/src/runtime.rs`) and
the answer comes back through `cx.spawn`.

## Building and testing

```sh
cargo test -p theta-core -p theta-sql -p theta-geo -p theta-editor      # seconds
cargo test -p theta-drivers                                                # needs PostgreSQL + MariaDB
cargo test -p theta-grid -p theta-editor-view -p theta-plan-view -p theta-map-view -p theta-ui
cargo test -p theta                                                        # the application: runs, edits, persistence
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

GPUI tests use `gpui-kit`'s `test-support` feature (`TestAppContext`), which runs without a window but
still links the platform libraries; on Linux run them under `xvfb-run` if the plain run complains about a
display. `THETA_FUZZ_SEED` / `THETA_FUZZ_ITERS` lengthen the editor fuzz test.

Wall-clock assertions in tests (`theta-editor/src/tests/perf.rs`, completion and FK inference bounds in
`theta-sql`, highlighting in `theta-editor`) widen their limits tenfold when the `CI` environment variable
is set, because shared runners are slow and noisy. They still catch an accidental O(n²).

### Test databases

| Server | URL | Used by |
| --- | --- | --- |
| PostgreSQL 16 + PostGIS | `postgres://postgres:postgres@127.0.0.1:5432/quarry_demo` | `theta-drivers`, `theta-geo` (`tests/postgis`) |
| MariaDB 10.11 | `mysql://quarry:quarry@127.0.0.1:3306/quarry_demo` | `theta-drivers` |

The other products read their server from an environment variable and are skipped (with a printed
`SKIPPED` line) when it is unset: `THETA_TEST_COCKROACH_URL`, `THETA_TEST_TIMESCALE_URL`,
`THETA_TEST_YUGABYTE_URL`, `THETA_TEST_REDSHIFT_URL`, `THETA_TEST_TIDB_URL`, `THETA_TEST_SQLSERVER_URL` and
`THETA_TEST_CLICKHOUSE_URL` (`crates/theta-drivers/tests/kinds_live.rs`, `clickhouse.rs`; the table of variables is
at the top of the first). DuckDB is embedded and always runs. The PostgreSQL and MariaDB URLs are
hard-coded in `crates/theta-drivers/tests/common/mod.rs`. Tests create uniquely named
schemas and databases, so they can run concurrently, and drop them afterwards; the MariaDB user therefore
needs global privileges (`GRANT ALL ON *.* TO 'quarry'@'%'`). One way to run both locally:

```sh
docker run -d --name theta-pg -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=quarry_demo -p 5432:5432 postgis/postgis:16-3.4
docker run -d --name theta-maria -e MARIADB_ROOT_PASSWORD=root -e MARIADB_DATABASE=quarry_demo \
  -e MARIADB_USER=quarry -e MARIADB_PASSWORD=quarry -p 3306:3306 mariadb:10.11
docker exec theta-maria mariadb -uroot -proot -e "GRANT ALL ON *.* TO 'quarry'@'%'; FLUSH PRIVILEGES;"
```

SSH tunnel tests need a real `sshd` and are skipped unless `THETA_TEST_SSH=1`:
`crates/theta-drivers/tests/ssh/setup.sh` (as root) starts one on `127.0.0.1:2222`. CI does this too.
`cargo run -p theta-drivers --example seed -- postgres|mysql|sqlite ...` fills a database with the demo
schema (`customers`, `orders`, `stores`, `routes` ...).

## The headless screenshot harness

`script/shot` renders the **real** application under Xvfb with software Vulkan (llvmpipe) and writes a PNG,
so a UI change can be looked at without a display:

```sh
cargo build -p theta
script/shot /tmp/shots/main.png -- --demo                  # arguments after `--` go to the app
script/shot /tmp/shots/palette.png steps.sh -- --demo      # drive the UI first
THETA_BIN=/path/to/theta script/shot out.png steps.sh -- --connect postgres://...
```

`steps.sh` is a bash snippet that the script `source`s once the window is visible and sized
(`SHOT_SIZE`, default `1600x1000`). It can call `xdotool key ctrl+shift+p`, `xdotool type --delay 15
'select'`, `xdotool mousemove 400 300 click 1`, `sleep 0.5`, and `snap NAME`, which writes `NAME.png` next to
the output file. After the steps the script waits `SHOT_WAIT` seconds (default 2), captures the output
file and kills the app. Environment:

| Variable | Default | Effect |
| --- | --- | --- |
| `THETA_BIN` | `target/debug/theta` | the binary to run (a view crate's example works too, e.g. `target/debug/examples/grid_demo`) |
| `SHOT_SIZE` | `1600x1000` | virtual screen and window size (also passed to the app as `THETA_WINDOW_SIZE`) |
| `SHOT_WAIT` | `2` | seconds between the last step and the final capture |
| `THETA_CONFIG_DIR` | a fresh temporary directory | set it to keep settings, consoles and history between runs, e.g. to test persistence |

Each run gets a throw-away `THETA_CONFIG_DIR` unless you set one, so shots are reproducible. The app's
stderr goes to `OUT.log` (next to the PNG), and if the app exits before its window appears the script prints
the end of that log. Needs `xvfb`, `xdotool`, ImageMagick (`import`) and a Vulkan driver
(`mesa-vulkan-drivers`).

Tips for steps that drive the editor:

- Helix starts in Normal mode. `i` enters Insert mode; `esc` leaves it. If the completion popup is open the
  first `esc` only closes the popup, so press `esc` twice.
- `ctrl+Return` runs the statement under the cursor from any mode; run results need a second or two on
  llvmpipe, so `sleep` before `snap`.
- To clear the editor in Normal mode send `%` then `d`. (`ctrl+a` is "increment number" in Helix.)
- Mouse positions are screen pixels at `SHOT_SIZE`. In the default layout the bottom-panel tabs are at
  y = 668 and the title-bar buttons at y = 16.
- The first screenshot after start-up can show the toast "Connected to …"; it disappears after a few seconds.

## Environment variables

| Variable | Effect |
| --- | --- |
| `THETA_CONFIG_DIR` | where settings, connections, themes, history, consoles and the layout live |
| `THETA_CACHE_DIR` | base of the map tile cache (`<dir>/theta/tiles`) |
| `THETA_DEMO=1` | same as `--demo` |
| `THETA_WINDOW_SIZE=WxH` | initial window size (used by `script/shot` and the demos) |
| `THETA_TILE_URL_OVERRIDE` | tile URL template that wins over the `map_tile_url` setting (a test hook) |
| `THETA_GRID_TIMING=1` | print grid frame timings |
| `THETA_EDITOR_PERF=1` | print editor frame costs (`editor_demo`, the app) |
| `THETA_MAP_STATS` | `map_demo` prints map statistics |
| `THETA_TZ_OFFSET_MIN` | fixed time-zone offset, in minutes, for the timestamps in the Messages tab (default UTC) |
| `THETA_TEST_SSH`, `THETA_TEST_SSH_DIR`, `THETA_TEST_SSH_PORT` | SSH integration tests |
| `THETA_FUZZ_SEED`, `THETA_FUZZ_ITERS` | editor fuzz test |
| `THETA_DUMP_PLANS` | `plan_insights` test: print captured plans |

## Adding a database driver

A driver turns one database's wire protocol into `theta-core`'s shapes. Everything downstream (grid, tree,
plan panel, completion) consumes those shapes, so you do not touch the UI for the data path. Using a
hypothetical "DuckDB" as the example:

1. **Teach the contract about the new kind** (`crates/theta-core/src/config.rs`). `DatabaseKind` is a
   closed enum: add a variant, add it to `ALL`, and add **one descriptor** (`KindInfo`) returned by
   `DatabaseKind::info`: label, icon key, default port, URL schemes, filter keywords, placeholders and the
   form `Sections` it uses (network fields, TLS, SSH, file chooser, "create if missing"). The connection
   dialog (type dropdown, which fields appear, port defaults), the URL parser and `ConnectionConfig::new`
   are driven by that descriptor, so the dialog needs no layout change; a test checks every descriptor for
   consistency (`every_kind_has_a_consistent_descriptor`). Then fix `quote_ident` and let the compiler point
   at every other exhaustive `match`; the places that decide dialect behaviour are:
   - `theta-sql`: `keywords.rs` (keywords, functions, types), `lexer.rs` (quoting, comments), `complete.rs`,
     `format.rs`, `lint.rs`, `generate.rs` (literals, identifier quoting), `fk.rs`, `plan.rs` (insights),
     `script.rs` (statement splitting and classification), `export.rs`;
   - `theta-plan-view/src/model.rs` and `sqlcolor.rs` (how a plan is labelled and highlighted);
   - `theta-ui/src/db_icon.rs` (`DbIcon::from_key`; a kind without a glyph gets the generic one) and, in
     the application, `sqlgen.rs` (explorer-generated SQL), `exec/pick.rs`, `exec/fk_ui.rs` and `exec/changes.rs`.

   Contract changes belong to the repository owner (AGENTS.md): propose them rather than working around them.
2. **Create the module** `crates/theta-drivers/src/duckdb/{mod.rs,introspect.rs,types.rs}` (or a single
   `duckdb.rs` for a small embedded one) and add the client crate to `crates/theta-drivers/Cargo.toml`.
   Mirror `sqlite.rs` for an embedded database and `postgres/` or `mysql/` for a server. Plan parsers live
   beside the module as `plan_pg.rs` and `plan_mysql.rs`.
3. **Implement `Connection`** (`crates/theta-core/src/driver.rs:14`). The contract is stricter than it looks:
   - `execute` runs **exactly one statement**, one at a time per connection, in call order; the application
     splits scripts with `theta_sql::split_statements`. Honour `ExecOptions::row_limit`
     (`crates/theta-core/src/value.rs:164`) by *stopping the server* producing rows, not by truncating
     afterwards, and set `ResultSet::truncated`.
   - Map every column to a `Value` and a `TypeKind` (the grid sorts, formats and edits by `TypeKind`). Fill
     `Column::origin` (schema, table, column) when the server reports it: the editable grid depends on it.
     Geometry must come back as `Value::Geometry(EWKB)`.
   - Return a `StatementOutcome` with `result` for row-returning statements, `rows_affected` for writes,
     any server `notices`, and the `elapsed` time you measured.
   - `cancel` may be called from the UI thread while `execute` is awaiting and must make that call return
     `Err(Error::Cancelled)`; it is `Ok` when nothing runs.
   - Report a dead session as `Error::Closed` (`crates/theta-core/src/error.rs:34`): the application
     reconnects when it sees it (`finish_statement` in `crates/theta/src/workspace/run.rs`).
   - `introspect` returns a `Catalog`: schemas, tables and views, columns, primary and foreign keys, indexes,
     routines, with row estimates where cheap (completion and plan insights use them).
   - `explain(sql, analyze)` returns a `Plan` tree of `PlanNode`s plus the raw server text. With
     `analyze = true` the statement really runs, so wrap writes in a transaction that is rolled back. Keep
     the raw-output parser a public function (see `parse_postgres_plan_json` in
     `crates/theta-drivers/src/lib.rs:23`) so the plan view can test against captured plans without a
     server.
   - `in_transaction` must report the server's view; `close` must release any resources, including the SSH
     tunnel if the connection owns one.
4. **Implement `Driver`** (`kind`, `connect`) and register it in `driver_for`
   (`crates/theta-drivers/src/lib.rs:58`) and, if it can be reached through SSH, in
   `connect_with_notices` in the same file (the tunnel hands you a local port).
5. **Test it** the way the others are: `crates/theta-drivers/tests/duckdb.rs` using the helpers in
   `tests/common/mod.rs` (unique schema names, `all()` for unlimited rows), covering types, NULLs, row
   limits, cancellation, errors with positions, introspection, `explain` and transactions. Put captured
   plans in `tests/fixtures/`. Add the engine to the service containers in `.github/workflows/ci.yml` if it
   needs a server, and extend `crates/theta-drivers/src/demo.rs` so `--demo`-style seeding works for it
   (`cargo run -p theta-drivers --example seed -- ...`).
6. **Docs**: the feature list in `README.md`, `crates/theta-drivers/README.md`, `docs/USER_GUIDE.md` (connection
   dialog fields) and `CHANGELOG.md`.

## Adding a theme

Themes are gpui-kit `ThemeSet` JSON files (one file holds a family, usually a light and a dark variant).

1. Drop the file into `crates/theta-ui/assets/themes/` and add an
   `include_str!` entry for it to the `BUNDLED` table in `crates/theta-ui/src/themes.rs`; that is all the
   registration there is, and the theme then appears in the theme picker.
2. Credit the author and licence in `crates/theta-ui/NOTICE`. Only add themes you may redistribute.
3. Check it in the gallery: `cargo run -p theta-ui --example gallery`, or with `theta --theme "<name>"`.

A user does not need to touch the source: *Import Zed Theme…* (command palette) converts a Zed theme
family (`zed.dev/schema/themes/v0.2.0.json`) and saves it as `<config>/themes/<slug>.json`;
dropping a ready-made gpui-kit theme file there works too. The converter
(`crates/theta-ui/src/zed_import.rs`) starts from the bundled Theta theme of the same appearance and
overrides only what the Zed file defines, so a sparse theme still produces a complete one.

## Adding a view feature, a rule of thumb

Put the decision in a pure function with a unit test (a sort comparator, a flame-graph layout, a plan
insight rule, a key translation), then call it from the GPUI layer, which should contain little more than
layout and event plumbing. The view crates follow this closely (`theta-grid` lists its pure modules in its
README). When you change a UI, build it and look at a `script/shot` image before calling it done.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and on pull requests:

| Job | What | Blocking |
| --- | --- | --- |
| `fmt` | `cargo fmt --all -- --check` | no (until every crate is formatted) |
| `libraries` | clippy and tests for the five pure crates, against PostGIS and MariaDB service containers, plus the SSH tests against a throw-away `sshd` | yes |
| `app` (Ubuntu 24.04, macOS 15, Windows Server 2022) | `cargo build --workspace --all-targets`, then `cargo test --workspace --exclude theta-drivers` (Linux under `xvfb-run` with software Vulkan) | yes |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | no (until clean) |

All jobs use `Swatinem/rust-cache`, `CARGO_INCREMENTAL=0` and `RUSTFLAGS=-C debuginfo=0` to keep build time and
the target directory small; Linux additionally links with `mold`. CI pins the exact toolchain through `RUST_TOOLCHAIN` / `RUSTUP_TOOLCHAIN` in the workflows (currently 1.99.1; gpui's macOS
backend needs at least 1.99), while `rust-toolchain.toml` just says `stable` for local work; bump the
workflow values deliberately. Watch a
run with `gh run watch` and read failures with `gh run view <id> --log-failed`.

## Releasing

Releases are cut from tags; `.github/workflows/release.yml` does the work.

1. Update `CHANGELOG.md` (move *Unreleased* under the new version) and `version` in the root
   `Cargo.toml` (`[workspace.package]`); run `cargo check` so `Cargo.lock` follows. The file names below use
   the version of the `theta` package, not the tag, so they must agree: for the tag `v0.1.0-alpha.2` set
   `version = "0.1.0-alpha.2"` first (the workspace already says so).
2. Tag with an annotation, which becomes the top of the release notes: `git tag -a v0.2.0 -m "Headline"` and
   `git push origin v0.2.0`.
3. The workflow builds four targets: Linux x86_64 (on Ubuntu 22.04 for an older glibc), macOS arm64 and x86_64,
   Windows x86_64. It produces:
   - `theta-<v>-linux-x86_64.tar.gz` (binary, `theta.desktop`, icons, `install.sh`, docs and notices) and
     `Theta-<v>-linux-x86_64.AppImage` (built with `appimagetool`, which the workflow downloads; if the
     download fails the AppImage is skipped. It does not bundle libraries, the host supplies Vulkan,
     xkbcommon, Wayland and fontconfig, exactly as for the tarball);
   - `Theta-<v>-macos-<arch>.zip` containing `Theta.app` (`packaging/macos/bundle.sh`: `Info.plist` with
     bundle id `dev.theta.Theta`, `.icns` from the icon via `sips` and `iconutil`, ad-hoc `codesign`);
   - `theta-<v>-windows-x86_64.zip` with `theta.exe` linked for the GUI subsystem and the icon and version
     resource embedded with `rcedit`;
   - `SHA256SUMS` and release notes (`packaging/release-notes.sh`: the tag annotation, then the commit log
     since the previous tag).
4. A **draft** release is created with everything attached (a tag with a `-` in it, such as
   `v0.1.0-alpha.2`, is marked as a pre-release). Review it, then publish by hand.

To make a draft for a tag that does not exist yet, run the workflow by hand with a tag name: `gh workflow run
release.yml --ref main -f ref=main -f dry_run=false -f tag=v0.1.0-alpha.2`. GitHub creates the tag at that
commit when the draft is published.

To test the pipeline without releasing anything: *Actions → Release → Run workflow* (or
`gh workflow run release.yml --ref main -f ref=main -f dry_run=true`). A dry run builds and packages
everything and uploads the files as workflow artifacts (`dist-*`, `release-files`) and creates no release.

Packaging assets live in `packaging/`: `icons/` (the SVGs, generated by `gen_icon.py`, and rasters produced by
`build.sh`), `linux/` (desktop file, `install.sh`, `package.sh`), `macos/` (`Info.plist.in`, `bundle.sh`),
`windows/theta.ico`, `release-notes.sh`.

Not notarised: macOS notarisation and Windows code signing need paid certificates. Until the owner adds
them, users see Gatekeeper and SmartScreen warnings (see the user guide).

## Third-party licences

`script/third-party.py` regenerates [THIRD_PARTY.md](THIRD_PARTY.md) from `cargo metadata`; rerun it after
changing dependencies (`--check` exits non-zero if the file is stale).
