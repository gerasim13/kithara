# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Configuration document for `kithara-app`: `app.yaml` plus an optional
  overlay, env-expanded before typing, each section carrying its owning
  crate's `#[derive(Patch)]` type. Open: assembly sits in `main.rs` where no
  test pins it, and twenty-two files take pools from `PoolsSection::default()`.

- Tooling parameter ownership: every policy number `xtask` and
  `kithara-devtools` held as a `const` has a config owner, spawned programs
  resolve through `ToolsConfig`, and Rust binaries replace their embedded
  shell.

- Lint debt worked down by autofix. `struct_init_order`, `derivable_from` and
  `qualified_path_depth` answer to the clippy gate they used to break, the arch
  baseline drops what nothing violates, and `lint fast` runs `style`, so the
  commit hook refuses what used to reach CI.

- Mac CI host cleanup: a watchdog ends a hung pass, and cleanup reclaims what
  the volume is short of the soft floor. Open: `deps:deny` gates a quarantine
  pipeline directly, so one network stall holds every pull request.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. Left: the deck scenario on a release
  build with the full model, and the size of the resume blob.

- `PlayerEvent::HandoverRequested` carries `ItemRole`, so the queue acts on it
  only when it names the track it is on.

## Next

- 678 comment findings are decisions `--fix` cannot make.
- `kithara-ui` warns under `--features render` and `--features vello`, where
  the widget layer compiles without a host: 627 items.
- `.wasm-slim.toml` budgets wasm at 29000 KiB against a local `dist` of 3565
  KiB; the `web-size` lane on GitLab settles which number is real.
- No runtime number backs the release optimization, and the workspace's own
  crates are still at `"z"`.
- `block` 0.1.6 is a future-incompat report nothing here can answer: it reaches
  the tree through `cpal` and has no successor.

## Blocked

- Nothing.
