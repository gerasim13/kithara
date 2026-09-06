# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Build and test warnings, cleared. `Atomic*::fetch_update` is deprecated as of
  1.95 and its replacement cannot be spoken here: `loom` 0.7.2 carries only the
  old name, so routing through `kithara_platform::sync::atomic` would break the
  loom lane. The four sites moved to the `compare_exchange_weak` loop
  `fetch_update` compiles into, which keeps every ordering. MSRV is 1.95,
  `rkyv` 0.8.18 and `bytecheck` 0.8.3 retire their own deprecations, and
  `kithara-app`'s GUI-only modules are gated on `gui` so a `lib-only` build
  stops warning about 57 unused items.

- The `sccache` trap in the Clippy path, closed. A workstation Clippy run set
  `CARGO_INCREMENTAL=1` to cancel the blanket `0` the `justfile` exports, but
  `sccache` reads that variable too and aborts rather than fall back, for any
  language: `btls-sys` reached its C compiler through a CMake launcher that
  refused to run, and printed no compiler error because no compiler ran. No
  site in the repository sets a non-zero `CARGO_INCREMENTAL` now.

- Where the machine keeps its tools is asked, not written down.
  `CiHost::brew_root` owns the answer the Windows guest's `qemu` pair spelled
  out as absolute paths into one Homebrew prefix, and a guard now keeps the
  executor's sources from writing one down again.

- Mac CI host cleanup. The hourly pass hung inside `opendir` on a volume that
  had stopped answering and launchd starts no second instance, so the host
  refused jobs for space for a day; a watchdog ends a pass at
  `cleanup_deadline_seconds`, and under `Aggressive` or `Reject` cleanup
  reclaims what the volume is short of the soft floor rather than judging by
  one cache's ceiling. `deps:deny` then spent twenty-five minutes on the
  `boringssl` submodule's refs because libgit2 does not read the
  `GIT_CONFIG_COUNT` that pins the HTTP version, so Cargo fetches through the
  git binary. The lane still gates a quarantine pipeline directly instead of
  reporting to the verdict, which let one network stall hold every pull
  request; that is open.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`, published at the tempo level the
  detector reports and tagged `grid_bpm_from_beats_v4`. Left: the deck scenario
  on a release build with the full model, and the size of the resume blob.

## Next

- The workspace's own crates are still at `"z"` - a per-package glob reaches
  every third-party package but not them, and raising them is a measured
  change of its own.
- No runtime number backs the release optimization. Decode throughput, stretch
  cost and render-budget headroom were never measured before or after, so the
  case rests on codegen rather than on a benchmark.
- `crates/kithara-ffi/.wasm-slim.toml` budgets the wasm bundle at
  29000/31000/33000 KiB against a May baseline of ~28.2 MiB, while a local
  `dist` weighs 3565 KiB. Either the gate is stale or the two numbers weigh
  different things, and the `web-size` lane on GitLab settles which.
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
