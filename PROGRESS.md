# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Configuration document for `kithara-app`: `app.yaml` plus an optional
  overlay, merged and env-expanded before typing, each section carrying its
  owning crate's `#[derive(Patch)]` type from the new `kithara-macros`. No
  patch struct is hand-written. Open: assembly sits in `main.rs` where no test
  pins it, so `downloader` and `flush` parsed and were dropped until a read
  found them; twenty-two files still take pools from `PoolsSection::default()`.

- Tooling parameter ownership. Every policy number and list `xtask` and
  `kithara-devtools` held as a `const` has a config owner, spawned programs
  resolve through `ToolsConfig`, and Rust binaries replace their embedded
  shell. Cleanup pruned `target-slots`, every Linux job's `CARGO_TARGET_DIR`,
  as retired.

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

- Premature track switch, and the census built to find it.
  `PlayerEvent::HandoverRequested` was a unit variant, so the queue applied the
  outgoing track's handover to the successor it had already selected, cutting
  it a block in. The request now carries `ItemRole` and the queue acts on it
  only when it names the track it is on, pinned over three tracks by
  `auto_advance::a_middle_track_is_heard_in_the_middle_of_its_own_span`. The
  census attributes every output frame to the track that produced it over every
  reader a track arrives through; writing it found `suite_network` dark since
  `#260`.

## Next

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
- `kithara-ui` still warns under `--no-default-features --features render` and
  `--features vello`, where the widget layer compiles without a host: 627 items.
- Lint debt is hand work: 678 comment findings are decisions `--fix` cannot
  make; the 612 mechanical ordering findings clear under one
  `just lint style --fix` that rewrites declarations across every crate.

## Blocked

- Nothing.
