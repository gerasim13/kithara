# Kithara Agent Index

This file is the canonical repo-level contract for Codex, Claude Code, Cursor,
and other coding agents. It carries only what must stop a wrong design before
code is written. Everything else is routed to an owner.

## Authority

- `AGENTS.md` owns repo-wide invariants, routing, and the red-flag gate.
- `docs/workflows/rust-ai.md` owns task setup, split, handoff, integration, and
  the task packet, handoff, and final report shapes.
- `docs/guides/*` holds lazy-loaded reference guidance. `docs/README.md` maps
  every guide to the trigger that should open it; open nothing else.
- `docs/rules/*` holds tool-neutral entry rules used through tool shims.
- On conflict: `AGENTS.md` wins over `docs/*`, which wins over crate
  `README.md` / `CONTEXT.md`, which wins over tool-specific shims.
- `docs/guides/rule-placement.md` decides where a new rule belongs.

## Sources Of Truth

Every fact below has one owner. Link to the owner; do not restate it.

| Fact | Owner |
| --- | --- |
| Capability status, roadmap, blockers | [GitHub Projects board](https://github.com/users/gerasim13/projects/3) |
| What is in flight right now | `PROGRESS.md` |
| Project architecture | `crates/kithara/CONTEXT.md` |
| Crate contracts, invariants, lifecycle | owning crate `CONTEXT.md` |
| Toolchain, image, and tool versions | `.config/ci-pins.toml` |
| Lint thresholds and baselines | `.config/arch/`, `.config/style/`, `.config/idioms/` |
| Command surface | root `justfile` and `.config/just/` |
| Released behavior | `CHANGELOG.md` |

## Core Principles

- Minimal magic and hidden dependencies. Predictability, testability, and
  reproducibility come first, and components stay loosely coupled.
- Code is the only source of truth. An entry in `CONTEXT.md` is admissible only
  when it cannot be expressed in the shape of the code or pinned by a test. An
  explanation that exists because the code is unclear means the code is wrong,
  and a comment without a marker is removed rather than kept.

## Non-Negotiables

- No speculative code. Do not add helpers, branches, or abstractions that the
  current task does not use, and choose the simplest implementation that fully
  meets the requirements.
- Workspace-first dependencies. Add versions only in the root workspace and
  reuse existing crates when possible.
- Encoded/container media types live in `kithara-stream`. Do not duplicate
  `AudioCodec`, `ContainerFormat`, or `MediaInfo` elsewhere. Decoded-audio
  signal value types and pure sample/time math live in `kithara-signal`.
- Do not use `unwrap()` or `expect()` in production code without a strong,
  explicit reason.
- Name the canonical owner before changing shared state, shared types, or
  cross-crate contracts. If the owner is unclear, stop and clarify.
- Do not introduce parallel mutable sources of truth. When old and new state
  must coexist, stage the ownership transfer in the task packet or plan.
- No fallback chains (`try A, else B, else C`) to paper over state-resolution
  bugs. If the primary path has no correct answer, the state contract is broken:
  fix the contract. A legitimate fallback (user-facing default, optional config,
  degraded mode) is justified in the owning crate `CONTEXT.md` or the task
  packet; a test that codifies one protects a symptom.
- Prefer generics and composition over near-duplicate protocol-specific types.
- Use `tracing`, not `println!` or `dbg!`, in production code.
- Do not use destructive git commands unless the user explicitly asks for them.
- Cancel-token hierarchy is typed and propagate-down. Hard-coded
  `CancelToken::root()` and `CancelToken::never()` are forbidden outside
  sanctioned owner sites. See `docs/guides/cancel-policy.md`.
- Zero tolerance for lint suppressions and baseline growth. An unavoidable lint
  means STOP and fix the underlying code. See `docs/guides/lint-policy.md`.
- Optimize for performance in hot paths. See `docs/guides/performance.md`.
- Use repo harnesses for acceptance and formatting: `just test` and `just fmt`.
  Raw `cargo test`, `cargo nextest`, or direct formatter commands are scoped
  probes only, not final validation claims.
- Do not preserve backward compatibility.

## Command Routing

Use these exact paths. Do not spend a turn on `just --list`; the root `justfile`
exposes domain modules only, and recipes live under `.config/just/`.

- Format: `just fmt`; check-only `just fmt check`.
- Compile and Clippy: `just check`; `just check clippy`.
- Lint: `just lint`; `just lint fast` is the commit gate; `just lint full`.
- Duplication report: `just lint similarity [<crate>/src ...]`.
- Test: `just test`; `just test run <args>`; `just test all` adds doc-tests.
- UI suites: `just test ui`; through a real window, `just test ui-window`.
- CI: `just ci gate`; `just ci audit <scope>`; `just ci health`;
  `just ci report --artifacts <dir>`.
- Repeated-run evidence: `just ci stress <args>`; `just ci stress-report <args>`.
- Architecture: `just arch viz` and its scope flags.
- Quality: `just quality lab list|run <profile>`; `just quality coverage-risk`;
  `just quality assess`.
- Platforms: `just platform apple|android|wasm ...`.
- Cached xtask access is exceptional: `just tooling xtask <subcommand>`.

Flags and output contracts for `arch viz` and `quality assess` live in
`docs/guides/tooling.md` and `docs/skills/quality-assessment/SKILL.md`; read the
printed `manifest.json` before using either as evidence.

## Agent Red-Flag Gate

Reject a design before coding when it:

- Has no canonical owner for touched state, shared types, or cross-crate
  contracts, or creates multiple mutable sources of truth.
- Masks state-contract bugs with fallback, retry, sentinel, or workaround
  branches.
- Crosses platform, protocol, surface, test, or crate-layer boundaries without
  owning the contract.
- Widens public API or adds ad-hoc Rust shapes instead of standard traits,
  domain types, config, or builders.
- Uses `Arc<...>`, especially `Arc<Atomic*>`, as ownership glue instead of an
  owner, command, or snapshot model.
- Introduces shared mutable god-state, globals, god objects, callback spirals,
  or unrelated responsibilities in one file, type, trait, or facade.
- Requires a lint suppress, new baseline entry, or "temporary" bypass to pass.

`docs/guides/red-flags.md` expands this gate for non-trivial work.

## Definition Of Done

A change is done only when all of these hold:

- A test that failed before the change now passes, and it pins the contract
  rather than an incidental detail.
- `just fmt check` and `just lint fast` are clean, with no new baseline entries
  and no lint suppressions.
- The acceptance target named in the task packet passes, and the claim cites
  harness output, not a scoped probe.
- Documents describing the changed contract are updated in the same change.
- `PROGRESS.md` names what landed and what is left.

## Working Rules

- One task in flight per session. Finish it or hand it off before opening a
  second front in the same files.
- Start from the task packet in `docs/workflows/rust-ai.md` when the task is
  non-trivial, shared, or coordinated; a small single-owner task goes direct.
- Treat a non-trivial task packet as incomplete until `Constraints`,
  `Non-goals`, and `Validation scope` are filled in.
- Read only the docs and crate files that match the owned paths.
- A task that needs a plan follows `docs/plans/_template.md`.
- If shared boundaries are unclear, stop and clarify before implementation.
- Do not restate a repo rule in tool-specific files; route to the canonical doc.

## Resolving Rule Conflicts

- If a product requirement conflicts with these rules, discuss the compromise
  first and update `AGENTS.md`.
- Any forced rule bypass is explained briefly in the owning crate `CONTEXT.md`.
