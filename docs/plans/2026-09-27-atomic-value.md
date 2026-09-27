# 2026-09-27-atomic-value

## Goal

Replace the config-only `LiveBool` and `LiveF32` wrappers with a reusable,
platform-owned atomic scalar whose value type and load/store orderings are part
of its Rust type. Reuse it for existing plain scalar fields outside configs
without changing their concurrency contracts.

## Understanding Summary

- Config and non-config structures repeat atomic scalar `load`/`store` code.
- Callers should choose load and store orderings once in the field type.
- The abstraction belongs to `kithara-platform`, not `kithara-config`.
- Normal, Loom, flash + Loom, and wasm builds must use the appropriate backend.
- Existing real-time reads and writes remain non-blocking and allocation-free.
- Synchronization protocols using CAS, epochs, or packed state are not a
  mechanical migration target.

## Assumptions

- This is a behavior-preserving refactor for existing fields; their current
  ordering, initial values, and ownership stay unchanged.
- There is no new security or privacy boundary. Reliability requires that
  Loom observes the atomic operations, rather than merely compiling the type.
- The number of atomic fields is small; no dynamic dispatch or runtime ordering
  selection is needed. The owning crate is responsible for maintenance.
- Public `const new` remains available in normal builds. Loom's atomic
  constructors are not `const`, so the Loom build uses runtime construction.

## Decision Log

1. Use `AtomicValue<T, Read, Write>` with zero-sized ordering marker types.
   Direct `Ordering` const parameters are not supported on stable Rust;
   numeric order codes would obscure the API and permit invalid combinations.
2. Own the mechanism in `kithara-platform`. A new crate would add an unnecessary
   boundary; keeping it in config would prevent reuse outside config.
3. Preserve `portable-atomic` for normal builds and select Loom atomics for
   native `loom` builds, including `flash + loom`. Always using standard
   atomics would change the current portability behavior.
4. Store `f32` as its exact `u32` bit pattern in an atomic `u32`. Loom has no
   atomic float, but does model atomic `u32`; the conversion preserves signed
   zero and NaN payloads.
5. Expose only fixed-order `load` and `store` for this scalar abstraction.
   CAS, RMW, and publication protocols retain their existing owners and APIs.

## Final Design

`AtomicValue<T, Read, Write>` maps each supported primitive to one backend
atomic. Read markers allow Relaxed, Acquire, or SeqCst; write markers allow
Relaxed, Release, or SeqCst. Invalid load/store orderings do not type-check.
Implement only primitive types used by the migration, initially `bool` and
`f32`. Add `u8` only if the warp ownership audit explicitly qualifies its
fields for migration. Provide descriptive
aliases for repeated Relaxed/Relaxed combinations instead of retaining the
config-specific `Live*` names. Cloning creates an independent value snapshot,
as the current config wrappers do; shared ownership remains the caller's job.

The platform module selects `portable-atomic` in ordinary builds and the
existing platform Loom atomic facade in native Loom builds. For `f32`, the
selected backend provides `AtomicU32`, with `to_bits`/`from_bits` at the edge.
Do not pass through `portable-atomic` when Loom is active. Keep the platform's
native/wasm and flash feature selection intact.

Remove the config-local wrappers and use platform aliases in `PlayerConfig`.
Migrate non-config fields only after checking every load/store and ownership
path: the held wasm player level, worker load meters, EQ gains, and engine
master volume are candidates. Make a separate go/no-go decision for warp's
keylock/backend controls after tracing their coupled domain invariant.
Leave lifecycle flags, playback epochs, queue IDs/positions, and packed
CAS-based controls on their existing domain-specific paths.

## Success Signal

- [ ] Existing config and selected non-config fields use one platform type;
      values and ordering semantics remain unchanged.
- [ ] Loom models both boolean and float-backed operations, including with
      flash enabled; ordinary and wasm builds retain their behavior.
- [ ] Existing config `Clone`/`Debug`, builder, and patch behavior still pass.
- [ ] PR #475 remains Draft until both exact-head forges pass again; never merge.

## Affected Paths

- `crates/kithara-platform` for the primitive and its backend/model tests.
- `crates/kithara-config` and selected consumers in `kithara-play` and
  `kithara-host`; `kithara-warp` only if its fields pass the ownership audit.
- This plan; the platform wiki only if its described contract changes.

## Required Reads

- `AGENTS.md`, `docs/workflows/rust-ai.md`, and relevant crate READMEs.
- `docs/guides/architecture-shape.md` and
  `docs/guides/review-validation.md` for ownership and PR readiness.

## Validation Scope

- Contract tests for primitive bit preservation, independent cloning, valid
  ordering policies, and a Loom model that asserts cross-thread boolean and
  float-backed value outcomes, not just compilation.
- Focused harness tests for migrated consumers; native Loom and flash + Loom
  runs; `just platform wasm check`.
- `just fmt check`, `just lint fast`, and `just test` on final source; then
  exact-head GitHub and GitLab CI before restoring Ready for review.

## Split Map

- One integrator owns the platform primitive, consumer migration, tests, and
  PR readiness. No parallel edits across this shared type boundary.

## Sequencing Dependencies

1. Verify current PR/base state and mark Draft before changing source.
2. Add and test the platform primitive, including Loom selection.
3. Migrate config, then individually audited non-config consumers.
4. Validate final source, push, and await both exact-head CI verdicts.

## Integrator

- The current Codex task owns the complete change and final evidence report.

## Risks And Non-Goals

- Risk: Loom cannot use `const new`; verify that no migrated field needs a
  constant initializer in Loom builds.
- Risk: replacing `AtomicF32` with a bit-preserving `AtomicU32` must not change
  float behavior or wasm support; test both explicitly.
- Non-goals: blanket conversion of all atomics, changing ordering strengths,
  adding CAS/RMW to the generic wrapper, or merging PR #475.
