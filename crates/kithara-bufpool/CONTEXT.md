# kithara-bufpool — Context

Contracts and invariants for kithara-bufpool; the README is the overview.

## Allocation Flow

`get`/`put` are lock-free: each shard is a bounded `crossbeam_queue::ArrayQueue`, so both producer and consumer recycle buffers without taking a lock — safe to call on the real-time produce/consume cores.

1. **Get:** `pop` from the home shard (`current_thread_id() % SHARDS`). On a miss, probe up to `Pool::MAX_PROBE` (4) neighbour shards for work-stealing. If those probes miss, allocate via `T::default()`; other unprobed shards may still hold buffers.
1. **Return (drop):** `Reuse::reuse(trim_capacity)` clears and optionally shrinks, then `push` onto the home shard. If `reuse()` rejects the buffer or the queue is full, the buffer is dropped and its bytes released from the budget.

Each shard's queue capacity is fixed at construction (`max_buffers / SHARDS`, clamped to `PoolShard::MAX_SLOTS` = 1024 so count-unbounded pools such as `BytePool` do not request an exabyte array). For those pools the byte budget — not the slot count — is the real memory cap.

## Region and the Shared Budget

`Region` is the canonical owner of one byte budget shared by a `BytePool` and a `SamplePool`; `RegionConfig` exposes exactly one knob, `max_bytes` (default 256 MB). Composition roots — app `main`, the FFI `NativeInner` and its asset store, a standalone `Queue` that builds its own player — construct one `Region` and pass region-derived pools down through configs. Library code never calls `BytePool::default()` / `SamplePool::default()` outside tests; those are process-wide `OnceLock` singletons kept for top-level convenience. Pools built via `new` / `with_byte_budget` own a private budget.

The budget counts **tracked pool capacity in bytes**, not RSS: allocator metadata, rounding, transient copies during growth, and plain `Vec`s outside the pool (e.g. time-stretch scratch) are not covered. Checked growth never crosses the configured cap. Infallible initialization can cross it, keeps the actual capacity tracked, and reports the event through `budget_overshoots`.

### Travelling charge

A buffer's byte charge is acquired at first growth and released only when a return is rejected (`put` into a full shard or a `reuse()` refusal). The charge **travels with the buffer**: `PooledOwned::into_inner()` does not release it, and `SharedPool::attach()` does not charge — so `into_inner → attach` round-trips (the time-stretch planar scratch in `kithara-stretch`) stay balanced. Two consequences:

- `attach` is only for values whose capacity this pool already accounts for; importing genuinely external memory needs a charging API, which is deliberately absent until a production consumer exists.
- Extracting a buffer via `into_inner` and dropping it without `recycle` leaks accounting (the bytes stay counted). All current call sites recycle in `Drop`.

### Controlled growth

`ensure_len` is transactional: it reserves the budget delta **before** allocating (`try_reserve_exact` into a fresh `Vec`), reconciles the actual capacity against the reservation, and rolls back fully on any failure — a failed call leaves length, capacity, and budget untouched. Growth is amortized (doubling, falling back to the exact request when the budget cannot afford the double) so incremental `ensure_len` loops stay O(n).

After acquisition, `ensure_len` is the only way to grow a `SampleBuffer`. The nominal guard derefs to `[f32]`, not `Vec<f32>`, so raw growth mutators (`resize`, `reserve`, `extend`, `extend_from_slice`, `push`, ...) do not exist on it. Length still shrinks through `clear`, `truncate`, `drain`, `retain`, and `dedup`; capacity and slice access come through the deref.

Initialization has separate, deliberate contracts:

- `get_with` exposes the inner `Vec` to an initializer, and `collect` clears then extends that same reused allocation from an iterator. Both operations are infallible: the pool measures their capacity delta afterward. Growth beyond the configured cap is retained and increments `PoolStats::budget_overshoots` / `RegionStats::budget_overshoots`.
- `pre_warm` initializes a new buffer before admission, then requests its full byte charge. If the budget rejects it, pre-warm drops that buffer and stops.
- `into_inner()` hands back the owned `Vec`; its existing charge must return through `recycle` or `attach` as described above.

The byte side keeps `PooledOwned<_, Vec<u8>>` deref to `Vec<u8>` because its I/O consumers grow through `Read`-style `&mut Vec<u8>` sinks; there raw `DerefMut` growth past the cap stays observable via `PoolStats::budget_overshoots` / `RegionStats::budget_overshoots` rather than compile-blocked.

## Integration

Pools are injected through configs (`AudioConfig::byte_pool` / `sample_pool`, `FileConfig::pool`, `HlsConfig::pool`, `ResamplerSettings::sample_pool`) so each surface owns its sizing policy.

`SharedPool`, `Pool`, `Pooled`, `PooledOwned`, `Reuse`, and `PoolStats` are re-exported `#[doc(hidden)]` for workspace-internal use only.
