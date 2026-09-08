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

- `kithara-analysis` builds and runs its pass on `wasm32`: the worker's compute
  seam spawns a thread per admitted job under the `OwnedPoolConfig` the native
  Rayon backend takes. `kithara-ffi --features wasm` exposes
  `AudioPlayer.analyze(trackId)` and `AudioPlayer.setAnalysisObserver(fn)`; a
  publication is one plain object with waveform and beat grid as copied typed
  arrays, one pass live per track, under `just platform wasm check`.

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

- Open defect, its own `kithara-analysis` task: a browser MP3 decodes from
  frame 1105, the LAME delay, while `prepare_detection` places windows on the
  global hop grid, `release_detected` drops the unread head, and `beat_state()`
  compares coverage against `Runs::taken`, which grows on intake, not on read.
  On `MP3_CLICKS126_30S` a pass publishes `settled=true, beats=0` in 4 of 6
  Safari runs, so `web-analysis` asserts the field set, the waveform and
  `beats >= 0`. The native scheduler opens runs the same way on a mid-gap
  seek, uncaught by any native test.

- Flake: one Safari run in five of `kithara-analysis --lib` fails in
  `tests::worker::a_pass_publishes_above_the_revision_its_caller_holds` with
  `Out of bounds memory access` in `Node::cancel` at teardown. Unconfirmed:
  `wasm_safe_thread` 0.1.1 decrements `exit_state` in three JS handlers
  without a once-guard while kithara drops the `JoinHandle` at spawn.
