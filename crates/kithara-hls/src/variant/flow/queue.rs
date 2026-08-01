use kithara_platform::time::Duration;
use kithara_test_utils::kithara;

use super::{HlsVariant, PlanCtx};
use crate::segment::PlannedFetch;

impl HlsVariant {
    /// Index of the first non-`Loaded` segment — interpreted as the
    /// "download head" by the ABR controller. Returns `num_segments()`
    /// when every segment is `Loaded`. Scans linearly; cheap because it
    /// only runs from `Abr::progress` (ABR tick cadence).
    pub(crate) fn download_head(&self) -> u32 {
        let head = self
            .segments
            .iter()
            .position(|s| !s.state().is_loaded())
            .unwrap_or(self.segments.len());
        u32::try_from(head).unwrap_or(u32::MAX)
    }

    /// True when every fetch a rebuild from `from_seg` would enqueue is
    /// already `Loaded` — the init (if any) plus every media segment in
    /// `[from_seg, num_segments)`. In that state a rebuild only reseeds
    /// entries `dispatch` immediately skips, so it is pure churn on a
    /// fully-cached seek. A partial cache returns `false` and rebuilds, so
    /// the prefetch tail is still re-aimed at the seek target whenever a
    /// fetch is actually outstanding.
    pub(super) fn fetch_plan_satisfied(&self, from_seg: u32) -> bool {
        if self.needs_init_fetch() {
            return false;
        }
        let segs_len = self.num_segments();
        (from_seg..segs_len).all(|idx| {
            self.segments
                .get(idx as usize)
                .is_some_and(|seg| seg.state().is_loaded())
        })
    }

    #[kithara::probe(
        variant = self.variant as u64,
        from_seg,
        old_queue_len = self.flow.queue.lock().len() as u64
    )]
    pub(crate) fn rebuild(&self, _ctx: &PlanCtx, from_seg: u32) {
        if self.fetch_plan_satisfied(from_seg) {
            return;
        }
        self.rebuild_queue(from_seg, None);
    }

    pub(crate) fn rebuild_at_time(&self, ctx: &PlanCtx, target: Duration) -> Option<u32> {
        let seg = self.segment_index_at_time(target)?;
        let fetch_start = self.seek_readahead_start_segment(seg);
        if let Some(byte) = self.segment_byte_offset(fetch_start) {
            self.set_prefetch_anchor(byte);
        }
        self.rebuild(ctx, fetch_start);
        Some(seg)
    }

    #[kithara::probe]
    pub(super) fn rebuild_queue(&self, from_seg: u32, probe_seg: Option<u32>) {
        let segs_len = self.num_segments();
        let init = self
            .needs_init_fetch()
            .then_some(PlannedFetch::Init)
            .into_iter();
        let probe = probe_seg
            .filter(|probe| *probe < from_seg)
            .map(PlannedFetch::Segment)
            .into_iter();
        let tail = (from_seg..segs_len).map(PlannedFetch::Segment);
        let mut queue = self.flow.queue.lock();
        queue.clear();
        queue.extend(init.chain(probe).chain(tail));
    }

    /// Fetch plan for a reader being prepared to land on `landing`.
    ///
    /// The landing segment leads. It is the only one the readiness gate waits
    /// for, and that gate is what holds the switch: whatever precedes it in the
    /// queue is served first and delays the transition by a whole segment.
    /// `landing - 1` is still planned — a demuxer handed a landing time parks at
    /// the packet boundary at or before it, which falls in the previous segment
    /// when the landing sits at a segment start — it just follows the landing
    /// instead of leading it.
    pub(super) fn rebuild_reader_queue(&self, landing: u32) {
        let segs_len = self.num_segments();
        let init = self
            .needs_init_fetch()
            .then_some(PlannedFetch::Init)
            .into_iter();
        let head = (landing < segs_len)
            .then_some(PlannedFetch::Segment(landing))
            .into_iter();
        let behind = landing
            .checked_sub(1)
            .map(PlannedFetch::Segment)
            .into_iter();
        let rest = (landing.saturating_add(1)..segs_len).map(PlannedFetch::Segment);
        let mut queue = self.flow.queue.lock();
        queue.clear();
        queue.extend(init.chain(head).chain(behind).chain(rest));
    }

    /// Same as [`Self::rebuild`] but also enqueues `seg 0` when
    /// `from_seg > 0`, so the decoder factory's probe has the container
    /// header to construct the codec. See the crate `CONTEXT.md`
    /// "Decoder-probe rebuild".
    #[kithara::probe(
        variant = self.variant as u64,
        from_seg,
        old_queue_len = self.flow.queue.lock().len() as u64
    )]
    pub(crate) fn rebuild_with_decoder_probe(&self, _ctx: &PlanCtx, from_seg: u32) {
        let from_seg = self.seek_readahead_start_segment(from_seg);
        self.set_segment_aware_seek_tail(from_seg);
        self.rebuild_queue(from_seg, (from_seg > 0).then_some(0));
    }
}
