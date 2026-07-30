#![forbid(unsafe_code)]

use std::{
    ops::Range,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use delegate::delegate;
use kithara_abr::{AbrHandle, AbrPublisher};
use kithara_assets::{AssetScope, ResourceKey};
use kithara_events::{AbrReason, DeferredBus, HlsEvent};
use kithara_platform::{
    CancelToken,
    sync::{Arc, ThreadGate, WaitGate},
    time::{Duration, Instant},
};
use kithara_storage::WaitOutcome;
use kithara_stream::{
    Activity, ByteMap, ContainerFormat, DeferredWake, MediaInfo, PendingReason, PlayheadRead,
    PlayheadState, PlayheadWrite, ReadOutcome, ReaderProfile, SeekControl, SeekObserve, SeekState,
    SegmentDescriptor, SourceError, SourcePhase, SourceSeekAnchor, StreamError, StreamResult,
    VariantControl, VariantReaderTake, VariantTransition, dl::FetchCmd,
};
use kithara_test_utils::kithara;

use super::{session::HlsSession, transition::SessionSlots};
use crate::{
    playlist::{PlaylistAccess, PlaylistState},
    signal::SizeSignal,
    variant::{HlsVariant, PlanCtx, SegmentActivateParams},
};

/// Watchdog timeout for the off-RT blocking `wait_range(_, None)`: must exceed
/// the `kithara-net` per-fetch total timeout so a stalled upstream is failed by
/// the network layer (the wait then returns a terminal `Err`) before this
/// deadlock watchdog fires. Mirrors `kithara-storage` `WAIT_HANG_TIMEOUT`. Only
/// a wait that never wakes after every signal site fired is a real deadlock.
const WAIT_HANG_TIMEOUT: Duration = Duration::from_secs(180);

/// Infrastructure handles shared with every [`HlsCoord`]:
/// the parent cancel token (cancel hierarchy owner of `HlsCoord.cancel`)
/// and the per-track [`AssetStore`] used by reader paths and by every
/// variant's `dispatch` closures.
pub(crate) struct HlsCoordEnv {
    pub(crate) scope: AssetScope,
    pub(crate) cancel: CancelToken,
    pub(crate) headers: Option<kithara_net::Headers>,
    pub(crate) emit: Arc<DeferredBus<HlsEvent>>,
    /// Unified reader-wake handle: the shared readiness gate paired with the
    /// late-bound audio-worker wake. Every transition that can flip a blocked
    /// reader's `wait_range` predicate (segment write/commit/fail, fence
    /// raise/clear, seek reset, cancel) signals the gate; the off-RT
    /// `wait_range(_, None)` parks on it instead of polling a wall-clock timer.
    /// The two downloader write/settle sites additionally re-tick the audio
    /// worker via [`SizeSignal::fire`]; the coord's RT-reachable fence/seek
    /// signals use [`SizeSignal::fire_ready_only`]. See `CONTEXT.md`
    /// "Event-driven read wait".
    pub(crate) signal: SizeSignal,
}

/// Coordinator over fixed variants and one authoritative active reader session.
/// The optional incoming session stays private until exact audio promotion.
pub(crate) struct HlsCoord {
    pub(crate) abr: AbrHandle,
    pub(super) abr_publisher: AbrPublisher,
    pub(crate) variants: Arc<[Arc<HlsVariant>]>,
    pub(crate) scope: AssetScope,
    pub(crate) cancel: CancelToken,
    pub(crate) headers: Option<kithara_net::Headers>,
    /// Backing playhead state — the coord owns the `Arc` directly and
    /// vends narrow trait-object handles from it.
    playhead: Arc<PlayheadState>,
    /// Narrow read-only playhead handle — derived from `playhead` at construction.
    /// Used by internal methods that only need committed position reads.
    playhead_read: Arc<dyn PlayheadRead>,
    playlist_state: Arc<PlaylistState>,
    /// Backing seek/activity state — the coord owns the `Arc` directly and
    /// vends narrow trait-object handles from it.
    seek: Arc<SeekState>,
    /// Narrow seek-observe handle — derived from `seek` at construction.
    /// Used by internal methods that only need epoch/target/pending reads.
    seek_obs: Arc<dyn SeekObserve>,
    /// One authoritative active session and at most one exact incoming session.
    pub(super) sessions: SessionSlots,
    /// Last generation acknowledged by the reader. When `<
    /// variant_generation` the read gate is closed; when equal the gate
    /// is open. [`Self::clear_variant_fence`] copies the current
    /// generation here.
    fence_at: AtomicU64,
    /// Monotonic counter bumped by [`Self::commit_variant_switch`] on
    /// every decoder-recreate switch. Same-codec switches use byte
    /// continuity and do not raise this fence. `read_at` / `wait_range`
    /// compare against [`Self::fence_at`] and short-circuit with
    /// `Pending(VariantChange)` / `Interrupted` until the audio FSM acks via
    /// [`Self::clear_variant_fence`]. Byte-continuity switches do not raise
    /// a fence.
    variant_generation: AtomicU64,
    /// Target variant index of the in-flight fence. Stored (`Release`)
    /// BEFORE [`Self::variant_generation`] is bumped, so an observer of
    /// a pending fence always sees the variant that fence demands
    /// ([`Self::variant_change_target`]). The decoder needs it to ack a
    /// fence whose target it is already aligned with (a seek recreate
    /// landed on the switch target before the commit raised the fence):
    /// no format diff is observable there, so without the target the
    /// fence would never clear.
    fence_target: AtomicUsize,
    /// Unified reader-wake handle for the off-RT blocking `wait_range(_, None)`.
    /// Shared with every variant's fetch closures (write/commit/fail
    /// [`SizeSignal::fire`] it) and signalled by the coord on fence/seek
    /// transitions ([`SizeSignal::fire_ready_only`]). See [`HlsCoordEnv::signal`].
    signal: SizeSignal,
    pub(crate) emit: Arc<DeferredBus<HlsEvent>>,
}

impl HlsCoord {
    /// Re-aim heartbeat for the off-RT blocking wait. The wait wakes immediately
    /// on any readiness signal (the fact of a write/commit/fence/seek) —
    /// event-driven. This interval bounds only the *quiet* case: if no signal
    /// arrives within it, the peer may be mis-aimed after a seek (it fetched,
    /// went idle, and the range the reader now wants is outside its prefetch
    /// window), so the wait yields `WaitBudgetExceeded` to let the off-RT reader
    /// re-assert the peer's aim (`notify_peer_wake`) and re-enter. It never polls
    /// for data — readiness is always learned from a signal, never from a timer.
    const READER_REAIM_INTERVAL: Duration = Duration::from_millis(25);

    pub(crate) fn new(
        env: HlsCoordEnv,
        playhead: Arc<PlayheadState>,
        seek: Arc<SeekState>,
        abr: AbrHandle,
        abr_publisher: AbrPublisher,
        variants: Arc<[Arc<HlsVariant>]>,
        playlist_state: Arc<PlaylistState>,
    ) -> Self {
        assert!(
            !variants.is_empty(),
            "HlsCoord constructed without variants — caller must supply at least one"
        );
        assert!(
            abr.current_variant_index().is_some(),
            "HlsCoord requires an AbrHandle with state — HlsPeer must construct AbrState"
        );
        let seek_obs = Arc::clone(&seek) as Arc<dyn SeekObserve>;
        let playhead_read = Arc::clone(&playhead) as Arc<dyn PlayheadRead>;
        let active_index = abr
            .current_variant_index()
            .expect("stateful ABR handle checked above");
        let active_variant = Arc::clone(
            variants
                .get(active_index)
                .expect("ABR current variant must exist in HLS variants"),
        );
        let active_session = Arc::new(HlsSession::active(
            env.cancel.child(),
            Arc::clone(&seek_obs),
            env.signal.clone(),
            active_index,
            Arc::clone(&active_variant),
            active_variant.get_position(),
        ));
        Self {
            playhead,
            seek,
            seek_obs,
            playhead_read,
            abr,
            abr_publisher,
            variants,
            playlist_state,
            cancel: env.cancel,
            scope: env.scope,
            headers: env.headers,
            emit: env.emit,
            variant_generation: AtomicU64::new(0),
            fence_at: AtomicU64::new(0),
            fence_target: AtomicUsize::new(0),
            sessions: SessionSlots::new(active_session),
            signal: env.signal,
        }
    }

    pub(crate) fn active(&self) -> Arc<HlsVariant> {
        self.active_session().variant()
    }

    pub(crate) fn activity(&self) -> Arc<dyn Activity> {
        Arc::clone(&self.seek) as Arc<dyn Activity>
    }

    /// Process one evicted resource key. Marks the lost segment
    /// `Missing` on every variant that owned it. When the active
    /// variant is among them, fires a full `rebuild` from the reader's
    /// current segment so the queue is refilled with the now-Missing
    /// slot reincluded. Non-active variants stay relaxed — their next
    /// activation (ABR flip) calls `rebuild` and picks up the Missing
    /// entries then.
    pub(crate) fn broadcast_eviction(&self, ctx: &PlanCtx, key: &ResourceKey, seg_at_reader: u32) {
        let active_idx = self.variant_index();
        let active_lost = self
            .variants
            .iter()
            .enumerate()
            .fold(false, |acc, (v_idx, v)| {
                let hit = v.on_evict(key).is_some() && v_idx == active_idx;
                acc || hit
            });
        if active_lost {
            self.active().rebuild(ctx, seg_at_reader);
        }
    }

    /// Notify the audio FSM that the cross-codec switch is acknowledged
    /// — opens the read gate by aligning `fence_at` to the current
    /// generation. Called from `HlsSource::clear_variant_fence` after
    /// the decoder has been recreated against the new variant.
    pub(crate) fn clear_variant_fence(&self) {
        let current_gen = self.variant_generation.load(Ordering::Acquire);
        self.fence_at.swap(current_gen, Ordering::AcqRel);
        self.emit.enqueue(HlsEvent::VariantSwitchAcked {
            variant: self.variant_index(),
            generation: current_gen,
        });
        // The fence gate opened: wake a reader parked in `wait_range(_, None)`
        // (it short-circuited on `variant_change_pending`) so it re-probes.
        self.signal.fire_ready_only();
    }

    /// Commit any ABR pending decision at the reader's segment boundary.
    /// Returns `true` when a switch landed.
    ///
    /// Two branches, selected by codec continuity:
    ///
    /// - **Byte-continuity switches** (same codec, plus raw PCM/WAV):
    ///   activate `v_new` at the boundary segment with `byte_shift` so the
    ///   existing decoder keeps reading aligned bytes from the new variant.
    ///   No fence, no recreate.
    /// - **Decoder-recreate switches** (known cross-codec, or codec
    ///   unknown): hard reset on `v_new` via [`HlsVariant::reset_to_full_range`],
    ///   reader position seeded to the segment covering the current timeline
    ///   position, and `variant_generation` bumped — the next [`Self::read_at`]
    ///   / [`Self::wait_range`] short-circuits with `Pending(VariantChange)` /
    ///   `Interrupted` until the audio FSM recreates the decoder and acks via
    ///   [`Self::clear_variant_fence`].
    pub(crate) fn commit_variant_switch(&self, ctx: &PlanCtx, from_seg: u32) -> bool {
        self.commit_variant_switch_starting_at(ctx, from_seg.saturating_add(1))
    }

    /// Commit a pending ABR switch whose first target-variant segment is
    /// `switch_at`. The seek-settle floor already names the target segment;
    /// unlike steady boundary crossing it is not the old segment before the
    /// boundary.
    pub(crate) fn commit_variant_switch_at_segment(&self, ctx: &PlanCtx, switch_at: u32) -> bool {
        self.commit_variant_switch_starting_at(ctx, switch_at)
    }

    fn commit_variant_switch_starting_at(&self, ctx: &PlanCtx, switch_at: u32) -> bool {
        self.sessions
            .commit_legacy(|| self.commit_legacy_variant_switch(ctx, switch_at))
    }

    fn commit_legacy_variant_switch(&self, ctx: &PlanCtx, switch_at: u32) -> bool {
        let current_before = self.variant_index();
        let Some(decision) = self.abr.peek_pending_decision() else {
            return false;
        };
        let new_v = decision.target().get();
        let Some(v_new) = self.variants.get(new_v) else {
            return false;
        };
        let v_old = self.variants.get(current_before);
        let old_codec = v_old.and_then(|_| self.playlist_state.variant_codec(current_before));
        let new_codec = self.playlist_state.variant_codec(new_v);
        let same_codec = matches!((old_codec, new_codec), (Some(a), Some(b)) if a == b);
        let is_cross_codec = matches!((old_codec, new_codec), (Some(a), Some(b)) if a != b);
        let needs_byte_continuity = variant_switch_uses_byte_continuity(
            same_codec,
            self.playlist_state.variant_container(new_v),
        );
        let active_position = if needs_byte_continuity {
            let switch_at = switch_at.min(v_new.num_segments());
            if !v_new.prepare_exact_prefix_for_boundary(switch_at) {
                return false;
            }
            let reader_pos = self.position();
            let seg_boundary = v_old
                .and_then(|v| v.segment_byte_offset(switch_at))
                .unwrap_or(reader_pos);
            if let Some(v_old) = v_old {
                v_old.set_served_until(switch_at);
            }
            v_new.activate_at_segment_with_shift(
                ctx,
                SegmentActivateParams {
                    seg_boundary,
                    reader_pos,
                    from_seg: switch_at,
                },
            );
            self.abr_publisher
                .apply_legacy_decision(&decision, Instant::now());
            reader_pos
        } else {
            v_new.reset_to_full_range();
            if is_cross_codec {
                v_new.invalidate_init();
            }
            let target_time =
                variant_switch_target_time(self.seek_obs.as_ref(), self.playhead_read.as_ref());
            let target_seg: u32 = self
                .playlist_state
                .find_seek_point_for_time(new_v, target_time)
                .and_then(|(seg, _, _)| u32::try_from(seg).ok())
                .unwrap_or(0);
            let target_byte = v_new.segment_byte_offset_natural(target_seg).unwrap_or(0);
            v_new.set_position(target_byte);
            self.fence_target.store(new_v, Ordering::Release);
            self.variant_generation.fetch_add(1, Ordering::Release);
            self.emit.enqueue(HlsEvent::VariantSwitchFenced {
                from_variant: current_before,
                to_variant: new_v,
                cross_codec: is_cross_codec,
            });
            self.abr_publisher
                .apply_legacy_decision(&decision, Instant::now());
            v_new.rebuild_with_decoder_probe(ctx, target_seg);
            target_byte
        };
        let active_session = Arc::new(HlsSession::active(
            self.cancel.child(),
            Arc::clone(&self.seek_obs),
            self.signal.clone(),
            new_v,
            Arc::clone(v_new),
            active_position,
        ));
        self.sessions.replace_legacy(active_session);
        let reader_pt = self.playhead_read.position();
        self.abr
            .notify_commit(decision, current_before, reader_pt, Instant::now());
        // Variant switched (fence raised on the structured-container branch, or
        // a byte-continuity reactivation): wake a parked reader to re-probe /
        // observe the new `Interrupted`(VariantChange) gate, and re-tick the RT
        // decoder's audio worker so it observes the new gate off its scheduler poll.
        self.signal.fire();
        true
    }

    /// Resolve the segment whose fetch queue owns the reader cursor.
    ///
    /// This is deliberately wider than [`Self::find_at_offset`]: a cursor in
    /// the active variant's init prefix is not inside any media segment, but it
    /// still demands the segment-0 decoder probe plan. Cross-variant media
    /// lookups keep the existing `find_at_offset` routing for shrunk outgoing
    /// variants.
    pub(crate) fn demand_segment_at_offset(&self, byte_offset: u64) -> Option<u32> {
        let active = self.active();
        active
            .demand_segment_at_offset(byte_offset)
            .or_else(|| self.find_at_offset(byte_offset).map(|(idx, _, _)| idx))
    }

    pub(crate) fn dispatch_pending_size_demands(
        &self,
        ctx: &PlanCtx,
        budget: usize,
    ) -> Vec<FetchCmd> {
        if self.exact_sessions_enabled() {
            return Vec::new();
        }
        let Some(decision) = self.abr.peek_pending_decision() else {
            return Vec::new();
        };
        let target = decision.target().get();
        if target == self.variant_index() {
            return Vec::new();
        }
        self.variants
            .get(target)
            .map_or_else(Vec::new, |variant| variant.dispatch_size_only(ctx, budget))
    }

    /// Cross-variant segment lookup. Mirrors [`Self::variant_serving`]'s
    /// priority: active first, then shrunk `v_old`s. Returns `None` if no
    /// engaged variant claims the offset.
    pub(crate) fn find_at_offset(&self, byte_offset: u64) -> Option<(u32, u64, u64)> {
        let active = self.active();
        if let Some(found) = active.find_at_offset(byte_offset) {
            return Some(found);
        }
        for v in self.variants.iter() {
            if Arc::ptr_eq(v, &active) {
                continue;
            }
            let shrunk = v.is_shrunk();
            if !shrunk {
                continue;
            }
            if let Some(found) = v.find_at_offset(byte_offset) {
                return Some(found);
            }
        }
        None
    }

    /// Public-API mirror of [`Self::variant_change_pending`] used by the
    /// audio decode loop to bail out of an `Ok(Pending(_))` spin when
    /// the underlying `VariantChangeError` was absorbed by the demuxer.
    pub(crate) fn has_variant_change_pending(&self) -> bool {
        self.variant_change_pending()
    }

    /// Total bytes are >0 — the value used by `Source::len` accessor.
    pub(crate) fn len(&self) -> Option<u64> {
        self.active().stream_len()
    }

    /// Active variant's media info. `HlsCoord` is constructed
    /// non-empty (asserted in [`Self::new`]) so this always succeeds —
    /// the `Source` trait's `Option<MediaInfo>` shape is restored at
    /// the [`HlsSource`](crate::stream::HlsSource) façade.
    pub(crate) fn media_info(&self) -> MediaInfo {
        self.active().media_info()
    }

    /// Track-level phase. Master-cancel takes precedence (terminal
    /// `Cancelled`); otherwise the variant that currently serves
    /// `range.start` decides — mid-buffer boundary cross resolves to
    /// the right `range_ready` / `is_flushing` / `total_bytes` view.
    pub(crate) fn phase_at(&self, range: Range<u64>) -> SourcePhase {
        if self.cancel.is_cancelled() {
            return SourcePhase::Cancelled;
        }
        self.variant_serving(range.start).phase_at(range)
    }

    pub(crate) fn playhead_read(&self) -> Arc<dyn PlayheadRead> {
        Arc::clone(&self.playhead) as Arc<dyn PlayheadRead>
    }

    pub(crate) fn playhead_write(&self) -> Arc<dyn PlayheadWrite> {
        Arc::clone(&self.playhead) as Arc<dyn PlayheadWrite>
    }

    /// Seek entry point. Collapses cross-variant byte-continuity layering,
    /// cancels the incoming exact session, and wakes a parked reader.
    ///
    /// The expensive layout collapse ([`Self::reset_for_seek`]) runs only
    /// when the active variant's offset table is not already the canonical
    /// full-range geometry with every served size exact — a fully-resolved
    /// single-variant track repeats the identical table, so the O(N) rebuild
    /// is skipped. The ABR invalidation and the reader wake stay
    /// unconditional, so the seek's cancel/wake semantics are unchanged for
    /// every track (cross-variant, partial-download, or fully cached).
    ///
    /// Legacy boundary switching drops a throughput-driven decision because it
    /// could commit immediately after the seek against a cold target. Exact
    /// sessions preserve the ticket and rebuild its incoming session in the new
    /// seek epoch; publication remains impossible until that replacement is
    /// ready.
    pub(crate) fn prepare_for_seek(&self) {
        self.cancel_incoming_for_seek();
        if !self.active().layout_seek_invariant() {
            self.reset_for_seek();
        }
        if !self.exact_sessions_enabled() {
            self.abr.invalidate_pending();
        }
        // A seek repositioned the active variant: wake a reader parked on the
        // pre-seek range so it re-probes against the new position / flush gate.
        self.signal.fire_ready_only();
    }

    /// Single wake-free readiness probe (the wake-free `HlsVariant::wait_range`
    /// behind the coord's fence/cancel short-circuits). Shared by the RT probe
    /// path and the off-RT blocking loop's per-iteration check.
    fn probe_range(
        &self,
        range: Range<u64>,
        timeout: Option<Duration>,
    ) -> StreamResult<WaitOutcome> {
        if self.cancel.is_cancelled() {
            return Err(StreamError::Source(crate::HlsError::Cancelled.into()));
        }
        if self.variant_change_pending() {
            return Ok(WaitOutcome::Interrupted);
        }
        self.variant_serving(range.start).wait_range(range, timeout)
    }

    pub(crate) fn read_at(&self, offset: u64, buf: &mut [u8]) -> StreamResult<ReadOutcome> {
        if self.cancel.is_cancelled() {
            return Err(StreamError::Source(crate::HlsError::Cancelled.into()));
        }
        if self.variant_change_pending() {
            return Ok(ReadOutcome::Pending(PendingReason::VariantChange));
        }
        self.variant_serving(offset).read_at(offset, buf)
    }

    /// The bare readiness gate, captured by the cancel waker's `on_cancel`
    /// closure (which needs a hard-`Send + Sync` handle). Re-vended from
    /// [`Self::signal`].
    pub(crate) fn ready_gate(&self) -> Arc<ThreadGate> {
        self.signal.ready_gate()
    }

    /// Reconcile the ABR escape flag against the live stall geometry. Edge-
    /// detected against [`AbrHandle::is_escaping`]: a rising edge — the reader
    /// is newly parked at a clean boundary on a non-delivering segment — marks
    /// escape and returns `true`, signalling the caller to trigger a controller
    /// re-tick OUTSIDE the HLS state lock (the tick reads `peer.progress()`,
    /// which re-locks it, so a synchronous tick here would deadlock). A falling
    /// edge — the segment now delivers, or a switch was published — clears it.
    /// Idempotent in the steady state.
    pub(crate) fn reconcile_escape(&self, reader_seg: u32) -> bool {
        let stalled = self
            .active()
            .segment_stalled_at_boundary(reader_seg, self.position());
        let was = self.abr.is_escaping();
        if stalled && !was {
            self.abr.mark_escape();
            true
        } else {
            if !stalled && was {
                self.abr.clear_escape();
            }
            false
        }
    }

    /// Collapse the active variant to a "fresh" single-variant layout on
    /// seek. Random seek may land far from the post-ABR-commit window, so
    /// reset `byte_shift` / `served_from` / `served_until` back to the
    /// natural range: subsequent ABR commits at boundary will re-build the
    /// layering as usual. Gated by [`Self::prepare_for_seek`] so it is not
    /// entered when the layout is already canonical.
    #[kithara::probe]
    pub(crate) fn reset_for_seek(&self) {
        self.active().reset_for_seek();
    }

    pub(crate) fn seek_control(&self) -> Arc<dyn SeekControl> {
        Arc::clone(&self.seek) as Arc<dyn SeekControl>
    }

    pub(super) fn commit_if_seek_epoch<T, F>(&self, epoch: u64, commit: F) -> Option<T>
    where
        F: FnOnce() -> T,
    {
        self.seek.commit_if_epoch(epoch, commit)
    }

    pub(crate) fn seek_epoch_handle(&self) -> Arc<AtomicU64> {
        self.seek.seek_epoch_arc()
    }

    pub(crate) fn seek_observe(&self) -> Arc<dyn SeekObserve> {
        Arc::clone(&self.seek) as Arc<dyn SeekObserve>
    }

    /// Install the peer's `reader_advanced` wake so the `on_slow` hook can
    /// re-poll the peer when an in-flight fetch stalls past `soft_timeout`.
    /// Called once by `HlsPeer::activate`.
    pub(crate) fn set_peer_wake(&self, wake: Arc<DeferredWake>) {
        self.signal.set_peer_wake(wake);
    }

    /// Install the audio worker's data-arrival wake (idempotent — only the
    /// first set sticks). Called by `HlsSource::set_worker_wake` once the
    /// worker exists; downloader fetch closures read it lock-free thereafter.
    pub(crate) fn set_worker_wake(&self, wake: Arc<dyn kithara_stream::WorkerWake>) {
        self.signal.set_worker_wake(wake);
    }

    /// The unified reader-wake handle, handed to [`PlanCtx`] so variant fetch
    /// closures can [`SizeSignal::fire`] on segment write/commit/fail.
    pub(crate) fn signal(&self) -> SizeSignal {
        self.signal.clone()
    }

    /// Generalised boundary-free escape for the startup/stall livelock: the
    /// reader is parked at a clean segment boundary on the active variant, that
    /// segment's in-flight fetch has crossed `soft_timeout` without settling
    /// (the variant cannot deliver the byte range the reader needs), and a
    /// switch is pending. Unlike [`Self::urgent_rescue_boundary`] this is
    /// direction- and container-agnostic: the commit reseeds the target at the
    /// stalled segment (recreate path reseeds by playhead time, byte-continuity
    /// at the boundary — both continuity-safe with nothing read past it).
    /// Distinct from a normal startup, where the needed segment settles before
    /// `soft_timeout` and the slow flag never sets.
    pub(crate) fn stalled_escape(&self, reader_seg: u32) -> bool {
        self.abr.peek_pending_decision().is_some()
            && self
                .active()
                .segment_stalled_at_boundary(reader_seg, self.position())
    }

    /// Mirror `abr.lock()` state to `seek_obs.is_pending()`.
    pub(crate) fn sync_abr_lock(&self) {
        let pending = self.seek_obs.is_pending();
        let locked = self.abr.is_locked();
        if pending && !locked {
            self.abr.lock();
        } else if !pending && locked {
            self.abr.unlock();
        }
    }

    /// Break the urgent-down-switch / blocked-reader deadlock: when the
    /// active (slow) variant cannot deliver the next segment the reader
    /// needs, an Auto-mode commit would otherwise wait for a boundary
    /// cross that the undelivered segment prevents. Return the segment to
    /// commit at (`download_head - 1`, so `commit_variant_switch`'s
    /// `from_seg + 1` lands `switch_at = download_head`) when a proactive
    /// rescue is both warranted and continuity-safe; otherwise `None`.
    ///
    /// Guards (all required):
    /// - a pending decision exists and its reason is
    ///   [`AbrReason::UrgentDownSwitch`] — only the rescue path commits
    ///   early; opportunistic up/down-switches keep boundary-cross
    ///   gating so `v_new` is not pinned prematurely;
    /// - the target is a WAV byte-continuity variant — the structured
    ///   recreate path reseeds by time and is not subject to this
    ///   circular dependency;
    /// - `download_head` is ahead of the reader's current segment, or
    ///   exactly equals it while the byte cursor is still pinned to that
    ///   segment boundary. This keeps the switch on a clean boundary:
    ///   the reader finishes `v_old`'s loaded prefix and `v_new` takes
    ///   over at `download_head`. If the cursor has advanced inside
    ///   `download_head`, rescue is unsafe because it would become a
    ///   mid-segment cross-bitrate switch;
    /// - `download_head < num_segments`, i.e. `v_old` genuinely has
    ///   un-downloaded tail (otherwise there is nothing to rescue from).
    pub(crate) fn urgent_rescue_boundary(&self, reader_seg: u32) -> Option<u32> {
        let decision = self.abr.peek_pending_decision()?;
        if decision.reason() != AbrReason::UrgentDownSwitch {
            return None;
        }
        if !matches!(
            self.playlist_state
                .variant_container(decision.target().get()),
            Some(ContainerFormat::Wav)
        ) {
            return None;
        }
        let head = self.download_head();
        let active = self.active();
        if head >= active.num_segments() {
            return None;
        }
        let at_head_boundary =
            head == reader_seg && active.segment_byte_offset(head) == Some(self.position());
        if head < reader_seg || (head == reader_seg && !at_head_boundary) {
            return None;
        }
        Some(head.saturating_sub(1))
    }

    fn variant_change_pending(&self) -> bool {
        self.variant_generation.load(Ordering::Acquire) > self.fence_at.load(Ordering::Acquire)
    }

    /// Target variant of the pending fence; `None` when no fence is up.
    /// The target store happens-before the generation bump, so a caller
    /// that observed the fence reads the variant that fence (or a newer
    /// one — latest wins, matching `clear_variant_fence` absorbing all
    /// outstanding generations) demands.
    pub(crate) fn variant_change_target(&self) -> Option<usize> {
        self.variant_change_pending()
            .then(|| self.fence_target.load(Ordering::Acquire))
    }

    /// Variant index of the authoritative active source session.
    pub(crate) fn variant_index(&self) -> usize {
        self.active_session().variant_index()
    }

    /// Find the variant whose served range covers `offset`. Priority:
    ///
    /// 1. The active session's variant — the normal steady-state hit.
    /// 2. Any non-active variant whose served range has been *shrunk*
    ///    from its default span by a prior ABR commit (i.e.
    ///    `served_from > 0` or `served_until < num_segments`). These
    ///    are `v_old`s that still serve their pre-switch byte range so
    ///    a reader crossing the boundary mid-buffer hits the right
    ///    payload.
    ///
    /// Idle variants with default served bounds are deliberately
    /// excluded: their layout overlaps the active range but their
    /// resources were never fetched, so routing to them would return
    /// `NotFound` / `Pending(Retry)`.
    pub(crate) fn variant_serving(&self, offset: u64) -> Arc<HlsVariant> {
        let active = self.active();
        if active.init_descriptor_at(offset).is_some() || active.find_at_offset(offset).is_some() {
            return active;
        }
        for v in self.variants.iter() {
            if Arc::ptr_eq(v, &active) {
                continue;
            }
            let shrunk = v.is_shrunk();
            if !shrunk {
                continue;
            }
            if v.init_descriptor_at(offset).is_some() || v.find_at_offset(offset).is_some() {
                return Arc::clone(v);
            }
        }
        active
    }

    pub(crate) fn wait_range(
        &self,
        range: Range<u64>,
        timeout: Option<Duration>,
    ) -> StreamResult<WaitOutcome> {
        match timeout {
            // RT / cooperative-yield probe path (`probe_read`): a single
            // wake-free probe, unchanged — never parks on the gate.
            Some(_) => self.probe_range(range, timeout),
            // Off-RT consumer (`Stream::read` / `prime_seek_range`): block on
            // the readiness gate until the range resolves, a segment fails, or
            // cancel fires. Event-driven — no wall-clock poll.
            None => self.wait_range_blocking(range),
        }
    }

    /// Off-RT blocking wait: park on the readiness gate until [`probe_range`]
    /// resolves (`Ready`/`Eof`/`Interrupted`) or returns a terminal error.
    /// Event-driven — every transition that can flip the probe (segment
    /// write/commit/fail, fence raise/clear, seek reset, cancel) `signal`s the
    /// gate. The pre-probe [`current`](WaitGate::current) snapshot + park-only-
    /// if-unchanged is a seqlock guard closing the lost-wakeup window even
    /// though the probe predicate and the gate sit under different locks
    /// (mirrors `kithara-storage` `wait_range_inner`). A genuine wedge (no
    /// signal at all) trips the hang watchdog rather than parking forever.
    #[kithara::hang_watchdog(timeout = WAIT_HANG_TIMEOUT)]
    fn wait_range_blocking(&self, range: Range<u64>) -> StreamResult<WaitOutcome> {
        // Cancel is the one transition with no producer-side signal; register a
        // waker that signals the gate so a parked wait observes it. The guard
        // unregisters when this wait returns (mirror storage `wait.rs`).
        let _cancel_wake = {
            let ready = self.ready_gate();
            self.cancel.on_cancel(move || ready.signal())
        };
        loop {
            hang_tick!();
            // Snapshot the gate BEFORE the probe: a signal landing between the
            // probe and the park advances the counter, so the park returns at
            // once and we re-probe — no lost wakeup.
            let since = self.signal.current();
            match self.probe_range(range.clone(), Some(Duration::from_millis(0))) {
                Ok(WaitOutcome::Ready) => return Ok(WaitOutcome::Ready),
                Ok(WaitOutcome::Eof) => return Ok(WaitOutcome::Eof),
                Ok(WaitOutcome::Interrupted) => return Ok(WaitOutcome::Interrupted),
                Err(StreamError::Source(SourceError::WaitBudgetExceeded)) => {
                    // Not ready: park on the gate until a signal advances it,
                    // bounded by the re-aim heartbeat.
                }
                Err(e) => return Err(e),
            }
            // Event-driven park: a write/commit/fence/seek/cancel signal wakes
            // us at once to re-probe (the fact of a write, never a timer). If
            // the gate stays quiet for the heartbeat the peer may be mis-aimed
            // after a seek; yield so the off-RT reader re-asserts its prefetch
            // aim and re-enters (mirrors the old per-iteration `notify_peer_wake`
            // without the wall-clock data poll).
            if self.signal.wait_timeout(since, Self::READER_REAIM_INTERVAL) {
                // Woke from a signal — activity, not a wedge: reset the watchdog.
                hang_reset!();
            } else {
                return Err(StreamError::Source(SourceError::WaitBudgetExceeded));
            }
        }
    }

    pub(crate) fn position(&self) -> u64 {
        self.active_session().position()
    }

    pub(crate) fn advance(&self, n: u64) {
        self.active_session().advance(n);
    }

    pub(crate) fn set_position(&self, pos: u64) {
        self.active_session().seek_to_byte(pos);
    }

    pub(crate) fn seek_time_anchor(
        &self,
        position: Duration,
    ) -> StreamResult<Option<SourceSeekAnchor>> {
        self.active_session().seek_time_anchor(position)
    }

    pub(crate) fn selected_variant_for_seek(&self) -> usize {
        self.abr
            .selected_variant_for_seek()
            .expect("HlsCoord always owns a stateful ABR handle")
    }

    delegate! {
        to self.active().as_ref() {
            pub(crate) fn download_head(&self) -> u32;
            pub(crate) fn format_change_segment_range(&self) -> StreamResult<Range<u64>>;
        }
    }
}

pub(super) fn variant_switch_target_time(
    seek_obs: &dyn SeekObserve,
    playhead_read: &dyn PlayheadRead,
) -> Duration {
    if seek_obs.is_pending() || seek_obs.is_flushing() {
        return seek_obs
            .target()
            .unwrap_or_else(|| playhead_read.position());
    }
    playhead_read.position()
}

fn variant_switch_uses_byte_continuity(
    same_codec: bool,
    container: Option<ContainerFormat>,
) -> bool {
    match container {
        Some(ContainerFormat::Wav) => true,
        Some(
            ContainerFormat::Adts
            | ContainerFormat::Flac
            | ContainerFormat::MpegAudio
            | ContainerFormat::MpegTs
            | ContainerFormat::Ogg,
        ) => same_codec,
        Some(
            ContainerFormat::Caf
            | ContainerFormat::Fmp4
            | ContainerFormat::Mkv
            | ContainerFormat::Mp4,
        )
        | None => false,
    }
}

/// `VariantControl` exposes the cross-variant fence/format-change surface
/// to the stream layer. The bodies are the coord's existing inherent
/// methods — non-adaptive sources vend `None` instead of implementing
/// these.
impl VariantControl for HlsCoord {
    fn enable_variant_sessions(&self) -> StreamResult<()> {
        Self::enable_variant_sessions(self)
    }

    fn prepare_variant_reader(
        &self,
        profile: ReaderProfile,
    ) -> StreamResult<Option<VariantTransition>> {
        Self::prepare_variant_reader(self, profile)
    }

    fn take_prepared_variant_reader(
        &self,
        transition: VariantTransition,
    ) -> StreamResult<VariantReaderTake> {
        Self::take_prepared_variant_reader(self, transition)
    }

    fn promote_variant(&self, transition: VariantTransition) -> bool {
        Self::promote_variant(self, transition)
    }

    fn abort_variant(&self, transition: VariantTransition) -> bool {
        Self::abort_variant(self, transition)
    }

    fn selected_variant_for_seek(&self) -> usize {
        Self::selected_variant_for_seek(self)
    }

    fn clear_variant_fence(&self) {
        Self::clear_variant_fence(self);
    }

    fn format_change_segment_range(&self) -> StreamResult<Range<u64>> {
        Self::format_change_segment_range(self)
    }

    fn has_variant_change_pending(&self) -> bool {
        Self::has_variant_change_pending(self)
    }

    fn variant_change_target(&self) -> Option<usize> {
        Self::variant_change_target(self)
    }
}

/// `ByteMap` delegates to the authoritative active session's variant.
impl ByteMap for HlsCoord {
    fn anchor_at_time(&self, position: Duration) -> StreamResult<Option<SourceSeekAnchor>> {
        self.prepare_for_seek();
        self.seek_time_anchor(position)
    }

    fn init_segment_range(&self) -> Range<u64> {
        self.active().init_byte_range()
    }

    fn len(&self) -> Option<u64> {
        self.active().stream_len()
    }

    fn segment_at_byte(&self, byte: u64) -> Option<SegmentDescriptor> {
        self.variant_serving(byte).descriptor_at_byte(byte)
    }

    fn segment_count(&self) -> Option<u32> {
        Some(self.active().num_segments())
    }

    fn segment_after_byte(&self, byte: u64) -> Option<SegmentDescriptor> {
        self.active().descriptor_after_byte(byte)
    }

    fn segment_at_index(&self, segment_index: u32) -> Option<SegmentDescriptor> {
        self.active().descriptor(segment_index as usize)
    }

    fn segment_at_time(&self, t: Duration) -> Option<SegmentDescriptor> {
        self.active().descriptor_at_time(t)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{ErrorKind, Read},
        num::NonZeroU64,
        sync::OnceLock,
    };

    use kithara_abr::{Abr, AbrController, AbrSettings, AbrState, PendingAbrClaim};
    use kithara_assets::{AssetResource, AssetSource, AssetStoreBuilder, StorageBackend};
    use kithara_events::{AbrMode, AbrReason, Event, EventBus, VariantIndex};
    use kithara_platform::sync::{Arc, ThreadGate};
    use kithara_stream::{
        AudioCodec, ContainerFormat, PlayheadWrite, ReaderInput, ReaderWarmup, SeekControl,
    };

    use super::*;
    use crate::{
        config::SizeProbeMethod,
        playlist::{PlaylistState, SegmentState, VariantState},
        segment::{MediaSegment, Segment, SegmentContent, SegmentSize, SegmentSlotState},
        variant::{PlanCtx, VariantParts},
    };

    struct TestAbrPeer {
        state: Arc<AbrState>,
        variants: Vec<kithara_events::VariantInfo>,
    }

    impl Abr for TestAbrPeer {
        fn state(&self) -> Option<Arc<AbrState>> {
            Some(Arc::clone(&self.state))
        }

        fn variants(&self) -> Vec<kithara_events::VariantInfo> {
            self.variants.clone()
        }
    }

    fn switch_coord() -> (Arc<HlsCoord>, EventBus, PlanCtx, Arc<AbrState>) {
        let bus = EventBus::new(8);
        let cancel = CancelToken::never();
        let store = Arc::new(
            AssetStoreBuilder::default()
                .backend(StorageBackend::Memory)
                .cancel(cancel.clone())
                .build(),
        );
        let signal = SizeSignal::new(Arc::new(ThreadGate::default()), Arc::new(OnceLock::new()));
        let ctx = PlanCtx {
            bus: bus.clone(),
            prefetch_budget: 1,
            master_cancel: cancel.clone(),
            scope: store
                .scope::<crate::Hls>(&AssetSource::Remote {
                    url: "https://example.com/master.m3u8"
                        .parse()
                        .expect("master url"),
                    discriminator: Some("coord-test".to_owned()),
                })
                .expect("coord asset scope"),
            seek_epoch: 0,
            look_ahead_bytes: None,
            look_ahead_segments: None,
            headers: None,
            size_probe_method: SizeProbeMethod::Head,
            signal: signal.clone(),
        };
        let playlist = Arc::new(PlaylistState::new(vec![
            VariantState {
                codec: Some(AudioCodec::AacLc),
                container: Some(ContainerFormat::Fmp4),
                init_url: None,
                segments: vec![SegmentState {
                    url: "https://example.com/v0-seg0.m4s".parse().expect("url"),
                    duration: Duration::from_secs(2),
                    byte_range_len: Some(100),
                    index: crate::ids::SegmentIndex::try_new(0, 1).expect("idx"),
                }],
            },
            VariantState {
                codec: Some(AudioCodec::Mp3),
                container: Some(ContainerFormat::MpegAudio),
                init_url: None,
                segments: vec![SegmentState {
                    url: "https://example.com/v1-seg0.m4s".parse().expect("url"),
                    duration: Duration::from_secs(2),
                    byte_range_len: Some(100),
                    index: crate::ids::SegmentIndex::try_new(0, 1).expect("idx"),
                }],
            },
        ]));
        let variants: Arc<[Arc<HlsVariant>]> = Arc::from(vec![
            VariantParts {
                init: None,
                segments: vec![Segment::Media(MediaSegment {
                    url: "https://example.com/v0-seg0.m4s".parse().expect("url"),
                    resource_id: ctx
                        .scope
                        .key(&AssetResource::Url(
                            "https://example.com/v0-seg0.m4s".parse().expect("url"),
                        ))
                        .expect("segment key"),
                    state: SegmentSlotState::missing(),
                    size: SegmentSize::seed(100),
                    content: SegmentContent::Plain,
                    decode_time: Duration::ZERO,
                    duration: Duration::from_secs(2),
                })],
                seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
                codec: playlist.variant_codec(0),
                container: playlist.variant_container(0),
            }
            .into_variant(0, &ctx),
            VariantParts {
                init: None,
                segments: vec![Segment::Media(MediaSegment {
                    url: "https://example.com/v1-seg0.m4s".parse().expect("url"),
                    resource_id: ctx
                        .scope
                        .key(&AssetResource::Url(
                            "https://example.com/v1-seg0.m4s".parse().expect("url"),
                        ))
                        .expect("segment key"),
                    state: SegmentSlotState::missing(),
                    size: SegmentSize::seed(100),
                    content: SegmentContent::Plain,
                    decode_time: Duration::ZERO,
                    duration: Duration::from_secs(2),
                })],
                seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
                codec: playlist.variant_codec(1),
                container: playlist.variant_container(1),
            }
            .into_variant(1, &ctx),
        ]);
        let abr_state = Arc::new(AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0)))));
        let abr_publisher = abr_state.publisher();
        let peer: Arc<dyn Abr> = Arc::new(TestAbrPeer {
            state: Arc::clone(&abr_state),
            variants: vec![
                kithara_events::VariantInfo {
                    variant_index: VariantIndex::new(0),
                    bandwidth_bps: Some(128_000),
                    codecs: Some("mp4a.40.2".into()),
                    container: Some("fmp4".into()),
                    duration: kithara_events::VariantDuration::Segmented(vec![
                        Duration::from_secs(2),
                    ]),
                    name: None,
                },
                kithara_events::VariantInfo {
                    variant_index: VariantIndex::new(1),
                    bandwidth_bps: Some(96_000),
                    codecs: Some("mp3".into()),
                    container: Some("mpeg-audio".into()),
                    duration: kithara_events::VariantDuration::Segmented(vec![
                        Duration::from_secs(2),
                    ]),
                    name: None,
                },
            ],
        });
        let controller = Arc::new(AbrController::new(
            AbrSettings::default(),
            CancelToken::never(),
        ));
        let handle = controller.register(&peer);
        handle
            .set_mode(AbrMode::Manual(VariantIndex::new(1)))
            .expect("manual target in range");
        let coord = Arc::new(HlsCoord::new(
            HlsCoordEnv {
                scope: ctx.scope.clone(),
                cancel,
                headers: None,
                emit: Arc::new(DeferredBus::new(bus.clone(), 8)),
                signal,
            },
            Arc::new(PlayheadState::new()),
            Arc::new(SeekState::new()),
            handle,
            abr_publisher,
            variants,
            playlist,
        ));
        (coord, bus, ctx, abr_state)
    }

    fn incremental_profile(read_ahead_bytes: u64) -> ReaderProfile {
        ReaderProfile::new(
            ReaderInput::Incremental,
            ReaderWarmup::None,
            NonZeroU64::new(read_ahead_bytes).expect("non-zero read ahead"),
        )
    }

    fn enable_exact_sessions(coord: &HlsCoord) {
        if let Some(pending) = coord.abr.claim_pending_decision() {
            assert!(coord.abr_publisher.abort_pending(pending.ticket()));
        }
        coord
            .enable_variant_sessions()
            .expect("enable exact sessions before intent");
    }

    fn enable_exact_target(coord: &HlsCoord, abr_state: &AbrState) {
        enable_exact_sessions(coord);
        abr_state.request_target(VariantIndex::new(1), AbrReason::ManualOverride);
    }

    fn take_ready_incremental_reader(
        coord: &HlsCoord,
        ctx: &PlanCtx,
        transition: VariantTransition,
    ) {
        let mut command = coord
            .dispatch_incoming(ctx, 1)
            .pop()
            .expect("incoming fetch");
        command.writer.take().expect("streaming writer")(&[1; 32]).expect("construction bytes");
        assert!(matches!(
            coord
                .take_prepared_variant_reader(transition)
                .expect("take ready reader"),
            VariantReaderTake::Ready(_)
        ));
    }

    #[kithara::test]
    fn variant_switch_target_uses_active_seek_target() {
        let seek = SeekState::new();
        let playhead = PlayheadState::new();
        playhead.set_position(Duration::from_secs(9));

        let _epoch = seek.begin(Duration::from_secs(5));

        assert_eq!(
            variant_switch_target_time(&seek, &playhead),
            Duration::from_secs(5)
        );
    }

    #[kithara::test]
    fn variant_switch_target_keeps_flushing_seek_target_after_decoder_applies() {
        let seek = SeekState::new();
        let playhead = PlayheadState::new();
        playhead.set_position(Duration::from_secs(9));

        let epoch = seek.begin(Duration::from_secs(5));
        seek.clear_pending(epoch);

        assert_eq!(
            variant_switch_target_time(&seek, &playhead),
            Duration::from_secs(5)
        );
    }

    #[kithara::test]
    fn variant_switch_target_ignores_completed_seek_target() {
        let seek = SeekState::new();
        let playhead = PlayheadState::new();

        let epoch = seek.begin(Duration::from_secs(5));
        seek.clear_pending(epoch);
        seek.complete(epoch);
        playhead.set_position(Duration::from_secs(9));

        assert_eq!(
            variant_switch_target_time(&seek, &playhead),
            Duration::from_secs(9)
        );
    }

    #[kithara::test]
    fn same_codec_fmp4_switch_recreates_decoder_boundary() {
        assert!(!variant_switch_uses_byte_continuity(
            true,
            Some(ContainerFormat::Fmp4)
        ));
    }

    #[kithara::test]
    fn same_codec_wav_switch_uses_byte_continuity() {
        assert!(variant_switch_uses_byte_continuity(
            true,
            Some(ContainerFormat::Wav)
        ));
    }

    #[kithara::test(tokio)]
    async fn variant_switch_fence_and_ack_publish_events() {
        let (coord, bus, ctx, _abr_state) = switch_coord();
        let mut events = bus.subscribe();

        assert!(coord.commit_variant_switch_at_segment(&ctx, 0));
        coord.clear_variant_fence();
        coord.emit.flush();

        assert!(matches!(
            events.try_recv().map(|envelope| envelope.event),
            Ok(Event::Hls(HlsEvent::VariantSwitchFenced {
                from_variant: 0,
                to_variant: 1,
                cross_codec: true,
            }))
        ));
        assert!(matches!(
            events.try_recv().map(|envelope| envelope.event),
            Ok(Event::Hls(HlsEvent::VariantSwitchAcked {
                variant: 1,
                generation: 1,
            }))
        ));
    }

    #[kithara::test]
    fn incoming_session_does_not_publish_variant_or_cursor_before_promotion() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        coord.set_position(17);

        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        assert_eq!(coord.sessions.resident_count(), 2);

        assert_eq!(coord.variant_index(), 0);
        assert_eq!(coord.position(), 17);
        assert!(matches!(
            coord
                .take_prepared_variant_reader(transition)
                .expect("probe incoming"),
            VariantReaderTake::Preparing
        ));
        assert!(coord.abort_variant(transition));
        assert_eq!(coord.sessions.resident_count(), 1);
        assert_eq!(coord.variant_index(), 0);
        assert_eq!(coord.position(), 17);
    }

    #[kithara::test]
    fn exact_session_resolution_follows_the_committed_abr_selector_before_collapse() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        let claim = coord
            .abr
            .claim_pending_decision()
            .expect("prepared transition keeps its exact claim");

        assert!(coord.abr_publisher.commit_pending(claim, Instant::now()));

        assert_eq!(
            coord.active_session().variant_index(),
            1,
            "the committed ABR selector must resolve through the resident pair before collapse"
        );
    }

    #[kithara::test]
    fn exact_session_resolution_retries_a_stale_selector_snapshot() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        let mut selector_observations = [0, 1, 1, 1].into_iter();

        let resolved = coord.sessions.active(|| {
            selector_observations
                .next()
                .expect("resolver performs two selector reads per attempt")
        });

        assert_eq!(resolved.variant_index(), 1);
        assert_eq!(selector_observations.next(), None);
    }

    #[kithara::test(tokio)]
    async fn incoming_readiness_tracks_required_bytes_not_delivery_chunks() {
        let (coord, _bus, ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        coord.set_position(17);
        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        let mut commands = coord.dispatch_incoming(&ctx, 1);
        assert_eq!(commands.len(), 1);
        let mut command = commands.pop().expect("target fetch");
        assert_eq!(command.url.as_str(), "https://example.com/v1-seg0.m4s");
        let mut writer = command.writer.take().expect("streaming writer");

        writer(&[1; 7]).expect("first delivery chunk");
        assert!(matches!(
            coord
                .take_prepared_variant_reader(transition)
                .expect("probe partial incoming"),
            VariantReaderTake::Preparing
        ));

        writer(&[2; 25]).expect("second delivery chunk");
        let VariantReaderTake::Ready(reader) = coord
            .take_prepared_variant_reader(transition)
            .expect("take ready incoming")
        else {
            panic!("reader must become ready at the declared byte requirement");
        };
        assert_eq!(reader.transition(), transition);
        assert_eq!(reader.media_info().variant_index, Some(1));
        let (_transition, _media_info, reader) = reader.split();
        let mut input = reader.into_inner();
        assert!(matches!(
            coord
                .take_prepared_variant_reader(transition)
                .expect("take reader twice"),
            VariantReaderTake::Taken
        ));

        writer(&[3; 68]).expect("remaining delivery body");
        command.on_complete.take().expect("completion handler")(100, None, None);
        let mut first = [0_u8; 8];
        assert_eq!(input.read(&mut first).expect("read incoming"), first.len());
        assert_eq!(coord.position(), 17);
        assert!(coord.promote_variant(transition));
        assert_eq!(coord.sessions.resident_count(), 1);
        assert_eq!(coord.variant_index(), 1);
        assert_eq!(coord.position(), first.len() as u64);

        let mut second = [0_u8; 8];
        assert_eq!(
            input.read(&mut second).expect("read promoted session"),
            second.len()
        );
        assert_eq!(coord.position(), (first.len() + second.len()) as u64);
    }

    #[kithara::test]
    fn promotion_requires_the_exact_taken_reader() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");

        assert!(!coord.promote_variant(transition));
        assert_eq!(coord.variant_index(), 0);
        assert!(coord.abort_variant(transition));
    }

    #[kithara::test]
    fn one_transition_rejects_a_different_reader_profile() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");

        let error = coord
            .prepare_variant_reader(incremental_profile(64))
            .expect_err("profile mismatch must fail");
        assert!(matches!(
            error,
            StreamError::Source(SourceError::Io(ref error))
                if error.kind() == ErrorKind::InvalidInput
        ));
        assert!(matches!(
            coord
                .take_prepared_variant_reader(transition)
                .expect("original transition remains live"),
            VariantReaderTake::Preparing
        ));
        assert!(coord.abort_variant(transition));
    }

    #[kithara::test]
    fn stale_same_target_identity_cannot_disturb_newer_incoming() {
        let (coord, _bus, ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let stale = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare first incoming")
            .expect("first pending switch");
        let mut stale_command = coord
            .dispatch_incoming(&ctx, 1)
            .pop()
            .expect("first session fetch");
        let stale_cancel = stale_command
            .cancel
            .as_ref()
            .expect("session fetch cancel")
            .clone();
        assert!(!stale_cancel.is_cancelled());

        abr_state.request_target(VariantIndex::new(1), AbrReason::ManualOverride);
        let current = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare replacement incoming")
            .expect("replacement pending switch");

        assert_ne!(stale.id(), current.id());
        assert!(stale_cancel.is_cancelled());
        stale_command.on_complete.take().expect("stale completion")(0, None, None);
        let current_command = coord
            .dispatch_incoming(&ctx, 1)
            .pop()
            .expect("replacement session fetch");
        let current_cancel = current_command
            .cancel
            .as_ref()
            .expect("replacement fetch cancel")
            .clone();
        assert!(!current_cancel.is_cancelled());
        assert!(!coord.promote_variant(stale));
        assert!(!coord.abort_variant(stale));
        assert!(!current_cancel.is_cancelled());
        assert!(matches!(
            coord
                .take_prepared_variant_reader(current)
                .expect("replacement remains live"),
            VariantReaderTake::Preparing
        ));
        assert!(coord.abort_variant(current));
        assert!(current_cancel.is_cancelled());
    }

    #[kithara::test]
    fn exact_session_mode_never_falls_back_to_legacy_commit() {
        let (coord, _bus, ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        assert!(coord.abort_variant(transition));

        abr_state.request_target(VariantIndex::new(1), AbrReason::ManualOverride);
        assert!(!coord.has_incoming());
        assert!(!coord.commit_variant_switch_at_segment(&ctx, 0));
        assert_eq!(coord.variant_index(), 0);
        assert!(coord.abr.claim_pending_decision().is_some());
    }

    #[kithara::test]
    fn locked_exact_intent_keeps_its_incoming_session() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");

        coord.abr.lock();
        assert_eq!(
            coord
                .prepare_variant_reader(incremental_profile(32))
                .expect("locked preparation probe"),
            Some(transition)
        );
        assert!(coord.has_incoming());

        coord.abr.unlock();
        assert_eq!(
            coord
                .prepare_variant_reader(incremental_profile(32))
                .expect("unlocked preparation probe"),
            Some(transition)
        );
    }

    #[kithara::test]
    fn locked_exact_intent_rejects_a_reader_profile_change() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        coord.abr.lock();

        let error = coord
            .prepare_variant_reader(incremental_profile(64))
            .expect_err("one transition cannot change reader profile while locked");

        assert!(matches!(
            error,
            StreamError::Source(SourceError::Io(ref error))
                if error.kind() == ErrorKind::InvalidInput
        ));
        assert!(coord.has_incoming());
    }

    #[kithara::test]
    fn exact_session_mode_is_enabled_before_selection_intent() {
        let (coord, _bus, ctx, abr_state) = switch_coord();
        let pending = coord
            .abr
            .claim_pending_decision()
            .expect("fixture pending intent");

        let error = coord
            .enable_variant_sessions()
            .expect_err("late activation must not race a pending intent");
        assert!(matches!(
            error,
            StreamError::Source(SourceError::Io(ref error))
                if error.kind() == ErrorKind::InvalidInput
        ));
        assert_eq!(coord.abr.claim_pending_decision(), Some(pending));

        assert!(coord.abr_publisher.abort_pending(pending.ticket()));
        coord
            .enable_variant_sessions()
            .expect("enable before selection");
        abr_state.request_target(VariantIndex::new(1), AbrReason::ManualOverride);

        assert!(!coord.commit_variant_switch_at_segment(&ctx, 0));
        assert_eq!(coord.variant_index(), 0);
        assert!(
            coord
                .prepare_variant_reader(incremental_profile(32))
                .expect("claim exact transition")
                .is_some()
        );
    }

    #[kithara::test]
    fn terminal_exact_preparation_error_aborts_its_claim() {
        let (coord, _bus, ctx, abr_state) = switch_coord();
        enable_exact_sessions(&coord);
        abr_state.set_mode(AbrMode::Auto(Some(VariantIndex::new(0))));
        abr_state.request_target(VariantIndex::new(9), AbrReason::UpSwitch);

        let error = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect_err("unknown target must fail");
        assert!(matches!(
            error,
            StreamError::Source(SourceError::VariantNotFound(_))
        ));
        assert!(coord.abr.claim_pending_decision().is_none());
        assert!(!coord.commit_variant_switch_at_segment(&ctx, 0));
        assert_eq!(coord.variant_index(), 0);
    }

    #[kithara::test]
    fn active_seek_anchor_updates_the_session_cursor() {
        let (coord, _bus, _ctx, _abr_state) = switch_coord();
        coord.set_position(17);

        let anchor = ByteMap::anchor_at_time(coord.as_ref(), Duration::from_secs(1))
            .expect("resolve seek anchor")
            .expect("segmented source anchor");

        assert_eq!(anchor.byte_offset, 0);
        assert_eq!(coord.position(), anchor.byte_offset);
    }

    #[kithara::test]
    fn seek_aborts_the_exact_incoming_session() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let transition = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");

        let _epoch = coord.seek_control().begin(Duration::from_secs(1));
        coord.prepare_for_seek();

        assert!(matches!(
            coord
                .take_prepared_variant_reader(transition)
                .expect("probe stale incoming"),
            VariantReaderTake::Stale
        ));
        assert_eq!(coord.variant_index(), 0);
    }

    #[kithara::test]
    fn seek_replaces_the_session_without_deleting_manual_selection() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let claim = coord
            .abr
            .claim_pending_decision()
            .expect("manual selection claim");
        let stale = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");

        let next_epoch = coord.seek_control().begin(Duration::from_secs(1));
        coord.prepare_for_seek();

        assert_eq!(coord.abr.claim_pending_decision(), Some(claim));
        let replacement = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare replacement")
            .expect("manual selection survives seek");
        assert_eq!(replacement.id().abr_ticket(), stale.id().abr_ticket());
        assert_eq!(replacement.id().seek_epoch(), next_epoch);
        assert_ne!(replacement, stale);
    }

    #[kithara::test]
    fn seek_replaces_the_session_without_deleting_automatic_selection() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_sessions(&coord);
        abr_state.set_mode(AbrMode::Auto(Some(VariantIndex::new(0))));
        abr_state.request_target(VariantIndex::new(1), AbrReason::UpSwitch);
        let claim = coord
            .abr
            .claim_pending_decision()
            .expect("automatic selection claim");
        let stale = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");

        let next_epoch = coord.seek_control().begin(Duration::from_secs(1));
        coord.prepare_for_seek();

        assert_eq!(coord.abr.claim_pending_decision(), Some(claim));
        let replacement = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare replacement")
            .expect("automatic selection survives seek");
        assert_eq!(replacement.id().abr_ticket(), stale.id().abr_ticket());
        assert_eq!(replacement.id().seek_epoch(), next_epoch);
        assert_ne!(replacement, stale);
    }

    #[kithara::test]
    fn seek_selection_reads_the_pending_target_while_abr_is_locked() {
        let (coord, _bus, _ctx, abr_state) = switch_coord();
        enable_exact_sessions(&coord);
        abr_state.set_mode(AbrMode::Auto(Some(VariantIndex::new(0))));
        abr_state.request_target(VariantIndex::new(1), AbrReason::UpSwitch);
        coord.abr.lock();

        assert_eq!(coord.selected_variant_for_seek(), 1);
        assert!(matches!(
            coord.abr.pending_claim(),
            PendingAbrClaim::Locked(_)
        ));
    }

    #[kithara::test(tokio)]
    async fn seek_epoch_rejects_taken_generation_and_preserves_its_selection() {
        let (coord, _bus, ctx, abr_state) = switch_coord();
        enable_exact_target(&coord, &abr_state);
        let claim = coord
            .abr
            .claim_pending_decision()
            .expect("manual selection claim");
        let stale = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare incoming")
            .expect("pending switch");
        take_ready_incremental_reader(&coord, &ctx, stale);

        let next_epoch = coord.seek_control().begin(Duration::from_secs(1));

        assert!(!coord.promote_variant(stale));
        assert_eq!(coord.variant_index(), 0);
        assert_eq!(coord.abr.claim_pending_decision(), Some(claim));
        let replacement = coord
            .prepare_variant_reader(incremental_profile(32))
            .expect("prepare replacement")
            .expect("manual selection survives epoch change");
        assert_eq!(replacement.id().abr_ticket(), stale.id().abr_ticket());
        assert_eq!(replacement.id().seek_epoch(), next_epoch);
    }
}
