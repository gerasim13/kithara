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
| `kithara-play/src/impls/player.rs:49-53` | `PendingNext { src, activated: bool, index }` — a hand-rolled two-phase commit. **Verified: not a bug** — both paths (`commit_next` for cf>0, `finalize_handover_if_armed` for cf=0) emit `CurrentItemChanged` exactly once and the scheme is documented. Remains a readability cost: the two-phase protocol would be clearer as an explicit `enum PendingSlot { Armed, Activated }` with one transition owner. |
| `kithara-play/src/impls/player_track.rs:105-107` | `notified_track_requested` / `notified_prefetch_requested` bool pair. **Verified: no race** — `PlayerTrack` is `&mut self` on the audio thread only; these are once-latches with deliberate retry-on-full-ring semantics. Readability-only finding (enum would name the cycle states). |
| `kithara-audio/src/pipeline/source.rs` | **Partially retracted on verification**: the FSM is more structured than first reported (`CurrentFsm`, `ApplySeekState`, `WaitContext`; resume state is a struct, not a tuple; `pending_skip_amount` is documented Option-chaining). The remaining valid criticism is file size and concern-mixing (see Theme 4), not flag soup. |
| `kithara-audio/src/worker/decoder_node.rs:16-46` | `DecoderRuntime { eof_sent, preloaded, seek_epoch, chunks_sent }` reset via struct-update; adding a field silently breaks the reset. |
| `kithara-decode/src/gapless/trimmer/core.rs` | `GaplessMode` + `tail_buffer` + `tail_buffered_frames` + dual-purpose `trailing_frames` ("sometimes metadata, sometimes scan window"). |
| `kithara-hls/src/variant.rs:174,254-265` | `DownloadClaim.settled: bool` — **verified: standard disarm-the-guard pattern**, not redundancy: `Drop` still runs at the end of `into_loaded`, so without the flag the code would need `ManuallyDrop`. Acceptable as-is; low-priority. |
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

- **HLS cross-variant lookup** (`kithara-hls/src/coord.rs:293-311`):
  **verified: by-design, not a fallback bug** — after an ABR switch the shrunk
  historical variant legitimately owns byte ranges before the switch boundary
  (the virtual stream is a concatenation across variants over time). Retracted
  as a finding; at most deserves a clearer name/doc.
- **Position/duration fallback** in
  `player_processor.rs::update_position_duration`: **verified: documented and
  bounded** (cold start / all-fading cycles only, rationale in the doc
  comment). Acceptable per the repo's "justified fallback" rule; optional
  improvement is enforcing a single-leader invariant.
- **Gapless source chain** (metadata → codec priming → heuristic) is silent;
  which path won is invisible to callers and logs (`composed.rs`,
  `gapless/trimmer`). Surface the chosen mode in `DecoderTrackInfo` + trace.
  (Confirmed: `parse_itunsmpb` returns `None` on malformed input with no log.)
- **Decoder seek recovery** (`source.rs::recover_from_decoder_seek_error`):
  **verified: documented in the crate README** ("Seek error recovery") and
  split by error variant — a legitimate documented recovery path under the
  repo rules. The open design question (detect format change before seeking)
  stands, but this is not an undocumented fallback chain.
- **Availability disk-probe fallback** (`kithara-assets/src/unified.rs`,
  `cache.rs::open_resource` linear rescan) patches gaps the index should not
  have.

## Theme 6 — Silently swallowed errors and lost signals

- `try_push(...).ok()` on the notification ring — **verified with nuance**:
  the near-end triggers (`emit_handover_requested` / `emit_track_requested`)
  deliberately retry next cycle on a full ring (flag set only on success).
  But terminal events are lost permanently: `PlaybackStopped`
  (`player_track.rs:301-308,320-327`) is pushed with `.ok()` while the state
  is set to `Finished` regardless — a full ring drops the stop event with no
  log. Fix the terminal-event sites; the trigger sites are fine.
- `try_lock()` in the audio path (`player_processor.rs:271,409,745`,
  `player_track.rs:240,403,540,618`): contention silently drops commands such
  as rate updates. RT-safe, but failure must be observable (atomic param push
  or deferred retry, plus a counter).
- `unwrap_or(0)` / `unwrap_or(u64::MAX)` sentinels in arithmetic — verified:
  `stream.rs:564` clamps an overflowed seek to `u64::MAX` (reads then hit EOF
  instead of an error); `variant/layout.rs:134`'s `unwrap_or(0)` is actually
  unreachable dead defense (value already checked `>= 0`) — noise rather than
  data loss.
- `Weak::upgrade()` on HLS fetch settle (`variant.rs:188-205`) — **verified:
  harmless**: the upgrade only fails when the variant (and its layout/store)
  is already dead, so there is nothing left to update. At most add a trace.
- Lease/pin Drop paths log-and-continue on persistence failure, letting disk
  and memory state diverge (`kithara-assets/src/lease.rs`).
- Decoder panics are caught per-call and converted into recoverable
  `DecodeError`s (`source.rs:734,774` catch_unwind). Design judgment call: it
  keeps one corrupt file from killing the app, at the cost of masking decoder
  bugs. Worth a counter/event so repeated panic-recoveries are visible.

## Theme 7 — Cross-crate seams

1. **kithara-play hard-links file/hls/abr/assets/net** and dispatches on
   `ResourceSrc` itself. Player logic can't be tested without the whole I/O
   stack. Wants a `StreamFactory`-style seam (or feature gates) with URL→source
   resolution at the facade.
2. **Command relay chain**: `WorkerCmd` (ffi) → `PlayerCmd` (processor) →
   `Cmd`/`Reply` (session) — every new command touches all of them. Verified
   nuance: the three enums cross genuinely different transport boundaries
   (serializable web-worker protocol / RT audio thread / session loop), so
   full collapse is wrong; the realistic fix is shrinking the overlap and
   generating the FFI mirror, not unifying the types.
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

1. Fix the terminal-event loss: `PlaybackStopped` pushed with `.ok()` while
   state goes `Finished` regardless (`player_track.rs:301-308,320-327`) — at
   minimum count+log, ideally a dedicated slot for terminal events.
2. Make remaining silent drops observable: counters/`warn!` on `try_lock()`
   misses in command handlers; a counter on decoder panic-recoveries
   (`source.rs:734,774`).
3. Replace sentinels (`u32::MAX`, `-1`) with `Option`/`Result` in hls
   layout/segment-store, and reject (don't clamp) overflowed seeks at
   `stream.rs:564`.
4. Route rate/param updates to the audio thread via the existing atomics
   instead of `try_lock` on the resource.
5. Readability refactors (no bug, verified): `PendingNext` two-phase bool →
   explicit enum; notification once-latches → named cycle states.

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

## Verification pass (2026-06-11, same day)

Every load-bearing claim above was re-checked by direct code reading after the
initial agent-assisted survey. Outcome:

**Confirmed:** PlayerImpl field sprawl; try_lock command drops; terminal
`PlaybackStopped` loss on full ring; u32::MAX / -1 sentinels; seek-overflow
clamp; two independent ABR/reader atomics + manual `sync_abr_lock`; assets
availability TOCTOU (red test at `store.rs:815`); `WriterCleanup.disarm`
ceremony; DownloaderConfig builder-default root cancel token; public re-export
of session internals (`play/src/lib.rs:33-45`); gapless chain opacity; god-file
sizes; kithara-net not using tower; Headers-over-HashMap; manual Display/Error
boilerplate; `Arc<str>` serde workaround.

**Corrected / softened:** PendingNext "lost event race" (not a bug — both
paths emit exactly once); notification flag "race" (single-threaded);
`settled` flag (standard guard disarm, not redundancy); cross-variant lookup
(by-design ABR layering, not a fallback bug); position/duration fallback
(documented and bounded); decoder seek recovery (README-documented);
`Weak::upgrade` on settle (harmless — target already dead); seek-epoch
"silent loss" (standard last-writer-wins); source.rs "flag soup" (FSM is
structured; file size remains the issue); command-relay "collapse" (the three
enums cross real transport boundaries).

**Previously retracted:** downloader JoinSet replacement; `peer_done` command
loss (see the NIH supplement's correction note).
