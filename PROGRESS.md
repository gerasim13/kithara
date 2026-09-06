# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Premature track switch in `kithara-app`. `PlayerEvent::HandoverRequested` was
  a unit variant, so the queue applied the outgoing track's handover to whatever
  its cursor held by then - the successor it had just selected, cut a block in.
  The request now carries `ItemRole`, and the queue acts on it only when it names
  the track it is on. Pinned by
  `auto_advance::a_middle_track_is_heard_in_the_middle_of_its_own_span`; two
  tracks cannot show it, there is no successor to jump to. Left: nothing.

- The Windows guest asks the machine's profile where `qemu` is. Both binaries
  it launches were absolute paths into one Homebrew prefix, so a host whose
  Homebrew answers elsewhere could not start the guest, and the error named
  neither `qemu` nor the prefix. `CiHost::brew_root` already owns that answer
  and fifteen other tools already go through it. The guard that kept the build
  configuration from writing a prefix down now reads the executor's sources as
  well, so the class cannot come back through Rust.

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

- Full-playthrough queue census. A queue is played from the first frame of the
  first track to the last frame of the last, and every output frame is
  attributed to the track that produced it, through a USDT probe on
  `PlayerTrack::render` naming the track and its own media clock. Both halves of
  a premature switch are pinned - a track must serve its whole length, and two
  tracks may share frames only inside the crossfade the queue announced - over
  HLS segments, a local FLAC, a FLAC body over HTTP and an MPEG body between two
  HLS tracks, each at cf=0 and cf>0, with a real-CDN counterpart in
  `real_playlist`. The wrong-duration family is closed by two negative results.
  Writing the seam test found `suite_network` dark since `#260`; the lane builds
  again. Left: the reported premature switch is open and its mechanism unknown.

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
- Work the comment queue down by hand: `--fix` is exhausted, so all 668
  findings are decisions.
- 439 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.

## Blocked

- Nothing.
