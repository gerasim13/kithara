# Real-time callback & locking

*Tier legend and the rest of the family: [performance.md](../performance.md).*

## RT-audio callback

The device callback runs under a hard sub-millisecond deadline, and every rule
below is one invariant: nothing reached from `process()` may block, wait
unboundedly, touch the allocator, or call the OS. Cross the boundary with a
wait-free SPSC ring for PCM and commands, and with atomics or `ArcSwap` for the
state the callback reads.

```rust
#[kithara::rtsan_forbid_blocking]
fn process(&mut self, out: &mut [f32]) {
    while let Some(frame) = self.cons.pop() { /* preallocated slots */ }
    if self.shared.playing.load(Relaxed) { /* atomic, no lock */ }
}
```

- **No blocking primitive** (I1). No lock guard, no `.await`, no channel `recv()`
  in the body: each can be preempted or wait unboundedly and produce an xrun.
  *detector: `kithara::rtsan_forbid_blocking` plus the rtsan and no_block lanes*
- **No logging, syscall, or I/O** (I4, I5). A subscriber formats, allocates, and
  locks on the emitting thread. `kithara::probe` (USDT) is the sanctioned RT-safe
  trace; otherwise bump a `Relaxed` counter the UI polls. A `warn!` is tolerable
  only on an already-lost cold branch such as panic or evict. *detector:
  `rust.no-debug-prints` covers `println!` and `dbg!`; tracing is manual*
- **No unbounded work or allocation** (I6). Preallocate to the declared maximum
  in `prepare()` or `new_stream()` and reuse the storage; the body indexes into
  it. No `Vec::new`, `Box`, `format!`, or map growth. See
  [allocation.md](allocation.md).
- **No std, tokio, or crossbeam channel across the boundary** (I7). mpsc
  allocates nodes and can park. Use a fixed-capacity `ringbuf` SPSC (`HeapRb`
  split into `HeapProd`/`HeapCons`); the RT side moves preallocated slots and
  coalesces or drops when full. *detector: `arch.no-raw-tokio-sync` (ast-grep),
  already-enforced*
- **No deallocation** (I8). Never let the last `Arc`, `Vec`, or `Box` drop inside
  `process()` - the `free()` then runs on the callback. Hand the released value
  back over a ring to an off-RT collector (`discard_track`).
- **No pool-miss fallback** (I9). Pre-fill the pool to a computed budget at
  startup, size that inventory with the buffer-pool benchmarks, acquire the
  checked guard during preparation, and count every miss so the budget can be
  fixed. A silent alloc-on-miss escape masks a broken budget contract; keep any
  fallback off the rtsan produce core. *detector: `perf.prefer-primitive-pool` +
  `perf.no-component-pool-construction`*
- **Thread priority** (I2). The device callback thread is created and
  QoS-promoted by cpal/firewheel, not by kithara, and the decode and produce
  threads are ring-decoupled from the callback deadline (degrade on underrun)
  rather than workgroup-bound. Promoting those owned feed threads to RT priority
  and joining the Apple os_workgroup is an open audit item, not current
  behaviour. Keep the RT thread count small and fixed.

*tier: hot | detector: rtsan and no_block lanes (mostly manual) | present in
kithara: SPSC rings rather than tokio or crossbeam mpsc, and tracing-free DSP
crates. The manual census scope is the kithara-play processor, not a workspace
sweep.*

Measuring this section - worst-case duration, xrun counters, profiler
perturbation - belongs to [benchmarking.md](benchmarking.md).

## Sync & atomics

**Read-mostly hot state uses `ArcSwap`, not a `RwLock` read.** `load()` is
wait-free; `read()` still CASes a shared cacheline under contention. Writers RCU
by storing a fresh `Arc`. Never `load_full()` per block - that is an atomic
refcount bump on every callback; clone the `Arc` once at setup and take a `Guard`
per block, and pre-warm the arc_swap debt node before playback so the first store
does not allocate on the RT thread. Serialize concurrent writers behind a small
write-side `Mutex` only when load-mutate-store must not race. Readers poll a
`snapshot()` (Relaxed loads of position, duration, frontier) instead of a channel
round-trip; channels carry commands and ownership transfer, not "where are we".
*tier: warm | detector: manual | present (offsets.rs migrated RwLock -> ArcSwap)*

**Weakest ordering that carries the invariant, never reflexive `SeqCst`.**
`SeqCst` is a full barrier on ARM and an independent counter needs none:
`Relaxed` for stats, a `Release` store paired with an `Acquire` load to publish
then consume data. Reserve `SeqCst` for a genuine total-order protocol - kithara
keeps it on the seek and play control flags - and record why. Downgrading an
existing `SeqCst` is bench-gated and correctness-sensitive, never a blanket
rewrite. *tier: hot | detector: manual | present (play/seek `SeqCst`, stats
`Relaxed`, seqlock `AcqRel`/`Release` plus fence)*

**One sanctioned lock: `kithara_platform::sync::Mutex`.** It wraps parking_lot
natively, carries a wasm implementation, and centralizes poison and hang
handling. parking_lot has no priority inheritance on Apple, so keep it off any
lock an RT or high-QoS thread can contend; cross that boundary with atomics or
SPSC instead. The std versus parking_lot swap debate is settled workspace-wide.
*tier: hot | detector: `arch.no-std-sync-mutex` (ast-grep) + devtools
platform_layer_hygiene | already-enforced*

**No priority inversion.** Without priority inheritance an RT thread that parks
on a lock a low-priority task holds stalls behind it. The RT side pulls a
pre-filled ring and degrades on underrun (`block_on_underrun` is offline opt-in
only); where waiting is legitimate use the ownership-carrying platform `Mutex`
and set explicit QoS on the producer thread. *tier: warm | detector: manual |
present (ring-decoupled produce)*

**No thundering-herd wakeups.** `notify_waiters`, broadcast, and condition
polling wake N threads to fight over one slot. Shard per loader lane so each item
is handed to exactly one receiver; `notify_one` and `Semaphore` wake bounded
waiters. mpsc means one worker per unit of work, watch means latest-value
fan-out. *tier: warm | detector: manual | present (loader lanes isolated per
track)*

**Latest-wins state does not belong in a queue.** A gain or playhead pushed
through mpsc leaves the consumer draining a stale backlog: store it in an atomic,
a watch channel, a triple buffer, or `ArcSwap`. Keep queues for event streams
where every element must be processed (the kithara-events broadcast). *tier: warm
| detector: manual | present*

## Watch-for (absent in kithara)

- **Non-lock-free AtomicCell** - the crossbeam type silently degrades to an
  internal spinlock on an oversized payload. If introduced, static-assert its
  lock-freedom or pack the state into a `u64`. Small state goes through plain
  atomics or `ArcSwap` today.
- **Naive spin-lock on the RT thread** - a hand-rolled `try_lock` loop, or the
  spin crate busy-waiting on the audio thread. RT sharing is lock-free
  ring/atomics/seqlock and non-RT goes through the platform `Mutex`. The existing
  `spin_loop()` is a bounded seqlock retry and the `compare_exchange()` sites are
  bounded CAS in bufpool budget accounting - neither is a busy-wait lock.
- **Naive seqlock** - reading the payload with plain non-atomic loads is UB even
  on retries that are discarded. Read per-word atomics `Relaxed`, then
  `fence()` with `Acquire` before rechecking the version. `SeqAnchorCell` in
  crates/kithara-hls/src/variant/flow/seqlock.rs is the reference form.
- **One global hot atomic counter** - a many-thread `fetch_add` on a single
  cacheline; shard it with cache padding, or accumulate per block and flush once
  with a `Relaxed` add. kithara's static atomics are rare id generators (one bump
  per worker or bus), not contended.
