use std::num::{NonZeroU32, NonZeroUsize};

use firewheel_core::param::smoother::{SmoothedParam, SmootherConfig};
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunkInfo, AudioSpec};
use kithara_stretch::{ElasticCursor, ElasticEngine, ElasticError, ElasticSpanPlan, StretchKind};

use crate::{
    ActiveRegion, RegionPlan, RenderReader, RenderSnapshot, StretchControls, WarpConfig,
    temporal::RateTarget,
};

#[cfg(test)]
mod tests;

pub(super) struct PreparedExact {
    pub(super) next_speed: SmoothedParam,
    pub(super) plan: ElasticSpanPlan,
    #[cfg(feature = "probe")]
    pub(super) rate: RateTarget,
    pub(super) snapshot: Option<RenderSnapshot>,
    pub(super) speed: f32,
}

pub(super) enum PreparedQuantum {
    Exact(PreparedExact),
    Legacy {
        source_frames: usize,
        rate: RateTarget,
        snapshot: Option<RenderSnapshot>,
        speed: f32,
    },
}

impl PreparedQuantum {
    pub(super) fn bind(&mut self, snapshot: Option<RenderSnapshot>) {
        match self {
            Self::Exact(exact) => exact.snapshot = snapshot,
            Self::Legacy {
                snapshot: bound, ..
            } => *bound = snapshot,
        }
    }

    pub(super) fn snapshot(&self) -> Option<&RenderSnapshot> {
        match self {
            Self::Exact(exact) => exact.snapshot.as_ref(),
            Self::Legacy { snapshot, .. } => snapshot.as_ref(),
        }
    }

    #[cfg(feature = "probe")]
    pub(super) const fn rate(&self) -> RateTarget {
        match self {
            Self::Exact(exact) => exact.rate,
            Self::Legacy { rate, .. } => *rate,
        }
    }

    #[cfg(feature = "probe")]
    pub(super) const fn speed(&self) -> f32 {
        match self {
            Self::Exact(exact) => exact.speed,
            Self::Legacy { speed, .. } => *speed,
        }
    }
}

/// Source-timeline exact-span time-stretch driven by shared live controls.
/// Unity speed without a region plan is a byte-identical passthrough.
#[non_exhaustive]
pub struct WarpRenderer<S> {
    pub(super) context: RenderReader,
    pub(super) committed: Option<RenderSnapshot>,
    pub(super) controls: Arc<StretchControls>,
    pub(super) engine: Option<Box<dyn ElasticEngine>>,
    /// Engine displaced by a checked render failure. The scheduler shell
    /// drops it from `prepare`, outside `produce_tick_rt`.
    pub(super) retired_engine: Option<Box<dyn ElasticEngine>>,
    /// Most recent input meta, carried onto each output chunk.
    pub(super) last_input_meta: Option<AudioChunkInfo>,
    /// Exact source coordinate at which the current output scratch begins.
    pub(super) output_start_meta: Option<AudioChunkInfo>,
    /// Region plan cached from the controls; `Arc::ptr_eq` detects a live swap.
    pub(super) plan: Option<Arc<RegionPlan>>,
    /// Region covering the playhead - the lookup cursor. `None` forces a
    /// fresh binary search (first chunk, plan swap, region exit, seek).
    pub(super) region: Option<ActiveRegion>,
    /// Exact rate plan paired with the source quantum prepared by the scheduler.
    pub(super) prepared_quantum: Option<PreparedQuantum>,
    #[cfg(feature = "probe")]
    pub(super) last_committed_rate_revision: u64,
    /// Renderer-owned applied speed. Shared controls contain only the target.
    pub(super) applied_speed: SmoothedParam,
    /// Exact source coordinate committed with the applied speed after rendering.
    pub(super) exact_cursor: Option<ElasticCursor>,
    pub(super) pools: PoolRegion<S>,
    pub(super) spec: AudioSpec,
    /// Maximum output frames between samples of live temporal controls.
    pub(super) render_quantum_frames: NonZeroUsize,
    /// Engine kind currently prepared by the scheduler shell.
    pub(super) current_kind: StretchKind,
    /// Interleaved output scratch prepared by the scheduler shell. A produced
    /// chunk takes this buffer; the consumed input becomes its replacement.
    pub(super) scratch: Option<SampleBuffer>,
    /// Consumed input retained until the scheduler shell can resize or recycle
    /// it outside the checked render core.
    pub(super) deferred_scratch: Option<SampleBuffer>,
    /// Whether previous input ran through the backend. Once active, the
    /// resident engine also owns exact-unity rendering.
    pub(super) active: bool,
    /// Last pitch factor pushed to the backend; avoids redundant updates.
    pub(super) applied_pitch: f64,
    /// Fractional output frames retained across exact-span requests.
    pub(super) output_remainder: f64,
    /// Source whose cumulative output is still below one representable frame.
    /// Capacity is reserved from the injected pool before the render loop.
    pub(super) pending_source: Option<SampleBuffer>,
    /// Earliest metadata represented by `pending_source`.
    pub(super) pending_meta: Option<AudioChunkInfo>,
    /// Exact decoded-source boundary represented by the latest emitted chunk.
    pub(super) rendered_source_end: Option<(u64, NonZeroU32, Duration)>,
    /// Source frames admitted since the last renderer reset.
    pub(super) source_frames_admitted: u64,
    /// Reset requested by a timeline discontinuity. The scheduler shell
    /// performs it outside the checked render core.
    pub(super) reset_pending: bool,
    /// One scheduler-shell rebuild requested after a checked engine failure.
    /// The intent is consumed even when preparation fails.
    pub(super) rebuild_pending: bool,
}

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    pub(super) const MAX_OUTPUT_FRAMES: usize = 163_840;
    pub(super) const MAX_SOURCE_FRAMES: usize = 8192;
    pub(super) const RATE_SMOOTH_SECONDS: f32 = 0.002;
    /// Re-apply pitch to the backend only when it moves this much.
    pub(super) const RATIO_EPS: f64 = 1e-4;

    /// Build the slot at the source `spec`, driven by the shared `controls`.
    pub(crate) fn new(
        config: &WarpConfig,
        context: RenderReader,
        spec: AudioSpec,
        pools: PoolRegion<S>,
    ) -> Self {
        let controls = Arc::clone(config.stretch());
        let current_kind = controls.backend();
        let plan = controls.region_plan();
        let speed = controls.speed();
        let target = Self::prepare_target(
            current_kind,
            spec,
            &pools,
            config.render_quantum_frames(),
            None,
            None,
        );
        Self {
            context,
            committed: None,
            engine: target.engine,
            retired_engine: None,
            current_kind,
            controls,
            pools,
            spec,
            render_quantum_frames: config.render_quantum_frames(),
            prepared_quantum: None,
            #[cfg(feature = "probe")]
            last_committed_rate_revision: 0,
            applied_speed: SmoothedParam::new(
                speed,
                SmootherConfig {
                    smooth_seconds: Self::RATE_SMOOTH_SECONDS,
                    ..SmootherConfig::default()
                },
                spec.sample_rate,
            ),
            exact_cursor: None,
            applied_pitch: f64::NAN,
            active: false,
            output_remainder: 0.0,
            pending_source: target.pending_source,
            pending_meta: None,
            rendered_source_end: None,
            source_frames_admitted: 0,
            reset_pending: false,
            rebuild_pending: false,
            last_input_meta: None,
            output_start_meta: None,
            scratch: target.scratch,
            deferred_scratch: None,
            plan,
            region: None,
        }
    }

    /// Push `pitch` to the backend when it moved beyond `RATIO_EPS`.
    pub(super) fn apply_pitch(&mut self, pitch: f64) -> Result<(), ElasticError> {
        if !self.applied_pitch.is_nan() && (pitch - self.applied_pitch).abs() <= Self::RATIO_EPS {
            return Ok(());
        }
        let engine = self
            .engine
            .as_mut()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
        engine.set_pitch(pitch)?;
        self.applied_pitch = pitch;
        Ok(())
    }

    pub(super) fn clear_pending_source(&mut self) {
        if let Some(source) = self.pending_source.as_mut() {
            source.clear();
        }
        self.pending_meta = None;
    }

    pub(super) fn retire_engine(&mut self) {
        debug_assert!(self.retired_engine.is_none());
        self.retired_engine = self.engine.take();
        self.rebuild_pending = true;
    }

    pub(super) fn clear_render_state(&mut self) {
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }
        self.clear_pending_source();
        self.last_input_meta = None;
        self.output_start_meta = None;
        self.applied_pitch = f64::NAN;
        self.output_remainder = 0.0;
        self.prepared_quantum = None;
        self.exact_cursor = None;
        self.rendered_source_end = None;
        self.source_frames_admitted = 0;
        self.active = false;
        self.region = None;
    }

    pub(super) fn defer_scratch(&mut self, replacement: Option<SampleBuffer>) {
        if let Some(replacement) = replacement {
            debug_assert!(self.deferred_scratch.is_none());
            self.deferred_scratch = Some(replacement);
        }
    }

    pub(super) fn snap_speed(&mut self) {
        self.applied_speed.set_value(self.controls.speed());
        self.applied_speed.reset_to_target();
        self.prepared_quantum = None;
        self.exact_cursor = None;
    }

    /// Region covering `frame`, plus whether the playhead just crossed out
    /// of a previously resolved region (a plan boundary or a seek).
    pub(super) fn region_for(&mut self, frame: u64) -> ActiveRegion {
        if let Some(r) = self.region
            && r.contains(frame)
        {
            return r;
        }
        let next = self
            .plan
            .as_ref()
            .map_or(ActiveRegion::UNBOUNDED, |p| p.region_at(frame));
        self.region = Some(next);
        next
    }

    /// Pull the live region plan handle; on a swap drop the region cursor.
    pub(super) fn sync_plan(&mut self) {
        let want = self.controls.region_plan();
        let same = match (&self.plan, &want) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !same {
            self.plan = want;
            self.region = None;
            self.prepared_quantum = None;
            self.exact_cursor = None;
        }
    }

    pub(super) fn unity_passthrough(&self, speed: f32) -> bool {
        self.plan.is_none() && (speed - 1.0).abs() <= f32::EPSILON
    }

    pub(super) fn can_passthrough(&self, speed: f32) -> bool {
        let channels = usize::from(self.spec.channels.max(1));
        !self.active && self.pending_frames(channels) == 0 && self.unity_passthrough(speed)
    }

    pub(super) fn pending_frames(&self, channels: usize) -> usize {
        self.pending_source
            .as_deref()
            .map_or(0, |source| source.len() / channels)
    }

    /// Whether the renderer can accept another source chunk without dropping it.
    #[doc(hidden)]
    #[must_use]
    pub fn accepts_input(&self) -> bool {
        self.can_passthrough(self.controls.speed())
            || (self.engine.is_some() && self.pending_source.is_some() && self.scratch.is_some())
    }

    pub(super) fn held_source_frames(&self) -> u64 {
        if !self.active {
            return 0;
        }
        let pending = u64::try_from(self.pending_frames(usize::from(self.spec.channels.max(1))))
            .unwrap_or(u64::MAX);
        let backend_admitted = self.source_frames_admitted.saturating_sub(pending);
        let latency = self
            .engine
            .as_ref()
            .map_or(0, |engine| engine.capabilities().latency().source_frames());
        let backend_held = u64::try_from(latency)
            .unwrap_or(u64::MAX)
            .min(backend_admitted);
        pending.saturating_add(backend_held)
    }

    pub(super) fn record_rendered_source_end(
        &mut self,
        meta: AudioChunkInfo,
        held_source_frames: u64,
        timestamp: Duration,
    ) {
        let admitted = meta.frame_offset.saturating_add(u64::from(meta.frames));
        self.rendered_source_end = Some((
            admitted.saturating_sub(held_source_frames),
            meta.spec.sample_rate,
            timestamp,
        ));
    }

    pub(super) fn commit_snapshot(
        &mut self,
        snapshot: RenderSnapshot,
        output_frames: usize,
    ) -> Option<crate::SessionFrame> {
        let (source, _, _) = self.rendered_source_end?;
        let committed = snapshot.advance(self.committed.as_ref(), source, output_frames)?;
        let output_frames = i64::try_from(output_frames).ok()?;
        let output_start = i64::from(committed.frontier().output()).checked_sub(output_frames)?;
        self.committed = Some(committed);
        Some(crate::SessionFrame::new(output_start))
    }

    /// Last context and frontier committed by a successful worker quantum.
    #[doc(hidden)]
    #[must_use]
    pub fn render_snapshot(&self) -> Option<&RenderSnapshot> {
        self.committed.as_ref()
    }

    /// Exact decoded-source boundary represented by the latest emitted samples.
    #[doc(hidden)]
    #[must_use]
    pub const fn rendered_source_end(&self) -> Option<(u64, NonZeroU32)> {
        match self.rendered_source_end {
            Some((frame, sample_rate, _)) => Some((frame, sample_rate)),
            None => None,
        }
    }

    pub(super) fn meta_at_frame(meta: AudioChunkInfo, frame_offset: u64) -> AudioChunkInfo {
        let mut start = meta;
        let delta = frame_offset.saturating_sub(meta.frame_offset);
        start.frame_offset = frame_offset;
        start.timestamp = meta.timestamp.saturating_add(
            meta.spec
                .duration_for(delta)
                .unwrap_or(Duration::from_nanos(u64::MAX)),
        );
        if delta > 0 {
            start.source_byte_offset = None;
            start.source_bytes = 0;
        }
        start
    }
}
