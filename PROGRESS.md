# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Configuration document for `kithara-app`. `app.yaml` plus an optional overlay,
  merged and env-expanded before typing; every section carries the owning crate's
  own patch type, so a value is spelled once. `#[derive(Patch)]` in the
  new `kithara-macros` generates every patch and its `apply`; `struct-patch` and
  every hand-written patch struct are gone. The output rate is named once,
  under `app`. Secrets stay `$KITHARA_...` references and one resolving nowhere
  stops startup. Merged with `production/main`: `broadcast`, `player.warp`, and
  the stretch backends' preparation geometry under `player.warp.backends` are
  sections now, and `app.crossfade_seconds` is gone in favour of
  `player.crossfade_duration`. The two configs that grew thread budgets since
  carry the derive too: `play_worker` names the one playback worker's budgets and
  `dispatcher` names every app-owned dispatcher's, minus the thread name each
  construction site keeps. Merged again over the beat split: the picking policy
  the `beat:` key writes now lives in `kithara_beat::nn`, and the `beat-dsp`
  `Tempo` is no document key, because its builder judges band, prior, tolerance
  and drift as one policy and a field-by-field merge has nothing to reject with.
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

- Wire the last section end to end: the three integration harnesses still build
  pools from `PoolsSection::default()`, not from the document they load.
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
