use kithara_platform::time::Duration;
use kithara_test_utils::kithara;

use super::{HlsVariant, PlanCtx, ReadLease, SegmentActivateParams, offsets::ActivateParams};
use crate::segment::{PlannedFetch, Segment};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetBodyReadiness {
    Pending,
    Ready,
    Failed,
}

/// Canonical decoder-body order: init, segment-zero probe, then the exact
/// media window. The iterator owns no collection, so rebuilding a lease's
/// preallocated queue cannot allocate or require a sort.
struct TargetBodyEntries {
    init: bool,
    probe: bool,
    next_segment: Option<u32>,
    through_segment: u32,
}

impl Iterator for TargetBodyEntries {
    type Item = PlannedFetch;

    fn next(&mut self) -> Option<Self::Item> {
        if self.init {
            self.init = false;
            return Some(PlannedFetch::Init);
        }
        if self.probe {
            self.probe = false;
            return Some(PlannedFetch::Segment(0));
        }
        let segment = self.next_segment?;
        if segment > self.through_segment {
            self.next_segment = None;
            return None;
        }
        self.next_segment = segment.checked_add(1);
        Some(PlannedFetch::Segment(segment))
    }
}

impl HlsVariant {
    fn target_body_entries(
        &self,
        first_segment: u32,
        through_segment: u32,
    ) -> Option<TargetBodyEntries> {
        if first_segment > through_segment || through_segment >= self.num_segments() {
            return None;
        }
        let start = self.seek_readahead_start_segment(first_segment);
        Some(TargetBodyEntries {
            init: self.init().is_some(),
            probe: start > 0,
            next_segment: Some(start),
            through_segment,
        })
    }

    fn target_body_segment(&self, planned: PlannedFetch) -> Option<&Segment> {
        match planned {
            PlannedFetch::Init => self.init(),
            PlannedFetch::Segment(index) => self.segments.get(index as usize),
        }
    }

    /// Decoder construction can start only after every segment in the exact
    /// canonical target body has settled successfully.
    pub(crate) fn target_body_readiness(
        &self,
        first_segment: u32,
        through_segment: u32,
    ) -> TargetBodyReadiness {
        let Some(entries) = self.target_body_entries(first_segment, through_segment) else {
            return TargetBodyReadiness::Failed;
        };
        let mut pending = false;
        for planned in entries {
            let Some(segment) = self.target_body_segment(planned) else {
                return TargetBodyReadiness::Failed;
            };
            if segment.state().is_failed() {
                return TargetBodyReadiness::Failed;
            }
            pending |= !segment.state().is_loaded();
        }
        if pending {
            TargetBodyReadiness::Pending
        } else {
            TargetBodyReadiness::Ready
        }
    }
}

impl ReadLease {
    /// Auto-mode switch activation. Two byte positions matter:
    ///
    /// - `seg_boundary` — the **virtual** byte where this variant's
    ///   segment `from_seg` should start in the combined stream. The
    ///   caller should pass the outgoing variant's segment boundary
    ///   (e.g. `v_old.segment_byte_offset(from_seg)`) so the byte
    ///   address space joins without gaps or overlaps; box scans across
    ///   the boundary stay correctly aligned.
    /// - `reader_pos` — the reader's current source position. Stored as
    ///   the session's `position` so `coord.position()` stays monotonic
    ///   after the active variant flips. May be `>= seg_boundary`
    ///   when the reader had already partially read into `from_seg`'s
    ///   byte range from the outgoing variant before the commit fired.
    ///
    /// `byte_shift` is derived from `seg_boundary`, not `reader_pos`, so
    /// segment offsets pin to actual fMP4 box boundaries.
    pub(crate) fn activate_at_segment_with_shift(
        &self,
        ctx: &PlanCtx,
        params: SegmentActivateParams,
    ) {
        let SegmentActivateParams {
            from_seg,
            seg_boundary,
            reader_pos,
        } = params;
        self.variant.rearm_cancel();
        let from_seg = from_seg.min(self.variant.num_segments());
        self.variant.layout.activate_with_shift(
            ActivateParams {
                from_seg,
                seg_boundary,
                init_size: self.variant.init_route_size(),
            },
            &self.variant.segments,
        );
        self.set_position_without_byte_demand(reader_pos);
        self.rebuild(ctx, from_seg);
    }

    #[kithara::probe(
        variant = self.variant.index() as u64,
        from_seg,
        old_queue_len = self.queue.lock().len() as u64
    )]
    pub(crate) fn rebuild(&self, _ctx: &PlanCtx, from_seg: u32) {
        if self.variant.fetch_plan_satisfied(from_seg) {
            return;
        }
        self.rebuild_queue(from_seg);
    }

    pub(crate) fn rebuild_at_time(&self, ctx: &PlanCtx, target: Duration) -> Option<u32> {
        let seg = self.variant.segment_index_at_time(target)?;
        let fetch_start = self.variant.seek_readahead_start_segment(seg);
        if let Some(byte) = self.variant.segment_byte_offset(fetch_start) {
            self.set_prefetch_anchor(byte);
        }
        self.rebuild(ctx, fetch_start);
        if let Some(byte) = self.variant.segment_byte_offset(seg) {
            self.set_exact_seek_demand(byte, seg);
        }
        Some(seg)
    }

    #[kithara::probe]
    pub(super) fn rebuild_queue(&self, from_seg: u32) {
        let segs_len = self.variant.num_segments();
        let init = self
            .variant
            .needs_init_fetch()
            .then_some(PlannedFetch::Init)
            .into_iter();
        let tail = (from_seg..segs_len).map(PlannedFetch::Segment);
        let mut queue = self.queue.lock();
        queue.clear();
        queue.extend(init.chain(tail));
    }

    /// Same as [`Self::rebuild`] but also enqueues `seg 0` when
    /// `from_seg > 0`, so the decoder factory's probe has the container
    /// header to construct the codec. See the crate `CONTEXT.md`
    /// "Decoder-probe rebuild".
    #[kithara::probe(
        variant = self.variant.index() as u64,
        from_seg,
        old_queue_len = self.queue.lock().len() as u64
    )]
    pub(crate) fn rebuild_with_decoder_probe(&self, _ctx: &PlanCtx, from_seg: u32) {
        let from_seg = self.variant.seek_readahead_start_segment(from_seg);
        self.set_segment_aware_seek_tail(from_seg);
        let segs_len = self.variant.num_segments();
        let init = self
            .variant
            .needs_init_fetch()
            .then_some(PlannedFetch::Init)
            .into_iter();
        let probe_seg = (from_seg > 0)
            .then_some(PlannedFetch::Segment(0))
            .into_iter();
        let tail = (from_seg..segs_len).map(PlannedFetch::Segment);
        let mut queue = self.queue.lock();
        queue.clear();
        queue.extend(init.chain(probe_seg).chain(tail));
    }

    /// Replace the fetch plan with one exact decoder-body coverage version.
    pub(crate) fn start_target_body(&self, first_segment: u32, through_segment: u32) -> bool {
        if self.variant.cancel_handle().is_cancelled() {
            self.variant.rearm_cancel();
        }
        let Some(entries) = self
            .variant
            .target_body_entries(first_segment, through_segment)
        else {
            return false;
        };
        if let Some(anchor) = self.variant.segment_byte_offset_natural(first_segment) {
            self.set_prefetch_anchor(anchor);
        }

        let mut queue = self.queue.lock();
        queue.clear();
        queue.extend(entries.filter(|planned| {
            self.variant
                .target_body_segment(*planned)
                .is_some_and(|segment| {
                    let state = segment.state();
                    !state.is_downloading() && !state.is_loaded() && !state.is_failed()
                })
        }));
        true
    }

    /// Rebuild a newer immutable coverage version in canonical order.
    ///
    /// Slot state remains the only source of truth for queued versus in-flight
    /// work. Rebuilding therefore preserves an existing download by omitting
    /// its claimed slot, while an evicted `Missing` prefix is naturally put
    /// back ahead of the suffix.
    pub(crate) fn extend_target_body(&self, first_segment: u32, through_segment: u32) -> bool {
        self.start_target_body(first_segment, through_segment)
    }
}
