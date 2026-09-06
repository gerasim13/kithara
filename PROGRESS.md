# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Lint debt worked down by autofix rather than by hand. `struct_init_order`
  and `derivable_from` were rewriting code the clippy gate then refused; both
  now answer to it. `qualified_path_depth` replaces the ast-grep rule that
  could only name a deep path: it writes the `use`, shortens every spelling of
  the item, and rewrites the `use` leaf the cut left naming nothing. The arch
  baseline drops the 24 entries the code no longer violates. `lint fast` now
  runs `style`, so the commit hook refuses the denies that used to reach CI.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. Left: the reported deck scenario on a
  release build with the full model, and the size of the resume blob.

## Next

- 668 comment findings are decisions rather than rewrites; `--fix` is spent.
- The `deps:deny` lane gates a quarantine pipeline directly instead of
  reporting to the verdict, which is what lets one network stall hold every
  pull request.
- `crates/kithara-ffi/.wasm-slim.toml` budgets wasm at 29000 KiB while a local
  `dist` weighs 3565 KiB. The `web-size` lane on GitLab is the only place that
  settles which number is real.
- No runtime number backs the release optimization, and the workspace's own
  crates are still at `"z"`. Decode throughput, stretch cost and render-budget
  headroom were never measured either side of it.

## Blocked

- Nothing.
