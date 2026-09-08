# Progress

Current work, next steps, and blockers. The
[Projects board](https://github.com/users/gerasim13/projects/3) owns roadmap
status; git owns completed changes. Keep this file short.

## In Flight

- Build warnings and the Clippy `sccache` configuration trap are resolved.
  MSRV is 1.95; GUI-only app modules require `gui`. Lint autofixes reduced
  debt, and the commit hook now includes the style gate.

- Configuration document for `kithara-app`: `app.yaml` plus an optional
  overlay, env-expanded before typing, each section carrying its owning
  crate's `#[derive(Patch)]` type. Open: assembly sits in `main.rs` where no
  test pins it, and twenty-two files take pools from `PoolsSection::default()`.

- Tooling parameter ownership: every policy number `xtask` and
  `kithara-devtools` held as a `const` has a config owner, and spawned programs
  resolve through `ToolsConfig`.

- Mac CI host cleanup gave the hourly pass a watchdog. Open: `deps:deny` gates
  a quarantine pipeline directly, so one network stall holds every pull
  request.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. Left: the deck scenario on a release
  build with the full model, and the size of the resume blob.

- PR #322 review fixes retain the app fixture ticker through teardown and
  complete async harness calls in the opt-in network suites. Both network
  binaries compile; the local ticker regression passes with `no_block` and
  fails when the ticker is stopped at construction.
  Analysis tests use generated fixture files instead of global `/tmp` paths.
  Remote playback and device acceptance remain unverified.

## Next

- 678 comment findings are decisions `--fix` cannot make.
- `kithara-ui` warns on 627 items where the widget layer compiles without a
  host, under `--features render` and `--features vello`.
- `.wasm-slim.toml` budgets wasm at 29000 KiB against a local `dist` of 3565
  KiB; the `web-size` lane on GitLab settles which number is real.
- No runtime number backs the release optimization, and the workspace's own
  crates are still at `"z"`.
- `block` 0.1.6 is a future-incompat report nothing here can answer: it reaches
  the tree through `cpal` and has no successor.

## Blocked

- Nothing.
