# Readability Refactor Roadmap (2026-06-11)

Context document for a multi-session readability refactor. Designed to be
loaded as the sole extra context by a CLI session: pick ONE work package (WP),
re-verify its evidence lines (code may have drifted), execute, validate, tick
the checkbox, commit. Repo rules come from `AGENTS.md` (loaded automatically);
this doc does not restate them.

## Goal

Make the code understandable instead of documented. The codebase carries
~12.7k comment lines (14.5% of production source) and ~1.9k lines of crate
READMEs; a large share of that prose states invariants the type system could
enforce. Prose and code drift independently — that drift is where the
recurring bugs live.

## Governing principle

**A refactor is done when the explanatory comment it targets can be deleted.**
Every WP lists a "prose deletion target". If after the change the comment (or
README section, or lint marker) is still needed, the change has not actually
encoded the invariant — rework it. Do not add new explanatory comments to
compensate; do not add new docs. Net prose must go down.

## Verification status

All findings below were verified by direct code reading on 2026-06-11 at
commit `24011d3`. Do NOT re-litigate the following — they were investigated
and **retracted** (the code is correct / by design):

- `PendingNext.activated` "lost CurrentItemChanged race" — both cf>0 and cf=0
  paths emit exactly once (`player.rs::commit_next` / `finalize_handover_if_armed`).
- Notification-flag "race" in `player_track.rs` — single-threaded `&mut self`;
  triggers deliberately retry on full ring.
- HLS cross-variant lookup (`coord.rs::find_at_offset`) — by-design ABR
  layering: shrunk variants legitimately own pre-switch byte ranges.
- Seek-epoch "silent loss" (`player_processor.rs::apply_seek`) — standard
  last-writer-wins.
- `Weak::upgrade` skip in `variant.rs::into_loaded` — target already dead,
  harmless.
- Downloader registry/batch slot scheduler — justified domain logic (dynamic
  re-prioritization, pull-model peers, cancellation-safe registration polling).
  Do not replace with JoinSet/FuturesUnordered.
- `peer_done` "command loss" — `cmd_rx` is drained before the check.
- `parse_itunsmpb` hex parsing — `from_str_radix(_, 16)` is the correct tool.
- Decoder seek recovery (`recover_from_decoder_seek_error`) — README-documented
  error-variant split, legitimate.

## Do-not-touch list (justified custom code)

RT/wasm constraints make these custom implementations correct; ecosystem
"equivalents" would regress:

- `kithara-platform` rt_cancel (tokio_util's `is_cancelled()` takes a mutex —
  trips RealtimeSanitizer), `CancelGroup` semantics, time/thread wasm glue.
- `kithara-bufpool` (sharded RT pool, byte budgets).
- `kithara-events` EventBus hierarchy + DeferredBus.
- `kithara-stream/src/dl/wake.rs` (arm/flush deferred notify).
- `kithara-abr` EWMA estimator; `kithara-drm` ported cipher.
- fmp4 ESDS descriptor walk (`fmp4/parsing.rs::extract_aac_asc_raw`) — re_mp4
  discards AudioSpecificConfig bytes 3+ needed for HE-AAC.
- `#[kithara::probe]` / `#[kithara::hang_watchdog]` test macros.

## Work packages

Ordered by ascending risk. WP1–WP6 are independent small PRs. WP7+ are larger;
each still fits one focused session. WP15 needs its own plan first.

### [ ] WP1 — play: stop losing terminal events

- Evidence: `player_track.rs:301-308,320-327` — `PlaybackStopped` pushed via
  `try_push(..).ok()` while state becomes `Finished` regardless; full ring
  permanently drops the stop event. Contrast with `emit_handover_requested`
  (`:246-258`) which correctly retries by latching only on success.
- Approach: terminal events must be delivered: latch-until-pushed (same
  pattern as the triggers), or reserve dedicated capacity. Add a counter +
  `warn!` for any dropped notification class. Same treatment for the
  `try_lock()` command-drop sites (`player_processor.rs:271,409,745`) — apply
  params via the existing shared atomics where possible, otherwise count+log.
- Prose deletion target: none (pure bug class). Add a red test first (ring
  full at EOF → event still observed).
- Validation: `cargo test -p kithara-play`, `cargo xtask lint`.

### [ ] WP2 — play: encode the two-phase handover in types

- Evidence: `player.rs:49-53` (`PendingNext { activated: bool }`),
  `:320-352` (`commit_next`), `:520-543` (`finalize_handover_if_armed` with
  the "Two paths" doc comment). Verified correct but prose-dependent.
- Approach: `enum PendingSlot { Armed { index, src }, Activated { index, src } }`
  (or equivalent) with transitions as consuming methods; same for the
  notification once-latches in `player_track.rs:105-107` (named cycle states).
- Prose deletion target: the "Two paths" doc block on
  `finalize_handover_if_armed`; the field comments on `PendingNext`.
- Validation: `cargo test -p kithara-play`.

### [ ] WP3 — hls/stream: kill sentinels

- Evidence: `variant/segment_store.rs:126-137` (`on_evict` returns `-1` for
  init); `variant.rs:983,1254,1267` (`u32::try_from(..).unwrap_or(u32::MAX)`);
  `variant/layout.rs:128-136` (unreachable `unwrap_or(0)` dead defense);
  `stream.rs:564` (overflowed seek clamps to `u64::MAX` → EOF instead of
  error).
- Approach: `enum Evicted { Init, Segment(u32) }`; `Option<u32>` instead of
  MAX; `stream.rs:564` returns `ErrorKind::InvalidInput` on overflow; delete
  the dead defense in layout.
- Prose deletion target: the `-1 for init` doc comment on `on_evict`.
- Validation: `cargo test -p kithara-hls -p kithara-stream`.

### [ ] WP4 — net: replace the hand-rolled decorators with the ecosystem

- Evidence: `kithara-net/src/retry.rs` (479 lines, textbook exponential
  backoff), `timeout.rs` (94 lines restating the `Net` trait per method),
  `types.rs` (`Headers` over `HashMap<String,String>`, case-sensitive).
  `tower` is a workspace dep and is unused by kithara-net.
- Approach: retry via `backon` (add to `[workspace.dependencies]` with a
  one-line justification, per the workspace-first rule) or `tower::retry` —
  session's choice, justify in the PR. Keep the wasm conditional from
  timeout.rs but inline `tokio::time::timeout` at call sites. Headers →
  `reqwest::header::HeaderMap` (re-export; avoids a new dep) — preserves
  case-insensitivity.
- Prose deletion target: retry/timeout module docs; ~500 net LOC.
- Validation: `cargo test -p kithara-net` plus one downstream smoke
  (`cargo test -p kithara-stream dl`).

### [ ] WP5 — stream/dl: semaphore + stream combinators

- Evidence: `batch.rs:164-166` — `while inflight >= max { yield_now().await }`
  busy-spin; `AtomicUsize` + `AtomicWaker` completion plumbing across
  batch/downloader/registry. `response.rs` hand-rolls per-chunk
  timeout+cancel via `stream::unfold` + `select!`.
- Approach: `tokio::sync::Semaphore` (permit acquired before spawn, dropped at
  task end); rewrite body wrapping on `tokio_stream::StreamExt::timeout()` +
  `take_until(cancel.cancelled())`. Keep the slot scheduler and watchdog
  intact (see do-not-touch rationale). Watchdog progress classification may
  need a small rework to observe permits instead of the raw counter.
- Prose deletion target: the waker-coordination comments in
  `downloader.rs:62-67`.
- Validation: `cargo test -p kithara-stream`, hang-watchdog tests stay green.

### [ ] WP6 — workspace: boilerplate sweep

- Evidence: hand-written `Display`+`Error` impls (`kithara-events/src/abr.rs:62-74`,
  `audio.rs`, `kithara-bufpool/src/growth.rs`); `arc_str` serde module
  (`kithara-assets/src/key.rs:39-52`) working around the missing serde `rc`
  feature; trivial newtype Display impls where `derive_more` (already used in
  the workspace) applies.
- Approach: thiserror derives; enable serde `rc` in workspace Cargo.toml and
  delete the module; derive_more for newtype Display.
- Prose deletion target: none (pure LOC). ~60 LOC.
- Validation: `cargo clippy --workspace`, `cargo test` for touched crates.

### [ ] WP7 — platform/play: cancel ownership in types, delete the lint

- Evidence: the cancel-token contract is maintained in four places: README
  section ("Cancel Hierarchy", `kithara-play/README.md`), AGENTS.md
  non-negotiable bullet, `// kithara:cancel:owner|bridge` markers, and the
  `cancel_hierarchy` rule in `cargo xtask lint arch`. Plus the
  `DownloaderConfig` builder default creating a root token
  (`dl/config.rs:13-15`) — two independent cancel trees if the caller forgets
  to pass one.
- Approach: `CancelRoot` newtype constructible only at owner sites; children
  derive via `root.child()`; config fields take a child type, never a root.
  Remove the builder default on `DownloaderConfig.cancel` (require explicit
  wiring from the owner). Then delete the markers, the xtask rule, and the
  README section; update the AGENTS.md bullet in the same PR (rule-conflict
  procedure).
- Prose deletion target: README "Cancel Hierarchy" section, all
  `kithara:cancel:*` markers, the xtask `cancel_hierarchy` module, the
  AGENTS.md bullet (replaced by one line: "cancel ownership is type-enforced").
- Validation: workspace build + `cargo xtask lint` + play/stream tests.

### [ ] WP8 — play: PlayerImpl drop-order and state consolidation

- Evidence: `player.rs:125-160` — drop-order invariants held by field
  declaration order plus comments ("Engine drops last…", "Items drop before
  engine…"); 8 atomics + 5 mutexes with interdependent updates.
- Approach: explicit `shutdown()` sequencing (or a small owner struct whose
  Drop encodes the order structurally), then delete the ordering comments.
  Consolidate the status-adjacent fields (`status`, `current_slot`,
  `pending_next`) behind one lock where contention allows; leave the
  RT-shared atomics (`rate`, `volume`, `playback_rate_shared`) alone.
- Prose deletion target: both drop-order field comments; the
  `current_abr_handle` rationale comment (if the consolidation makes the
  lifetime obvious).
- Validation: `cargo test -p kithara-play -p kithara-queue`.

### [ ] WP9 — stream/hls: coordinate-space newtypes

- Evidence: virtual/committed/segment-relative byte offsets all bare `u64`
  (`stream.rs`, `timeline.rs` — note `timeline.rs:108 segment_position`, set
  only by the read path, flagged transitional in the README); HLS segment
  indices mix playlist-local and download-head spaces; seek epochs are bare
  `u64` compared ad hoc in audio/stream/play.
- Approach: minimal newtype layer (`VirtualByte`, `SegmentIdx`, `SeekEpoch`)
  with conversions only at named boundaries. Start with one space per PR;
  byte offsets first (highest bug surface). Resolve the `segment_position`
  transitional split as part of the byte-offset pass.
- Prose deletion target: the coordinate-space warning paragraph in
  `.docs/workflow/rust-ai.md` (cross-domain guardrails) once types enforce it;
  the transitional note in the stream README.
- Validation: `cargo test -p kithara-stream -p kithara-hls -p kithara-audio`.

### [ ] WP10 — abr/hls: single boundary snapshot

- Evidence: `AbrState.current_variant: Arc<AtomicUsize>`
  (`abr/state/core.rs:22`) and `HlsPeer.reader_segment: Arc<AtomicUsize>`
  (`hls/peer.rs:33,55`) are independent update streams read together by
  boundary-commit logic; `coord.rs:376-385 sync_abr_lock` manually mirrors
  timeline seek state into the ABR lock — forgetting a call site breaks the
  invariant silently.
- Approach: one snapshot type (`SegmentBoundary { variant, segment }`) behind
  a single lock or seqlock; replace `sync_abr_lock` with a `SeekGate` owning
  both the seek-epoch bump and the ABR lock as one operation.
- Prose deletion target: the hls README paragraph documenting the dual-atomic
  invariant.
- Validation: `cargo test -p kithara-abr -p kithara-hls`.

### [ ] WP11 — assets: transactional delete + consuming writer guard

- Evidence: red test `store.rs:815`
  (`red_test_delete_asset_strands_availability_index`); delete flow mutates
  in-memory availability, on-disk pins/lru, and the FS as three separate
  steps. `lease.rs:210-245` `WriterCleanup` with manual `disarm()`.
- Approach: fix the delete ordering so the red test goes green (index clear
  before FS op, or availability inside the atomic write boundary — decide at
  the code, the contract owner is kithara-assets). Replace
  `WriterCleanup.disarm()` with a consuming commit/fail API so the guard
  cannot be forgotten.
- Prose deletion target: the README caveat describing the stranding case
  (delete it together with the red_ prefix on the test).
- Validation: `cargo test -p kithara-assets -p kithara-storage`.

### [ ] WP12 — decode: delete duplicated parsing

- Evidence: `fmp4/parsing.rs::parse_flac_sample_entry` (~150 LOC) duplicates
  symphonia-format-isomp4's dfLa handling; `mp4/scan.rs` box navigation
  (`next_box`, header parsing) re-implements `re_mp4::BoxHeader::read` (~200
  LOC); `gapless/mp4.rs` repeats box plumbing (~100 LOC). Keep the ESDS walk
  (see do-not-touch) and the gapless frame-scaling math.
- Approach: route FLAC through the symphonia path; rebase scan/gapless box
  navigation on re_mp4 types, keeping only domain logic. `parse_itunsmpb`:
  add a `warn!` on malformed input (currently silent `None`). Surface the
  chosen gapless source (metadata/priming/heuristic) in `DecoderTrackInfo` +
  a trace line.
- Prose deletion target: the box-walking helper comments in scan.rs.
- Validation: `cargo test -p kithara-decode` (gapless fixture tests must stay
  bit-exact).

### [ ] WP13 — audio: split `pipeline/source.rs` (2620 lines)

- Evidence: one file mixes shared-stream wrapper, format-change handling,
  seek recovery, gapless drain, effects, decoder recreation. Verified note:
  the FSM itself is structured (`CurrentFsm`, `WaitContext`) — the problem is
  colocation, not flag soup.
- Approach: mechanical module split along existing seams (`format_change`,
  `seek_recovery`, `gapless`, orchestrator). No behavior change; moves only.
  Unify the EOF effect-drain path with the normal chain if it falls out
  naturally; otherwise leave for a follow-up.
- Prose deletion target: section-banner-style orientation comments that exist
  only because the file is huge.
- Validation: `cargo test -p kithara-audio`, `cargo xtask lint` (file-size /
  style rules).

### [ ] WP14 — hls: split `variant.rs` (1504 lines)

- Evidence: `HlsVariant` owns fetch scheduling (`dispatch`), segment metadata,
  and playback read state (`read_at` with twin init/media cursor loops);
  `Layout`/`SegmentStore` commit ordering is documented-not-enforced
  (`layout.rs` comment near `apply_commit`).
- Approach: extract `SegmentQueue` (dispatch), metadata view, and a read
  cursor; provide one `apply_commit_atomic` entry point so the ordering
  invariant is structural.
- Prose deletion target: the commit-ordering comment in layout.rs.
- Validation: `cargo test -p kithara-hls`.

### [ ] WP15 — play: decouple from concrete sources (needs its own plan)

- Evidence: kithara-play hard-links file/hls/abr/assets/net and dispatches on
  `ResourceSrc` itself; `lib.rs:33-45` re-exports session internals (`Cmd`,
  `CmdMsg`, `Reply`, `run_cmd`, `SharedEq`) that ffi/app couple to.
- This changes public contracts: write a dedicated plan via
  `.docs/plans/_template.md` before starting. Note for that plan: the three
  command enums (WorkerCmd/PlayerCmd/Cmd) cross genuinely different transport
  boundaries — shrink overlap or generate the FFI mirror; do NOT unify them.

## Session protocol

1. Re-verify the WP's evidence lines before editing (this doc ages too).
2. TDD where behavior changes (red test first — see WP1, WP11).
3. Respect the zero-suppress lint policy; if a lint fires, fix the code.
4. Acceptance = tests green + listed prose actually deleted + net comment
   count in touched files not increased.
5. Tick the WP checkbox in this doc in the same commit.
6. Validation baseline for every WP: `cargo fmt --check`,
   `cargo clippy -p <touched>`, `cargo test -p <touched>`, `cargo xtask lint`.

## Sequencing dependencies

- WP1–WP6: any order, independent.
- WP7 before WP8 (PlayerImpl holds the master cancel; do ownership first).
- WP9 before WP10 (boundary snapshot wants typed indices).
- WP13/WP14 after the small WPs in their crates to avoid churn conflicts.
- WP15 last, separate plan.
