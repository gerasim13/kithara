# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Configuration document for `kithara-app`. `app.yaml` plus an optional overlay,
  merged and env-expanded before typing; every section carries the owning crate's
  own patch type, so a value is spelled once. `#[derive(Patch)]` in the new
  `kithara-macros` generates every patch and its `apply`; `struct-patch` and every
  hand-written patch struct are gone. Secrets stay `$KITHARA_...` references and
  one resolving nowhere stops startup. The derive carries three forms beyond a
  plain key: `nested` recurses, `validate`/`fallible` lets a merge refuse what a
  document said and report it under the key that carried it, and `wire`/`from`
  gives a key a type of its own where the field holds something a document cannot
  spell. Every field a document has business naming is a key; what stays skipped
  is a live object the construction site owns, argued field by field in the owning
  crate's `CONTEXT.md`. Left: the three integration harnesses.
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
