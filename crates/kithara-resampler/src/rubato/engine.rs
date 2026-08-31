use std::num::NonZeroUsize;

use rubato::{
    Async, Fft, FixedAsync, FixedSync, PolynomialDegree, ResampleError,
    Resampler as RubatoResamplerTrait, ResamplerConstructionError, SincInterpolationParameters,
    SincInterpolationType, WindowFunction, audioadapter_buffers::direct::SequentialSliceOfSlices,
};

use super::{RubatoAlgorithm, RubatoConfig};
use crate::{ResamplerOptions, ResamplerProcess, ResamplerQuality};

pub(super) struct RubatoEngine {
    inner: Box<dyn RubatoResamplerTrait<f32>>,
}

impl RubatoEngine {
    pub(super) fn new(
        config: RubatoConfig,
        quality: ResamplerQuality,
        source_rate: u32,
        target_rate: u32,
        channels: NonZeroUsize,
        options: ResamplerOptions,
    ) -> Result<Self, ResamplerConstructionError> {
        match config.algorithm {
            RubatoAlgorithm::Async => {
                Self::new_async(quality, source_rate, target_rate, channels, options)
            }
            RubatoAlgorithm::Fft => Self::new_fft(source_rate, target_rate, channels, options),
        }
    }

    fn new_async(
        quality: ResamplerQuality,
        source_rate: u32,
        target_rate: u32,
        channels: NonZeroUsize,
        options: ResamplerOptions,
    ) -> Result<Self, ResamplerConstructionError> {
        let ratio = ratio_for_target(source_rate, target_rate);
        match quality {
            ResamplerQuality::Fast => {
                let poly = Async::new_poly(
                    ratio,
                    options.max_ratio_adjustment,
                    PolynomialDegree::Cubic,
                    options.chunk_size,
                    channels.get(),
                    FixedAsync::Input,
                )?;
                Ok(Self {
                    inner: Box::new(poly),
                })
            }
            ResamplerQuality::Normal | ResamplerQuality::Good | ResamplerQuality::High => {
                let sinc = Async::new_sinc(
                    ratio,
                    options.max_ratio_adjustment,
                    &SincInterpolationParameters::from(quality),
                    options.chunk_size,
                    channels.get(),
                    FixedAsync::Input,
                )?;
                Ok(Self {
                    inner: Box::new(sinc),
                })
            }
        }
    }

    fn new_fft(
        source_rate: u32,
        target_rate: u32,
        channels: NonZeroUsize,
        options: ResamplerOptions,
    ) -> Result<Self, ResamplerConstructionError> {
        let fft = Fft::<f32>::new_custom(
            source_rate as usize,
            target_rate as usize,
            options.chunk_size,
            2,
            channels.get(),
            WindowFunction::BlackmanHarris2,
            FixedSync::Input,
        )?;
        Ok(Self {
            inner: Box::new(fft),
        })
    }

    pub(super) fn process_into_buffer(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
    ) -> Result<ResamplerProcess, ResampleError> {
        let channels = input.len();
        let input_frames = input.first().map_or(0, |channel| channel.len());
        let input_adapter =
            SequentialSliceOfSlices::new(input, channels, input_frames).map_err(|_| {
                ResampleError::InsufficientInputBufferSize {
                    actual: input_frames,
                    expected: self.input_frames_next(),
                }
            })?;
        let caller_output_frames =
            validate_caller_output(output, channels, self.output_frames_next())?;
        let process = if channels <= 8 {
            let mut output_refs: [&mut [f32]; 8] = std::array::from_fn(|_| &mut [] as &mut [f32]);
            for (target, channel) in output_refs.iter_mut().zip(output.iter_mut()) {
                *target = &mut **channel;
            }
            let mut output_adapter = SequentialSliceOfSlices::new_mut(
                &mut output_refs[..channels],
                channels,
                caller_output_frames,
            )
            .map_err(|_| ResampleError::InsufficientOutputBufferSize {
                actual: caller_output_frames,
                expected: self.output_frames_next(),
            })?;
            self.inner
                .process_into_buffer(&input_adapter, &mut output_adapter, None)
        } else {
            let mut output_refs = output
                .iter_mut()
                .take(channels)
                .map(|channel| &mut **channel)
                .collect::<Vec<&mut [f32]>>();
            let mut output_adapter =
                SequentialSliceOfSlices::new_mut(&mut output_refs, channels, caller_output_frames)
                    .map_err(|_| ResampleError::InsufficientOutputBufferSize {
                        actual: caller_output_frames,
                        expected: self.output_frames_next(),
                    })?;
            self.inner
                .process_into_buffer(&input_adapter, &mut output_adapter, None)
        };
        let (input_frames, output_frames) = process?;

        Ok(ResamplerProcess::new(input_frames, output_frames))
    }

    delegate::delegate! {
        to self.inner {
            pub(super) fn input_frames_max(&self) -> usize;
            pub(super) fn input_frames_next(&self) -> usize;
            pub(super) fn output_delay(&self) -> usize;
            pub(super) fn output_frames_max(&self) -> usize;
            pub(super) fn output_frames_next(&self) -> usize;
            pub(super) fn resample_ratio(&self) -> f64;
            pub(super) fn reset(&mut self);
        }
    }
}

impl From<ResamplerQuality> for SincInterpolationParameters {
    fn from(quality: ResamplerQuality) -> Self {
        const CUTOFF: f32 = 0.95;
        const LEN_GOOD: usize = 128;
        const LEN_HIGH: usize = 256;
        const LEN_NORMAL: usize = 64;
        const OVERSAMPLING_HIGH: usize = 256;
        const OVERSAMPLING_NORMAL: usize = 128;

        match quality {
            ResamplerQuality::Good => Self {
                sinc_len: LEN_GOOD,
                f_cutoff: Some(CUTOFF),
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: OVERSAMPLING_HIGH,
                window: WindowFunction::BlackmanHarris2,
            },
            ResamplerQuality::High => Self {
                sinc_len: LEN_HIGH,
                f_cutoff: Some(CUTOFF),
                interpolation: SincInterpolationType::Cubic,
                oversampling_factor: OVERSAMPLING_HIGH,
                window: WindowFunction::BlackmanHarris2,
            },
            ResamplerQuality::Normal | ResamplerQuality::Fast => Self {
                sinc_len: LEN_NORMAL,
                f_cutoff: Some(CUTOFF),
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: OVERSAMPLING_NORMAL,
                window: WindowFunction::BlackmanHarris2,
            },
        }
    }
}

fn validate_caller_output(
    output: &[&mut [f32]],
    channels: usize,
    frames: usize,
) -> Result<usize, ResampleError> {
    if output.len() < channels {
        return Err(ResampleError::WrongNumberOfOutputChannels {
            actual: output.len(),
            expected: channels,
        });
    }
    let actual = output
        .iter()
        .take(channels)
        .map(|channel| channel.len())
        .min()
        .unwrap_or(0);
    if actual < frames {
        return Err(ResampleError::InsufficientOutputBufferSize {
            actual,
            expected: frames,
        });
    }

    Ok(actual)
}

fn ratio_for_target(source_rate: u32, target_rate: u32) -> f64 {
    f64::from(target_rate) / f64::from(source_rate)
}
