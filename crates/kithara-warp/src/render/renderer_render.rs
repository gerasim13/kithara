use kithara_bufpool::HasPool;
use kithara_dsp::param::SmoothedParam;
use kithara_signal::{AudioChunkInfo, FrameCount, SampleCount, SourceSpan};
use kithara_stretch::{ElasticCapabilities, ElasticError, ElasticRequest};
use kithara_test_macros as kithara;
use num_traits::ToPrimitive;

use super::renderer::WarpRenderer;

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    pub(super) fn render_varispeed(&mut self, span: SourceSpan) -> Result<(), ElasticError> {
        let channels = usize::from(self.spec.channels);
        let frames =
            usize::try_from(span.output_frames()).map_err(|_| ElasticError::SampleCountOverflow)?;
        let samples = frames
            .checked_mul(channels)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let scratch = self.scratch.as_mut().ok_or(ElasticError::PoolCapacity)?;
        if samples > scratch.capacity() {
            return Err(ElasticError::PoolCapacity);
        }
        scratch
            .ensure_len(samples)
            .map_err(|_| ElasticError::PoolCapacity)?;
        let resident = self.residency.as_ref().ok_or(ElasticError::PoolCapacity)?;
        for frame in 0..frames {
            let (numerator, denominator) = span
                .source_ratio_at(
                    u64::try_from(frame).map_err(|_| ElasticError::SampleCountOverflow)?,
                )
                .ok_or(ElasticError::SampleCountOverflow)?;
            let speed = super::renderer_mapping::span_speed(
                span,
                u64::try_from(frame).map_err(|_| ElasticError::SampleCountOverflow)?,
            )?;
            for channel in 0..channels {
                scratch[frame * channels + channel] = super::source_sample::source_sample(
                    resident,
                    (numerator, denominator),
                    speed,
                    self.terminal_source_end,
                    channels,
                    channel,
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn advance_speed(
        &mut self,
        target: f32,
        output_frames: usize,
    ) -> Result<(), ElasticError> {
        if !self.requires_staging() {
            return Ok(());
        }
        let Some(applied) = self.applied_speed else {
            return Ok(());
        };
        let (_, next, endpoint) = Self::smoothed_speed(applied, target, output_frames)?;
        self.applied_speed = Some(next);
        kithara::probe_event!(
            rate_smoothed,
            frames = output_frames,
            multiplier_bits = endpoint.to_bits(),
            target_bits = target.to_bits()
        );
        Ok(())
    }

    pub(super) fn preview_speed(
        &self,
        target: f32,
        output_frames: usize,
    ) -> Result<f32, ElasticError> {
        if !self.requires_staging() {
            return Ok(1.0);
        }
        let Some(applied) = self.applied_speed else {
            return Ok(target);
        };
        let (speed, _, _) = Self::smoothed_speed(applied, target, output_frames)?;
        Ok(speed)
    }

    fn smoothed_speed(
        mut applied: SmoothedParam,
        target: f32,
        output_frames: usize,
    ) -> Result<(f32, SmoothedParam, f32), ElasticError> {
        if output_frames == 0 {
            return Err(ElasticError::EmptyOutput);
        }
        applied.set_value(target);
        let mut total = 0.0_f64;
        let mut endpoint = target;
        for _ in 0..output_frames {
            endpoint = applied.next_smoothed();
            if applied.settle() {
                endpoint = target;
            }
            total += f64::from(endpoint);
        }
        let frames = output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let speed = (total / frames)
            .to_f32()
            .filter(|speed| speed.is_finite() && *speed > 0.0)
            .ok_or(ElasticError::InvalidRate(total / frames))?;
        Ok((speed, applied, endpoint))
    }
}

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    fn append_pending_source(
        &mut self,
        source: &[f32],
        meta: AudioChunkInfo,
        frame_offset: u64,
    ) -> Result<(), ElasticError> {
        let channels = usize::from(self.spec.channels.max(1));
        let pending_frames = self.pending_frames(channels);
        if let Some(start) = self.pending_meta {
            let expected = start
                .frame_offset
                .checked_add(
                    u64::try_from(pending_frames).map_err(|_| ElasticError::SampleCountOverflow)?,
                )
                .ok_or(ElasticError::SampleCountOverflow)?;
            if expected != frame_offset {
                return Err(ElasticError::DiscontinuousSource {
                    expected: expected.to_f64().ok_or(ElasticError::SampleCountOverflow)?,
                    actual: frame_offset
                        .to_f64()
                        .ok_or(ElasticError::SampleCountOverflow)?,
                });
            }
        }
        let pending = self
            .pending_source
            .as_mut()
            .ok_or(ElasticError::PoolCapacity)?;
        let start = pending.len();
        let end = start
            .checked_add(source.len())
            .ok_or(ElasticError::SampleCountOverflow)?;
        if end > pending.capacity() {
            return Err(ElasticError::SourceFrameLimit {
                frames: end / channels,
                limit: pending.capacity() / channels,
            });
        }
        pending
            .ensure_len(end)
            .map_err(|_| ElasticError::PoolCapacity)?;
        pending[start..end].copy_from_slice(source);
        self.pending_meta
            .get_or_insert_with(|| Self::meta_at_frame(meta, frame_offset));
        Ok(())
    }

    pub(super) fn balanced_source_block(remaining: usize, limit: usize) -> usize {
        let partitions = remaining.div_ceil(limit);
        remaining.div_ceil(partitions)
    }

    /// Backends require a non-empty output, so a sub-frame source span stays pending until its
    /// cumulative exact output reaches one full frame; EOF rounds the final residual once.
    pub(super) fn output_frames(
        source_frames: usize,
        stretch: f64,
        remainder: f64,
    ) -> Result<(usize, f64), ElasticError> {
        if !stretch.is_finite() || stretch <= 0.0 {
            return Err(ElasticError::InvalidRate(stretch.recip()));
        }
        let source_frames = source_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let exact = source_frames.mul_add(stretch, remainder);
        if !exact.is_finite() {
            return Err(ElasticError::SampleCountOverflow);
        }
        let output_frames = if exact < 1.0 { 0.0 } else { exact.round() };
        let output_frames = output_frames
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let emitted = output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok((output_frames, exact - emitted))
    }

    /// The output of a quantum's last request when the quantum ends on a
    /// scheduled frame: what `emitted` leaves of `landing`, with the exact
    /// output it does not cover carried as the remainder.
    fn landing_output_frames(
        source_frames: usize,
        stretch: f64,
        remainder: f64,
        landing: usize,
        emitted: usize,
    ) -> Result<(usize, f64), ElasticError> {
        let output = landing
            .checked_sub(emitted)
            .filter(|output| *output > 0)
            .ok_or(ElasticError::OutputFrameLimit {
                frames: emitted,
                limit: landing,
            })?;
        let exact = source_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?
            .mul_add(stretch, remainder);
        Ok((
            output,
            exact - output.to_f64().ok_or(ElasticError::SampleCountOverflow)?,
        ))
    }

    fn quantized_source_span(
        frames: usize,
        pending_frames: usize,
        stretch: f64,
        remainder: f64,
        capabilities: ElasticCapabilities,
        output_limit: usize,
    ) -> Result<Option<usize>, ElasticError> {
        let envelope = capabilities.rate_envelope();
        let mut source = frames;
        while source > 0 {
            let (output, _) = Self::output_frames(source, stretch, remainder)?;
            if output == 0 {
                return Ok(Some(source));
            }
            let total_source = pending_frames
                .checked_add(source)
                .and_then(|frames| frames.to_f64())
                .ok_or(ElasticError::SampleCountOverflow)?;
            let output_f = output.to_f64().ok_or(ElasticError::SampleCountOverflow)?;
            let rate = total_source / output_f;
            if output <= output_limit && envelope.contains_rate(rate) {
                return Ok(Some(source));
            }
            let next = if output <= output_limit && rate > envelope.max_source_frames_per_output() {
                (output_f * envelope.max_source_frames_per_output())
                    .floor()
                    .to_usize()
                    .ok_or(ElasticError::SampleCountOverflow)?
                    .saturating_sub(pending_frames)
            } else {
                let next_output = output.saturating_sub(1).min(output_limit);
                let rounding_boundary = next_output
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?
                    + 0.5;
                ((rounding_boundary - remainder) / stretch)
                    .ceil()
                    .max(0.0)
                    .to_usize()
                    .ok_or(ElasticError::SampleCountOverflow)?
                    .saturating_sub(1)
            };
            source = next.min(source - 1);
        }
        Ok(None)
    }

    pub(super) fn source_block_limit(
        stretch: f64,
        capabilities: ElasticCapabilities,
        output_limit: usize,
    ) -> Result<usize, ElasticError> {
        if !stretch.is_finite() || stretch <= 0.0 {
            return Err(ElasticError::InvalidRate(stretch));
        }
        let envelope = capabilities.rate_envelope();
        let rate = stretch.recip();
        let boundary = [
            envelope.min_source_frames_per_output(),
            envelope.max_source_frames_per_output(),
        ]
        .into_iter()
        .find(|boundary| boundary.to_f32() == rate.to_f32());
        if let Some(request) = boundary.and_then(|boundary| {
            envelope.largest_request_at(boundary, capabilities.max_source_frames(), output_limit)
        }) {
            return Ok(request.source_frames());
        }
        let output_limit = output_limit
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_budget = (output_limit - Self::OUTPUT_ROUNDING_MARGIN).max(1.0);
        let source_limit = (output_budget / stretch)
            .floor()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let source_limit = source_limit.min(capabilities.max_source_frames());
        if source_limit == 0 {
            return Err(ElasticError::InvalidRate(1.0 / stretch));
        }
        Ok(source_limit)
    }

    /// Source frames for the next quantum and, when `output_limit` ends it on a
    /// scheduled frame, the exact output it renders there.
    pub(super) fn source_frames_for_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
        speed: f32,
        output_limit: usize,
    ) -> Result<(usize, Option<usize>), ElasticError> {
        if remaining == 0 {
            return Err(ElasticError::EmptySource);
        }
        if !self.active
            && !self.transition_pending()
            && self.pending_frames(usize::from(self.spec.channels.max(1))) == 0
            && self.unity_passthrough(speed)
        {
            let frames = if self.requires_staging() {
                self.render_quantum_frames
                    .map_or(remaining, |frames| remaining.min(frames.get()))
            } else {
                remaining
            };
            return Ok((frames.min(output_limit), None));
        }

        let channels = usize::from(self.spec.channels.max(1));
        let region = self.region_for(meta.frame_offset);
        let region_frames = usize::try_from(
            region
                .end()
                .checked_sub(meta.frame_offset)
                .ok_or(ElasticError::SampleCountOverflow)?
                .min(u64::try_from(remaining).map_err(|_| ElasticError::SampleCountOverflow)?),
        )
        .map_err(|_| ElasticError::SampleCountOverflow)?;
        if region_frames == 0 {
            return Err(ElasticError::StationarySourceSpan);
        }
        let stretch = (1.0 / f64::from(speed)) * region.correction();
        let capabilities = self
            .engine
            .as_ref()
            .map(|engine| engine.capabilities())
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
        let quantum_limit = self.render_quantum_frames.map_or_else(
            || capabilities.max_output_frames(),
            |frames| capabilities.max_output_frames().min(frames.get()),
        );
        let pending_frames = self.pending_frames(channels);
        if output_limit <= quantum_limit {
            let source_room = capabilities
                .max_source_frames()
                .saturating_sub(pending_frames);
            let landing = Self::landing_source_span(output_limit, stretch, self.output_remainder)?;
            return Ok(if landing <= region_frames.min(source_room) {
                (landing, Some(output_limit))
            } else {
                (region_frames.min(source_room).max(1), None)
            });
        }
        let output_limit = quantum_limit;
        let source_limit = Self::source_block_limit(stretch, capabilities, output_limit)?;
        let available =
            source_limit
                .checked_sub(pending_frames)
                .ok_or(ElasticError::SourceFrameLimit {
                    frames: pending_frames,
                    limit: source_limit,
                })?;
        if available == 0 {
            return Err(ElasticError::InvalidRate(stretch.recip()));
        }
        let frames = region_frames.min(available);
        Ok((
            Self::quantized_source_span(
                frames,
                pending_frames,
                stretch,
                self.output_remainder,
                capabilities,
                output_limit,
            )?
            .unwrap_or(frames),
            None,
        ))
    }

    /// The fewest source frames whose exact output, after `remainder`, rounds
    /// to at least `output` frames: rendered as exactly `output`, they end a
    /// quantum on that frame and carry the rest as the remainder.
    fn landing_source_span(
        output: usize,
        stretch: f64,
        remainder: f64,
    ) -> Result<usize, ElasticError> {
        let output = output.to_f64().ok_or(ElasticError::SampleCountOverflow)?;
        ((output - Self::OUTPUT_ROUNDING_MARGIN - remainder) / stretch)
            .ceil()
            .max(1.0)
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)
    }
}

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    /// Render `frames` source frames; with `landing`, the last engine request
    /// emits exactly what is left of that many output frames.
    pub(super) fn render_active(
        &mut self,
        meta: AudioChunkInfo,
        samples: &[f32],
        speed: f32,
        channels: usize,
        frames: usize,
        landing: Option<usize>,
    ) -> Result<(), ElasticError> {
        let base = 1.0 / f64::from(speed);
        let pitch = if self.current_keylock {
            1.0
        } else {
            f64::from(speed)
        };
        let mut consumed = 0usize;
        let mut frame = meta.frame_offset;
        self.apply_pitch(pitch)?;
        for _ in 0..frames {
            if consumed == frames {
                return Ok(());
            }
            let region = self.region_for(frame);
            let left = u64::try_from(frames - consumed).unwrap_or(u64::MAX);
            let span = region.end().saturating_sub(frame).min(left).max(1);
            let stretch = base * region.correction();
            let capabilities = self
                .engine
                .as_ref()
                .map(|engine| engine.capabilities())
                .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
            let source_limit =
                Self::source_block_limit(stretch, capabilities, capabilities.max_output_frames())?;
            let remaining = usize::try_from(span).unwrap_or(frames - consumed);
            let pending_frames = self.pending_frames(channels);
            let available =
                source_limit
                    .checked_sub(pending_frames)
                    .ok_or(ElasticError::SourceFrameLimit {
                        frames: pending_frames,
                        limit: source_limit,
                    })?;
            if available == 0 {
                return Err(ElasticError::InvalidRate(stretch.recip()));
            }
            let sub = Self::balanced_source_block(remaining, available);
            let landing = landing.filter(|_| consumed + sub == frames);
            let request_span = if landing.is_some() {
                Some(sub)
            } else {
                Self::quantized_source_span(
                    sub,
                    pending_frames,
                    stretch,
                    self.output_remainder,
                    capabilities,
                    capabilities.max_output_frames(),
                )?
            };
            let sub = request_span.unwrap_or(sub);
            let (output_frames, next_remainder) =
                self.request_output_frames(sub, stretch, landing, channels)?;
            let part = &samples[consumed * channels..(consumed + sub) * channels];
            if output_frames == 0 || request_span.is_none() {
                self.append_pending_source(part, meta, frame)?;
                self.output_remainder = next_remainder
                    + output_frames
                        .to_f64()
                        .ok_or(ElasticError::SampleCountOverflow)?;
                consumed += sub;
                frame = frame.saturating_add(
                    u64::try_from(sub).map_err(|_| ElasticError::SampleCountOverflow)?,
                );
                continue;
            }
            if output_frames > capabilities.max_output_frames() {
                return Err(ElasticError::OutputFrameLimit {
                    frames: output_frames,
                    limit: capabilities.max_output_frames(),
                });
            }
            let source_frames = pending_frames
                .checked_add(sub)
                .ok_or(ElasticError::SampleCountOverflow)?;
            let output_frames = FrameCount::new(output_frames);
            let request = ElasticRequest::new(source_frames, output_frames.get())?;
            let output_samples = output_frames
                .get()
                .checked_mul(channels)
                .map(SampleCount::new)
                .ok_or(ElasticError::SampleCountOverflow)?;
            let start = self.scratch.as_deref().map_or(0, <[f32]>::len);
            let end = start
                .checked_add(output_samples.get())
                .ok_or(ElasticError::SampleCountOverflow)?;
            if pending_frames > 0 {
                self.append_pending_source(part, meta, frame)?;
            }
            if start == 0 && self.output_start_meta.is_none() {
                self.output_start_meta = if pending_frames > 0 {
                    self.pending_meta
                } else {
                    Some(Self::meta_at_frame(meta, frame))
                };
            }
            let scratch = self
                .scratch
                .as_mut()
                .ok_or(ElasticError::EnginePreparation(
                    "output scratch is unavailable",
                ))?;
            if end > scratch.capacity() {
                return Err(ElasticError::OutputFrameLimit {
                    frames: end / channels,
                    limit: scratch.capacity() / channels,
                });
            }
            scratch
                .ensure_len(end)
                .map_err(|_| ElasticError::PoolCapacity)?;
            let source = self
                .pending_source
                .as_deref()
                .filter(|_| pending_frames > 0)
                .unwrap_or(part);
            let engine = self
                .engine
                .as_mut()
                .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
            if let Err(error) = engine.process(request, source, &mut scratch[start..end]) {
                scratch.truncate(start);
                return Err(error);
            }
            if pending_frames > 0 {
                self.clear_pending_source();
            }
            self.output_remainder = next_remainder;
            self.active = true;
            consumed += sub;
            frame = frame
                .saturating_add(u64::try_from(sub).map_err(|_| ElasticError::SampleCountOverflow)?);
        }
        if consumed == frames {
            Ok(())
        } else {
            Err(ElasticError::EnginePreparation(
                "time-stretch render exceeded its source-frame iteration bound",
            ))
        }
    }

    /// Output frames one engine request emits for `sub` source frames and the
    /// remainder it leaves; a `landing` request emits what is left of it.
    fn request_output_frames(
        &self,
        sub: usize,
        stretch: f64,
        landing: Option<usize>,
        channels: usize,
    ) -> Result<(usize, f64), ElasticError> {
        landing.map_or_else(
            || Self::output_frames(sub, stretch, self.output_remainder),
            |landing| {
                let emitted = self.scratch.as_deref().map_or(0, <[f32]>::len) / channels;
                Self::landing_output_frames(sub, stretch, self.output_remainder, landing, emitted)
            },
        )
    }

    pub(super) fn render_terminal_pending(
        &mut self,
        channels: usize,
        output_limit: usize,
    ) -> Result<(), ElasticError> {
        let source_frames = self.pending_frames(channels);
        if source_frames == 0 {
            self.output_remainder = 0.0;
            return Ok(());
        }
        let output_frames = self
            .output_remainder
            .round()
            .max(0.0)
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        if output_frames == 0 {
            self.clear_pending_source();
            self.output_remainder = 0.0;
            return Ok(());
        }

        let total_output_frames = output_frames;
        let output_frames = FrameCount::new(output_frames.min(output_limit));
        let admitted_source_frames = if output_frames.get() == total_output_frames {
            source_frames
        } else {
            source_frames
                .checked_mul(output_frames.get())
                .and_then(|frames| frames.checked_div(total_output_frames))
                .filter(|frames| *frames > 0)
                .ok_or(ElasticError::EmptySource)?
        };
        let request = ElasticRequest::new(admitted_source_frames, output_frames.get())?;
        let admitted_samples = admitted_source_frames
            .checked_mul(channels)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let next_meta = if admitted_source_frames == source_frames {
            None
        } else {
            let meta = self.pending_meta.ok_or(ElasticError::EmptySource)?;
            let next = meta
                .frame_offset
                .checked_add(
                    u64::try_from(admitted_source_frames)
                        .map_err(|_| ElasticError::SampleCountOverflow)?,
                )
                .ok_or(ElasticError::SampleCountOverflow)?;
            Some(Self::meta_at_frame(meta, next))
        };
        let output_samples = output_frames
            .get()
            .checked_mul(channels)
            .map(SampleCount::new)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let start = self.scratch.as_deref().map_or(0, <[f32]>::len);
        let end = start
            .checked_add(output_samples.get())
            .ok_or(ElasticError::SampleCountOverflow)?;
        let scratch = self
            .scratch
            .as_mut()
            .ok_or(ElasticError::EnginePreparation(
                "output scratch is unavailable",
            ))?;
        if end > scratch.capacity() {
            return Err(ElasticError::OutputFrameLimit {
                frames: end / channels,
                limit: scratch.capacity() / channels,
            });
        }
        scratch
            .ensure_len(end)
            .map_err(|_| ElasticError::PoolCapacity)?;
        let source = self
            .pending_source
            .as_deref()
            .ok_or(ElasticError::PoolCapacity)?;
        let engine = self
            .engine
            .as_mut()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
        if let Err(error) = engine.process(
            request,
            &source[..admitted_samples],
            &mut scratch[start..end],
        ) {
            scratch.truncate(start);
            return Err(error);
        }
        self.output_start_meta = self.pending_meta;
        drop(
            self.pending_source
                .as_mut()
                .ok_or(ElasticError::PoolCapacity)?
                .drain(..admitted_samples),
        );
        self.pending_meta = next_meta;
        self.output_remainder -= output_frames
            .get()
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        if admitted_source_frames == source_frames {
            self.output_remainder = 0.0;
        }
        self.active = true;
        Ok(())
    }
}
