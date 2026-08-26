use std::ops::Range;

use bungee_sys::InputChunk;
use kithara_bufpool::{PcmBuf, PcmPool};
use num_traits::ToPrimitive;

use super::ffi::{AnalysisInput, NativeStretcher};
use crate::{ElasticConfig, ElasticError};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct PooledPlanar {
    #[field(get, copy, vis = "pub(super)")]
    channels: usize,
    pub(super) samples: PcmBuf,
    pub(super) stride: usize,
}

impl PooledPlanar {
    pub(super) fn new(
        pool: &PcmPool,
        channels: usize,
        stride: usize,
    ) -> Result<Self, ElasticError> {
        let samples = channels
            .checked_mul(stride)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let mut storage = pool.get();
        storage
            .ensure_len(samples)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        Ok(Self {
            channels,
            samples: storage,
            stride,
        })
    }

    fn channel(&self, channel: usize) -> &[f32] {
        let begin = channel * self.stride;
        &self.samples[begin..begin + self.stride]
    }

    fn channel_mut(&mut self, channel: usize) -> &mut [f32] {
        let begin = channel * self.stride;
        &mut self.samples[begin..begin + self.stride]
    }

    pub(super) fn copy_interleaved(
        &self,
        source: Range<usize>,
        output: &mut [f32],
        output_frame: usize,
    ) {
        for (relative_frame, frame) in source.enumerate() {
            for channel in 0..self.channels {
                output[(output_frame + relative_frame) * self.channels + channel] =
                    self.channel(channel)[frame];
            }
        }
    }

    fn prepare_stride(&mut self, stride: usize) -> Result<(), ElasticError> {
        let samples = self
            .channels
            .checked_mul(stride)
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.samples
            .ensure_len(samples)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        self.stride = stride;
        Ok(())
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct InputBuffer {
    analysis: PooledPlanar,
    audio: PooledPlanar,
    begin: i32,
    #[field(get, copy, vis = "pub(super)")]
    end: i32,
    #[field(set, vis = "pub(super)")]
    requested: InputChunk,
}

impl InputBuffer {
    pub(super) fn new(
        config: &ElasticConfig,
        max_input_frames: usize,
        max_source_frames: usize,
    ) -> Result<Self, ElasticError> {
        let capacity = max_input_frames
            .checked_add(max_source_frames)
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok(Self {
            analysis: PooledPlanar::new(config.pool(), config.channels(), max_input_frames)?,
            audio: PooledPlanar::new(config.pool(), config.channels(), capacity)?,
            begin: 0,
            end: 0,
            requested: InputChunk { begin: 0, end: 0 },
        })
    }

    pub(super) fn append(
        &mut self,
        source: Option<&[f32]>,
        input_frames: usize,
    ) -> Result<(), ElasticError> {
        let input_frames_i32 = i32::try_from(input_frames)
            .map_err(|_| ElasticError::SourceFrameLimitOutOfRange(input_frames))?;
        let mut discard = 0;
        if self.requested.begin < self.end {
            if self.begin < self.requested.begin {
                let shift = usize::try_from(self.requested.begin - self.begin)
                    .map_err(|_| ElasticError::SampleCountOverflow)?;
                let retained = usize::try_from(self.end - self.begin)
                    .map_err(|_| ElasticError::SampleCountOverflow)?;
                for channel in 0..self.audio.channels {
                    self.audio
                        .channel_mut(channel)
                        .copy_within(shift..retained, 0);
                }
                self.begin = self.requested.begin;
            }
        } else {
            let skipped = self
                .requested
                .begin
                .checked_sub(self.begin)
                .ok_or(ElasticError::SampleCountOverflow)?;
            discard = usize::try_from(skipped)
                .map_err(|_| ElasticError::SampleCountOverflow)?
                .min(input_frames);
            self.begin = self.end;
        }

        let buffered = usize::try_from(self.end - self.begin)
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        let appended = input_frames - discard;
        let required = buffered
            .checked_add(appended)
            .ok_or(ElasticError::SampleCountOverflow)?;
        if required > self.audio.stride {
            return Err(ElasticError::InputStorage {
                required,
                capacity: self.audio.stride,
            });
        }
        let channels = self.audio.channels;
        for channel in 0..channels {
            let destination = &mut self.audio.channel_mut(channel)[buffered..required];
            if let Some(source) = source {
                for (destination, frame) in destination.iter_mut().zip(discard..input_frames) {
                    *destination = source[frame * channels + channel];
                }
            } else {
                destination.fill(0.0);
            }
        }
        self.begin = self
            .begin
            .checked_add(i32::try_from(discard).map_err(|_| ElasticError::SampleCountOverflow)?)
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.end = self
            .end
            .checked_add(input_frames_i32)
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok(())
    }

    pub(super) fn prepare_source_capacity(&mut self, capacity: usize) -> Result<(), ElasticError> {
        self.audio.prepare_stride(capacity)
    }

    fn requested_frames(&self) -> Result<usize, ElasticError> {
        usize::try_from(
            self.requested
                .end
                .checked_sub(self.requested.begin)
                .ok_or(ElasticError::SampleCountOverflow)?,
        )
        .map_err(|_| ElasticError::EnginePreparation("Bungee requested an invalid input grain"))
    }

    pub(super) fn requested_window(&self, position: f64) -> Result<(usize, usize), ElasticError> {
        let position = position
            .to_i32()
            .filter(|converted| f64::from(*converted) == position)
            .ok_or(ElasticError::EnginePreparation(
                "Bungee reported an invalid processing center",
            ))?;
        let history = position
            .checked_sub(self.requested.begin)
            .and_then(|frames| usize::try_from(frames).ok());
        let lookahead = self
            .requested
            .end
            .checked_sub(position)
            .and_then(|frames| usize::try_from(frames).ok());
        history
            .zip(lookahead)
            .ok_or(ElasticError::EnginePreparation(
                "Bungee reported an input window outside its processing center",
            ))
    }

    pub(super) fn analyse(
        &mut self,
        native: &mut NativeStretcher,
        valid: bool,
        end_of_input: bool,
    ) -> Result<(), ElasticError> {
        self.analysis.samples.fill(0.0);
        if !valid {
            return native.analyse(AnalysisInput {
                samples: &self.analysis.samples,
                channel_stride: self.analysis.stride,
                mute_head: 0,
                mute_tail: 0,
            });
        }

        let requested_frames = self.requested_frames()?;
        if requested_frames > self.analysis.stride {
            return Err(ElasticError::EnginePreparation(
                "Bungee requested an oversized input grain",
            ));
        }
        let available_begin = self.requested.begin.max(self.begin);
        let available_end = self.requested.end.min(self.end).max(available_begin);
        let copied = usize::try_from(
            available_end
                .checked_sub(available_begin)
                .ok_or(ElasticError::SampleCountOverflow)?,
        )
        .map_err(|_| ElasticError::SampleCountOverflow)?;
        if copied > 0 {
            let source_begin = usize::try_from(
                available_begin
                    .checked_sub(self.begin)
                    .ok_or(ElasticError::SampleCountOverflow)?,
            )
            .map_err(|_| ElasticError::SampleCountOverflow)?;
            let destination_begin = usize::try_from(
                available_begin
                    .checked_sub(self.requested.begin)
                    .ok_or(ElasticError::SampleCountOverflow)?,
            )
            .map_err(|_| ElasticError::SampleCountOverflow)?;
            for channel in 0..self.audio.channels {
                let source = &self.audio.channel(channel)[source_begin..source_begin + copied];
                self.analysis.channel_mut(channel)[destination_begin..destination_begin + copied]
                    .copy_from_slice(source);
            }
        }
        let mute_head =
            usize::try_from((i64::from(self.begin) - i64::from(self.requested.begin)).max(0))
                .map_err(|_| ElasticError::SampleCountOverflow)?
                .min(requested_frames);
        let mute_tail =
            usize::try_from((i64::from(self.requested.end) - i64::from(self.end)).max(0))
                .map_err(|_| ElasticError::SampleCountOverflow)?
                .min(requested_frames);
        if mute_tail > 0 && mute_head < requested_frames && !end_of_input {
            return Err(ElasticError::EnginePreparation(
                "Bungee requested unavailable future input",
            ));
        }
        native.analyse(AnalysisInput {
            samples: &self.analysis.samples,
            channel_stride: self.analysis.stride,
            mute_head,
            mute_tail,
        })
    }

    pub(super) fn clear(&mut self) {
        self.set_position(0);
    }

    pub(super) fn set_position(&mut self, position: i32) {
        self.begin = position;
        self.end = position;
        self.requested = InputChunk {
            begin: position,
            end: position,
        };
    }
}
