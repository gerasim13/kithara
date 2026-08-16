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
    /// Emits prioritized fetches within construction and look-ahead bounds.
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
                    let Some(init) = self.init() else {
                        continue;
                    };
                    let Some(handle) = init.state().try_claim(
                        PlannedFetch::Init,
                        Arc::downgrade(self),
                        ctx.signal.clone(),
                    ) else {
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
                    // WHY: A decoder cannot start until its init fetch completes.
                    cmd.set_priority(RequestPriority::High);
                    out.push(cmd);
                }
                PlannedFetch::Segment(seg_idx) => {
                    let Some(entry) = self.segments.get(seg_idx as usize) else {
                        continue;
                    };
                    let Some(handle) = entry.state().try_claim(
                        PlannedFetch::Segment(seg_idx),
                        Arc::downgrade(self),
                        ctx.signal.clone(),
                    ) else {
                        // WHY: An orphaned download may return to `Missing` and need another claim.
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
                        // WHY: A reverted claim must remain queued for another acquisition attempt.
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
                // A concurrent claim's Drop may have requeued this entry
                // between the pop above and this write-back (a downloader
                // teardown racing the dispatch) — never double-plan it.
                if !queue.contains(&planned) {
                    queue.push_front(planned);
                }
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

    fn segment_window_entry_byte(&self, ctx: &PlanCtx, seg_idx: u32) -> Option<u64> {
        let window = look_ahead_segments(ctx)?;
        self.segment_byte_offset(seg_idx.saturating_sub(window.saturating_sub(1)))
    }
}

fn look_ahead_segments(ctx: &PlanCtx) -> Option<u32> {
    let window = ctx.look_ahead_segments?;
    Some(u32::try_from(window.max(1)).unwrap_or(u32::MAX))
}
