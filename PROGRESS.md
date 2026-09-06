# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Build and test warnings, cleared. `Atomic*::fetch_update` is deprecated as of
  1.95, and its replacement cannot be spoken here: `loom` 0.7.2 carries only the
  old name, so anything routed through `kithara_platform::sync::atomic` would
  break the loom lane. The four sites moved to the `compare_exchange_weak` loop
  `fetch_update` is documented to compile into, which both toolchains have and
  which keeps every ordering. MSRV is 1.95, `rkyv` 0.8.18 and `bytecheck` 0.8.3
  retire their own deprecations, and `kithara-app`'s GUI-only modules are gated
  on `gui` so a `lib-only` build stops warning about 57 unused items.
- The `sccache` trap in the Clippy path, closed. A workstation Clippy run set
  `CARGO_INCREMENTAL=1` to cancel the blanket `0` the `justfile` exports, but
  `sccache` reads that variable too and aborts rather than fall back, for any
  language: `btls-sys` reached its own C compiler through a CMake launcher that
  refused to run, and printed no compiler error because no compiler ran.
  Clearing both variables says the same thing to Cargo - workspace crates are
  incremental by default and registry ones never - and nothing to anyone else.
  No site in the repository sets a non-zero `CARGO_INCREMENTAL` now.
- Mac CI host cleanup. The host spent a day refusing jobs for space while its
  hourly pass was gone: the agent hung inside `opendir` on a volume that had
  stopped answering, and launchd starts no second instance while the first is
  alive. A watchdog thread now ends a pass at `cleanup_deadline_seconds`. The
  pass that did run freed nothing, because `build_cache_size` is a ceiling over
  one cache and says nothing about whether the volume has room; under
  `Aggressive` or `Reject` it now reclaims what the volume is short of the soft
  floor as well. Cleanup also judges by the volume it measured rather than
  reading free space a second time.

  Verifying any of this was blocked by a second defect. `deps:deny` spent
  twenty-five minutes listing the `boringssl` submodule's refs before its
  job timed out: `GIT_CONFIG_COUNT` pins the HTTP version for the git
  binary, and libgit2, which Cargo fetches with, does not read it. Cargo
  now fetches through the binary, so both halves see one version. The lane
  still gates a quarantine pipeline directly instead of reporting to the
  verdict, which is what let one network stall hold every pull request;
  that is open.

  Then six pull requests went red at once for a queue someone emptied. The
  bridge read a cancelled pipeline as a verdict and recorded it terminal, so
  nothing addressed them again; a cancellation now releases the run and opens
  the next attempt. The sweep that removes verification branches also kept
  every ref naming the current base, including the ones whose pull request had
  moved on, and cancels a ref's queued run before deleting it.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. The grid is published at the tempo
  level the detector reports, tagged `grid_bpm_from_beats_v4`. Left: the
  reported deck scenario on a release build with the full model, and the size
  of the resume blob.

- Harness and document revision. `AGENTS.md` routes instead of restating; the
  `style` namespace budgets documents with `doc_size`, blocks drift with
  `doc_staleness`, and holds every crate README to one shape with
  `readme_shape`. All three queues are at zero, and `just lint full` runs the
  namespace on the Apple lint lane.

## Next

- The workspace's own crates are still at `"z"` - a per-package glob reaches
  every third-party package but not them, and raising them is a measured
  change of its own.
- No runtime number backs the release optimization. Decode throughput, stretch
  cost and render-budget headroom were never measured before or after, so the
  case rests on codegen rather than on a benchmark.
- `crates/kithara-ffi/.wasm-slim.toml` budgets the wasm bundle at
  29000/31000/33000 KiB against a May baseline of ~28.2 MiB, while a local
  `dist` weighs 3565 KiB. Either the gate is an order of magnitude stale or the
  two numbers weigh different things; the `web-size` lane on GitLab is the only
  place that settles it.
- `block` 0.1.6 is a future-incompat report no change here can answer: it
  reaches the tree through `cpal` and has no published successor.
- `kithara-ui` still warns under `--no-default-features --features render` and
  `--features vello`, where the widget layer compiles without a host. That is
  627 items and its own change.
- Work the comment queue down by hand: `--fix` is exhausted, so all 668
  findings are decisions.
- 439 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.

## Blocked

- Nothing.
