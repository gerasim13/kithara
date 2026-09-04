# Allocation & memory

*Tier legend and the rest of the family: [performance.md](../performance.md).*

## Pooled buffers (flagship)

Every recurring decoded-sample or byte buffer on a hot or warm path leases from
the injected `PoolRegion`. A fresh `Vec` per block, per packet, or per segment
churns the long-lived heap, and a shrink_to_fit on a reused buffer hands the
pages straight back.

```rust
let mut buf = pools.get::<f32>();       // guard recycles on drop
buf.ensure_len(frames * channels)?;
for packet in packets { buf.clear(); buf.try_extend_from_slice(packet)?; decode(&buf); }
```

- The region is injected from the composition root; a component never builds one.
  `PoolConfig::initial_buffers` and `initial_capacity` size the inventory there,
  and `PoolConfig::trim_capacity` trims oversized returns at the lifecycle
  boundary. There is no separate warm-up path.
- `clear()` keeps the guard's capacity for the next pass; `ensure_len()` and
  `try_extend_from_slice()` grow only after checking both budgets.
- Never escape a budget rejection into a raw `Vec`: that hides a broken budget
  contract. See [realtime.md](realtime.md).
- Parse-and-discard scratch that never leaves one thread may live in a
  `thread_local!` `RefCell` cleared on entry. Pools exist for cross-task
  ownership; keep both kinds of churn off long-lived caches.

*tier: hot | detector: `perf.prefer-primitive-pool` + `perf.no-component-pool-construction` (ast-grep) | already-enforced*

## Layout and draw scratch

UI crates own no sample pool and their passes rerun for every frame that resizes
a document, so `perf.reuse-layout-scratch` covers `kithara-ui` and `kithara-app`
in place of `perf.prefer-primitive-pool`. It names the primitive types only: an
untyped `collect::<Vec<_>>()` in these crates almost always collects widgets,
brushes, or nodes, which no pool would own, and a buffer that leaves as
`Arc<[T]>`, `Rc<[T]>`, or `Box<[T]>` has escaped. The retained widget, or the
state its tree keeps, owns the storage; a list walked once should be an iterator
parameter, not a collection.

*tier: hot | detector: `perf.reuse-layout-scratch` (ast-grep) | already-enforced*

## Ownership & sharing

**Media payloads are ref-counted, never deep-copied.** Hand them across
fetch -> cache -> decode as `bytes::Bytes`: `clone()` is a refcount bump,
`slice()` is an O(1) alloc-free view, and the producer freezes once with
`BytesMut::freeze()`. Never `to_vec()` a range in order to share it. Share
immutable ids and URLs as `Arc<str>` - kithara-events already threads `item_id`
and `src` that way, and no string-interning dependency is needed. Stray copies
survive only in cold FFI spots (symphonia demuxer, android CSD).
*tier: warm | detector: manual | present*

**`Arc` is not ownership glue.** One owner plus borrows; snapshot or command
models across threads; one coarse `Arc` only where sharing is real. Per-field
`Arc`, `Arc<Atomic*>` glue, and `Arc<Mutex<Collection>>` god-maps are rejected by
the AGENTS.md red-flag gate and by `arch.no-arc-mutex-godmap` /
`arch.no-arc-mutex-collection`. *tier: warm | detector: ast-grep | already-enforced*

**Error paths do not allocate eagerly.** Model expected conditions as thiserror
enum variants or `Option`, not boxed strings, and build context lazily on the
error branch only. snafu was rejected. *tier: warm | detector:
`rust.no-inherent-to-string` + `rust.no-to-string-method` | already-enforced*

**A wasm transient peak is permanent.** `memory.grow` is one-way, so a
whole-segment collect pins its peak for the life of the process: stream through a
pooled guard sized at the composition root. Never wee_alloc. *tier: warm |
detector: manual | preventive (kithara ships a wasm build)*

**Global allocator.** kithara ships the system allocator; RT lanes are
allocation-free regardless of the choice, so a swap needs a per-target benchmark
(p99 decode loop plus phys_footprint, not RSS) and one place to live. If mimalloc
is ever adopted on Apple its MADV_FREE inflates RSS - pin >= 3.1.4 or set
MIMALLOC_PURGE_DECOMMITS=1; jemalloc on arm64 needs lg-page=14 (16K). *tier: cold
| detector: manual | no swap present*

## Cheap wins, each already caught by a lint

- **Collect-then-iterate roundtrip** - sum or fold the iterator; do not
  materialise a `Vec` to walk it once. *`perf.no-collect-iter-roundtrip`*
- **Needless clone / clone-assign** - `clone_from` reuses the destination's
  allocation in a reuse loop; a value only borrowed downstream needs no clone.
  *opt-in `just lint audit-clippy`*
- **Grow without reserve** - `Vec::with_capacity` plus `extend` when the length
  is known; the beat analyzer reserves `frames * 2`. *manual, red-flags*
- **Owned where borrowed would do** - `&str`/`&[u8]` parameters when the body
  only reads; `Arc<str>`/`Arc<[u8]>` instead of `Arc<String>`/`Arc<Vec<u8>>`.
  *clippy `unnecessary_to_owned`, `ptr_arg`=deny, `redundant_allocation`,
  `box_collection`*
- **`format!` for concatenation, or comparing an owned string** - push onto an
  owned `String`, and compare the typed value instead of its rendering. *clippy
  `useless_format` / `cmp_owned`*
- **`vec!` for fixed data** - iterate the array literal. *clippy `useless_vec`*
- **drain-collect** - `mem::take` for the whole buffer; `drain(..)` fed into
  `extend` or `for_each` when the tail must survive. *clippy `drain_collect`*
- **Fat enum variant / by-value parameter** - an unboxed fat variant sizes every
  value of the enum to its largest arm; box the payload and pass `&AudioChunk`.
  *clippy `large_enum_variant` / `needless_pass_by_value`*
- **Write-only collection** - delete the collection and the loop that fills it.
  *opt-in `just lint audit-clippy`*

## Watch-for (no such shape in kithara today)

- **Boxed node graph instead of an arena** - per-node `Box`/`Rc` object soup in a
  phase-scoped tree; use bumpalo (Copy/POD) or typed-arena (runs Drop), scoped to
  the phase.
- **`Rc<RefCell<_>>` or `Arc<RwLock<_>>` pointer web as the data model** - use
  slotmap index handles, or an owner plus ids. Only config-level `Arc<RwLock<_>>`
  exists, already governed by `arch.no-arc-mutex-godmap`.
- **serde owned `String`/`Vec` for borrowable input** - deserialize borrowed and
  keep the source `Bytes` alive. kithara uses rkyv plus a hand-written HLS parser,
  so no serde-borrow path exists.
- **Manual zeroing loop** - `vec![0.0f32; n]` lowers to `alloc_zeroed` for a fresh
  buffer, and `fill()` on the live slice reuses an RT buffer; never hand-roll the
  loop. Existing single-element `push` sites are padding, not zero-fills.
- **dhat heap-count assertions in the parallel libtest harness** - libtest runs
  tests in parallel, so a no-alloc RT contract assertion needs its own
  integration-test file (its own process).
