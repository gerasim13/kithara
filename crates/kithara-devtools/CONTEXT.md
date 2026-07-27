# kithara-devtools — contracts

Owning-crate contracts for the reusable xtask command core. The [`README.md`](README.md)
is the overview; this file owns the invariants a consumer or contributor relies on.

## Ctx lifecycle

`Ctx::load()` resolves the workspace root once via `cargo metadata` and parses
`.config/xtask.toml` into `Ctx::config`. Every command takes `&Ctx`; a command
must not re-resolve the root or re-read the config file. `Ctx::config` is the
single parsed configuration for the run.

## Configuration contract

- The config file is `<workspace-root>/.config/xtask.toml` and is **optional**.
  A missing file yields `ProjectConfig::default()` — documented code defaults,
  not a fallback chain. This is the zero-config adoption path.
- `project.name` defaults from cargo metadata when the config omits it: the
  workspace-root package name, else the sole workspace package, else the
  workspace directory name. It is a human-facing label (report titles, temp-dir
  prefixes) — a sanctioned user-facing default, not state resolution.
- Every config struct carries `#[serde(default, deny_unknown_fields)]`. An
  unknown key in a core section is a typed parse error naming the offending
  token — misconfiguration fails loud, it is never silently ignored.
- **`[ext]` ownership rule.** The core schema names only generic concerns
  (`project`, `workspace_scan`, `lint_exclude`, `health`, `test`, `perf`,
  `orphans`, `quality`). Anything project-specific lives under `[ext.*]`, exposed as the
  raw `ext: toml::Table` passthrough. The core never interprets `[ext]`; the
  consuming bin deserializes its own typed view from it (kithara does this in
  `xtask/src/config.rs`). This keeps `deny_unknown_fields` strict on core
  sections while letting any project add its own without touching the core.
- `[workspace-scan] exclude` holds globs applied in `workspace_rs_files_scoped`
  (the lint scan path) so a project can drop media/venv/generated trees from
  every scanning namespace. Raw `walk_rs_files` stays a pure directory walk.
- `[perf]` owns the generic test-suite performance pipeline configuration:
  lane matrix entries (`flash`, `backend`), optional `primary_lane` for ranking
  and profile/report defaults, optional `frame_prefix` for gecko own-frame
  attribution, and `nextest_profile` (default `perf`). The selected nextest
  profile must expose `[profile.<name>.junit] path = "junit.xml"` because
  `perf matrix` copies `target/nextest/<name>/junit.xml` into the run data.

## CLI surface

`CoreCommand` is a `clap::Subcommand` a consumer flattens into its own enum via
`#[command(flatten)]`; `run(&CoreCommand, &Ctx)` dispatches it. Command names,
flags, and help text are the public surface — treat changes to them as
API changes covered by the consuming project's expectations.

## Quality Lab ownership

`quality lab` owns heavyweight external code analysis that must stay outside
`lint-fast`, the normal audit, and pre-commit. Its required
`.config/quality-lab.toml` is loaded exactly once by the command and has a
strict schema independent of the general `.config/xtask.toml`.

- `coverage` owns the `cargo-crap` coverage-risk gate. A production run without
  `--baseline` emits the absolute JSON artifact; a pull-request run supplies
  that artifact and gates regressed entries plus new functions above CRAP 30.
  The wrapper checks delta JSON because cargo-crap's `--fail-regression` exit
  code does not include new functions.
- `scheduled` runs Cha history/layers/smells, rustqual test-quality checks, and
  cargo-dupes sub-function duplication. Findings are advisory; missing tools,
  invalid reports, version mismatches, and timeouts are tool errors.
- `manual` adds read-only PMAT `repo-score --format json --deep`. Missing
  executables are `skipped`; findings stay advisory. A direct non-coverage tool
  run follows this manual policy.
- Every external version must match the exact pin before analysis. Native
  output, stderr, per-tool `manifest.json`, and JSON/Markdown summaries live
  below `target/quality-lab/<revision>/`.
- Cha runs only from a clean, non-shallow source worktree and analyzes a
  disposable local clone. The clone is deleted after the run so Cha cache state
  cannot leak into the source checkout.

KISS is not executed directly because it overlaps the existing stack and
writes hidden user-level state. PMAT remains manual because its breadth and
runtime make it unsuitable for a routine gate. Promoting a unique external
check into `syn`, Cargo metadata, Git, or ast-grep requires repeated actionable,
deterministic evidence and two comparison runs before retiring the adapter.

## Feature gating

`lint` (arch/style/idioms/lint) and `viz` are default-on cargo features that
gate the syn-heavy command modules. Gates live only at the `lib.rs`
module-declaration, enum-variant, and match-arm sites — never as inline `cfg`
inside logic files. `syn`/`proc-macro2` stay non-optional because `common`
(`parse`, `exclude`) uses them unconditionally, so the features gate the check
modules rather than the AST stack itself.

## Public API surface

`common` is intentionally public so a consumer can build custom checks on the
shared walker / violation / baseline / report / parse infrastructure. Keep
additions to it deliberate and documented; internal helpers stay `pub(crate)`.
