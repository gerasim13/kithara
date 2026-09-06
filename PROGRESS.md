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
  the `beat:` key writes now lives in `kithara_beat::nn`. The derive learned to
  refuse, so the `beat-dsp` `Tempo` is a document key too: a struct declares
  `#[patch(validate = ..., error = ...)]` and its merge stages a whole copy,
  judges it, and commits only what the check accepted; a parent carries a
  child's refusal with `#[patch(nested, fallible)]` and reports it under the
  document key that carried it. The declaration is the struct's, never read off
  the surviving fields, so `apply` keeps one signature whichever detector a
  build selects. `beat:` is wired end to end: it was a declared-but-dead
  section, and now `Config::beat` merges it onto the analyzer's own defaults and
  a band the comb never scores stops the launch by name. The last four skipped
  fields are keys now. The derive learned a wire form,
  `#[patch(wire = <type>, from = <path>)]`, for a field holding something a
  document cannot spell: the key parses as the wire type and the merge converts,
  so `worker.pool` names the compute pool minus the variant carrying a live
  `rayon::ThreadPool`, and the top-level `worker_pool:` section that existed only
  for want of it is gone. `app.palette` names the theme one colour at a time,
  `app.ui_package` joins `--ui-package` and the package beside the executable as
  the middle of three sources, and `audio.decoder` names the backend and the
  gapless mode. `AudioDecoderConfig::resampler` stays skipped: it carries the
  backend object the construction site owns.
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

- Wire the last section end to end: the three integration harnesses still build
  pools from `PoolsSection::default()`, not from the document they load.
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
