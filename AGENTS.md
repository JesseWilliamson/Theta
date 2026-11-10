# Working in this repository

Quarry is a keyboard-first database IDE built on GPUI (via the `gpui-kit` crate).
Read `docs/ARCHITECTURE.md` before changing anything.

## Rules

- **Stay inside your assigned crate/paths.** Other agents work in sibling crates at
  the same time. Never edit `crates/quarry-core` unless your task says so — it is the
  shared contract. If the contract is genuinely missing something, stop and report
  it in your final message instead of working around it.
- **No stubs, no `todo!()`, no placeholder data in shipped code paths.** If something
  cannot be done, say so in your final report; do not fake it.
- **Tests are part of the task.** Pure logic gets unit tests next to the code. Run
  `cargo test -p <your-crate>` and `cargo clippy -p <your-crate> -- -D warnings`
  before reporting; paste the summary lines in your report.
- **Never invent an API.** For GPUI / gpui-kit, grep the real source under
  `/home/user/refs/gpui-kit` (same version we depend on) and
  `~/.cargo/registry/src/*/gpui-pre-0.3.12`. Start with
  `/home/user/refs/gpui-kit/skills/gpui-kit/SKILL.md` and its `references/`.
- Add dependencies to the root `[workspace.dependencies]` only by appending new
  lines (others edit that file too — keep edits minimal, re-read before writing).
- Comments explain *why* (constraints, invariants, trade-offs), not what the next
  line does. The repository owner reads this code to learn from it.
- Rust 2024 edition, `rustfmt` defaults with `max_width = 110` (see rustfmt.toml).

## Building

- 4 cores, shared machine. Build only your crate: `cargo build -p <crate>`.
- If your brief gives you a `CARGO_TARGET_DIR`, export it in every cargo command so
  you do not block other agents on the build lock.

## Committing

Commit your own paths only, small logical commits, imperative subject line:

    git add crates/<your-crate> && git commit -m "drivers: add MySQL introspection"

If `git` reports `index.lock` exists, another agent is committing: wait 2s and retry.
Do not push, do not rebase, do not touch other agents' uncommitted files, never use
`git add -A` or `git commit -a`.

## Local test services (already running)

- PostgreSQL 16 + PostGIS: `postgres://postgres:postgres@127.0.0.1:5432/quarry_demo`
- MariaDB 10.11: `mysql://quarry:quarry@127.0.0.1:3306/quarry_demo`

## Seeing the UI

`cargo build -p quarry && script/shot /tmp/shots/name.png [steps.sh]` renders the
real app under Xvfb and writes a PNG (read `script/shot` for the step language).
Look at the PNG with the Read tool. A UI task is not done until you have looked at
a screenshot of it and it looks right: aligned, no clipped text, consistent spacing.
