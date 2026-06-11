# Architecture Review — 2026-06-11

Scope: full workspace pass focused on readability, "C-style vs Rust" patterns, and
the architectural roots of recurring bugs. Findings were collected per crate
(audio, play, stream/net, hls/abr, assets/storage, decode) plus a cross-crate
seam audit, then deduplicated into the systemic themes below.

Verdict in one line: the workspace has good bones (typestate FSMs, decorator
stores, sealed phases), but state is still managed C-style — scattered flags,
bare integers across coordinate spaces, parallel sources of truth, and fallback
chains — and that is exactly where the recurring bugs come from.

## Theme 1 — State machines smeared across flags/atomics instead of enums

The single biggest readability and correctness problem. Recurring shape: a
struct accumulates `bool` / `Option` / atomic fields that together encode a
state machine, but the states and transitions are never named, so invariants
live in the maintainer's head.

| Site | Evidence |
|---|---|
| `kithara-play/src/impls/player.rs:125-160` | `PlayerImpl`: 8 atomics + 5 mutex fields + cancel/engine/bus/pool. Interdependent fields (`current_index`, `pending_next`, `status`) are updated independently with no single transition point. |
| `kithara-play/src/impls/player.rs:49-53` | `PendingNext { src, activated: bool, index }` — a hand-rolled two-phase commit. `commit_next` and `finalize_handover_if_armed` race over `activated`; if the audio thread wins, `CurrentItemChanged` is skipped and queue `current_index` goes stale. Direct bug source. |
| `kithara-play/src/impls/player_track.rs:105-107` | `notified_track_requested` / `notified_prefetch_requested` bool pair reset in 3 places; ordering-dependent duplicate/missing notifications. |
| `kithara-audio/src/pipeline/source.rs` | `StreamAudioSource` resume state: `resume_target` as `(epoch, Duration)` tuple + `eof_drain_queue: Option<VecDeque<_>>` + stat counters that drive logic. Epoch mismatch silently no-ops (`pending_skip_amount`). |
| `kithara-audio/src/worker/decoder_node.rs:16-46` | `DecoderRuntime { eof_sent, preloaded, seek_epoch, chunks_sent }` reset via struct-update; adding a field silently breaks the reset. |
| `kithara-decode/src/gapless/trimmer/core.rs` | `GaplessMode` + `tail_buffer` + `tail_buffered_frames` + dual-purpose `trailing_frames` ("sometimes metadata, sometimes scan window"). |
| `kithara-hls/src/variant.rs:174,254-265` | `DownloadClaim.settled: bool` duplicates what the `Segment<Downloading>` typestate already proves; the Drop-side warning is a runtime patch over a compile-time guarantee. |
| `kithara-audio/src/audio.rs` | `preloaded: bool` shadows `ConsumerPhase` (two sources of truth for the same fact). |

Fix direction: per site, name the states (`enum PendingSlot { None, Armed{..},
Activated{..} }`, `enum NotificationState`, `enum SeekResume`), make transitions
single methods, delete the shadow flags. This is mechanical, low-risk, and
directly removes race windows.

## Theme 2 — Parallel sources of truth that must be hand-synced

The repo contract explicitly forbids this, yet it recurs at every layer and is
the most plausible root of the "periodic bugs":

1. **Assets: availability index vs filesystem** (`kithara-assets/src/disk_store.rs`,
   `index/availability/core.rs`). Deletion mutates in-memory index, on-disk
   pins/lru indexes, and the FS as three separate steps; a crash or interleave
   strands stale availability (the known red test
   `red_test_delete_asset_strands_availability_index`). Needs a transactional
   delete order (index clear before FS op, or availability inside the atomic
   write boundary).
2. **Stream timeline: `segment_position` vs committed position**
   (`kithara-stream/src/timeline.rs`). Read path sets one, decoder path the
   other; never reconciled. Flagged as transitional in README — should be
   collapsed, not kept as a footgun.
3. **HLS/ABR: `AbrState::current_variant` and `HlsPeer::reader_segment` as two
   independent `Arc<AtomicUsize>`** (`kithara-abr/src/state/core.rs`,
   `kithara-hls/src/peer.rs`). Boundary-commit logic can observe a torn pair.
   A single snapshot (`SegmentBoundary { variant, segment }` behind one lock or
   seqlock) removes the class of bug.
4. **Play/queue: player `items` + `current_index` vs queue ownership**
   (`kithara-play/src/impls/player.rs:136`). README says the queue owns
   `current_index`, but the player mirrors it and `PendingNext` tries to sync it
   back. One owner, one writer.
5. **HLS layout/segment-store ordering** (`variant/layout.rs` +
   `variant/segment_store.rs`): `apply_commit` must run before
   `apply_loaded_size`; documented but not enforced — needs a single atomic
   `apply_commit_atomic` entry point.

## Theme 3 — Coordinate spaces on bare `u64`/`usize`/`u32` (primitive obsession)

`.docs/workflow/rust-ai.md` explicitly demands translation boundaries between
coordinate spaces; the code has none:

- **Byte offsets** in kithara-stream mix virtual-stream, committed, and
  segment-relative spaces, all `u64` (`stream.rs`, `timeline.rs`).
- **HLS segment indices** mix playlist-local and download-head spaces, with
  `u32::MAX` as "unknown" sentinel (`variant.rs:983,1254,1267`) and `-1` as
  "init segment" sentinel (`segment_store.rs:126-137`) — literal C error codes.
- **Decode** mixes demux-time / decoded-time / output-time durations and frame
  counts with saturating casts (`composed.rs:160-214`,
  `gapless/trimmer/core.rs`).
- **Seek epochs** are bare `u64` compared ad hoc in audio, stream, and play.

Fix direction: small newtype layer (`VirtualByte`, `SegmentIdx`, `SeekEpoch`,
`FrameCount`), conversions only at named boundaries, `Option` instead of MAX/-1
sentinels. High leverage: the compiler starts catching the bug class the owner
keeps hitting at runtime.

## Theme 4 — God files / god functions

- `kithara-audio/src/pipeline/source.rs` — **2620 lines**, mixes shared-stream
  wrapper, format-change FSM, seek recovery, gapless, effects, decoder
  recreation. Split into `format_change.rs`, `seek_recovery.rs`, `gapless.rs`,
  orchestrator-only `source.rs`.
- `kithara-hls/src/variant.rs` — **1504 lines**; `HlsVariant` owns fetch
  scheduling, segment metadata, and playback read state. Extract
  `SegmentQueue` / metadata view / read cursor.
- `kithara-play/src/impls/player_processor.rs::render_audio` (~150 lines):
  mixing + handover detection + preload scheduling in one loop, with handover
  state computed twice. Extract pure `detect_handover` step.
- `kithara-decode/src/composed.rs::next_chunk_inner`: decode + trim + skip +
  pool management in one loop; `ComposedDecoder` itself carries 6+ orthogonal
  concerns.
- `kithara-play/src/impls/session/state.rs`: hand-rolled command FSM across 15+
  helpers with mixed `String` error styles; transitions invisible.
- `kithara-stream/src/dl/batch.rs::deliver` (~lines 367-462): 2×2×2 nested
  match over delivery outcomes, duplicated per closure arm — wants a
  `DeliveryState` enum.

## Theme 5 — Fallback chains papering over state bugs (forbidden by AGENTS.md)

- **HLS cross-variant fallback search** (`kithara-hls/src/coord.rs:293-311`,
  `variant.rs::variant_serving`): when the active variant can't resolve a byte,
  silently search shrunk historical variants. Masks `byte_shift` computation
  bugs; should fail loudly / trigger reset.
- **Position/duration fallback** in
  `player_processor.rs::update_position_duration`: "no leading track produced
  an outcome" is patched by re-reading per-track state instead of guaranteeing
  exactly one leader.
- **Gapless source chain** (metadata → codec priming → heuristic) is silent;
  which path won is invisible to callers and logs (`composed.rs`,
  `gapless/trimmer`). Surface the chosen mode in `DecoderTrackInfo` + trace.
- **Decoder seek recovery** (`source.rs::recover_from_decoder_seek_error`):
  try-seek / catch / recreate-decoder retry loop compensates for stale
  Symphonia state after variant switches instead of detecting the format change
  before seeking.
- **Availability disk-probe fallback** (`kithara-assets/src/unified.rs`,
  `cache.rs::open_resource` linear rescan) patches gaps the index should not
  have.

## Theme 6 — Silently swallowed errors and lost signals

- `try_push(...).ok()` on the notification ring (15+ sites in
  `player_processor.rs` / `player_track.rs`): events (`Unloaded`,
  `PlaybackStarted`) vanish when the consumer lags, no log, no counter.
- `try_lock()` in the audio path (`player_processor.rs:271,409,745`,
  `player_track.rs:240,403,540,618`): contention silently drops commands such
  as rate updates. RT-safe, but failure must be observable (atomic param push
  or deferred retry, plus a counter).
- `unwrap_or(0)` / `unwrap_or(u64::MAX)` sentinels in arithmetic
  (`variant/layout.rs:134`, `stream.rs:564`, `composed.rs:173`): overflow
  becomes a plausible-looking value.
- `Weak::upgrade()` returning `None` on HLS fetch settle drops the committed
  segment size forever, silently (`variant.rs:88-107,170-202`).
- Lease/pin Drop paths log-and-continue on persistence failure, letting disk
  and memory state diverge (`kithara-assets/src/lease.rs`).
- Decoder panics are caught per-call and converted into recoverable
  `DecodeError`s (`source.rs` catch_unwind) — recovery hides real decoder bugs;
  catch at worker top level and fail the track instead.

## Theme 7 — Cross-crate seams

1. **kithara-play hard-links file/hls/abr/assets/net** and dispatches on
   `ResourceSrc` itself. Player logic can't be tested without the whole I/O
   stack. Wants a `StreamFactory`-style seam (or feature gates) with URL→source
   resolution at the facade.
2. **Command relay chain**: `WorkerCmd` (ffi) → `PlayerCmd` (processor) →
   `Cmd`/`Reply` (session) — three near-parallel enums; every new command
   touches all of them. Collapse to one command type with per-layer adapters
   only where representation genuinely differs.
3. **Config explosion**: `AudioConfig<T>` (14 pub fields, nested `T::Config`),
   `PlayerConfig` (12), `HlsConfig` (9+), `DownloaderConfig` — threaded through
   five layers, mixed builder styles, no composite validation (pool sizes,
   timeouts). A validation step (`build() -> Result<_, ConfigError>`) and
   pushing optional fields down to their consumers would shrink this.
4. **Cancel hierarchy**: marked owner sites exist, but `DownloaderConfig`'s
   builder default creates its own root token — combined with a caller-supplied
   token this yields two independent cancel trees. The lint covers token
   creation but not "two roots wired into one pipeline".
5. **Public API**: `kithara-play/src/lib.rs` re-exports `impls::*` (`Cmd`,
   `Reply`, `SessionDispatcher`, `SharedEq`); ffi/app couple to internals.
6. **kithara-events `downloader` feature depends on kithara-net types**
   (`RequestPriority` etc.) — events should stay domain-level.
7. **Dev-dep cycles**: audio/decode dev-depend on file/hls/storage; moves those
   integration tests to `tests/` and the graph untangles.
8. **`PcmSpec`/`PcmMeta`/`PcmChunk` live in kithara-decode** while the repo
   declares kithara-stream the home of shared media types; bridged by ad-hoc
   `From` impls. Decide the owner explicitly (likely fine in decode, but then
   document it as canonical there).

## Prioritized fix plan

Quick wins (small diffs, immediate bug-risk reduction):

1. Kill `PendingNext.activated` race — replace with an explicit pending-slot
   enum and one transition owner (play).
2. Make every silent drop observable: counters/`warn!` on `try_push().ok()`,
   `try_lock()` misses, `Weak::upgrade()` failures.
3. Replace sentinels (`u32::MAX`, `-1`, `unwrap_or(0)`) with `Option`/`Result`
   in hls layout + stream seek arithmetic.
4. Remove redundant runtime flags already proven by types
   (`DownloadClaim.settled`, `Audio.preloaded`).
5. Route rate/param updates to the audio thread via the existing atomics
   instead of `try_lock` on the resource.

Medium (per-crate refactors, the readability payoff):

6. Newtype pass for coordinate spaces (stream bytes, hls segment indices, seek
   epochs, frame counts) with conversions at named boundaries.
7. Split `pipeline/source.rs` and `variant.rs` along the concern lines above.
8. Single-snapshot ABR/reader boundary (`SegmentBoundary`), removing the dual
   atomics; same pattern for layout/segment-store commit.
9. Transactional asset deletion ordering (closes the known red test).
10. Make the gapless fallback chain explicit and observable.

Large (needs a plan doc per `.docs/plans/_template.md`):

11. Decouple kithara-play from concrete sources (`StreamFactory` seam) and
    shrink its public surface to traits.
12. Collapse the WorkerCmd→PlayerCmd→Cmd relay into one command vocabulary.
13. Config consolidation + validation phase.

Items 1–5 are independent and safe to land incrementally; 6–10 each fit a task
packet; 11–13 change public contracts and need plans.
