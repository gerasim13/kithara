# Tooling Policy

Use this when touching repo tooling, formatter/lint config, or dependency audit
policy. Keep `AGENTS.md` short; put command details here.

## Fast Gate

- `just fmt check`: Rust fmt, Cargo manifest dependency order (`kithara-*`
  first), formatted non-Cargo TOML, and sorted JSON/JSONC.
- `just check clippy`: workspace Clippy with warnings denied.
- `just lint ast-grep`: structural policy rules from `.config/ast-grep/`.
- `just lint arch`: fast architecture gate used by pre-commit.

These are suitable for local pre-commit feedback.

## Autofix

Every ratchet that can rewrite code does so under `--fix`. Prefer it to editing
by hand: it applies the check's own rule mechanically, and the diff is the
review. It rewrites everything in the scope you give it, including the test code
the report leaves out of the production baseline, so the diff is wider than the
report - it prints the file count before the report for that reason.

- `just lint style --fix`, `just lint idioms --fix`, and `just lint all --fix`
  (both namespaces at once) rewrite in place, then re-run the checks so the
  printed report is what remains.
- `just lint arch --fix` only prints the plan; add `--apply` to write.
- `just lint ast-grep --fix` applies the rules that declare a `fix:` block in
  `.config/ast-grep/*.yml`. The rest stay reporting-only.
- `just lint typos --fix` and `just lint audit-clippy --fix` apply their own
  tools' machine-applicable suggestions.
- `just ci audit --autofix` runs the whole chain - fmt, Clippy twice, typos,
  ast-grep, xtask lint - before the read-only audit.

A reordering fix can leave the crate not compiling: sorting a struct's fields
does not follow through to its literals, and `clippy::inconsistent_struct_constructor`
then rejects what the ratchet calls a fixpoint. Compile after a `--fix` that
touched declarations; a second lint pass will not tell you.

Every `--fix` refuses to run on a dirty tree. Commit first, so the diff holds
only what the tool did; `--allow-dirty` mixes its edits into yours. The three
lint namespaces scope with `--crate <name>` or `--path <path>`, which takes a
directory or a single file; `typos`, `ast-grep`, and `similarity` take their
paths positionally.

| Namespace | Check | Rewrite |
| --- | --- | --- |
| `style` | `comment_hygiene` | promotes a comment above an item to `///`, deletes short unmarked prose |
| `style` | `struct_field_order`, `struct_init_order`, `trait_item_order` | reorders declarations and literals |
| `idioms` | `derivable_from`, `derivable_display`, `derivable_deref`, `derivable_getter`, `derivable_delegation` | collapses a hand-written impl onto the repo macro |
| `arch` | `dead_exports` | deletes an unused export (needs `--apply`) |

`comment_hygiene --fix` makes the two rewrites that cannot be wrong. A standalone
`//` block directly above an item becomes that item's `///`: the comment already
documents the item and only the marker was missing. A single `//` line of at most
30 characters with no digit, backtick, bracket, `=`, `:`, or second capital is
deleted; prose that small carries nothing a reader loses. Longer prose is never
deleted, because a deleted sentence is irreversible.

Everything else is a decision you owe. A comment inside a function body has no
mechanical destination: being the only comment there does not make it the
function's documentation - far more often it annotates the first statement of a
long body, and a fix that hoisted it would publish a wrong contract. So a clean
fix run is not a clean file, and adding a marker to silence the check is the
worse of the two answers. `comment_hygiene` reports size and density separately
for the same reason, and neither has an autofix.

## Architecture Analysis

- `just arch viz` automatically collects the workspace source graph, runs all
  configured runtime scenarios, asks rust-analyzer to resolve selected calls,
  and writes the Mermaid diagram, linked crate/hotspot-subsystem pages, and
  graph-derived complexity report below `target/architecture/<revision>/`.
- Scope and detail are independent: `--crate`/`--module` choose what, `--lod`
  chooses how deep. `just arch viz --help` owns the flags but not what the
  levels hold - 0 crates, 1 subsystems, 2 abstractions, 3 constructors,
  boundary methods, resources, messages, and tasks, 4 the complete focused call
  graph. A crate scope hides Cargo dependencies and incoming callers while
  keeping concrete outgoing public interactions as compact external ports.
- `--view` changes only the projection; `--semantic`, `--runtime`, `--scenario`,
  and `--trace` control evidence collection.
- `[architecture.filters]` supplies project-default crate/module exclusions.
  Repeat `--exclude-crate <glob>` or `--exclude-module <glob>` for additive
  one-off exclusions. Excluded runtime-test packages may still produce
  evidence, but they do not enter semantic selection, the diagram,
  `projection.json`, findings, or architecture counters. `manifest.json`
  records the effective filters and excluded counts.
- `metrics.json` holds resolved-static and candidate profiles for the scope and
  every generated contour. Runtime evidence is an overlay and cannot change the
  stable score. The ACI is diagnostic, not a CI budget.
- There is no diagram node budget. A hidden method is lifted to its visible
  abstraction and its evidence is retained in `projection.json`; LOD 4 writes an
  index plus linked contour pages rather than dropping nodes.
- Read `manifest.json` before using a result as architecture evidence.
  Every schema-v5 status preserves a reusable static projection. `complete` and
  `runtime-enriched` include their named overlays; `truncated` names an
  evidence-collection limit, never diagram node removal; `static-only` has no
  semantic overlay; `runtime-degraded` names a degraded runtime observation.
  Overlay diagnostics remain in the manifest and Markdown report.
- Runtime traces and scenario stdout/stderr stay beside the diagram. Trace
  absence never proves a path is dead, and unresolved calls are never assigned
  a guessed target.

## Full Audit

- `just ci audit`: scoped Rust fmt, Clippy, ast-grep, xtask lint, typos,
  similarity, and scoped orphan-module checks. With no scope, the orphan stage
  is latency-capped; `just ci health` owns the full workspace orphan sweep.
- `just lint full`: fast lint plus xtask self-tests and quality scans.
- `just ci health`: broad local health report; heavy or environment-sensitive
  stages may report SKIP.
- Audit and health consume one canonical argv source for their shared stages and
  validate each xtask command shape in the `kithara-devtools` unit tests.

## Decision-Oriented Assessment

- `just quality assess` rebuilds the standard product evidence and writes
  `manifest.json`, `assessment.json`, and `assessment.md` below
  `target/quality-assessment/<revision>/product-standard/`.
- Standard runs the portable format, compiler/lint, quality, unused-public,
  test, similarity, and architecture stages independently. Full `health` and
  dependency/API/security sweeps belong to `--depth deep`.
- `--profile complete` adds what `product` excludes by project default:
  integration tests, test and tooling crates, xtask, and devtools. `--depth
  deep` adds the heavyweight analyzers and project/platform scenarios.
  `--reuse-existing` skips every refresh and federates only compatible existing
  artifacts. `just quality assess --help` owns the rest.
- A global unversioned report is not treated as reusable evidence merely
  because the file exists. Fresh stages must create or update their declared
  artifacts; otherwise the assessment is `partial`.
- Start with the assessment manifest. `partial` means at least one required
  stage is broken; preserved logs are evidence of the gap, not a complete
  result. A `refactor` verdict is advisory and still exits successfully when
  analysis is complete.
- Baseline entries are debt. The target is zero, the workspace refactor
  threshold is 100, and smaller scopes use the LOC-proportional threshold
  recorded in the report. ACI is diagnostic; use it to rank contours and seek
  corroboration rather than inventing a score gate.
- For source-aware synthesis and deep-report behavior, use
  `docs/skills/quality-assessment/SKILL.md`.

## Similarity Analysis

- `just lint similarity` automatically combines native abstraction analysis
  with the established `similarity-rs` function-copy scan. Append one or more
  crate `src/` paths for a focused run; no discovery or secondary visualization
  command is required.
- Native artifacts live under `target/similarity/<revision>/`. Start with
  `report.md`: its Mermaid graph aggregates every candidate by crate pair and
  its findings explain state, behavior, matched fields, type-family scores,
  substitution direction, and caveats. `report.json` and `graph.json` retain
  exhaustive unaggregated evidence; `manifest.json` records the profile, scan
  size, candidate count, and cache use.
- `.config/similarity.toml` owns project exclusions and optional type
  dictionaries. `[[types.relations]]` gives a pair a `similarity` in
  `[0.0, 1.0]`, `substitution` (`safe`, `conditional`, or `incompatible`),
  `direction` (`bidirectional`, `left-to-right`, or `right-to-left`), and
  `caveats`. `[types.families.<name>]` supplies `members` and
  `default_similarity`.
- Audit and advisory analyze production source; strict also includes test paths
  and `#[cfg(test)]` items. Native analysis is diagnostic and does not alter
  existing CI thresholds or latency budgets. A high score is a refactoring
  candidate, never proof of behavioral equivalence.

## Dependency Policy

`AGENTS.md` owns the workspace-first rule. Beyond it: a crate reaches a version
with `{ workspace = true }` and never spells one out itself, a heavy crate taken
for a small utility has its cost checked first, and a new dependency is justified
in the task, plan, or PR description.

## Dependency And Surface Tools

- `cargo-deny`: licenses, bans, advisories, and source policy.
- `cargo-machete`: unused dependency smoke test.
- `cargo-shear`: unused, misplaced, and unlinked dependency/file audit. Treat
  new findings as dependency-boundary debt; ignore only with documented metadata.
- `cargo-hack`: feature-powerset compatibility.
- `cargo-semver-checks`: release-facing public API compatibility.
- `cargo-public-api`: manual public surface listing/diff for planned API
  changes; use one package at a time.
- `cargo-geiger`: unsafe inventory. It is evidence for audit, not a security
  verdict by itself.
- Dylint or Semgrep: add only for rule classes that ast-grep and xtask cannot
  express cleanly. Do not create a second custom-rule stack for existing rules.

## Formatting Ownership

- `just fmt` is the formatter harness and `just fmt check` is its gate. Use
  `just tooling xtask format --only rust|manifest|toml|json|markdown` only for
  scoped formatter work.
- `rustfmt.toml` owns `.rs` formatting.
- `.config/tomlfmt.toml` plus `cargo-sort` provide the mechanical `Cargo.toml`
  write pass. `just deps manifest dependency-order` owns the gate: internal
  `kithara` / `kithara-*` dependencies stay above external crates, and each
  dependency group stays sorted by key.
- Do not use `cargo sort --check` in gates: it conflicts with the repo's
  internal-first dependency policy after the post-pass.
- `taplo` owns non-Cargo TOML formatting.
- `tidy-json` owns JSON/JSONC sorting and formatting.
- `mdfmt` owns Markdown formatting. It is an explicit recipe/advisory health
  signal until the historical Markdown tree is cleaned up.
- Do not add a second formatter for the same file class unless the owner is
  changed here and in `.config/just/fmt.just`.
