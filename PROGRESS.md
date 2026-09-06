# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Where the machine keeps its tools is asked, not written down, and asking is
  cheap. `CiHost::brew_root` answers for the executor, `qemu` included. The
  root `justfile` ran `brew --prefix`, which `just` evaluates before it knows
  the recipe, so every nested invocation a test drove waited on it; it reads
  the prefix off where `brew` sits now. A guard holds each half.

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

- Premature track switch, and the census built to find it.
  `PlayerEvent::HandoverRequested` was a unit variant, so the queue applied the
  outgoing track's handover to the successor it had already selected, cutting
  it a block in. The request now carries `ItemRole` and the queue acts on it
  only when it names the track it is on, which
  `auto_advance::a_middle_track_is_heard_in_the_middle_of_its_own_span` pins
  over three tracks. The census that framed it attributes every output frame to
  the track that produced it over every reader a track arrives through, and
  writing it found `suite_network` dark since `#260`.

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
  `--features vello`, where the widget layer compiles without a host: 627
  items, its own change.
- Work the comment queue down by hand: `--fix` is exhausted, so all 668
  findings are decisions.
- 439 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.

## Blocked

- Nothing.
