# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. Left: the reported deck scenario on a
  release build with the full model, and the size of the resume blob.

- Tooling parameter ownership. Every policy number and list `xtask` and
  `kithara-devtools` carried as a `const` now has a config owner: eight
  `CiHost` keys, the architecture thresholds, the style checks, and the stress,
  quality and architecture render budgets. Spawned programs resolve through
  `ToolsConfig`, one Rust binary per crate replaces the shell each embedded,
  and `xtask/Cargo.toml` names `default-run`. Writing the host's live cache
  namespaces down surfaced one the list never held: `target-slots`, every Linux
  job's `CARGO_TARGET_DIR`, was pruned as retired. Left: nothing.

## Next

- Raise the workspace's own crates off `opt-level = "z"` - `kithara-audio`,
  `-decode`, `-resampler` and the rest, which the per-package glob cannot
  reach. A measured change of its own.
- No runtime number backs the release optimization. Decode throughput, stretch
  cost and render-budget headroom were never measured before or after, so the
  case rests on codegen.
- `crates/kithara-ffi/.wasm-slim.toml` budgets 29000/31000/33000 KiB against a
  ~28.2 MiB baseline while a local `dist` weighs 3565 KiB. Either the gate is
  an order of magnitude stale or the two weigh different things; only the
  GitLab `web-size` lane settles it.
- Work the comment queue down by hand: `--fix` is exhausted for comments, so
  all 665 `comment_hygiene` warns are decisions.
- 593 ordering findings (`struct_field_order` 229, `struct_init_order` 115,
  `trait_item_order` 249) are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.

## Blocked

- Nothing.
