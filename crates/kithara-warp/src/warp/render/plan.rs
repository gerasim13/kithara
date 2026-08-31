use firewheel_core::param::smoother::SmoothedParam;
use kithara_signal::AudioChunkInfo;
use kithara_stretch::{ElasticCursor, ElasticError, ElasticSpan, ElasticSpanConfig};
use num_traits::ToPrimitive;

use super::renderer::{PreparedExact, PreparedQuantum, WarpRenderer};

impl WarpRenderer {
    pub(super) fn preview_speed_from(
        mut applied_speed: SmoothedParam,
        target: f32,
        output_frames: usize,
    ) -> Result<(f64, f32, SmoothedParam), ElasticError> {
        if output_frames == 0 {
            return Err(ElasticError::EmptyOutput);
        }
        applied_speed.set_value(target);
        let mut total = 0.0_f64;
        for _ in 0..output_frames {
            total += f64::from(applied_speed.next_smoothed());
        }
        applied_speed.settle();
        let output_frames = output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let speed = (total / output_frames)
            .to_f32()
            .filter(|speed| speed.is_finite() && *speed > 0.0)
            .ok_or(ElasticError::InvalidRate(total / output_frames))?;
        Ok((total, speed, applied_speed))
    }

    pub(super) fn advance_speed(
        applied_speed: SmoothedParam,
        target: f32,
        output_frames: usize,
    ) -> Result<SmoothedParam, ElasticError> {
        if output_frames == 0 {
            let mut held = applied_speed;
            held.set_value(target);
            return Ok(held);
        }
        Self::preview_speed_from(applied_speed, target, output_frames)
            .map(|(_, _, preview)| preview)
    }

    pub(super) fn output_quantum_limit(&self) -> usize {
        self.engine.as_ref().map_or_else(
            || self.render_quantum_frames.get(),
            |engine| {
                engine
                    .capabilities()
                    .max_output_frames()
                    .min(self.render_quantum_frames.get())
            },
        )
    }

    fn build_exact_plan(
        &self,
        meta: AudioChunkInfo,
        output_frames: usize,
        applied_speed: SmoothedParam,
        cursor: Option<ElasticCursor>,
        target: f32,
    ) -> Result<PreparedExact, ElasticError> {
        let (source_advance, speed, next_speed) =
            Self::preview_speed_from(applied_speed, target, output_frames)?;
        let source_start = cursor.map_or_else(
            || {
                meta.frame_offset
                    .to_f64()
                    .ok_or(ElasticError::SpanArithmeticOverflow)
            },
            |cursor| Ok(cursor.continuous()),
        )?;
        let source_end = source_start + source_advance;
        let span = ElasticSpan::try_from((source_start..source_end, output_frames))?;
        let capabilities = self
            .engine
            .as_ref()
            .map(|engine| engine.capabilities())
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?;
        let config = ElasticSpanConfig::builder().build()?;
        let plan = kithara_stretch::ElasticSpanPlan::new([span], cursor, capabilities, config)?;
        Ok(PreparedExact {
            next_speed,
            plan,
            speed,
        })
    }

    pub(super) fn prepared_source_frames(
        prepared: &PreparedQuantum,
    ) -> Result<usize, ElasticError> {
        match prepared {
            PreparedQuantum::Exact(exact) => Self::exact_source_frames(exact),
            PreparedQuantum::Legacy { source_frames, .. } => Ok(*source_frames),
        }
    }

    fn exact_source_frames(exact: &PreparedExact) -> Result<usize, ElasticError> {
        exact
            .plan
            .segments()
            .iter()
            .try_fold(0usize, |total, segment| {
                total
                    .checked_add(segment.request().source_frames())
                    .ok_or(ElasticError::SpanArithmeticOverflow)
            })
    }

    pub(super) fn exact_plan_for_remaining(
        &self,
        meta: AudioChunkInfo,
        remaining: usize,
        applied_speed: SmoothedParam,
        cursor: Option<ElasticCursor>,
        target: f32,
    ) -> Result<Option<PreparedExact>, ElasticError> {
        if remaining == 0 {
            return Err(ElasticError::EmptySource);
        }
        let output_limit = self.output_quantum_limit();
        let full = self.build_exact_plan(meta, output_limit, applied_speed, cursor, target)?;
        if Self::exact_source_frames(&full)? <= remaining {
            return Ok(Some(full));
        }

        let mut best = None;
        let mut low = 1usize;
        let mut high = output_limit.saturating_sub(1);
        while low <= high {
            let output_frames = low + (high - low) / 2;
            let candidate =
                self.build_exact_plan(meta, output_frames, applied_speed, cursor, target)?;
            let source_frames =
                candidate
                    .plan
                    .segments()
                    .iter()
                    .try_fold(0usize, |total, segment| {
                        total
                            .checked_add(segment.request().source_frames())
                            .ok_or(ElasticError::SpanArithmeticOverflow)
                    })?;
            if source_frames <= remaining {
                best = Some(candidate);
                low = output_frames.saturating_add(1);
            } else if output_frames == 1 {
                break;
            } else {
                high = output_frames - 1;
            }
        }
        Ok(best)
    }

    fn exact_plan_enabled(&self, target: f32) -> bool {
        let channels = usize::from(self.spec.channels.max(1));
        self.plan.is_none()
            && self.pending_frames(channels) == 0
            && self.output_remainder == 0.0
            && !self.can_passthrough(target)
    }

    fn legacy_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
        applied_speed: SmoothedParam,
    ) -> Result<PreparedQuantum, ElasticError> {
        let target = self.controls.speed();
        let (_, speed, _) =
            Self::preview_speed_from(applied_speed, target, self.output_quantum_limit())?;
        let source_frames = self.source_frames_for_quantum(meta, remaining, speed)?;
        Ok(PreparedQuantum::Legacy {
            source_frames,
            speed,
            target,
        })
    }

    pub(super) fn direct_plan(
        &mut self,
        meta: AudioChunkInfo,
        frames: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let target = self.controls.speed();
        if frames > 0
            && self.exact_plan_enabled(target)
            && let Some(exact) = self.exact_plan_for_remaining(
                meta,
                frames,
                self.applied_speed,
                self.exact_cursor,
                target,
            )?
        {
            return Ok(PreparedQuantum::Exact(exact));
        }
        let (_, speed, _) =
            Self::preview_speed_from(self.applied_speed, target, self.output_quantum_limit())?;
        Ok(PreparedQuantum::Legacy {
            source_frames: frames,
            speed,
            target,
        })
    }

    pub(super) fn scheduler_plan(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let target = self.controls.speed();
        if self.exact_plan_enabled(target)
            && let Some(exact) = self.exact_plan_for_remaining(
                meta,
                remaining,
                self.applied_speed,
                self.exact_cursor,
                target,
            )?
        {
            return Ok(PreparedQuantum::Exact(exact));
        }
        self.legacy_quantum(meta, remaining, self.applied_speed)
    }

    pub(super) fn source_block_limit(
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

    pub(super) fn balanced_source_block(remaining: usize, limit: usize) -> usize {
        let partitions = remaining.div_ceil(limit);
        remaining.div_ceil(partitions)
    }
}
