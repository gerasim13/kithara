# Tooling Policy

Use this when touching repo tooling, formatter/lint config, or dependency audit
policy. Keep `AGENTS.md` short; put command details here.

## Fast Gate

`.config/just/lint.just` owns what each chain runs. The fact the recipe cannot
tell you: `just lint fast` is the commit gate and it does not run `style` - and
neither does any CI lane, so comment, document, and ordering debt accumulates
silently until someone runs `just lint style` by hand. A warm workspace run of it
costs under a minute.

## Autofix

Every ratchet that can rewrite code does so under `--fix`. Prefer it to editing by
hand: it applies the check's own rule mechanically, and the diff is the review. It
rewrites the whole scope you give it, including the test code the report leaves out
of the production baseline, so the diff is wider than the report - hence the file
count printed ahead of it.

- `just lint style --fix`, `just lint idioms --fix`, and `just lint all --fix`
  rewrite in place, then re-run the checks so the printed report is what remains.
- `just lint arch --fix` only prints the plan; add `--apply` to write.
- `just lint ast-grep --fix` applies the rules declaring a `fix:` block; the rest
  stay reporting-only. `just lint typos --fix` and `just lint audit-clippy --fix`
  apply their own tools' machine-applicable suggestions.
- `just ci audit --autofix` runs the whole chain before the read-only audit.

| Namespace | Check | Rewrite |
| --- | --- | --- |
| `style` | `comment_hygiene` | promotes a comment above an item to `///`, deletes short unmarked prose |
| `style` | `struct_field_order`, `struct_init_order`, `trait_item_order` | reorders declarations and literals |
| `style` | `qualified_path_depth` | trades a deep path for the `use` that shortens it |
| `idioms` | `derivable_from`, `derivable_display`, `derivable_deref`, `derivable_getter`, `derivable_delegation` | collapses a hand-written impl onto the repo macro |
| `arch` | `dead_exports` | deletes an unused export (needs `--apply`) |

Every `--fix` refuses to run on a dirty tree. Commit first, so the diff holds only
what the tool did; `--allow-dirty` mixes its edits into yours. The three lint
namespaces scope with `--crate <name>` or `--path <path>`, which takes a directory
or a single file; `typos`, `ast-grep`, and `similarity` take their paths
positionally.

A rewrite answers to the clippy gate as well as to its own rule.
`struct_init_order` reads an all-shorthand literal in the order its type
declares, which is what `clippy::inconsistent_struct_constructor` demands;
`qualified_path_depth` shortens every path its import names and drops the `use`
that import leaves naming nothing. Compile after a `--fix` all the same: a
second lint pass calls its own output a fixpoint.

`comment_hygiene --fix` makes only the two rewrites that cannot be wrong. A
standalone `//` block directly above an item becomes that item's `///`: it already
documents the item and only the marker was missing. A `//` line of at most 30
characters with no digit, backtick, bracket, `=`, `:`, or second capital is
deleted; prose that small loses a reader nothing. Longer prose is never deleted,
because a deleted sentence is irreversible.

Everything else is a decision you owe. A comment inside a function body has no
mechanical destination: being the only comment there does not make it the
function's documentation - far more often it annotates the first statement of a
long body, and a hoist would publish a wrong contract. A clean fix run is therefore
not a clean file, and silencing the check with a marker is the worse of the two
answers. Size and density are reported separately for the same reason, and neither
has an autofix.

## Architecture Analysis

`just arch viz` collects the workspace source graph, runs the configured runtime
scenarios, resolves selected calls through rust-analyzer, and writes the Mermaid
diagram, linked crate and hotspot pages, and a complexity report below
`target/architecture/<revision>/`.

Scope and detail are independent: `--crate` and `--module` choose what, `--lod`
chooses how deep. `--help` owns the flags but not what the levels hold - 0 crates,
1 subsystems, 2 abstractions, 3 constructors, boundary methods, resources,
messages, and tasks, 4 the complete focused call graph. A crate scope hides Cargo
dependencies and incoming callers, keeping concrete outgoing public interactions as
compact external ports. `--view` changes the projection alone; `--semantic`,
`--runtime`, `--scenario`, and `--trace` control evidence collection.
`[architecture.filters]` holds the project-default exclusions and the
`--exclude-crate` / `--exclude-module` globs add to them; an excluded package may
still emit evidence, but it enters neither semantic selection, the diagram,
`projection.json`, findings, nor the counters.

Read `manifest.json` before citing a result as evidence. It records the effective
filters and excluded counts, and every schema-v5 status preserves a reusable static
projection: `truncated` names an evidence-collection limit and never node removal,
`static-only` has no semantic overlay, `runtime-degraded` a degraded observation.
Runtime evidence is an overlay on the profiles in `metrics.json` and cannot move
the stable score; the ACI is diagnostic, not a CI budget. There is no diagram node
budget - a hidden method is lifted to its visible abstraction with its evidence
retained in `projection.json`, and LOD 4 writes an index plus linked contour pages
rather than dropping nodes. Trace absence never proves a path dead, and an
unresolved call is never assigned a guessed target.

## Full Audit

`just ci audit` runs scoped fmt, Clippy, ast-grep, xtask lint, typos, similarity,
and orphan-module checks; unscoped, its orphan stage is latency-capped and the full
workspace sweep belongs to `just ci health`, whose heavy or environment-sensitive
stages may report SKIP. `just lint full` is the fast chain plus xtask self-tests,
the quality scans, `style`, and every idiom check. Audit and health consume one
canonical argv source for their shared stages, and each xtask command shape is
validated in the `kithara-devtools` unit tests.

## Decision-Oriented Assessment

`just quality assess` rebuilds the standard product evidence and writes
`manifest.json`, `assessment.json`, and `assessment.md` below
`target/quality-assessment/<revision>/product-standard/`.

- Standard runs its stages independently; full health and the dependency, API, and
  security sweeps belong to `--depth deep`. `--profile complete` adds what
  `product` excludes by project default: integration tests, test and tooling
  crates, xtask, and devtools. `--reuse-existing` skips every refresh and federates
  only compatible existing artifacts.
- A global unversioned report is not reusable evidence merely because the file
  exists: a fresh stage must create or update its declared artifacts, or the
  assessment is `partial` - a required stage is broken, and the preserved logs are
  evidence of the gap, not a result. A `refactor` verdict is advisory and still
  exits successfully.
- Baseline entries are debt. The target is zero, the workspace refactor threshold
  is 100, and smaller scopes use the LOC-proportional threshold recorded in the
  report.
- `docs/skills/quality-assessment/SKILL.md` owns source-aware synthesis and
  deep-report behavior.

## Similarity Analysis

`just lint similarity` combines native abstraction analysis with the
`similarity-rs` function-copy scan in one run; append crate `src/` paths to focus
it. Native artifacts live under `target/similarity/<revision>/`.

- Start with `report.md`, whose graph aggregates candidates by crate pair;
  `report.json` and `graph.json` retain the unaggregated evidence, and
  `manifest.json` records the profile, scan size, candidate count, and cache use.
- `.config/similarity.toml` owns the project exclusions and the optional type
  dictionaries that give a pair its similarity, substitution safety, direction, and
  caveats.
- Audit and advisory analyze production source; strict also includes test paths and
  `#[cfg(test)]` items. The analysis is diagnostic: it alters no CI threshold or
  latency budget, and a high score is a refactoring candidate, never proof of
  behavioral equivalence.

## Dependency Policy

`AGENTS.md` owns the workspace-first rule. Beyond it: a crate reaches a version
with `{ workspace = true }` and never spells one out itself, a heavy crate taken
for a small utility has its cost checked first, and a new dependency is justified
in the task, plan, or PR description.

Tools: `cargo-deny` (licenses, bans, advisories, sources); `cargo-machete` and
`cargo-shear` (unused, misplaced, unlinked dependencies - new findings are boundary
debt, ignored only with documented metadata); `cargo-hack` (feature powerset);
`cargo-semver-checks` (release-facing API); `cargo-public-api` (manual surface
diff, one package at a time); `cargo-geiger` (unsafe inventory - audit evidence,
not a verdict). Add Dylint or Semgrep only for rule classes ast-grep and xtask
cannot express cleanly; never a second custom-rule stack for existing rules.

## Formatting Ownership

`just fmt` is the harness and `just fmt check` its gate;
`just tooling xtask format --only rust|manifest|toml|json|markdown` is for scoped
formatter work alone. Owners: `rustfmt.toml` for `.rs`, `.config/tomlfmt.toml` plus
`cargo-sort` for the mechanical `Cargo.toml` write pass, `taplo` for other TOML,
`tidy-json` for JSON/JSONC, `mdfmt` for Markdown - advisory until the historical
Markdown tree is cleaned up.

`just deps manifest dependency-order` owns the manifest gate: internal `kithara`
and `kithara-*` dependencies above external crates, each group sorted by key.
`cargo sort --check` must stay out of gates; it conflicts with that policy after
the post-pass. A file class gets a second formatter only when the owner changes
here and in `.config/just/fmt.just`.
