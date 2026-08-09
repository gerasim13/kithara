use kithara_events::RequestPriority;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_stream::dl::FetchCmd;
use kithara_test_utils::kithara;
use tracing::debug;

use super::{HlsVariant, PlanCtx, core::NO_PREFETCH_DEFERRAL};
use crate::segment::{Downloading, FetchClaim, PlannedFetch};

impl HlsVariant {
    #[kithara::probe(
        variant = self.variant as u64,
        budget = budget as u64,
        queue_len = self.flow.queue.lock().len() as u64
    )]
    /// Owed fetches carry `High`. The downloader serves its slots in strict
    /// order and drains the tagged one completely first, so an untagged fetch
    /// queues behind every tagged one — which makes the tag, not the peer's
    /// budget split, the thing that decides who waits for whom.
    ///
    /// A session that is not yet audible has no look-ahead of its own: it asks
    /// only for what a decoder being built or primed is blocked on, and its
    /// pass is already bounded by the construction window or by that decoder's
    /// reads. Every fetch it plans is owed. Bounding it the way an audible
    /// session is bounded strands the priming phase, whose reads sit at the
    /// landing while the session's own position still reads zero.
    ///
    /// The audible session is the opposite: it plans look-ahead every poll, and
    /// only the segment its reader is stopped at is owed. Leaving that segment
    /// untagged is what let a cold target — whose segments are deliberately
    /// slow — hold the downloader while the speaker ran dry.
    ///
    /// So both owed sets share one slot, where arrival order decides: the
    /// audible variant adds a single segment per poll and cannot bury a
    /// decoder being built, and the session being prepared cannot outrank the
    /// audio that is playing.
    #[kithara::hang_watchdog]
    pub(crate) fn dispatch_from(
        self: &Arc<Self>,
        ctx: &PlanCtx,
        budget: usize,
        position: u64,
        construction_segment_end: Option<u32>,
        audible: bool,
        cancel: CancelToken,
    ) -> Vec<FetchCmd> {
        let mut out = Vec::new();
        // Popped segments that could not be dispatched this pass but are NOT
        // terminal (a slot still `Downloading` under an orphaned/in-flight
        // fetch, or one that raced back to `Missing`): re-queued at the front
        // after the pass so a later dispatch re-claims them once the slot
        // frees. Without this, a seek that re-queues the target while an old
        // prefetch still holds it `Downloading` would pop+drop the target —
        // the orphaned fetch settles back to `Missing` but the queue no longer
        // references it, so it is never re-fetched and the reader hangs (the
        // `player_worker_hls_then_unavailable_mp3_then_mp3_recovery` deadlock).
        let mut deferred: Vec<PlannedFetch> = Vec::new();
        let owed_through = audible
            .then(|| self.find_at_offset(position).map(|(seg_idx, _, _)| seg_idx))
            .flatten();
        let owed = |seg_idx: u32| !audible || owed_through.is_some_and(|last| seg_idx <= last);
        let mut remaining = budget;
        self.dispatch_size_demands(ctx, &mut out, &mut remaining, &cancel);
        let prefetch_base = position.max(self.prefetch_anchor());
        let prefetch_byte_cap = ctx
            .look_ahead_bytes
            .map(|n| prefetch_base.saturating_add(n));
        let prefetch_segment_cap = self.prefetch_segment_cap(ctx, prefetch_base);
        // Cursor byte at which the segment a cap turned away enters the window.
        // Published after the pass so the reader crossing it wakes the peer for
        // exactly that segment; `None` leaves the deferral cleared.
        let mut resume_at: Option<u64> = None;
        while remaining > 0 {
            hang_tick!();
            let planned = {
                let mut queue = self.flow.queue.lock();
                match queue.front().copied() {
                    None => break,
                    Some(PlannedFetch::Init) => queue.pop_front(),
                    Some(PlannedFetch::Segment(seg_idx)) => {
                        if construction_segment_end.is_some_and(|end| seg_idx > end) {
                            break;
                        }
                        if let Some(cap) = prefetch_byte_cap
                            && let Some(seg_off) = self.segment_byte_offset(seg_idx)
                            && seg_off > cap
                        {
                            resume_at = ctx
                                .look_ahead_bytes
                                .map(|window| seg_off.saturating_sub(window));
                            break;
                        }
                        if let Some(cap) = prefetch_segment_cap
                            && seg_idx > cap
                        {
                            resume_at = self.segment_window_entry_byte(ctx, seg_idx);
                            break;
                        }
                        queue.pop_front()
                    }
                }
            };
            let Some(planned) = planned else { break };
            match planned {
                PlannedFetch::Init => {
                    // Only a present init is ever enqueued (the `rebuild`
                    // gate skips a `None` init), so a missing slot here is
                    // unreachable; skip defensively rather than claim a slot
                    let Some(init) = self.init() else {
                        continue;
                    };
                    let Some(handle) = init
                        .state()
                        .try_claim(PlannedFetch::Init, Arc::downgrade(self))
                    else {
                        if !init.state().is_loaded() && !init.state().is_failed() {
                            deferred.push(planned);
                        }
                        continue;
                    };
                    if let Some(actual) = self.init_committed_final_len() {
                        handle.into_loaded(actual);
                        ctx.signal.fire();
                        continue;
                    }
                    let Some(mut cmd) = self.build_init_cmd(ctx, handle, cancel.clone()) else {
                        if self
                            .init()
                            .is_some_and(|i| !i.state().is_loaded() && !i.state().is_failed())
                        {
                            deferred.push(planned);
                        }
                        continue;
                    };
                    // A decoder cannot start without its init, so it is never
                    cmd.set_priority(RequestPriority::High);
                    out.push(cmd);
                }
                PlannedFetch::Segment(seg_idx) => {
                    let Some(entry) = self.segments.get(seg_idx as usize) else {
                        continue;
                    };
                    let Some(handle) = entry
                        .state()
                        .try_claim(PlannedFetch::Segment(seg_idx), Arc::downgrade(self))
                    else {
                        // Claim failed — another claim owns the slot. Re-queue
                        // unless terminal (`Loaded` = already fetched, `Failed`
                        // = gave up): a `Downloading` orphan settles back to
                        // `Missing` and must stay re-claimable from the queue.
                        if !entry.state().is_loaded() && !entry.state().is_failed() {
                            deferred.push(planned);
                        }
                        continue;
                    };
                    if let Some(actual) = self.committed_final_len(seg_idx) {
                        handle.into_loaded(actual);
                        ctx.signal.fire();
                        continue;
                    }
                    let Some(mut cmd) = self.emit_fetch_cmd(ctx, seg_idx, handle, cancel.clone())
                    else {
                        // Acquire raced (the claim was reverted to `Missing`
                        // inside `emit_fetch_cmd`): re-queue, don't drop it.
                        deferred.push(planned);
                        continue;
                    };
                    if owed(seg_idx) {
                        cmd.set_priority(RequestPriority::High);
                    }
                    out.push(cmd);
                }
            }
            remaining -= 1;
        }
        if !deferred.is_empty() {
            let mut queue = self.flow.queue.lock();
            for planned in deferred.into_iter().rev() {
                queue.push_front(planned);
            }
        }
        self.defer_prefetch_until(resume_at.unwrap_or(NO_PREFETCH_DEFERRAL));
        out
    }

    #[kithara::probe(
        seek_epoch = ctx.seek_epoch,
        segment_index = u64::from(seg_idx),
        variant = self.variant as u64
    )]
    fn emit_fetch_cmd(
        self: &Arc<Self>,
        ctx: &PlanCtx,
        seg_idx: u32,
        handle: FetchClaim<Downloading>,
        cancel: CancelToken,
    ) -> Option<FetchCmd> {
        let entry = &self.segments[seg_idx as usize];
        let Some(resource_handle) = self.segment_handle(seg_idx) else {
            let _ = handle.into_missing();
            return None;
        };
        let resource = match resource_handle.acquire(entry.content()) {
            Ok(r) => r,
            Err(err) => {
                debug!(
                    variant = self.variant,
                    seg_idx,
                    error = %err,
                    "emit_fetch_cmd: acquire_resource dropped (variant switch in flight)"
                );
                let _ = handle.into_missing();
                return None;
            }
        };
        self.build_cmd(
            resource_handle.url().clone(),
            resource,
            handle,
            ctx.signal.clone(),
            cancel,
        )
    }

    fn prefetch_segment_cap(&self, ctx: &PlanCtx, prefetch_base: u64) -> Option<u32> {
        let window = look_ahead_segments(ctx)?;
        let base = self.descriptor_after_byte(prefetch_base)?.segment_index;
        Some(base.saturating_add(window.saturating_sub(1)))
    }

    /// First byte of the segment the cursor must reach for `seg_idx` to fall
    /// inside a `look_ahead_segments` window — the segment-count counterpart of
    /// `seg_off - look_ahead_bytes`.
    fn segment_window_entry_byte(&self, ctx: &PlanCtx, seg_idx: u32) -> Option<u64> {
        let window = look_ahead_segments(ctx)?;
        self.segment_byte_offset(seg_idx.saturating_sub(window.saturating_sub(1)))
    }
}

/// Media-segment look-ahead width, normalised to at least one segment.
fn look_ahead_segments(ctx: &PlanCtx) -> Option<u32> {
    let window = ctx.look_ahead_segments?;
    Some(u32::try_from(window.max(1)).unwrap_or(u32::MAX))
}
