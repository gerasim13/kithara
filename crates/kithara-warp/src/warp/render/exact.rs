use firewheel_core::param::smoother::SmoothedParam;
use kithara_signal::AudioChunkInfo;
use kithara_stretch::{ElasticCursor, ElasticError};
use num_traits::ToPrimitive;

use super::renderer::{PreparedExact, WarpRenderer};

impl WarpRenderer {
    fn render_exact_plan(
        &mut self,
        meta: AudioChunkInfo,
        samples: &[f32],
        channels: usize,
        exact: &PreparedExact,
    ) -> Result<usize, ElasticError> {
        let pitch = if self.controls.keylock() {
            1.0
        } else {
            f64::from(exact.speed)
        };
        self.apply_pitch(pitch)?;

        let mut consumed = 0usize;
        for segment in exact.plan.segments() {
            let frame_offset = meta
                .frame_offset
                .checked_add(
                    u64::try_from(consumed).map_err(|_| ElasticError::SpanArithmeticOverflow)?,
                )
                .ok_or(ElasticError::SpanArithmeticOverflow)?;
            let actual = frame_offset
                .to_i64()
                .ok_or(ElasticError::SpanArithmeticOverflow)?;
            if segment.source_start() != actual {
                return Err(ElasticError::DiscontinuousSource {
                    expected: segment
                        .source_start()
                        .to_f64()
                        .ok_or(ElasticError::SpanArithmeticOverflow)?,
                    actual: actual
                        .to_f64()
                        .ok_or(ElasticError::SpanArithmeticOverflow)?,
                });
            }

            let request = segment.request();
            let source_end = consumed
                .checked_add(request.source_frames())
                .ok_or(ElasticError::SpanArithmeticOverflow)?;
            let source_start_sample = consumed
                .checked_mul(channels)
                .ok_or(ElasticError::SampleCountOverflow)?;
            let source_end_sample = source_end
                .checked_mul(channels)
                .ok_or(ElasticError::SampleCountOverflow)?;
            let source = samples.get(source_start_sample..source_end_sample).ok_or(
                ElasticError::SourceSampleCount {
                    actual: samples.len(),
                    expected: source_end_sample,
                },
            )?;
            let output_samples = request
                .output_frames()
                .checked_mul(channels)
                .ok_or(ElasticError::SampleCountOverflow)?;
            let start = self.scratch.as_deref().map_or(0, <[f32]>::len);
            let end = start
                .checked_add(output_samples)
                .ok_or(ElasticError::SampleCountOverflow)?;
            if start == 0 {
                self.output_start_meta = Some(Self::meta_at_frame(meta, frame_offset));
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
                .map_err(|_| ElasticError::SamplePoolBudgetExhausted)?;
            let engine = self
                .engine
                .as_mut()
                .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
            if let Err(error) = engine.process(request, source, &mut scratch[start..end]) {
                scratch.truncate(start);
                return Err(error);
            }
            self.active = true;
            consumed = source_end;
        }
        Ok(consumed)
    }

    pub(super) fn render_prepared_exact(
        &mut self,
        meta: AudioChunkInfo,
        samples: &[f32],
        channels: usize,
        first: PreparedExact,
        direct: bool,
    ) -> Result<(SmoothedParam, Option<ElasticCursor>), ElasticError> {
        let frames = samples.len() / channels;
        let mut consumed = 0usize;
        let mut applied_speed: SmoothedParam;
        let mut cursor: Option<ElasticCursor>;
        let mut next = Some(first);
        for _ in 0..frames {
            let exact = next.take().ok_or(ElasticError::EmptySpanPlan)?;
            let frame_offset = meta
                .frame_offset
                .checked_add(
                    u64::try_from(consumed).map_err(|_| ElasticError::SpanArithmeticOverflow)?,
                )
                .ok_or(ElasticError::SpanArithmeticOverflow)?;
            let current_meta = Self::meta_at_frame(meta, frame_offset);
            let source_frames = self.render_exact_plan(
                current_meta,
                &samples[consumed * channels..],
                channels,
                &exact,
            )?;
            if source_frames == 0 {
                return Err(ElasticError::StationarySourceSpan);
            }
            consumed = consumed
                .checked_add(source_frames)
                .ok_or(ElasticError::SpanArithmeticOverflow)?;
            applied_speed = exact.next_speed;
            cursor = Some(exact.plan.cursor());

            if consumed == frames {
                return Ok((applied_speed, cursor));
            }
            if !direct {
                let expected = consumed
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                return Err(ElasticError::SourceSampleCount {
                    actual: samples.len(),
                    expected,
                });
            }

            let frame_offset = meta
                .frame_offset
                .checked_add(
                    u64::try_from(consumed).map_err(|_| ElasticError::SpanArithmeticOverflow)?,
                )
                .ok_or(ElasticError::SpanArithmeticOverflow)?;
            let next_meta = Self::meta_at_frame(meta, frame_offset);
            let target = self.controls.speed();
            if let Some(exact) = self.exact_plan_for_remaining(
                next_meta,
                frames - consumed,
                applied_speed,
                cursor,
                target,
            )? {
                next = Some(exact);
                continue;
            }

            let (_, speed, _) =
                Self::preview_speed_from(applied_speed, target, self.output_quantum_limit())?;
            let output_start = self.scratch.as_deref().map_or(0, <[f32]>::len) / channels;
            self.render_active(
                next_meta,
                &samples[consumed * channels..],
                speed,
                channels,
                frames - consumed,
            )?;
            let output_end = self.scratch.as_deref().map_or(0, <[f32]>::len) / channels;
            return Ok((
                Self::advance_speed(
                    applied_speed,
                    target,
                    output_end.saturating_sub(output_start),
                )?,
                None,
            ));
        }
        Err(ElasticError::EnginePreparation(
            "time-stretch exact render exceeded its source-frame iteration bound",
        ))
    }
}
