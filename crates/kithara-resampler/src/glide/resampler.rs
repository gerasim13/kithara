use std::{
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
};

use kithara_bufpool::HasPool;
use kithara_dsp::interp::{Interpolation, RateRamp};
use num_traits::cast::ToPrimitive;

use super::{
    GlideConfig,
    engine::{GlideEngine, RenderRequest},
};
use crate::{
    RatioGlide, Resampler, ResamplerBuildError, ResamplerCapabilities, ResamplerControl,
    ResamplerError, ResamplerMode, ResamplerOptions, ResamplerProcess, ResamplerSettings,
};

pub struct GlideResampler {
    config: GlideConfig,
    engine: GlideEngine,
    ramp: RateRamp,
    channels: NonZeroUsize,
    mode: ResamplerMode,
    options: ResamplerOptions,
    cursor: f64,
    input_frames: usize,
}

impl GlideResampler {
    pub(super) fn new<S>(
        backend: &'static str,
        config: GlideConfig,
        settings: &ResamplerSettings<S>,
    ) -> Result<Self, ResamplerBuildError>
    where
        S: HasPool<f32>,
    {
        let ratio = initial_ratio(settings.mode);
        validate_ratio_bounds(backend, settings.options, ratio)?;
        let ramp = initial_ramp(backend, settings.mode, settings.options, ratio)?;
        let input_frames = block_frames(settings.options.chunk_size, config.interpolation);
        let engine = GlideEngine::new(settings, input_frames, config.interpolation, backend)?;
        Ok(Self {
            config,
            engine,
            ramp,
            channels: settings.channels,
            input_frames,
            mode: settings.mode,
            options: settings.options,
            cursor: 0.0,
        })
    }

    fn can_passthrough(&self) -> bool {
        self.ramp
            .held()
            .is_some_and(|ratio| (ratio - 1.0).abs() <= self.options.passthrough_tolerance)
    }

    fn output_ratio(&self) -> f64 {
        self.ramp.current().min(self.ramp.target())
    }

    /// Render one exact source/output span without backend buffering.
    ///
    /// # Errors
    /// Returns [`ResamplerError`] when the planar shape or prepared limits do
    /// not match the requested span.
    pub fn process_exact_span<I, O>(
        &mut self,
        input: &[I],
        output: &mut [O],
    ) -> Result<(), ResamplerError>
    where
        I: Deref<Target = [f32]>,
        O: DerefMut<Target = [f32]>,
    {
        let input_frames = validate_input(input, self.channels.get())?;
        let output_frames = validate_output(output, self.channels.get())?;
        if input_frames == 0 || output_frames == 0 {
            return Err(ResamplerError::InvalidBuffer {
                detail: "exact Glide spans must be non-empty",
            });
        }
        if input_frames > self.input_frames || output_frames > self.engine.position_capacity() {
            return Err(ResamplerError::InvalidBuffer {
                detail: "exact Glide span exceeds prepared frame limits",
            });
        }
        let ratio = input_frames
            .to_f64()
            .and_then(|input| output_frames.to_f64().map(|output| input / output))
            .ok_or(ResamplerError::InvalidBuffer {
                detail: "exact Glide span ratio is not representable",
            })?;
        validate_runtime_ratio(self.options, ratio)?;
        self.engine.seed(input);
        let positions = self.engine.positions_mut(output_frames)?;
        let end = input_frames.saturating_add(2).to_f64().unwrap_or(0.0);
        if RateRamp::hold(ratio).positions(1.0, end, positions) < output_frames {
            return Err(ResamplerError::InvalidBuffer {
                detail: "exact Glide source position is not representable",
            });
        }
        let within = (ratio - 1.0).abs() <= self.options.passthrough_tolerance;
        self.engine.render(RenderRequest {
            input,
            output,
            filter_ratio: (self.config.anti_alias && !within).then_some(ratio),
            produced: output_frames,
            consumed: input_frames,
        })?;
        self.ramp = RateRamp::hold(ratio);
        self.cursor = 0.0;
        Ok(())
    }

    fn render_interpolated(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        input_frames: usize,
        output_capacity: usize,
    ) -> Result<(usize, usize), ResamplerError> {
        let position_capacity = self.engine.position_capacity().min(output_capacity);
        let positions = self.engine.positions_mut(position_capacity)?;
        let after = usize::from(self.config.interpolation.padding().1);
        let end = input_frames
            .saturating_add(1)
            .saturating_sub(after)
            .to_f64()
            .unwrap_or(0.0);
        let ramp = self.ramp;
        let produced = ramp.positions(self.cursor + 1.0, end, positions);
        let cursor = self.cursor + ramp.offset(produced);
        let consumed = cursor
            .floor()
            .to_usize()
            .unwrap_or(usize::MAX)
            .min(input_frames.saturating_sub(1));
        self.engine.render(RenderRequest {
            input,
            output,
            produced,
            consumed,
            filter_ratio: self.config.anti_alias.then(|| ramp.peak(produced)),
        })?;
        self.cursor = cursor - consumed.to_f64().unwrap_or(0.0);
        self.ramp = ramp.after(produced);
        Ok((consumed, produced))
    }

    fn render_passthrough(&self, input: &[&[f32]], output: &mut [&mut [f32]], frames: usize) {
        for channel in 0..self.channels.get() {
            output[channel][..frames].copy_from_slice(&input[channel][..frames]);
        }
    }
}

impl Resampler for GlideResampler {
    fn capabilities(&self) -> ResamplerCapabilities {
        ResamplerCapabilities::FIXED_RATIO
            | ResamplerCapabilities::VARIABLE_RATIO
            | ResamplerCapabilities::RATIO_GLIDE
            | ResamplerCapabilities::REALTIME_SAFE
            | ResamplerCapabilities::STANDALONE
    }

    fn channels(&self) -> NonZeroUsize {
        self.channels
    }

    fn control_mut(&mut self) -> Option<&mut dyn ResamplerControl> {
        Some(self)
    }

    fn input_frames_max(&self) -> usize {
        self.input_frames
    }

    fn input_frames_next(&self) -> usize {
        self.input_frames
    }

    fn mode(&self) -> ResamplerMode {
        self.mode
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        frames_for_ratio(input_frames, self.output_ratio())
    }

    fn output_frames_max(&self) -> usize {
        self.output_frames_next()
    }

    fn output_frames_next(&self) -> usize {
        self.output_frames_for_input(self.input_frames)
            .saturating_add(2)
    }

    fn process_into_buffer(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
    ) -> Result<ResamplerProcess, ResamplerError> {
        let input_frames = validate_input(input, self.channels.get())?;
        let output_capacity = validate_output(output, self.channels.get())?;
        if input_frames == 0 || output_capacity == 0 {
            return Ok(ResamplerProcess::new(0, 0));
        }
        self.engine.seed(input);
        let (consumed, produced) = if self.can_passthrough() {
            let frames = input_frames.min(output_capacity);
            self.render_passthrough(input, output, frames);
            self.engine.pass(input, frames);
            (frames, frames)
        } else {
            self.render_interpolated(input, output, input_frames, output_capacity)?
        };
        Ok(ResamplerProcess::new(consumed, produced))
    }

    fn reset(&mut self) {
        self.ramp = RateRamp::hold(initial_ratio(self.mode));
        self.engine.reset();
        self.cursor = 0.0;
    }
}

impl ResamplerControl for GlideResampler {
    fn glide_ratio(&mut self, glide: RatioGlide) -> Result<(), ResamplerError> {
        validate_runtime_ratio(self.options, glide.target_ratio)?;
        self.ramp = RateRamp::new(self.ramp.current(), glide.target_ratio, glide.frames);
        Ok(())
    }

    fn set_ratio(&mut self, ratio: f64) -> Result<(), ResamplerError> {
        validate_runtime_ratio(self.options, ratio)?;
        self.ramp = RateRamp::hold(ratio);
        Ok(())
    }
}

/// Frames of one buffered block: `chunk_size`, but at least one more than the
/// interpolation reads past a position. In a shorter block no position can
/// sample, so the block would never be taken.
fn block_frames(chunk_size: usize, interpolation: Interpolation) -> usize {
    chunk_size.max(usize::from(interpolation.padding().1).saturating_add(1))
}

fn frames_for_ratio(input_frames: usize, ratio: f64) -> usize {
    let Some(input_frames) = input_frames.to_f64() else {
        return usize::MAX;
    };
    let frames = (input_frames / ratio).ceil();
    if !frames.is_finite() || frames <= 0.0 {
        return 0;
    }
    frames.to_usize().unwrap_or(usize::MAX)
}

fn initial_ramp(
    backend: &'static str,
    mode: ResamplerMode,
    options: ResamplerOptions,
    initial_ratio: f64,
) -> Result<RateRamp, ResamplerBuildError> {
    let ResamplerMode::VariableRatio {
        glide: Some(glide), ..
    } = mode
    else {
        return Ok(RateRamp::hold(initial_ratio));
    };
    validate_ratio_bounds(backend, options, glide.target_ratio)?;
    Ok(RateRamp::new(
        initial_ratio,
        glide.target_ratio,
        glide.frames,
    ))
}

fn initial_ratio(mode: ResamplerMode) -> f64 {
    match mode {
        ResamplerMode::FixedRatio {
            source_sample_rate,
            target_sample_rate,
        } => f64::from(source_sample_rate.get()) / f64::from(target_sample_rate.get()),
        ResamplerMode::VariableRatio { initial_ratio, .. } => initial_ratio,
    }
}

fn validate_input<I: Deref<Target = [f32]>>(
    input: &[I],
    channels: usize,
) -> Result<usize, ResamplerError> {
    if input.len() < channels {
        return Err(ResamplerError::InvalidBuffer {
            detail: "not enough input channels for glide resampler",
        });
    }
    let frames = input[0].deref().len();
    if input
        .iter()
        .take(channels)
        .any(|channel| channel.deref().len() != frames)
    {
        return Err(ResamplerError::InvalidBuffer {
            detail: "input channels must have equal frame counts",
        });
    }
    Ok(frames)
}

fn validate_output<O: Deref<Target = [f32]>>(
    output: &[O],
    channels: usize,
) -> Result<usize, ResamplerError> {
    if output.len() < channels {
        return Err(ResamplerError::InvalidBuffer {
            detail: "not enough output channels for glide resampler",
        });
    }
    let frames = output[0].deref().len();
    if output
        .iter()
        .take(channels)
        .any(|channel| channel.deref().len() != frames)
    {
        return Err(ResamplerError::InvalidBuffer {
            detail: "output channels must have equal frame counts",
        });
    }
    Ok(frames)
}

fn validate_ratio_bounds(
    backend: &'static str,
    options: ResamplerOptions,
    ratio: f64,
) -> Result<(), ResamplerBuildError> {
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(ResamplerBuildError::InvalidRatio {
            ratio,
            resource: "glide",
        });
    }
    let min = 1.0 / options.max_ratio_adjustment;
    if ratio < min || ratio > options.max_ratio_adjustment {
        return Err(ResamplerBuildError::BackendBuild {
            backend,
            detail: "glide ratio exceeds configured max_ratio_adjustment".into(),
        });
    }
    Ok(())
}

fn validate_runtime_ratio(options: ResamplerOptions, ratio: f64) -> Result<(), ResamplerError> {
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(ResamplerError::Backend {
            op: "glide ratio",
            detail: "ratio must be finite and positive".into(),
        });
    }
    let min = 1.0 / options.max_ratio_adjustment;
    if ratio < min || ratio > options.max_ratio_adjustment {
        return Err(ResamplerError::Backend {
            op: "glide ratio",
            detail: "ratio exceeds configured max_ratio_adjustment".into(),
        });
    }
    Ok(())
}
