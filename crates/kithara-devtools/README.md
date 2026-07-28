<div align="center">
  <img src="../../logo.svg" alt="kithara" width="300">
</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-devtools.svg)](https://crates.io/crates/kithara-devtools)
[![docs.rs](https://docs.rs/kithara-devtools/badge.svg)](https://docs.rs/kithara-devtools)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-devtools

Reusable, config-driven command core for cached xtask build tooling invoked
through `just tooling xtask`. It holds
the project-agnostic commands so several workspaces can share one implementation
and keep only their own project-specific commands in a thin `xtask` binary.

Contracts and invariants live in [`CONTEXT.md`](CONTEXT.md); this file is the
overview.

## Commands

Exposed through the `CoreCommand` subcommand enum:

- `init` — scaffold the workspace tooling config and lint baselines.
- `lint` — architectural / style / idiomatic fitness functions (`arch`, `style`,
  `idioms`), ratcheted against a baseline. *(feature `lint`)*
- `format` — Rust, Cargo manifests, TOML, JSON, and Markdown formatting.
- `typos`, `similarity`, `ast-grep` — thin wrappers over the matching CLIs with
  the workspace config pinned.
- `manifest`, `orphans` — Cargo manifest hygiene and per-package orphan checks.
- `test` — workspace tests through `cargo nextest` with lane / backend / feature
  selection.
- `health` — aggregated workspace health report.
- `quality` — rstest / unimock / trait-mock audits plus the opt-in Quality Lab
  for heavyweight external analyzers.
- `scope` — translate scope tokens to tool-specific flags.
- `perf-compare` — compare hotpath timing tables against a baseline.
- `perf` — test-suite performance pipeline: matrix, slow aggregation, samply
  profiling, merged report, and xctrace escalation.
- `viz` — LOD-controlled Mermaid architecture diagrams from source evidence,
  written below `target/architecture/<revision>/`. Configured runtime scenarios
  and rust-analyzer semantic evidence enrich the same graph automatically.
  `--crate <package>` keeps the selected crate and its immediate incoming and
  outgoing workspace neighbors; it needs no runtime scenario. `--lod
  auto|0|1|2|3|4` moves from crates through modules and abstractions to the
  complete call graph. Repeatable `--exclude-crate <glob>` and
  `--exclude-module <glob>` filters remove non-product contours from the
  projection and report.
  *(feature `viz`)*

## Consuming it

Add the dependency and flatten `CoreCommand` into your own bin's subcommand
enum, keeping your project-specific commands alongside it:

```rust
#[derive(clap::Subcommand)]
enum Command {
    #[command(flatten)]
    Core(kithara_devtools::CoreCommand),
    // ... your project-specific commands
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let ctx = kithara_devtools::Ctx::load()?;
    match cli.command {
        Command::Core(cmd) => kithara_devtools::run(&cmd, &ctx),
        // ... your arms
    }
}
```

## Configuration

Everything project-specific comes from `.config/xtask.toml`, parsed once into
`Ctx::config`. The file is optional: a project with none gets documented code
defaults, and `project.name` is derived from cargo metadata. Unknown top-level
sections are a typed error (`deny_unknown_fields`); a project puts its own
sections under `[ext.*]`, which the core passes through untouched.

The shared `[workspace-scan] exclude` globs drop directories (media trees,
virtualenvs, …) from the scanning commands.

`[architecture.filters]` provides additive project defaults for the same
repeatable CLI filters:

```toml
[architecture.filters]
exclude_crates = ["integration-tests", "xtask"]
exclude_modules = ["*::tests"]
```

Crate patterns match Cargo package names. Module patterns match canonical
`package::module` names and exclude the complete matched subtree. Excluded
packages may still run as runtime evidence producers; they do not enter
semantic selection, Mermaid, `projection.json`, findings, or architecture
counters. The complete diagnostic evidence remains in `graph.json`, while
manifest schema v3 records the effective filters and excluded node/edge counts.

`[architecture.runtime]` can declare portable tests, binaries, or existing
JSONL traces that exercise representative flows:

```toml
[[architecture.runtime.scenarios]]
name = "queue-playback"
command = "test"
package = "my-integration-tests"
test = "architecture"
filter = "queue_playback"
ignored = true
timeout_secs = 120
```

The test or binary receives `ARCHITECTURE_TRACE_PATH`. It can write neutral
records through `viz::trace::{TraceRecord, TraceRecordKind, TraceSource,
TraceWriter}`; no Kithara domain type or macro is required. `just arch viz`
runs every configured scenario. `just arch viz --scenario queue-playback`
runs and projects one flow, while `--trace <path>` merges an existing trace as
manual evidence. `--semantic off|required` and `--runtime off` control optional
enrichment without changing the artifact layout.

The crate selector is a projection over the full workspace graph, not a
package-only scan. It includes direct normal Cargo dependencies, direct
workspace dependents, and resolved cross-package call endpoints. Relations
between neighboring packages are hidden so the view does not expand into a
second dependency level.

The canonical graph groups concrete types and their methods, trait contracts,
and each module's free functions. Hidden method relations lift to the nearest
visible contour without merging calls with ownership, messaging, transfer, or
spawn relations. Optional and target-gated Cargo dependencies are styled as
conditional rather than required.

Each run writes `architecture.md`, `architecture.mmd`, `graph.json`,
`projection.json`, and `manifest.json`; runtime traces and captured process
logs are preserved in adjacent `traces/` and `logs/` directories. There is no
diagram node cap. LOD 4 writes an index plus linked documents below
`contours/`, with manifest coverage proving that partitioning did not remove
nodes. The manifest status is `complete`, `truncated`, `static-only`,
`runtime-enriched`, or `incomplete`; `truncated` applies to evidence collection,
not diagram size. An incomplete run returns an error after preserving partial
artifacts.

`[perf]` configures the generic test-suite performance pipeline:

- `lanes` is the matrix of `{ flash, backend }` combinations to measure.
- `primary_lane` is the lane used for ranking/profile defaults; an empty value
  means the first configured lane.
- `frame_prefix` overrides gecko profile frame attribution; if omitted, the
  project name is used.
- `nextest_profile` names the nextest profile used by `perf matrix`; it defaults
  to `perf`.

The selected nextest profile must write JUnit at `junit.xml`, for example:

```toml
[profile.perf.junit]
path = "junit.xml"
```

Quality Lab intentionally has a separate, required
`.config/quality-lab.toml`. It pins analyzer versions, tool/profile time
budgets, and the output directory without adding heavyweight tools to the fast
lint path. Use `quality lab list` to inspect policy and `quality lab run
coverage|scheduled|manual|<tool>` to execute it.

## Features

- `lint` (default) — the syn-based `arch`/`style`/`idioms` lint family.
- `viz` (default) — architecture visualization.

Both are on by default; `--no-default-features` drops those command families for
a project that only wants format/test/health and friends.
