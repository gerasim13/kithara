use std::{
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
};

use kithara_bufpool::{HasPool, SampleBuffer};
use kithara_dsp::filter::{Biquad, FilterError, rbj};
use num_traits::cast::ToPrimitive;
use smallvec::SmallVec;

use super::GlideInterpolation;
use crate::{ResamplerBuildError, ResamplerError, ResamplerMode, ResamplerSettings};

pub(in crate::glide) struct RenderRequest<'a, I, O> {
    pub(in crate::glide) input: &'a [I],
    pub(in crate::glide) output: &'a mut [O],
    /// Peak rate of the block when the anti-alias filter runs.
    pub(in crate::glide) filter_ratio: Option<f64>,
    pub(in crate::glide) produced: usize,
    pub(in crate::glide) consumed: usize,
}

mod consts {
    pub(super) const CUTOFF_TO_NYQUIST: f64 = 0.9;
    pub(super) const LOW_PASS_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
    pub(super) const FILTER_OP: &str = "glide anti-alias filter";
    pub(super) const INPUT_OP: &str = "glide input";
    pub(super) const POSITIONS_OP: &str = "glide positions";
}

/// Per channel the window is `[history | input | tail]`: history is the last
/// consumed frame, the tail repeats the last input frame. With the filter
/// on, the persistent filter advances over consumed frames only and a
/// lookahead copy filters the rest, so the output does not depend on how
/// the stream is chunked.
#[derive(fieldwork::Fieldwork)]
pub(in crate::glide) struct GlideEngine {
    windows: SmallVec<[SampleBuffer; 8]>,
    positions: SampleBuffer,
    levels: SampleBuffer,
    filter: Biquad,
    lookahead: Biquad,
    tuned: Option<f64>,
    seeded: bool,
    interpolation: GlideInterpolation,
    sample_rate: f64,
    max_input_frames: usize,
    #[field(get(copy, name = position_capacity, vis = "pub(in crate::glide)"))]
    max_output_frames: usize,
}

impl GlideEngine {
    pub(in crate::glide) fn new<S>(
        settings: &ResamplerSettings<S>,
        interpolation: GlideInterpolation,
        backend: &'static str,
    ) -> Result<Self, ResamplerBuildError>
    where
        S: HasPool<f32>,
    {
        let pools = &settings.pools;
        let channels = settings.channels;
        let max_input_frames = settings.options.chunk_size;
        let max_output_frames =
            max_output_frames(max_input_frames, settings.options.max_ratio_adjustment);
        let mut positions = pools.get::<f32>();
        ensure_build_len(&mut positions, max_output_frames, backend)?;
        let mut levels = pools.get::<f32>();
        ensure_build_len(&mut levels, channels.get(), backend)?;
        let mut windows = SmallVec::new();
        for _ in 0..channels.get() {
            let mut window = pools.get::<f32>();
            ensure_build_len(&mut window, max_input_frames.saturating_add(2), backend)?;
            window.fill(0.0);
            windows.push(window);
        }
        Ok(Self {
            windows,
            positions,
            levels,
            filter: low_pass_filter(channels, backend)?,
            lookahead: low_pass_filter(channels, backend)?,
            tuned: None,
            seeded: false,
            interpolation,
            sample_rate: sample_rate(settings.mode),
            max_input_frames,
            max_output_frames,
        })
    }

    /// Starts the history at the first input frame of a fresh stream.
    pub(in crate::glide) fn seed<I: Deref<Target = [f32]>>(&mut self, input: &[I]) {
        if self.seeded {
            return;
        }
        for (window, input) in self.windows.iter_mut().zip(input) {
            window[0] = input.first().copied().unwrap_or(0.0);
        }
        self.seeded = true;
    }

    /// Keeps the history in step with an unfiltered passthrough block; the
    /// filter re-enters through `settle`.
    pub(in crate::glide) fn pass(&mut self, input: &[&[f32]], produced: usize) {
        if let Some(last) = produced.checked_sub(1) {
            for (window, input) in self.windows.iter_mut().zip(input) {
                window[0] = input[last];
            }
        }
        self.tuned = None;
    }

    pub(in crate::glide) fn positions_mut(
        &mut self,
        frames: usize,
    ) -> Result<&mut [f32], ResamplerError> {
        if frames > self.max_output_frames {
            return Err(ResamplerError::Backend {
                op: consts::POSITIONS_OP,
                detail: "output frame request exceeds preallocated position buffer".into(),
            });
        }
        Ok(&mut self.positions[..frames])
    }

    pub(in crate::glide) fn render<I, O>(
        &mut self,
        request: RenderRequest<'_, I, O>,
    ) -> Result<(), ResamplerError>
    where
        I: Deref<Target = [f32]>,
        O: DerefMut<Target = [f32]>,
    {
        let RenderRequest {
            input,
            output,
            filter_ratio,
            produced,
            consumed,
        } = request;
        let frames = input.first().map_or(0, |channel| channel.deref().len());
        if frames > self.max_input_frames {
            return Err(ResamplerError::Backend {
                op: consts::INPUT_OP,
                detail: "input frame count exceeds preallocated source buffer".into(),
            });
        }
        let end = frames.saturating_add(1);
        for (window, source) in self.windows.iter_mut().zip(input) {
            window[1..end].copy_from_slice(source.deref());
        }
        match filter_ratio {
            Some(ratio) => self.filter_window(ratio, consumed, end)?,
            None => self.tuned = None,
        }
        for (window, target) in self.windows.iter_mut().zip(output.iter_mut()) {
            window[end] = window[frames];
            backend::interpolate(
                self.interpolation,
                &window[..=end],
                &self.positions[..produced],
                &mut target.deref_mut()[..produced],
            );
            if consumed > 0 {
                window[0] = window[consumed];
            }
        }
        Ok(())
    }

    pub(in crate::glide) fn reset(&mut self) {
        self.seeded = false;
        self.tuned = None;
    }

    fn filter_window(
        &mut self,
        ratio: f64,
        consumed: usize,
        end: usize,
    ) -> Result<(), ResamplerError> {
        self.tune(ratio)?;
        let split = consumed.saturating_add(1);
        self.filter
            .process(&mut self.windows[..], 1..split)
            .map_err(filter_error)?;
        self.lookahead
            .copy_state(&self.filter)
            .map_err(filter_error)?;
        self.lookahead
            .process(&mut self.windows[..], split..end)
            .map_err(filter_error)
    }

    /// Retunes both filters when the cutoff moves; entering filtering
    /// settles the persistent filter on the history frame.
    fn tune(&mut self, ratio: f64) -> Result<(), ResamplerError> {
        let cutoff = low_pass_cutoff(self.sample_rate, ratio);
        if self.tuned == Some(cutoff) {
            return Ok(());
        }
        let low_pass =
            rbj::low_pass(self.sample_rate, cutoff, consts::LOW_PASS_Q).map_err(filter_error)?;
        self.filter.retune(0, low_pass).map_err(filter_error)?;
        self.lookahead.retune(0, low_pass).map_err(filter_error)?;
        if self.tuned.is_none() {
            let channels = self.windows.len();
            for (level, window) in self.levels.iter_mut().zip(&self.windows) {
                *level = window[0];
            }
            self.filter
                .settle(&self.levels[..channels])
                .map_err(filter_error)?;
        }
        self.tuned = Some(cutoff);
        Ok(())
    }
}

fn filter_error(err: FilterError) -> ResamplerError {
    ResamplerError::Backend {
        op: consts::FILTER_OP,
        detail: err.to_string(),
    }
}

fn low_pass_filter(
    channels: NonZeroUsize,
    backend: &'static str,
) -> Result<Biquad, ResamplerBuildError> {
    Biquad::new(channels, NonZeroUsize::MIN).map_err(|err| ResamplerBuildError::BackendBuild {
        backend,
        detail: err.to_string(),
    })
}

fn ensure_build_len(
    buffer: &mut SampleBuffer,
    frames: usize,
    backend: &'static str,
) -> Result<(), ResamplerBuildError> {
    buffer
        .ensure_len(frames)
        .map_err(|err| ResamplerBuildError::BackendBuild {
            backend,
            detail: err.to_string(),
        })
}

fn low_pass_cutoff(sample_rate: f64, ratio: f64) -> f64 {
    consts::CUTOFF_TO_NYQUIST * sample_rate / (2.0 * ratio.max(1.0))
}

fn max_output_frames(input_frames: usize, max_ratio_adjustment: f64) -> usize {
    let Some(input_frames) = input_frames.to_f64() else {
        return usize::MAX;
    };
    let frames = (input_frames * max_ratio_adjustment).ceil();
    frames.to_usize().unwrap_or(usize::MAX).saturating_add(2)
}

fn sample_rate(mode: ResamplerMode) -> f64 {
    match mode {
        ResamplerMode::FixedRatio {
            source_sample_rate, ..
        } => f64::from(source_sample_rate.get()),
        ResamplerMode::VariableRatio { sample_rate, .. } => f64::from(sample_rate.get()),
    }
}

#[cfg(all(
    feature = "apple-accelerate",
    any(target_os = "macos", target_os = "ios")
))]
mod backend {
    use kithara_apple::accelerate::{linear_interpolate_f32, quadratic_interpolate_f32};

    use super::GlideInterpolation;

    pub(super) fn interpolate(
        kind: GlideInterpolation,
        source: &[f32],
        positions: &[f32],
        target: &mut [f32],
    ) {
        match kind {
            GlideInterpolation::Linear => linear_interpolate_f32(source, positions, target),
            GlideInterpolation::Quadratic => quadratic_interpolate_f32(source, positions, target),
        };
    }
}

#[cfg(not(all(
    feature = "apple-accelerate",
    any(target_os = "macos", target_os = "ios")
)))]
mod backend {
    use num_traits::cast::ToPrimitive;

    use super::GlideInterpolation;

    pub(super) fn interpolate(
        kind: GlideInterpolation,
        source: &[f32],
        positions: &[f32],
        target: &mut [f32],
    ) {
        match kind {
            GlideInterpolation::Linear => {
                interpolate_with::<LinearInterpolation>(source, positions, target);
            }
            GlideInterpolation::Quadratic => {
                interpolate_with::<QuadraticInterpolation>(source, positions, target);
            }
        }
    }

    trait Interpolation {
        fn sample(source: &[f32], base: usize, frac: f32) -> f32;
    }

    struct LinearInterpolation;

    impl Interpolation for LinearInterpolation {
        fn sample(source: &[f32], base: usize, frac: f32) -> f32 {
            let center = source.get(base).copied().unwrap_or(0.0);
            let right = source.get(base.saturating_add(1)).copied().unwrap_or(0.0);
            center.mul_add(1.0 - frac, right * frac)
        }
    }

    struct QuadraticInterpolation;

    impl Interpolation for QuadraticInterpolation {
        fn sample(source: &[f32], base: usize, frac: f32) -> f32 {
            let left = if base == 0 {
                source.first().copied().unwrap_or(0.0)
            } else {
                source.get(base.saturating_sub(1)).copied().unwrap_or(0.0)
            };
            let center = source.get(base).copied().unwrap_or(0.0);
            let right = source.get(base.saturating_add(1)).copied().unwrap_or(0.0);
            let slope = 0.5 * (right - left);
            let curve = 0.5 * (right - 2.0 * center + left);
            center + frac * slope + frac * frac * curve
        }
    }

    fn interpolate_with<I>(source: &[f32], positions: &[f32], target: &mut [f32])
    where
        I: Interpolation,
    {
        for (position, output) in positions.iter().zip(target.iter_mut()) {
            let base = position.floor().to_usize().unwrap_or(usize::MAX);
            let frac = position - base.to_f32().unwrap_or(0.0);
            *output = I::sample(source, base, frac);
        }
    }
}
