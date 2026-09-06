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
  extent per pass in `kithara-analysis`. Left: the reported deck scenario on a
  release build with the full model, and the size of the resume blob.

- Tooling parameter ownership. Every policy number and list `xtask` and
  `kithara-devtools` carried as a `const` now has a config owner, spawned
  programs resolve through `ToolsConfig`, and one Rust binary per crate
  replaces the shell each embedded. Writing the namespaces down surfaced one
  it never held: `target-slots`, every Linux job's `CARGO_TARGET_DIR`, was
  pruned as retired. Left: nothing.

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
- Work the comment queue down by hand: `--fix` is exhausted for comments, so
  all 665 `comment_hygiene` warns are decisions.
- 593 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.

## Blocked

- Nothing.
