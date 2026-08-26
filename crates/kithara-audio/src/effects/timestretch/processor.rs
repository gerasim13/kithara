#[cfg(test)]
use std::collections::HashSet;

#[path = "render.rs"]
mod render;

use kithara_bufpool::{PcmBuf, PcmPool};
use kithara_decode::{PcmChunk, PcmMeta, PcmSpec, duration_for_frames};
use kithara_platform::sync::Arc;
use kithara_stretch::{ElasticConfig, ElasticEngine, ElasticError, StretchKind, build_engine};
use num_traits::ToPrimitive;
use tracing::warn;

use super::StretchControls;
use crate::{
    region::{ActiveRegion, RegionPlan},
    traits::AudioEffect,
};

#[derive(Default)]
struct PreparedTarget {
    engine: Option<Box<dyn ElasticEngine>>,
    pending_source: Option<PcmBuf>,
    scratch: Option<PcmBuf>,
}

/// Source-timeline exact-span time-stretch driven by shared live controls.
/// Unity speed without a region plan is a byte-identical passthrough.
pub struct TimeStretchProcessor {
    controls: Arc<StretchControls>,
    engine: Option<Box<dyn ElasticEngine>>,
    /// Engine displaced by a checked render failure. The scheduler shell
    /// drops it from `service_deferred`, outside `produce_tick_rt`.
    retired_engine: Option<Box<dyn ElasticEngine>>,
    /// Most recent input meta, carried onto each output chunk.
    last_input_meta: Option<PcmMeta>,
    /// Exact source coordinate at which the current output scratch begins.
    output_start_meta: Option<PcmMeta>,
    /// Region plan cached from the controls; `Arc::ptr_eq` detects a live swap.
    plan: Option<Arc<RegionPlan>>,
    /// Region covering the playhead — the lookup cursor. `None` forces a
    /// fresh binary search (first chunk, plan swap, region exit, seek).
    region: Option<ActiveRegion>,
    pool: PcmPool,
    spec: PcmSpec,
    /// Engine kind currently prepared by the scheduler shell.
    current_kind: StretchKind,
    /// Interleaved output scratch prepared by the scheduler shell. A produced
    /// chunk takes this buffer; the consumed input becomes its replacement.
    scratch: Option<PcmBuf>,
    /// Consumed input retained until the scheduler shell can resize or recycle
    /// it outside the checked render core.
    deferred_scratch: Option<PcmBuf>,
    /// Whether previous input ran through the backend. Drives a clean backend
    /// reset when the processor returns to unity passthrough.
    active: bool,
    /// Last pitch factor pushed to the backend; avoids redundant updates.
    applied_pitch: f64,
    /// Fractional output frames retained across exact-span requests.
    output_remainder: f64,
    /// Source whose cumulative output is still below one representable frame.
    /// Capacity is reserved from the injected pool before the render loop.
    pending_source: Option<PcmBuf>,
    /// Earliest metadata represented by `pending_source`.
    pending_meta: Option<PcmMeta>,
    /// Reset requested by seek or a return to unity passthrough. The scheduler
    /// shell performs it outside the checked render core.
    reset_pending: bool,
}

impl TimeStretchProcessor {
    /// Floor for the shared playback speed before inverting to a stretch
    /// factor. At `speed = 0.05` the stretch is already 20x, beyond which
    /// time-stretch quality collapses, so there is no point clamping lower.
    const MIN_SPEED: f32 = 0.05;
    const MAX_OUTPUT_FRAMES: usize = 163_840;
    const MAX_SOURCE_FRAMES: usize = 8192;
    const OUTPUT_ROUNDING_MARGIN: f64 = 0.5;
    /// Re-apply pitch to the backend only when it moves this much.
    const RATIO_EPS: f64 = 1e-4;

    /// Build the slot at the source `spec`, driven by the shared `controls`.
    pub fn new(controls: Arc<StretchControls>, spec: PcmSpec, pool: PcmPool) -> Self {
        let current_kind = controls.backend();
        let plan = controls.region_plan();
        let target = Self::prepare_target(current_kind, spec, &pool, None, None);
        Self {
            engine: target.engine,
            retired_engine: None,
            current_kind,
            controls,
            pool,
            spec,
            applied_pitch: f64::NAN,
            active: false,
            output_remainder: 0.0,
            pending_source: target.pending_source,
            pending_meta: None,
            reset_pending: false,
            last_input_meta: None,
            output_start_meta: None,
            scratch: target.scratch,
            deferred_scratch: None,
            plan,
            region: None,
        }
    }

    fn prepare_target(
        kind: StretchKind,
        spec: PcmSpec,
        pool: &PcmPool,
        reusable_pending: Option<PcmBuf>,
        reusable_scratch: Option<PcmBuf>,
    ) -> PreparedTarget {
        let result = Self::config_for(kind, spec, pool)
            .and_then(build_engine)
            .and_then(|engine| {
                let channels = usize::from(spec.channels.max(1));
                let pending_samples = Self::MAX_SOURCE_FRAMES
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                let mut pending = reusable_pending.unwrap_or_else(|| pool.get());
                pending
                    .ensure_len(pending_samples)
                    .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
                pending.clear();
                let scratch_samples = Self::scratch_samples(engine.as_ref(), spec)?;
                let mut scratch = reusable_scratch.unwrap_or_else(|| pool.get());
                scratch
                    .ensure_len(scratch_samples)
                    .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
                scratch.clear();
                Ok((engine, pending, scratch))
            });
        match result {
            Ok((engine, pending, scratch)) => PreparedTarget {
                engine: Some(engine),
                pending_source: Some(pending),
                scratch: Some(scratch),
            },
            Err(error) => {
                warn!(%kind, %error, "time-stretch engine preparation failed");
                PreparedTarget::default()
            }
        }
    }

    /// Push `pitch` to the backend when it moved beyond `RATIO_EPS`.
    fn apply_pitch(&mut self, pitch: f64) -> Result<(), ElasticError> {
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

    fn clear_pending_source(&mut self) {
        if let Some(source) = self.pending_source.as_mut() {
            source.clear();
        }
        self.pending_meta = None;
    }

    fn retire_engine(&mut self) {
        debug_assert!(self.retired_engine.is_none());
        self.retired_engine = self.engine.take();
    }

    fn clear_render_state(&mut self) {
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }
        self.clear_pending_source();
        self.last_input_meta = None;
        self.output_start_meta = None;
        self.applied_pitch = f64::NAN;
        self.output_remainder = 0.0;
        self.active = false;
        self.region = None;
    }

    /// Assemble an output chunk from `scratch`, preserving the exact source
    /// start and the latest decoder frontier. `replacement` is retained for
    /// shell-side preparation before the next checked tick.
    fn emit(&mut self, replacement: Option<PcmBuf>) -> Option<PcmChunk> {
        let total = self.scratch.as_deref().map_or(0, <[f32]>::len);
        if total == 0 {
            self.defer_scratch(replacement);
            return None;
        }
        let channels = usize::from(self.spec.channels.max(1));
        let mut meta = self.last_input_meta.unwrap_or_default();
        // The output carries real audio, so its spec must be the live source
        // spec — never the `PcmMeta::default()` sentinel (channels 0, placeholder
        // rate) that `unwrap_or_default()` yields on a flush with no prior input
        // meta. A stretch only retimes; it preserves channels and sample rate.
        // Leaving the sentinel spec on a non-empty chunk breaks the downstream
        // `spec.channels > 0` chunk invariant (the resampler divides by it).
        meta.spec = self.spec;
        meta.frames = u32::try_from(total / channels).unwrap_or(u32::MAX);
        if let Some(start) = self.output_start_meta.take() {
            if start.frame_offset != meta.frame_offset {
                meta.source_byte_offset = None;
                meta.source_bytes = 0;
            }
            meta.frame_offset = start.frame_offset;
            meta.timestamp = start.timestamp;
        }
        let pcm = self.scratch.take()?;
        self.defer_scratch(replacement);
        Some(PcmChunk::new(meta, pcm))
    }

    fn defer_scratch(&mut self, replacement: Option<PcmBuf>) {
        if let Some(replacement) = replacement {
            debug_assert!(self.deferred_scratch.is_none());
            self.deferred_scratch = Some(replacement);
        }
    }

    fn config_for(
        backend: StretchKind,
        spec: PcmSpec,
        pool: &PcmPool,
    ) -> Result<ElasticConfig, ElasticError> {
        ElasticConfig::builder()
            .backend(backend)
            .sample_rate(spec.sample_rate.get())
            .channels(usize::from(spec.channels.max(1)))
            .pool(pool.clone())
            .max_source_frames(Self::MAX_SOURCE_FRAMES)
            .max_output_frames(Self::MAX_OUTPUT_FRAMES)
            .build()
    }

    fn scratch_samples(engine: &dyn ElasticEngine, spec: PcmSpec) -> Result<usize, ElasticError> {
        let capabilities = engine.capabilities();
        capabilities
            .max_output_frames()
            .max(capabilities.terminal_chunk_frames().saturating_add(1))
            .checked_mul(usize::from(spec.channels.max(1)))
            .ok_or(ElasticError::SampleCountOverflow)
    }

    fn service_scratch(&mut self) {
        if self.scratch.is_some() {
            drop(self.deferred_scratch.take());
            return;
        }
        let Some(engine) = self.engine.as_deref() else {
            drop(self.deferred_scratch.take());
            return;
        };
        let required = match Self::scratch_samples(engine, self.spec) {
            Ok(required) => required,
            Err(error) => {
                warn!(%error, "time-stretch output scratch sizing failed");
                drop(self.deferred_scratch.take());
                return;
            }
        };
        let mut scratch = self
            .deferred_scratch
            .take()
            .unwrap_or_else(|| self.pool.get());
        if scratch.ensure_len(required).is_err() {
            warn!("PCM pool budget exhausted while preparing time-stretch output scratch");
            return;
        }
        scratch.clear();
        self.scratch = Some(scratch);
    }

    /// Service backend/spec changes and deferred destruction from the
    /// scheduler shell, never from the checked render core.
    fn service_target(&mut self, spec: PcmSpec) {
        drop(self.retired_engine.take());
        self.sync_plan();

        let kind = self.controls.backend();
        if kind != self.current_kind || spec != self.spec {
            drop(self.deferred_scratch.take());
            self.clear_render_state();
            let reusable_pending = self.pending_source.take();
            let reusable_scratch = self.scratch.take();
            drop(self.engine.take());
            let target =
                Self::prepare_target(kind, spec, &self.pool, reusable_pending, reusable_scratch);
            self.engine = target.engine;
            self.pending_source = target.pending_source;
            self.scratch = target.scratch;
            self.current_kind = kind;
            self.spec = spec;
            self.reset_pending = false;
            return;
        }

        self.service_scratch();

        if !self.reset_pending {
            return;
        }
        self.reset_pending = false;
        if let Some(engine) = self.engine.as_mut()
            && let Err(error) = engine.reset()
        {
            warn!(%error, "time-stretch deferred reset failed");
            self.engine = None;
        }
    }

    /// Region covering `frame`, plus whether the playhead just crossed out
    /// of a previously resolved region (a plan boundary or a seek).
    fn region_for(&mut self, frame: u64) -> ActiveRegion {
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

    fn reset_for_passthrough(&mut self) {
        let has_pending = self
            .pending_source
            .as_deref()
            .is_some_and(|source| !source.is_empty());
        if !self.active && !has_pending {
            return;
        }
        self.reset_pending = self.active;
        self.clear_pending_source();
        self.applied_pitch = f64::NAN;
        self.output_remainder = 0.0;
        self.active = false;
    }

    /// Pull the live region plan handle; on a swap drop the region cursor.
    fn sync_plan(&mut self) {
        let want = self.controls.region_plan();
        let same = match (&self.plan, &want) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !same {
            self.plan = want;
            self.region = None;
        }
    }

    fn unity_passthrough(&self, speed: f32) -> bool {
        self.plan.is_none() && (speed - 1.0).abs() <= f32::EPSILON
    }

    fn source_block_limit(
        stretch: f64,
        max_source_frames: usize,
        max_output_frames: usize,
    ) -> Result<usize, ElasticError> {
        if !stretch.is_finite() || stretch <= 0.0 {
            return Err(ElasticError::InvalidRate(stretch));
        }
        let output_limit = max_output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_budget = (output_limit - Self::OUTPUT_ROUNDING_MARGIN).max(1.0);
        let source_limit = (output_budget / stretch)
            .floor()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let source_limit = source_limit.min(max_source_frames);
        if source_limit == 0 {
            return Err(ElasticError::InvalidRate(1.0 / stretch));
        }
        Ok(source_limit)
    }

    fn balanced_source_block(remaining: usize, limit: usize) -> usize {
        let partitions = remaining.div_ceil(limit);
        remaining.div_ceil(partitions)
    }

    fn pending_frames(&self, channels: usize) -> usize {
        self.pending_source
            .as_deref()
            .map_or(0, |source| source.len() / channels)
    }

    fn meta_at_frame(meta: PcmMeta, frame_offset: u64) -> PcmMeta {
        let mut start = meta;
        let delta = frame_offset.saturating_sub(meta.frame_offset);
        start.frame_offset = frame_offset;
        start.timestamp = meta
            .timestamp
            .saturating_add(duration_for_frames(meta.spec.sample_rate.get(), delta));
        if delta > 0 {
            start.source_byte_offset = None;
            start.source_bytes = 0;
        }
        start
    }

    fn process_unity(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        let channels = usize::from(self.spec.channels.max(1));
        if self.pending_frames(channels) == 0 {
            self.reset_for_passthrough();
            return Some(chunk);
        }

        let PcmChunk { meta, samples } = chunk;
        self.last_input_meta = Some(meta);
        self.output_start_meta = None;
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }
        let rounded = self.output_remainder.round().max(0.0).to_usize();
        match rounded {
            Some(0) => {
                self.reset_for_passthrough();
                Some(PcmChunk::new(meta, samples))
            }
            Some(1) => {
                let result = self
                    .render_terminal_pending(channels)
                    .and_then(|()| self.append_scratch(&samples, channels));
                if let Err(error) = result {
                    warn!(%error, "time-stretch transition to passthrough failed; dropping chunk");
                    self.retire_engine();
                    self.clear_render_state();
                    self.defer_scratch(Some(samples));
                    return None;
                }
                self.reset_for_passthrough();
                self.emit(Some(samples))
            }
            Some(frames) => {
                warn!(
                    frames,
                    "time-stretch pending span rounded outside one-frame bound"
                );
                self.retire_engine();
                self.clear_render_state();
                self.defer_scratch(Some(samples));
                None
            }
            None => {
                warn!("time-stretch pending span could not be represented");
                self.retire_engine();
                self.clear_render_state();
                self.defer_scratch(Some(samples));
                None
            }
        }
    }
}

impl AudioEffect for TimeStretchProcessor {
    fn service_deferred(&mut self, spec: PcmSpec) {
        self.service_target(spec);
    }

    fn flush(&mut self) -> Option<PcmChunk> {
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        } else {
            warn!("time-stretch output scratch was not serviced before flush");
            return None;
        }
        self.output_start_meta = None;
        let channels = usize::from(self.spec.channels.max(1));
        let result = self.render_terminal_pending(channels).and_then(|()| {
            if self.active {
                let tail_frames = self
                    .engine
                    .as_ref()
                    .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
                    .capabilities()
                    .terminal_chunk_frames();
                let tail_samples = tail_frames
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                let scratch = self
                    .scratch
                    .as_mut()
                    .ok_or(ElasticError::EnginePreparation(
                        "output scratch is unavailable",
                    ))?;
                let start = scratch.len();
                let end = start
                    .checked_add(tail_samples)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                if end > scratch.capacity() {
                    return Err(ElasticError::OutputFrameLimit {
                        frames: end / channels,
                        limit: scratch.capacity() / channels,
                    });
                }
                scratch
                    .ensure_len(end)
                    .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
                let rendered_frames = self
                    .engine
                    .as_mut()
                    .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
                    .flush(&mut scratch[start..end])?;
                if rendered_frames > tail_frames {
                    return Err(ElasticError::EngineOutputFrameCount {
                        actual: rendered_frames,
                        expected: tail_frames,
                    });
                }
                let rendered_samples = rendered_frames
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                scratch.truncate(start + rendered_samples);
            }
            Ok(())
        });
        if let Err(error) = result {
            warn!(%error, "time-stretch engine flush failed");
            self.retire_engine();
            self.clear_render_state();
            return None;
        }
        self.emit(None)
    }

    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        if chunk.spec() != self.spec {
            warn!(
                expected = %self.spec,
                actual = %chunk.spec(),
                "time-stretch target was not serviced before a format change"
            );
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let speed = self.controls.speed().max(Self::MIN_SPEED);
        if self.unity_passthrough(speed) {
            return self.process_unity(chunk);
        }
        if self.engine.is_none() || self.scratch.is_none() {
            warn!("time-stretch target was not prepared before rendering");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let PcmChunk { meta, samples } = chunk;
        self.last_input_meta = Some(meta);
        self.output_start_meta = None;
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }

        let channels = usize::from(self.spec.channels.max(1));
        let frames = samples.len() / channels;
        if frames > Self::MAX_SOURCE_FRAMES {
            let error = ElasticError::SourceFrameLimit {
                frames,
                limit: Self::MAX_SOURCE_FRAMES,
            };
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.defer_scratch(Some(samples));
            return None;
        }
        let rendered = self.render_active(meta, &samples, speed, channels, frames);
        if let Err(error) = rendered {
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(samples));
            return None;
        }
        self.emit(Some(samples))
    }

    fn reset(&mut self) {
        self.reset_pending = true;
        self.clear_render_state();
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use kithara_bufpool::{ByteBudget, PcmPool};
    use kithara_decode::{PcmMeta, PcmSpec};
    use kithara_platform::time::Duration;
    use kithara_test_utils::kithara;
    use realfft::RealFftPlanner;

    use super::*;
    use crate::region::GridSegment;

    struct Consts;

    impl Consts {
        const CH: u16 = 2;
        const F0: f64 = 440.0;
        /// FFT length for the pitch (dominant-frequency) check.
        const N: usize = 1 << 14;
        const SR: u32 = 44_100;
    }

    fn f32_of(x: f64) -> f32 {
        num_traits::cast(x).unwrap_or_default()
    }

    fn f64_of(x: usize) -> f64 {
        num_traits::cast(x).unwrap_or_default()
    }

    /// Interleaved stereo sine at `F0`, phase-accumulated to avoid drift.
    fn sine(frames: usize) -> Vec<f32> {
        let inc = std::f64::consts::TAU * Consts::F0 / f64::from(Consts::SR);
        let mut phase = 0.0_f64;
        let mut out = Vec::with_capacity(frames * usize::from(Consts::CH));
        for _ in 0..frames {
            let s = f32_of(0.5 * phase.sin());
            out.push(s);
            out.push(s);
            phase += inc;
        }
        out
    }

    fn chunk(samples: &[f32]) -> PcmChunk {
        let frames = samples.len() / usize::from(Consts::CH);
        PcmChunk::new(
            PcmMeta {
                spec: PcmSpec {
                    channels: Consts::CH,
                    sample_rate: NonZero::new(Consts::SR).unwrap(),
                },
                frames: u32::try_from(frames).unwrap_or(0),
                timestamp: Duration::ZERO,
                ..Default::default()
            },
            PcmPool::default().attach(samples.to_vec()),
        )
    }

    /// Index of the strongest spectral bin (skipping DC) of a mono window
    /// taken from the middle of `mono`.
    fn dominant_bin(mono: &[f32]) -> usize {
        let start = (mono.len().saturating_sub(Consts::N)) / 2;
        let seg = &mono[start..start + Consts::N];
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(Consts::N);
        let mut input = fft.make_input_vec();
        input.copy_from_slice(seg);
        let mut spectrum = fft.make_output_vec();
        fft.process(&mut input, &mut spectrum).unwrap();
        spectrum
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
            .map_or(0, |(i, _)| i)
    }

    fn expected_bin(freq: f64) -> usize {
        num_traits::cast((freq * f64_of(Consts::N) / f64::from(Consts::SR)).round()).unwrap_or(0)
    }

    fn spec() -> PcmSpec {
        PcmSpec {
            channels: Consts::CH,
            sample_rate: NonZero::new(Consts::SR).unwrap(),
        }
    }

    fn processor(controls: Arc<StretchControls>) -> TimeStretchProcessor {
        TimeStretchProcessor::new(controls, spec(), PcmPool::default())
    }

    fn process_serviced(fx: &mut TimeStretchProcessor, input: PcmChunk) -> Option<PcmChunk> {
        fx.service_deferred(spec());
        let output = fx.process(input);
        fx.service_deferred(spec());
        output
    }

    fn flush_serviced(fx: &mut TimeStretchProcessor) -> Option<PcmChunk> {
        fx.service_deferred(spec());
        let output = fx.flush();
        fx.service_deferred(spec());
        output
    }

    #[kithara::test]
    fn exact_output_frames_do_not_drift_across_partitions() {
        let stretch = 1.0 / 1.3;
        let partitions = [127, 509, 2048, 17, 4096];
        let mut remainder = 0.0;
        let mut actual = 0;
        for frames in partitions {
            let (output, next_remainder) =
                TimeStretchProcessor::output_frames(frames, stretch, remainder)
                    .expect("invariant: finite positive stretch");
            actual += output;
            remainder = next_remainder;
        }
        let source_frames = partitions.into_iter().sum::<usize>();
        let expected = (f64_of(source_frames) * stretch)
            .round()
            .to_usize()
            .expect("invariant: fixture output span fits usize");

        assert_eq!(actual, expected);
        assert_eq!(
            TimeStretchProcessor::balanced_source_block(8193, 8192),
            4097
        );

        let mut remainder = 0.0;
        let actual = [1, 1, 4096]
            .into_iter()
            .map(|frames| {
                let (output, next_remainder) =
                    TimeStretchProcessor::output_frames(frames, 0.5, remainder)
                        .expect("singleton spans retain their quantization debt");
                remainder = next_remainder;
                output
            })
            .sum::<usize>();
        assert_eq!(actual, 2049);

        let mut remainder = 0.0;
        let outputs = [1, 1, 1, 1].map(|frames| {
            let (output, next_remainder) =
                TimeStretchProcessor::output_frames(frames, 0.25, remainder)
                    .expect("four sub-frame spans form one exact output frame");
            remainder = next_remainder;
            output
        });
        assert_eq!(outputs, [0, 0, 0, 1]);
        assert_eq!(remainder, 0.0);
    }

    #[kithara::test]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
    fn one_frame_regions_accumulate_into_one_portable_request(#[case] backend: StretchKind) {
        let controls = StretchControls::new(1.0);
        controls.set_keylock(true);
        controls.set_backend(backend);
        controls.set_region_plan(Some(Arc::new(
            RegionPlan::new(vec![
                GridSegment::new(0, 1, 0.125),
                GridSegment::new(1, 2, 0.25),
                GridSegment::new(2, 3, 0.125),
                GridSegment::new(3, 4, 0.5),
            ])
            .expect("one-frame regions are ordered and non-empty"),
        )));
        let mut fx = processor(controls);
        let source = sine(4);

        for frame in 0..3_u64 {
            let start = usize::try_from(frame).unwrap_or_default() * usize::from(Consts::CH);
            let mut input = chunk(&source[start..start + usize::from(Consts::CH)]);
            input.meta.frame_offset = frame;
            assert!(process_serviced(&mut fx, input).is_none());
        }

        let mut input = chunk(&source[3 * usize::from(Consts::CH)..]);
        input.meta.frame_offset = 3;
        let output = process_serviced(&mut fx, input)
            .expect("the fourth source frame completes one output frame");
        assert_eq!(output.frames(), 1);
        assert_eq!(output.meta.frame_offset, 0);
        let mut tail_chunks = 0;
        while let Some(tail) = flush_serviced(&mut fx) {
            assert!(tail.frames() > 0, "a flush chunk contains real frames");
            assert_eq!(tail.spec(), spec());
            tail_chunks += 1;
            assert!(tail_chunks < 32, "terminal drain must converge");
        }
        assert!(
            tail_chunks > 0,
            "an active engine exposes its terminal tail"
        );
    }

    #[kithara::test]
    fn pending_span_uses_earliest_start_and_latest_frontier() {
        let controls = StretchControls::new(1.0);
        controls.set_keylock(true);
        controls.set_region_plan(Some(Arc::new(
            RegionPlan::new(vec![
                GridSegment::new(0, 1, 1.0),
                GridSegment::new(1, 2, 0.75),
                GridSegment::new(2, 3, 0.25),
            ])
            .expect("fixture regions are contiguous"),
        )));
        let mut fx = processor(controls);
        let source = sine(3);
        let mut first = chunk(&source[..2 * usize::from(Consts::CH)]);
        first.meta.end_timestamp = Duration::from_millis(20);
        first.meta.segment_index = Some(1);
        first.meta.variant_index = Some(1);
        first.meta.epoch = 1;
        first.meta.source_byte_offset = Some(10);
        first.meta.source_bytes = 20;
        let first_output = process_serviced(&mut fx, first).expect("first frame renders");

        let mut second = chunk(&source[2 * usize::from(Consts::CH)..]);
        second.meta.frame_offset = 2;
        second.meta.timestamp = Duration::from_millis(20);
        second.meta.end_timestamp = Duration::from_millis(30);
        second.meta.segment_index = Some(2);
        second.meta.variant_index = Some(2);
        second.meta.epoch = 2;
        second.meta.source_byte_offset = Some(30);
        second.meta.source_bytes = 10;
        let second_output =
            process_serviced(&mut fx, second).expect("pending span completes on the next chunk");

        assert!(first_output.meta.end_timestamp < second_output.meta.end_timestamp);
        assert_eq!(second_output.meta.frame_offset, 1);
        assert_eq!(
            second_output.meta.timestamp,
            duration_for_frames(Consts::SR, 1)
        );
        assert_eq!(second_output.meta.end_timestamp, Duration::from_millis(30));
        assert_eq!(second_output.meta.segment_index, Some(2));
        assert_eq!(second_output.meta.variant_index, Some(2));
        assert_eq!(second_output.meta.epoch, 2);
        assert_eq!(second_output.meta.source_byte_offset, None);
        assert_eq!(second_output.meta.source_bytes, 0);
    }

    #[kithara::test]
    fn pending_span_is_committed_before_live_unity_passthrough() {
        let controls = StretchControls::new(1.0);
        controls.set_keylock(true);
        controls.set_region_plan(Some(Arc::new(
            RegionPlan::new(vec![GridSegment::new(0, 1, 0.75)]).expect("fixture region is valid"),
        )));
        let mut fx = processor(Arc::clone(&controls));
        let source = sine(3);
        let mut pending = chunk(&source[..usize::from(Consts::CH)]);
        pending.meta.end_timestamp = Duration::from_millis(10);
        assert!(process_serviced(&mut fx, pending).is_none());

        controls.set_region_plan(None);
        let mut unity = chunk(&source[usize::from(Consts::CH)..2 * usize::from(Consts::CH)]);
        unity.meta.frame_offset = 1;
        unity.meta.timestamp = Duration::from_millis(10);
        unity.meta.end_timestamp = Duration::from_millis(20);
        let transition = process_serviced(&mut fx, unity)
            .expect("rounded pending frame precedes the unity frame");
        assert_eq!(transition.frames(), 2);
        assert_eq!(transition.meta.frame_offset, 0);
        assert_eq!(transition.meta.end_timestamp, Duration::from_millis(20));

        let mut next = chunk(&source[2 * usize::from(Consts::CH)..]);
        next.meta.frame_offset = 2;
        let next_samples = next.samples.to_vec();
        let passthrough = process_serviced(&mut fx, next).expect("unity remains zero-copy");
        assert_eq!(&passthrough.samples[..], &next_samples);
        assert!(flush_serviced(&mut fx).is_none());
    }

    #[kithara::test]
    fn negative_rounding_debt_adds_no_frame_at_unity_transition() {
        let controls = StretchControls::new(1.0);
        controls.set_keylock(true);
        controls.set_region_plan(Some(Arc::new(
            RegionPlan::new(vec![
                GridSegment::new(0, 1, 1.6),
                GridSegment::new(1, 2, 0.25),
            ])
            .expect("fixture regions are contiguous"),
        )));
        let mut fx = processor(Arc::clone(&controls));
        let source = sine(3);
        let first = process_serviced(&mut fx, chunk(&source[..usize::from(Consts::CH)]))
            .expect("the first span rounds to two frames");
        assert_eq!(first.frames(), 2);

        let mut debt = chunk(&source[usize::from(Consts::CH)..2 * usize::from(Consts::CH)]);
        debt.meta.frame_offset = 1;
        assert!(process_serviced(&mut fx, debt).is_none());

        controls.set_region_plan(None);
        let mut unity = chunk(&source[2 * usize::from(Consts::CH)..]);
        unity.meta.frame_offset = 2;
        let expected = unity.samples.to_vec();
        let output = process_serviced(&mut fx, unity).expect("unity chunk passes through");
        assert_eq!(output.frames(), 1);
        assert_eq!(&output.samples[..], &expected);
    }

    #[kithara::test]
    fn reset_discards_pending_span_before_new_timeline() {
        let controls = StretchControls::new(1.0);
        controls.set_keylock(true);
        controls.set_region_plan(Some(Arc::new(
            RegionPlan::new(vec![GridSegment::new(0, 1, 0.75)]).expect("fixture region is valid"),
        )));
        let mut fx = processor(Arc::clone(&controls));
        let source = sine(2);
        assert!(process_serviced(&mut fx, chunk(&source[..usize::from(Consts::CH)])).is_none());

        fx.reset();
        controls.set_region_plan(None);
        fx.service_deferred(spec());
        let mut landed = chunk(&source[usize::from(Consts::CH)..]);
        landed.meta.frame_offset = 100;
        landed.meta.timestamp = Duration::from_secs(1);
        landed.meta.end_timestamp = Duration::from_millis(1_010);
        let expected = landed.samples.to_vec();
        let output = process_serviced(&mut fx, landed).expect("post-seek unity passes through");
        assert_eq!(output.meta.frame_offset, 100);
        assert_eq!(output.meta.timestamp, Duration::from_secs(1));
        assert_eq!(&output.samples[..], &expected);
    }

    fn keylocked(kind: StretchKind, speed: f32) -> TimeStretchProcessor {
        let controls = StretchControls::new(speed);
        controls.set_keylock(true);
        controls.set_backend(kind);
        processor(controls)
    }

    fn vinyl(kind: StretchKind, speed: f32) -> TimeStretchProcessor {
        let controls = StretchControls::new(speed);
        controls.set_keylock(false);
        controls.set_backend(kind);
        processor(controls)
    }

    fn render_with_tail(fx: &mut TimeStretchProcessor, input: &[f32]) -> (Vec<f32>, usize) {
        let mut out: Vec<f32> = Vec::new();
        let mut tail_frames = 0;
        let block = 4096 * usize::from(Consts::CH);
        for data in input.chunks(block) {
            if let Some(c) = process_serviced(fx, chunk(data)) {
                assert_eq!(
                    c.spec().sample_rate.get(),
                    Consts::SR,
                    "stretch preserves sample rate"
                );
                assert_eq!(c.spec().channels, Consts::CH);
                out.extend_from_slice(&c.samples);
            }
        }
        while let Some(c) = flush_serviced(fx) {
            // A non-empty flush chunk carries real audio, so its spec must stay
            // the source spec — never the `PcmMeta::default()` sentinel (0
            // channels) that a `None` `last_input_meta` would otherwise yield.
            assert_eq!(c.spec().channels, Consts::CH, "flush preserves channels");
            assert_eq!(
                c.spec().sample_rate.get(),
                Consts::SR,
                "flush preserves sample rate"
            );
            tail_frames += c.frames();
            out.extend_from_slice(&c.samples);
        }
        (out, tail_frames)
    }

    fn render(fx: &mut TimeStretchProcessor, input: &[f32]) -> Vec<f32> {
        render_with_tail(fx, input).0
    }

    fn run_keylocked_with_tail(
        kind: StretchKind,
        speed: f32,
        in_frames: usize,
    ) -> (Vec<f32>, usize) {
        let input = sine(in_frames);
        render_with_tail(&mut keylocked(kind, speed), &input)
    }

    fn run_vinyl(kind: StretchKind, speed: f32, in_frames: usize) -> Vec<f32> {
        let input = sine(in_frames);
        render(&mut vinyl(kind, speed), &input)
    }

    /// Half playback speed -> stretch 2.0 -> ~double duration, pitch held.
    /// Shared across every compiled-in backend.
    fn assert_half_speed_contract(kind: StretchKind) {
        let channels = usize::from(Consts::CH);
        let in_frames = usize::try_from(Consts::SR).unwrap() * 2; // 2 s
        let (out, tail_frames) = run_keylocked_with_tail(kind, 0.5, in_frames);
        let out_frames = out.len() / channels;
        let timeline_frames = out_frames - tail_frames;
        let expected_timeline = in_frames * 2;

        assert_eq!(
            timeline_frames, expected_timeline,
            "{kind:?}: exact half-speed timeline"
        );
        assert!(tail_frames > 0, "{kind:?}: terminal history is drained");

        // Pitch is still measured over the complete emitted stream, including
        // the latency fill and its matching terminal drain.
        assert!(
            out_frames >= expected_timeline,
            "{kind:?}: terminal drain cannot shorten the exact timeline"
        );

        // Pitch preserved: dominant bin still at F0 (the load-bearing check —
        // a resampler-in-disguise would shift it).
        let mono: Vec<f32> = out.iter().step_by(channels).copied().collect();
        assert!(
            mono.len() >= Consts::N,
            "{kind:?}: not enough output for the FFT window"
        );
        let peak = dominant_bin(&mono);
        let want = expected_bin(Consts::F0);
        assert!(
            peak.abs_diff(want) <= 3,
            "{kind:?}: pitch moved under time-stretch: peak bin {peak}, expected {want}"
        );
    }

    fn assert_unity_contract(kind: StretchKind) {
        let in_frames = usize::try_from(Consts::SR).unwrap() * 2;
        let input = sine(in_frames);
        let out = render(&mut keylocked(kind, 1.0), &input);
        assert_eq!(out, input, "{kind:?}: unity speed must bypass byte-exact");
    }

    #[kithara::test]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
    fn half_speed_and_unity_contracts(#[case] backend: StretchKind) {
        assert_half_speed_contract(backend);
        assert_unity_contract(backend);
    }

    #[kithara::test]
    fn output_meta_preserves_decoder_timeline() {
        let channels = usize::from(Consts::CH);
        let mut fx = keylocked(StretchKind::default(), 0.5);
        let cf = 1024usize;
        let block = sine(cf);
        let mut fed_ends = HashSet::new();
        let mut emitted = Vec::new();
        for i in 0..40u64 {
            let mut c = chunk(&block);
            let end = Duration::from_millis(i * 100 + 100);
            c.meta.timestamp = Duration::from_millis(i * 100);
            c.meta.end_timestamp = end;
            c.meta.frame_offset = i * u64::try_from(cf).unwrap();
            fed_ends.insert(end);
            if let Some(o) = process_serviced(&mut fx, c) {
                emitted.push(o);
            }
        }
        while let Some(o) = flush_serviced(&mut fx) {
            emitted.push(o);
        }
        assert!(!emitted.is_empty(), "stretch produced no output");
        for o in &emitted {
            assert_eq!(
                o.spec(),
                PcmSpec {
                    channels: Consts::CH,
                    sample_rate: NonZero::new(Consts::SR).unwrap()
                },
                "spec (incl. sample rate) preserved verbatim"
            );
            assert_eq!(
                usize::try_from(o.meta.frames).unwrap(),
                o.samples.len() / channels,
                "frames recomputed to the actual output count"
            );
            assert!(
                fed_ends.contains(&o.meta.end_timestamp),
                "end_timestamp carried verbatim from an input chunk (source-track time)"
            );
        }
    }

    /// Key-lock off is vinyl mode: speed changes duration and pitch in the
    /// stretch slot, with no resampler-rate handoff.
    #[kithara::test]
    fn vinyl_speed_scales_duration_and_pitch() {
        let channels = usize::from(Consts::CH);
        let in_frames = usize::try_from(Consts::SR).unwrap() * 2;
        let out = run_vinyl(StretchKind::default(), 2.0, in_frames);
        let out_frames = out.len() / channels;
        assert!(
            out_frames * 10 >= in_frames * 4 && out_frames * 10 <= in_frames * 6,
            "vinyl 2x should roughly halve duration, got {out_frames} from {in_frames}"
        );
        let mono: Vec<f32> = out.iter().step_by(channels).copied().collect();
        assert!(
            mono.len() >= Consts::N,
            "not enough vinyl output for the FFT window"
        );
        let peak = dominant_bin(&mono);
        let want = expected_bin(Consts::F0 * 2.0);
        assert!(
            peak.abs_diff(want) <= 4,
            "vinyl pitch did not follow speed: peak bin {peak}, expected {want}"
        );
    }

    #[kithara::test]
    fn live_speed_change_updates_stretch_duration() {
        let controls = StretchControls::new(1.0);
        controls.set_keylock(true);
        controls.set_backend(StretchKind::default());
        let mut fx = processor(Arc::clone(&controls));
        let block = sine(4096);
        let unity = process_serviced(&mut fx, chunk(&block)).expect("unity bypass emits");
        assert_eq!(&unity.samples[..], &block[..], "unity phase bypasses");

        controls.set_speed(0.5);
        let mut stretched: Vec<f32> = Vec::new();
        for _ in 0..24 {
            if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
                stretched.extend_from_slice(&c.samples);
            }
        }
        while let Some(c) = flush_serviced(&mut fx) {
            stretched.extend_from_slice(&c.samples);
        }
        assert!(
            stretched.len() > block.len() * 24,
            "half-speed key-lock should lengthen output after a live speed change"
        );
    }

    /// Flipping key-lock mid-stream switches from vinyl pitch shift to
    /// pitch-preserving stretch — no reload.
    #[kithara::test]
    fn live_keylock_toggle_switches_pitch_mode() {
        let controls = StretchControls::new(0.5);
        controls.set_keylock(false);
        controls.set_backend(StretchKind::default());
        let mut fx = processor(Arc::clone(&controls));
        let block = sine(4096);

        let mut vinyl_out: Vec<f32> = Vec::new();
        for _ in 0..24 {
            if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
                vinyl_out.extend_from_slice(&c.samples);
            }
        }
        let vinyl_mono: Vec<f32> = vinyl_out
            .iter()
            .step_by(usize::from(Consts::CH))
            .copied()
            .collect();
        assert!(
            dominant_bin(&vinyl_mono).abs_diff(expected_bin(Consts::F0 * 0.5)) <= 4,
            "off: vinyl pitch follows speed"
        );

        controls.set_keylock(true);
        let mut stretched: Vec<f32> = Vec::new();
        for _ in 0..24 {
            if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
                stretched.extend_from_slice(&c.samples);
            }
        }
        while let Some(c) = flush_serviced(&mut fx) {
            stretched.extend_from_slice(&c.samples);
        }
        let mono: Vec<f32> = stretched
            .iter()
            .step_by(usize::from(Consts::CH))
            .copied()
            .collect();
        assert!(
            mono.len() >= Consts::N,
            "on: not enough output for the FFT window"
        );
        assert!(
            dominant_bin(&mono).abs_diff(expected_bin(Consts::F0)) <= 3,
            "on: pitch preserved after live toggle"
        );
    }

    /// Swapping the backend mid-stream keeps the stream flowing and pitch-locked.
    #[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[kithara::test]
    fn live_backend_swap_continues_and_keeps_pitch() {
        let controls = StretchControls::new(0.5);
        controls.set_keylock(true);
        controls.set_backend(StretchKind::Bungee);
        let mut fx = processor(Arc::clone(&controls));
        let block = sine(4096);
        let mut out: Vec<f32> = Vec::new();
        for i in 0..24 {
            if i == 6 {
                controls.set_backend(StretchKind::Signalsmith);
                fx.service_deferred(spec());
            }
            if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
                out.extend_from_slice(&c.samples);
            }
        }
        while let Some(c) = flush_serviced(&mut fx) {
            out.extend_from_slice(&c.samples);
        }
        let mono: Vec<f32> = out
            .iter()
            .step_by(usize::from(Consts::CH))
            .copied()
            .collect();
        assert!(
            mono.len() >= Consts::N,
            "not enough output after swap for the FFT window"
        );
        assert!(
            dominant_bin(&mono).abs_diff(expected_bin(Consts::F0)) <= 3,
            "pitch preserved after live backend swap"
        );
    }

    #[cfg(feature = "stretch-signalsmith")]
    #[kithara::test]
    fn target_rebuild_reuses_one_target_pool_budget() {
        let initial = spec();
        let rebuilt = PcmSpec {
            sample_rate: NonZero::new(48_000).unwrap(),
            ..initial
        };
        let target_bytes = [initial, rebuilt]
            .map(|target_spec| {
                let pool = PcmPool::new(8, 0);
                let controls = StretchControls::new(0.5);
                controls.set_keylock(true);
                controls.set_backend(StretchKind::Signalsmith);
                let target = TimeStretchProcessor::new(controls, target_spec, pool.clone());
                assert!(target.engine.is_some());
                pool.stats().allocated_bytes
            })
            .into_iter()
            .max()
            .expect("the target matrix is non-empty");

        let pool = PcmPool::with_byte_budget(8, 0, ByteBudget(target_bytes));
        let controls = StretchControls::new(0.5);
        controls.set_keylock(true);
        controls.set_backend(StretchKind::Signalsmith);
        let mut fx = TimeStretchProcessor::new(controls, initial, pool.clone());
        assert!(fx.engine.is_some());
        assert!(fx.pending_source.is_some());
        assert!(fx.scratch.is_some());

        let overshoots = pool.stats().budget_overshoots;
        fx.service_deferred(rebuilt);

        assert_eq!(fx.spec, rebuilt);
        assert!(fx.engine.is_some());
        assert!(fx.pending_source.is_some());
        assert!(fx.scratch.is_some());
        assert_eq!(pool.stats().budget_overshoots, overshoots);
    }
}
