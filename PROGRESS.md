# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Lint debt worked down by autofix. `struct_init_order`, `derivable_from` and
  `qualified_path_depth` now answer to the clippy gate they used to break, the
  arch baseline drops the 24 entries nothing violates, and `lint fast` runs
  `style`, so the commit hook refuses the denies that used to reach CI.

- Where the machine keeps its tools is asked, not written down:
  `CiHost::brew_root` answers for the executor.

- Mac CI host cleanup: a watchdog ends a hung pass, and cleanup reclaims what
  the volume is short of the soft floor.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. Left: the deck scenario on a release
  build with the full model, and the size of the resume blob.

- `PlayerEvent::HandoverRequested` carries `ItemRole`, so the queue acts on it
  only when it names the track it is on.

## Next

- 668 comment findings are decisions rather than rewrites; `--fix` is spent.
- `deps:deny` gates a quarantine pipeline directly instead of reporting to the
  verdict, which lets one network stall hold every pull request.
- `kithara-ui` warns under `--features render` and `--features vello`, where
  the widget layer compiles without a host: 627 items.
- `.wasm-slim.toml` budgets wasm at 29000 KiB while a local `dist` weighs 3565
  KiB; the `web-size` lane on GitLab settles which number is real.
- No runtime number backs the release optimization, and the workspace's own
  crates are still at `"z"`.
- `block` 0.1.6 is a future-incompat report no change here can answer: it
  reaches the tree through `cpal` and has no published successor.

## Blocked

- Nothing.
