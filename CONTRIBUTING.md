# Contributing

Thanks for helping with Quarry. The short version:

1. Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). The first rule of
   the codebase is that anything testable without a window lives outside the GUI crates.
2. Build and test the crate you are changing, not the whole workspace (GPUI builds are slow):
   `cargo test -p quarry-sql`. Run `cargo fmt` and `cargo clippy -p <crate> --all-targets -- -D warnings`.
3. Pure logic gets unit tests next to the code. Bug fixes get a test that fails without the fix. UI changes
   come with a look at a `script/shot` screenshot (aligned, nothing clipped, sensible empty and error states).
4. No stubs, `todo!()` or placeholder data in shipped code paths. If something cannot be done, say so.
5. Comments explain *why* (constraints, invariants, trade-offs), not what the next line does.
6. Small, focused commits with an imperative subject (`grid: keep scroll position when filtering`). Open a
   pull request against `main`; CI must pass (`libraries` and the three `app` builds are required, `fmt` and
   `clippy` are advisory for now).
7. Changes to `quarry-core` are contract changes: describe the need in the pull request and expect discussion.
8. Add a line under *Unreleased* in [CHANGELOG.md](CHANGELOG.md) for anything a user would notice.

The repository's licence has not been chosen yet; please ask the owner before contributing larger changes.
