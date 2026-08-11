# kithara-devtools — contracts

Owning-crate contracts for the reusable xtask command core. [`README.md`](README.md)
is the overview; this file owns the invariants a consumer or contributor relies on.

## Ctx lifecycle

`Ctx::load()` / `Ctx::load_from_manifest` resolve the workspace root once via
`cargo metadata --no-deps`, parse `.config/xtask.toml` into `Ctx::config` and
`.config/similarity.toml` into `Ctx::similarity`, and retain the metadata for
`ctx.metadata()`. `Ctx::new(root, config)` builds a metadata-less context; a command
reaching `ctx.metadata()` there fails with a typed error instead of re-shelling to
cargo. Commands take `&Ctx` and must not re-resolve the root or re-parse config. The
one sanctioned exception is `common::walker`: the scoped walkers reload
`ProjectConfig` themselves because the lint namespaces (`lint::run`) run without a
`Ctx`.

## Configuration contract

- `<workspace-root>/.config/xtask.toml` is **optional**. A missing file yields
  `ProjectConfig::default()` — documented code defaults, not a fallback chain. `init`
  scaffolds it plus empty `.config/{arch,style,idioms}/baseline.toml`.
- `project.name` defaults from cargo metadata when omitted: workspace-root package name,
  else the sole workspace package, else the workspace directory name, else `xtask`. It
  is a human-facing label (report titles, temp-dir prefixes) — a sanctioned user-facing
  default, not state resolution.
- Every config struct carries `#[serde(default, deny_unknown_fields)]`. An unknown key in
  a core section is a typed parse error naming the offending token.
- **`[ext]` ownership rule.** The core schema names only generic concerns
  (`architecture`, `audit_clippy`, `health`, `lint_exclude`, `orphans`, `perf`,
  `project`, `quality`, `test`, `workspace-scan`). Anything project-specific lives
  under `[ext.*]`, exposed as the raw `ext: toml::Table` passthrough. The core never
  interprets it; the consuming bin deserializes its own typed view (kithara:
  `xtask/src/config.rs`).
- `[workspace-scan] exclude` globs apply in the scoped walkers (the lint scan path);
  raw `walk_rs_files` stays a pure directory walk.
- `[perf]`: lane matrix entries (`flash`, `backend`), `primary_lane` for ranking and
  profile/report defaults, optional `frame_prefix` for gecko own-frame attribution,
  `nextest_profile` (default `perf`). That nextest profile must expose
  `[profile.<name>.junit] path = "junit.xml"` because `perf matrix` copies
  `target/nextest/<name>/junit.xml` into the run data; a lane without junit is skipped
  and fails the command.
- `[quality]` owns `unimock_traits_dir` plus `[quality.assessment]`, which declares only
  project-specific deep-stage execution: argv, owned tool names, expected artifacts,
  optional platforms, `hard_invariant`, `complete_only`. No source relationships, no
  metric values; delegated commands keep their budgets and output schemas. Validation
  rejects duplicate stage names, empty commands, stages with no owned tool, and a tool
  listed both as configured and not-applicable.

## CLI surface, lint baselines, and exclusion

`CoreCommand` is flattened into the consumer's own subcommand enum and dispatched by
`run(&CoreCommand, &Ctx)`. Command names, flags, and help text are the public surface —
treat changes as API changes covered by the consuming project.

`audit-clippy` reports every configured advisory lint across all targets. It selects the
workspace unless explicit package selection (`-p` / `--package`, `--manifest-path`,
`--workspace`, or `--all`) replaces that default; Cargo modifiers such as
`--all-features` and `--exclude` retain workspace selection. `--fix` first applies
Clippy's machine-applicable suggestions over the exact same scope, guarded by a
clean-tree check unless `--allow-dirty` is explicit, and then reruns the report. A
failed fix stops before reporting and preserves Cargo's exit status in the command
error. Both passes disable the workstation sccache wrapper and retain incremental
Clippy builds, matching the repository's Clippy cache contract.

`Baseline` (`.config/<namespace>/baseline.toml`) compares on a line-insensitive
canonical key: a leading `path:line[:col]` prefix is stripped only when a symbolic tail
follows, so reformatting that shifts lines never re-fingerprints an unchanged
violation, while positional-only keys keep their line as their sole handle. A violation
with no baseline entry fails only at `Severity::Deny`; an observed count above the
recorded count is a regression, a lower count an improvement.

Exclusion runs in three complementary passes: path globs (`lint_exclude.paths`), AST
`#[cfg(test)]` ranges (only `test`-keyed cfgs; `#[cfg(feature = ...)]` is untouched),
and inline-module globs (`lint_exclude.modules`). Unparseable files contribute no
ranges — their violations are kept. `lint_exclude.scan_all_rules` names ast-grep rule
IDs that re-run over the full tree, tests included.

## Architecture visualization (`viz`)

`viz` has no required nested subcommand. It builds one lossless source-evidence graph,
applies scope and LOD, and writes Mermaid, Markdown, contour JSON, metrics JSON, graph
JSON, projection JSON, and a manifest below `target/architecture/<revision>/` (short
HEAD, else `working-tree`). `--view hierarchy` / `--view ownership` and
`--crate <package>` are projections of that same graph, not independent analyzers: a
crate scope needs no configured runtime scenario and no extra discovery command, and
never expands external packages. Static call targets stay candidates until
rust-analyzer resolves them via `textDocument/prepareCallHierarchy` and
`callHierarchy/outgoingCalls`.

LOD is independent from scope. Concrete types own their `impl` methods, traits are
contracts connected by `implements`, free functions belong to one module-functions
abstraction. Workspace output links per-crate diagrams and hotspot subsystems (module
degree >= 4, plus the maximum-degree module); crate output links every subsystem's
abstraction diagram. Page selection is navigation, not evidence truncation: every
source contour stays in `contours.json`. An endpoint hidden by LOD lifts to its nearest
visible owner; equal visible endpoint/kind pairs aggregate while retaining the original
method pairs, occurrence count, evidence origins, and style in `projection.json`, and
relation kinds stay distinct. There is no diagram node budget: LOD 4 is partitioned by
semantic contours into an index and linked pages, and manifest schema v4 records
complete visible-node coverage plus hierarchical artifact paths. Optional and
target-gated Cargo dependencies carry conditional evidence; unconditional normal
dependencies stay resolved structural facts. The metrics profile in `metrics.json` is
computed from the same contracted relations as Mermaid. Resolved static evidence owns
the stable profile; candidates and runtime observations stay separate comparisons.
Bottleneck concentration and external coupling
are multiplied by actual boundary load before entering the experimental ACI, which is
the mean of its available contributions; the ACI is diagnostic and alters no CI budget.

Project defaults under `[architecture.filters]` and repeatable `--exclude-crate` /
`--exclude-module` compile into one additive projection filter. It removes matching
symbols before semantic selection and matching contours plus incident edges from the
`DiagramModel`; it never alters raw `graph.json` evidence or disables an excluded
package used as a runtime scenario. A module pattern matching any ancestor of a
canonical `package::module` path excludes the descendant; relations never lift through
an excluded endpoint. Manifest schema v4 records the effective patterns and excluded
counts. `--include-default-excluded` drops project defaults while keeping explicit CLI
filters. An emptied projection is an error, not an empty diagram.

`[architecture.runtime.scenarios]` is the only project-specific runtime evidence input.
Its strict tagged schema accepts Cargo integration tests, Cargo binaries, and existing
trace paths, validated for unique ASCII-safe names and positive timeouts. Test and
binary targets are validated against Cargo metadata and launched with structured
arguments, a bounded timeout, captured logs, and `ARCHITECTURE_TRACE_PATH`; no shell
command is stored in config. With no selector `viz` runs every configured scenario
(`--runtime off` skips them); `--scenario <name>` runs one and projects only nodes
carrying that scenario's trace evidence. Runtime producers use the public,
domain-neutral `viz::trace` JSONL API, whose records carry versioned source, span, task,
thread, correlation, and resource identity.
Cross-thread sends connect only through an explicit correlation identifier. Source
matching enriches existing syntax nodes; unmatched records stay visible runtime events
rather than guessed static targets. Manual `--trace` input carries the `Manual`
evidence class and its own styling. Runtime enrichment precedes semantic resolution, so
a selected scenario limits rust-analyzer work to source functions observed in that
trace; both enrich the same graph before any view or prose is produced. The Markdown
report derives from the visible `DiagramModel` and its contracted metrics graph; every
finding and relation points to a visible Mermaid contour.

Artifact status is `complete`, `truncated`, `static-only`, `runtime-enriched`, or
`incomplete`. Timeout, failed execution, malformed trace, or failed semantic resolution
preserves partial artifacts and fails the command. Missing optional rust-analyzer
yields static/runtime output unless `--semantic required` was requested. Truncation is
explicit, applies only to evidence collection, and never removes nodes because of
diagram size.

## Health stage provisioning

`health.rs` runs each stage's tool as pinned/installed by `.config/ci-pins.toml`
`[cargo_tools]` and `xtask/src/ci/image.rs`; `ENV_SKIP_MARKERS` exists so a stage
whose *provisioned* tool is transiently missing (a dev box mid-bootstrap) reads
as SKIP instead of a false FAIL. Two stages needed a different answer:

- `semver-checks` compares against `--baseline-rev origin/main`, not the
  registry default. No workspace crate is published to crates.io — this is an
  application, not a crate release train — so the registry lookup could only
  ever fail. Comparing against `main` is a no-op there and a real check on a
  branch that has drifted from it.
- `lockbud-deadlock` is `.strict()`: lockbud has no entry in `[cargo_tools]`
  and no install step in `image.rs`/`docker/ci.Dockerfile`, so the pinned CI
  image never carries it — `no such command` there is not noise, it is the
  honest state. `.config/just/tooling.just` installs it with a bare
  `cargo install lockbud`, but lockbud is not on crates.io at all; it only
  installs from its git repository, built against the specific nightly
  toolchain it links `rustc_driver` against (currently not the toolchain this
  repository pins). Wiring it into the image for real means picking a nightly
  that satisfies both lockbud and the rest of the toolchain, or carrying a
  second nightly in the image just for it — an infra decision, not a stage
  tweak. Until that lands, `.strict()` keeps a missing lockbud reading as FAIL
  rather than a harmless SKIP.

## Quality assessment contract

`quality assess` is an artifact federation layer. Existing linters, architecture,
similarity, health, Quality Lab, test, dependency, concurrency, performance, and
platform commands remain canonical owners; the assessment normalizes and correlates
their output without reimplementing their metrics. `complete` disables project-default
architecture and similarity exclusions and includes integration tests, test/tooling
crates, and other workspace surfaces; an explicitly selected crate or canonical
`package::module` scope is included even when defaults exclude it. `standard` depth
executes each portable gate separately so stage evidence is attributable and does not
pay for the heavyweight sections of `health`; `deep` runs the full `health` pipeline
plus the registered rare stages. Configured stages are advisory unless
`hard_invariant = true` records an already-established project gate.

Artifacts are deterministic JSON/Markdown plus a manifest under
`target/quality-assessment/<revision>/<profile>-<depth>/`, with stage JSON and logs in
sibling directories. A dirty worktree uses `<head>-dirty-<digest>` and records the
content digest; committed Quality Lab output, especially Cha, must not claim coverage of
dirty content. `--reuse-existing` rebuilds the report from stage artifacts already on
disk. The workspace debt target is zero and the refactor threshold is 100; a smaller scope
uses `max(1, ceil(100 * scope_LOC / workspace_LOC))`. Existing lint baseline entries
count as debt; baseline growth is a regression. A hard invariant, debt at or above
threshold, debt regression against `--baseline`, or same-location corroboration by two
independent tools yields `refactor`; otherwise diagnostic findings yield `investigate`,
remaining debt `stable-with-debt`, uncovered signals `evidence-gap`, and a clean run
`healthy`. ACI stays diagnostic with no invented gate. Verdicts are advisory and do not
fail a complete command; a broken stage marks the analysis partial, while invalid input
or broken required analysis preserves a partial artifact and returns an error. The tool
coverage matrix must account for every known signal as `executed`, `reused`,
`covered-by`, `not-applicable`, or `evidence-gap`; a tool declared under
`[[quality.assessment.not_applicable_tools]]` stays visible in the matrix with its
reason and can never be scheduled as a deep stage.

## Behavioral similarity ownership

`similarity` owns native source-level comparison of Rust abstractions. A run first
parses the selected production sources and writes `report.md`, `report.json`,
`graph.json`, and `manifest.json` below `target/similarity/<revision>/`, then runs the
external `similarity-rs` function-copy profile. Native findings are diagnostic and do
not change the budgets owned by that external profile: audit `0.96` / min-lines 12 /
skip-test / fail-on-duplicates, advisory `0.85` / 10 / skip-test, strict `0.80` / 8.
Only strict includes test paths and `#[cfg(test)]` items; audit and advisory keep the
production-only policy in both the native and external pass. Type shapes are interned
bottom-up and pair comparisons memoized per run. Generic parameter names normalize by
position, derive attributes do not change source shape,
nested container arguments compare recursively, and `SmallVec<[T; N]>` /
`ArrayVec<[T; N]>` expose `T` as the container element. Built-in `std` families carry
conservative similarity degrees; dependency families activate only when Cargo metadata
shows the dependency (`smallvec`, `arrayvec`), and `.config/similarity.toml` may add
project families (two or more members) or directional pair relations with substitution
caveats.

Behavior is a normalized source graph over signatures, control flow, calls, field
access, effects, and literal values. Generic and local names are erased where they carry
no semantics; domain types, constructors, significant macro symbols, and effects remain.
Candidate buckets precede three rounds of Weisfeiler-Lehman refinement and bounded
method alignment. `impl` blocks in separate files attach only when their owner resolves
uniquely in the workspace or, failing that, uniquely within the owning crate. Partial
state overlap without matching behavior is a review finding; composition is recommended
only when aligned `impl` behavior supports it. `report.json` and `graph.json` are
exhaustive; the Mermaid view aggregates candidates by crate pair, so rendering stays
useful without a node or finding limit. Manifest schema v2 records the exact roots and
whether project-default exclusions were disabled (`--include-default-excluded`), so an
assessment never reuses evidence from another profile or scope. Proc-macro output is not
expanded, similarity never proves substitutability, and caveats must be checked before
refactoring.

## Quality Lab ownership

`quality lab` owns heavyweight external analysis that must stay outside `lint-fast`, the
normal audit, and pre-commit. Its **required** `.config/quality-lab.toml` is loaded
exactly once, with a strict versioned schema independent of `.config/xtask.toml`: the
three profiles below with per-profile time budgets, plus an exact version pin and
timeout per tool.

- `coverage` owns the `cargo-crap` coverage-risk gate. A production run without
  `--baseline` emits the absolute JSON artifact; a pull-request run supplies that
  artifact (with `--lcov`) and gates regressed entries plus new functions above CRAP 30.
  The wrapper checks delta JSON because cargo-crap's `--fail-regression` exit code does
  not include new functions. A failing instrumented test run still writes Cobertura and
  LCOV and runs cargo-crap; the combined stage preserves the test exit as findings
  instead of losing the risk evidence.
- `scheduled` runs Cha history/layers/smells, rustqual test-quality checks, and
  cargo-dupes sub-function duplication. Findings are advisory; missing tools, invalid
  reports, version mismatches, and timeouts are tool errors.
- `manual` adds read-only PMAT `repo-score --format json --deep`. Missing executables
  are `skipped`; findings stay advisory. A direct non-coverage tool run follows this
  manual policy.
- Every external version must match the exact pin before analysis. Native output,
  stderr, per-tool `manifest.json`, and JSON/Markdown summaries live below
  `<output_dir>/<revision>/` (`target/quality-lab` in kithara).
- Cha runs only from a clean, non-shallow source worktree and analyzes a disposable
  local clone whose revision is verified against HEAD. The clone is deleted after the
  run so Cha cache state cannot leak into the source checkout.

KISS is never executed: it overlaps the existing stack and writes hidden user-level
state. Promoting a unique external check into `syn`, Cargo metadata, Git, or ast-grep
requires repeated actionable, deterministic evidence and two comparison runs before
retiring the adapter.

## Feature gating and public API surface

`lint` (arch, style, idioms, the `lint` dispatcher, `audit`) and `viz` are default-on
cargo features gating the syn-heavy command modules. Gates live only at the `lib.rs`
module-declaration, enum-variant, and match-arm sites — never as inline `cfg` inside
logic files. `syn` / `proc-macro2` stay non-optional because `common` (`parse`,
`exclude`) uses them unconditionally; `lint` additionally turns on the optional `quote`
dependency, used only by check modules — the features gate the check modules, not the
AST stack.

`common` is intentionally public so a consumer can build custom checks on the shared
`walker` / `violation` / `baseline` / `report` / `parse` / `exclude` / `suppress` /
`fix` / `scope` infrastructure. Keep additions deliberate and documented; internal
helpers stay `pub(crate)` (`common::process`, and every `viz` module except
`viz::trace`).
