# kithara-storage — Context

Detailed contracts and invariants for the kithara-storage crate; the README is the overview.

## Blocking coordination

An async writer and one or more sync readers share a resource: the downloader writes through `write_at`, readers park in `wait_range` until the requested byte range lands.

- `wait_range` blocks the calling thread on `kithara_platform::sync::CondvarGate`, which unifies the guarded readiness state and its condvar behind one lock. The wait is event-driven; there is no polling interval and no timer. `wait_range_with_cancel` adds a per-wait token: it wakes and interrupts only that caller and never mutates the shared resource lifecycle.
- Readiness transitions that notify the gate: bytes written, `commit`, `fail`, `reactivate`. Resource and per-wait cancellation have no lifecycle notify of their own, so each wait registers the relevant cancel wakers under the *same* state mutex it is about to release. That closes the lost-wakeup window; without it a cancel could only be learned by re-polling.
- Outcomes: `Ready` once the range is covered (also when a committed resource exposes no `valid_window`), `Eof` when the resource is committed and the range starts at or beyond `final_len`, and typed errors otherwise — `Failed`, `Cancelled`, `InvalidRange` for `start > end`.
- `WaitOutcome::Interrupted` exists for wrappers but is never produced by this crate. Upstream layers synthesize it (`kithara-assets` processing gate, `kithara-hls`, `kithara-file`) when a seek or an uncommitted processing pipeline must abort the read.
- The 180 s `WAIT_HANG_TIMEOUT` watchdog is a deadlock detector, not a timeout. It is sized well above the `kithara-net` inactivity timeout plus retry backoff so a stalled upstream is failed by the network layer first, and it resets whenever the available prefix of the waited range advances — a slow but progressing fetch never trips it.

## Lifecycle invariants

- `len()` reports `Some` only while the *lifecycle* is committed. A reactivated (being-rewritten) resource has no known total even though its committed snapshot stays published, so readers keep serving consistent bytes during the rewrite.
- Dropping an uncommitted writer marks the core failed, except when the resource is already committed, already failed, or cancelled — cancellation is routine shutdown, not a writer error.
- `ResourceStatus` priority: `Failed` and `Committed` outrank `Cancelled`, which outranks `Active`. A resource that produced committed bytes before its token fired still reports `Committed` so observers can read those bytes.
- `AvailabilityObserver` hooks (`on_write`, `on_commit`) fire after the state lock is released, so implementations may take their own locks without deadlocking against `wait_range` waiters. `commit(None)` is silent.

## Mmap vs Mem

| Aspect | `MmapDriver` | `MemDriver` |
| --- | --- | --- |
| Backing | `mmap-io::MemoryMappedFile` | `ByteBuffer` plus an `ArcSwapOption<Vec<u8>>` committed snapshot |
| Lock-free fast path | Yes — `SegQueue` of ready ranges consumed by `try_fast_check` | No |
| Growth | `growth_factor`x on overflow, 2x by default | Checked extend on write, charged against the buffer's region budget |
| `path()` | `Some` | `None` |

Neither driver evicts: `valid_window()` is `None`, so a published committed snapshot implies gap-free coverage of `[0, committed_len)` and `contains_range` takes a lock-free fast path.

Lock-free is only half of what the produce core needs from `contains_range`: an active resource answers from the `available_snapshot` generation, `write_at` publishes a new one on every write, and a read racing that write can end up the *last* owner of the replaced generation — freeing a range tree on the audio thread (`RTSan`: unsafe-library-call in `free`). So a read never drops the snapshot it loaded: it parks the reference in the resource's retire bin, and the write side (`write_at`, `commit`/`seal`) drains the bin and pays the frees. A full bin leaks rather than freeing on the reader, which can only happen while writers are idle — exactly when no generation is being replaced.

Both option types are `#[non_exhaustive]` `bon` builders: `MmapOptions::for_path(path)` (then `mode` / `initial_len` / `growth_factor`) and `MemOptions::builder()` (`buffer`, `initial_data`, `capacity`). `initial_len` defaults to one 64 KiB block and `growth_factor` to 2; the defaults live in the builder attributes, so the options type is the only place that states them. `initial_len` is also the size a write to an empty mapping creates the file at, so a caller that asks for `0` gets a file sized to the write rather than a default-sized one.

## Chunked atomic claim

`AtomicChunked::open(canonical_path, factory)` opens a fresh chunked-atomic resource. `factory` opens the inner resource at a given filesystem path and is called twice: once with `<canonical>.tmp` and `OpenIntent::Fresh` during the constructor, and once with the canonical path and `OpenIntent::Reopen` after the commit rename. The factory MUST honour the intent and return a `Committed`-status resource for `Reopen`, otherwise the wrapping lease layer mistakes the just-renamed file for an abandoned writer and deletes it.

`open` claims `<canonical>.tmp` by taking an exclusive advisory lock on it (`File::try_lock` — `flock(LOCK_EX)` on Unix, `LockFileEx` on Windows), held for the claim's lifetime. The lock, not the file, is the claim: the OS grants it to exactly one open file description, so a second concurrent open of the same tmp path loses it and gets `StorageError::TmpClaimed` — no in-process registry is involved. The caller polls until the holder releases (commit, fail, or drop) and then retries, or takes a passthrough view once committed. `kithara-file` implements that poll loop for remote file sources.

Stale temp from a crashed run is reclaimed by the next `open`: the OS releases the lock when the owning process dies, so a leftover from `kill -9` (which skips `Drop`) carries no lock and the next claimant takes it over, truncating it first. Liveness is therefore the lock, never the file's existence — the two are indistinguishable on disk, and a segment tmp is preallocated to its reservation on the first open, so size and mtime say nothing either. `TmpClaimed` means a *live* writer, which is what makes a caller's poll loop terminate; callers own no cleanup.

`commit` flushes durably (`sync_data`), renames tmp → canonical, then swaps the inner via the factory, so an external observer of the canonical path sees either no file or the fully durable committed bytes.

## Uniform decorator wrapping

Every `StorageResource` variant wraps its inner in an `AtomicChunked`: fresh segment writes use it in atomic mode, while re-opens of already-committed files and memory-backed inners use `AtomicChunked::passthrough` (no atomicity, no cost beyond the `Arc`). Uniform wrapping means no code path can accidentally bypass the atomic-on-commit guarantee.

## In-place decorator lifecycle

The public `Resource<Active>` lifecycle is consume-self: `commit`, `reactivate`, and the failure transition move the writer handle, so a second commit or a write after commit is a compile error for external callers.

`commit_in_place`, `reactivate_in_place`, and `fail_in_place` are `pub(crate)` hooks for the single-owner storage decorators (`Atomic`, `AtomicChunked`) that must rewrite a file in place: `OpenMode::ReadWrite` index files and the chunked tmp/commit cycle. Those decorators own their writer exclusively and never clone it, so the in-place transition creates no second mutable owner and does not weaken the external consume-self contract.

## Notes on the `redundant_reexport` audit warning

`just ci audit kithara-storage` flags `MemOptions` and `MmapOptions` as surfaced twice — via `pub use` from `lib.rs` and via the `<Driver>::Options` associated type. The duplication is intentional and documented here per `AGENTS.md`:

- `MmapOptions` / `MemOptions` are the canonical user-facing constructor types, reached as `kithara_storage::{MmapOptions, MemOptions}` by callers across the workspace.
- `Driver::Options` is the binding that lets the generic `Resource::<D>::open(cancel, opts)` pick the driver from the option type alone via call-site inference. Removing it would force every caller to pre-qualify with a driver alias (`MmapResource::open` / `MemResource::open`) — a wider API ripple than the duplicated path is worth.
- Dropping the `pub use` (the audit's literal suggested fix) would break every external caller.
