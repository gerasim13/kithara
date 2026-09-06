# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

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

- `SpectralBeats`, a beat detector needing no model, beside the neural one. It
  searches the `Tempo` its caller hands it, and a build picks the model it
  embeds; the cache tag names both. Left: nothing.

- Harness and document revision. `AGENTS.md` routes instead of restating; the
  `style` namespace budgets documents with `doc_size`, blocks drift with
  `doc_staleness`, and holds every crate README to one shape with
  `readme_shape`. All three queues are at zero.

## Next

- Work the comment queue down by hand: `--fix` is exhausted for comments, so
  all 668 are decisions (497 body comments, 105 long doc blocks, 50 oversized
  inline comments, 16 dense functions).
- 439 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.
- Wire `just lint style` to a gate: nothing runs it today. A warm run is 58 s,
  too much for every commit, nothing for a lane. The lane catalog owns that.

## Blocked

- Nothing.
