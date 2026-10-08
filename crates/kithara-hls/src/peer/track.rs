use std::{
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Waker},
};

use kithara_assets::ResourceKey;
use kithara_bufpool::HasPool;
use kithara_platform::{sync::Arc, tokio::sync::mpsc};
use kithara_stream::SeekObserve;
use kithara_test_utils::kithara;

use crate::{stream::HlsCoord, variant::PlanCtx};

pub(super) struct HlsTrackState<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) coord: Arc<HlsCoord<S>>,
    /// Reused from the parent peer: the reader's last-known segment index,
    /// observed by ABR progress and initialized when the peer activates.
    reader_segment: Arc<AtomicUsize>,
    seek_obs: Arc<dyn SeekObserve>,
    /// Forward-seek target held until the physical reader cursor catches up.
    /// Boundary reconciliation treats it as a floor while decoder recreation
    /// still exposes the old byte position, then clears it at the landing.
    seek_settle_floor: Option<u32>,
    pub(super) waker: Option<Waker>,
    eviction_rx: mpsc::UnboundedReceiver<ResourceKey>,
    last_seek_epoch: u64,
    /// Variant against which the stored reader segment was resolved. A flip
    /// re-keys the byte space even when the segment index stays unchanged.
    reader_variant: usize,
}

pub(super) struct PollOutcome<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) coord: Arc<HlsCoord<S>>,
    pub(super) ctx: PlanCtx<S>,
    pub(super) evictions: Vec<ResourceKey>,
    pub(super) seg_at_reader: u32,
    pub(super) needs_retick: bool,
}

impl<S> HlsTrackState<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) fn new(
        coord: Arc<HlsCoord<S>>,
        reader_segment: Arc<AtomicUsize>,
        seek_obs: Arc<dyn SeekObserve>,
        eviction_rx: mpsc::UnboundedReceiver<ResourceKey>,
    ) -> Self {
        Self {
            reader_variant: coord.variant_index(),
            coord,
            reader_segment,
            seek_obs,
            eviction_rx,
            last_seek_epoch: 0,
            seek_settle_floor: None,
            waker: None,
        }
    }

    /// Reconcile under the peer's state guard. The returned work runs after
    /// that guard drops: ABR reevaluation reads peer progress and re-locks it.
    pub(super) fn reconcile(&mut self, cx: &mut Context<'_>) -> Option<PollOutcome<S>> {
        self.waker = Some(cx.waker().clone());
        let coord = Arc::clone(&self.coord);
        if coord.cancel.is_cancelled() {
            return None;
        }
        let ctx = self.plan_ctx();
        self.apply_seek_change(&coord, &ctx);
        let seg_at_reader = self.apply_boundary_crossing(&coord, &ctx);
        let needs_retick = coord.reconcile_escape(seg_at_reader);
        let evictions = self.drain_evictions();
        Some(PollOutcome {
            coord,
            ctx,
            evictions,
            seg_at_reader,
            needs_retick,
        })
    }

    /// Track the reader's segment and re-aim the plan when a seek or variant
    /// change re-keys the byte space. Exact incoming sessions own promotion.
    fn apply_boundary_crossing(&mut self, coord: &HlsCoord<S>, ctx: &PlanCtx<S>) -> u32 {
        let pos = coord.position();
        let prev = self.reader_segment.load(Ordering::Acquire);
        let variant_now = coord.variant_index();
        let variant_changed = self.reader_variant != variant_now;
        self.reader_variant = variant_now;
        let demand_segment = coord.demand_segment_at_offset(pos);
        let resolved = demand_segment.unwrap_or_else(|| u32::try_from(prev).unwrap_or(0));
        if let Some(floor) = self.seek_settle_floor {
            if demand_segment.is_some_and(|idx| idx >= floor) {
                self.seek_settle_floor = None;
            } else if let Some(landing) = demand_segment
                && floor.saturating_sub(landing) == 1
            {
                coord.active().rebuild(ctx, landing);
                self.seek_settle_floor = Some(landing);
                return landing;
            } else {
                return floor;
            }
        }
        let resolved_us = resolved as usize;
        let boundary_crossed = prev != resolved_us;
        if boundary_crossed {
            self.reader_segment.store(resolved_us, Ordering::Release);
        }
        let prev_u32 = u32::try_from(prev).unwrap_or(0);
        let discontinuous_advance = boundary_crossed && resolved != prev_u32.saturating_add(1);
        let aligned_rescue =
            variant_changed && demand_segment.is_some() && coord.active().served_from() == 0;
        if aligned_rescue {
            coord.active().rebuild_with_decoder_probe(ctx, resolved);
        } else if discontinuous_advance {
            coord.active().rebuild(ctx, resolved);
        }
        resolved
    }

    fn apply_seek_change(&mut self, coord: &HlsCoord<S>, ctx: &PlanCtx<S>) {
        let cur_seek = self.seek_obs.epoch();
        if cur_seek == self.last_seek_epoch {
            return;
        }
        self.last_seek_epoch = cur_seek;
        kithara::probe_event!(
            seek_epoch_reset,
            seek_epoch = cur_seek,
            segment_index = self.reader_segment.load(Ordering::Acquire),
            variant = coord.variant_index()
        );
        if let Some(target) = self.seek_obs.target()
            && let Some(seg) = coord.active().rebuild_at_time(ctx, target)
        {
            self.reader_segment.store(seg as usize, Ordering::Release);
            self.reader_variant = coord.variant_index();
            self.seek_settle_floor = Some(seg);
        }
    }

    /// Buffer eviction work for the lock-free dispatch phase.
    fn drain_evictions(&mut self) -> Vec<ResourceKey> {
        let mut out: Vec<ResourceKey> = Vec::new();
        while let Ok(key) = self.eviction_rx.try_recv() {
            out.push(key);
        }
        out
    }

    fn plan_ctx(&self) -> PlanCtx<S> {
        PlanCtx {
            bus: self.coord.emit.bus().clone(),
            scope: self.coord.scope.clone(),
            config: Arc::clone(&self.coord.config),
            look_ahead_segments: self.coord.look_ahead_segments,
            seek_epoch: self.seek_obs.epoch(),
            signal: self.coord.signal(),
        }
    }
}
