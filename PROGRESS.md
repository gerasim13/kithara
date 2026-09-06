# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Build and test warnings, cleared. `Atomic*::fetch_update` is deprecated as of
  1.95 and its replacement cannot be named here: `loom` 0.7.2 carries only the
  old name, so routing through `kithara_platform::sync::atomic` would break the
  loom lane. The four sites moved to the `compare_exchange_weak` loop it
  compiles into, keeping every ordering. MSRV is 1.95; `rkyv` 0.8.18 and
  `bytecheck` 0.8.3 retire theirs, and `kithara-app`'s GUI-only modules are
  gated on `gui` so a `lib-only` build no longer warns on 57 items.

- The `sccache` trap in the Clippy path, closed. A workstation Clippy run set
  `CARGO_INCREMENTAL=1` to cancel the blanket `0` the `justfile` exports, but
  `sccache` reads that variable too and aborts rather than fall back, for any
  language: `btls-sys` reached its C compiler through a CMake launcher that
  refused to run, so no compiler error was printed. No site sets a non-zero
  `CARGO_INCREMENTAL` now.

- Configuration document for `kithara-app`: `app.yaml` plus an optional
  overlay, merged and env-expanded before typing, each section carrying its
  owning crate's `#[derive(Patch)]` type from the new `kithara-macros`. No
  patch struct is hand-written. Open: assembly sits in `main.rs` where no test
  pins it, so `downloader` and `flush` parsed and were dropped until a read
  found them; twenty-two files still take pools from `PoolsSection::default()`.

- Mac CI host cleanup. The hourly pass hung inside `opendir` on a volume that
  had stopped answering and launchd starts no second instance, so the host
  refused jobs for space for a day; a watchdog ends a pass at
  `cleanup_deadline_seconds`, and under `Aggressive` or `Reject` cleanup
  reclaims what the volume is short of the soft floor rather than judging by
  one cache's ceiling. `deps:deny` then spent twenty-five minutes on the
  `boringssl` submodule's refs because libgit2 ignores the `GIT_CONFIG_COUNT`
  that pins the HTTP version, so Cargo fetches through the git binary. Open:
  the lane gates a quarantine pipeline directly instead of reporting to the
  verdict, so one network stall holds every pull request.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`, published at the tempo the detector
  reports and tagged `grid_bpm_from_beats_v4`. Left: the deck scenario on a
  release build with the full model, and the size of the resume blob.

## Next

- `suite_network` has been dark since `#260`; the handover census found it.
- The workspace's own crates are still at `"z"`: a per-package glob reaches
  every third-party package but not them, and raising them is its own
  measured change.
- No runtime number backs the release optimization: decode throughput, stretch
  cost and render-budget headroom were never measured, so the case rests on
  codegen rather than on a benchmark.
- `crates/kithara-ffi/.wasm-slim.toml` budgets the wasm bundle at
  29000/31000/33000 KiB against a May baseline of ~28.2 MiB while a local
  `dist` weighs 3565 KiB; the `web-size` lane on GitLab settles whether the
  gate is stale or the two numbers weigh different things.
- `block` 0.1.6 is a future-incompat report nothing here can answer: it reaches
  the tree through `cpal` and has no published successor.
- `kithara-ui` warns on 627 items where the widget layer compiles without a
  host: `--no-default-features --features render`, and `--features vello`.
- Lint debt: 668 comment findings are decisions `--fix` cannot make, and the
  439 ordering findings clear under one `just lint style --fix` that rewrites
  declarations across every crate.

## Blocked

- Nothing.
