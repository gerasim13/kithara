use std::num::NonZeroUsize;

use kithara_bufpool::HasPool;
use kithara_signal::{AudioChunk, AudioChunkInfo, FrameCount};
use kithara_stretch::{ElasticError, ElasticRequest};
use kithara_test_macros as kithara;
use num_traits::ToPrimitive;
use tracing::warn;

use super::{
    renderer::{PreparedActivation, PreparedQuantum, WarpRenderer},
    renderer_projection::ProjectionPreparation,
};

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    /// Prime the engine from the residency so its first output is the source
    /// frame presented last; the engine then reads on from the residency.
    pub(super) fn activate_prepared_quantum(
        &mut self,
        chunk: &AudioChunk,
        prepared: PreparedQuantum,
    ) -> Result<(), ElasticError> {
        let Some(activation) = prepared.activation else {
            return Ok(());
        };
        let prefix_frames = activation.prefix_frames()?;
        let (cue, sample_rate) =
            self.rendered_source_end
                .ok_or(ElasticError::EnginePreparation(
                    "Warp renderer has no presented source frontier",
                ))?;
        if chunk.meta.spec.sample_rate != sample_rate {
            return Err(ElasticError::DiscontinuousSource {
                expected: cue.to_f64().ok_or(ElasticError::SampleCountOverflow)?,
                actual: chunk
                    .meta
                    .frame_offset
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
            });
        }
        if chunk.frames() != prepared.frames {
            return Err(ElasticError::SourceFrameLimit {
                frames: chunk.frames(),
                limit: prepared.frames,
            });
        }

        let channels = usize::from(self.spec.channels.max(1));
        let history_samples = activation
            .history_frames
            .checked_mul(channels)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let discard_samples = activation
            .warm
            .output_frames()
            .checked_mul(channels)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let lookahead_end = cue
            .checked_add(
                u64::try_from(activation.history_frames)
                    .map_err(|_| ElasticError::SampleCountOverflow)?,
            )
            .ok_or(ElasticError::SampleCountOverflow)?;
        let prefix_end = cue
            .checked_add(
                u64::try_from(prefix_frames).map_err(|_| ElasticError::SampleCountOverflow)?,
            )
            .ok_or(ElasticError::SampleCountOverflow)?;
        let pitch = if self.current_keylock {
            1.0
        } else {
            f64::from(prepared.rate.speed())
        };
        self.apply_pitch(pitch)?;

        let residency = self.residency.as_ref().ok_or(ElasticError::PoolCapacity)?;
        let history_start = i64::try_from(cue)
            .ok()
            .and_then(|cue| cue.checked_sub(i64::try_from(activation.history_frames).ok()?))
            .ok_or(ElasticError::SampleCountOverflow)?;
        let range = residency.range(history_start.max(residency.start), cue, channels)?;
        let resident_history = &residency.samples[range];
        let history = if resident_history.len() == history_samples {
            resident_history
        } else {
            let history = self
                .pending_source
                .as_mut()
                .ok_or(ElasticError::PoolCapacity)?;
            history
                .ensure_len(history_samples)
                .map_err(|_| ElasticError::PoolCapacity)?;
            let missing = history_samples
                .checked_sub(resident_history.len())
                .ok_or(ElasticError::SampleCountOverflow)?;
            history[..missing].fill(0.0);
            history[missing..].copy_from_slice(resident_history);
            history.as_ref()
        };
        let resident_from = |start: u64, end: u64| {
            i64::try_from(start)
                .map_err(|_| ElasticError::SampleCountOverflow)
                .and_then(|start| residency.range(start, end, channels))
        };
        let lookahead = &residency.samples[resident_from(cue, lookahead_end)?];
        let warm = &residency.samples[resident_from(lookahead_end, prefix_end)?];
        let scratch = self
            .activation_scratch
            .as_mut()
            .ok_or(ElasticError::EnginePreparation(
                "activation scratch is unavailable",
            ))?;
        scratch
            .ensure_len(discard_samples)
            .map_err(|_| ElasticError::PoolCapacity)?;
        kithara::probe_event!(
            prime_activation,
            request_revision = prepared.rate.revision(),
            target_rate_bits = prepared.rate.speed().to_bits(),
            source_frames = activation.warm.source_frames(),
            output_frames = activation.warm.output_frames()
        );
        self.engine
            .as_mut()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
            .prime(activation.warm, history, lookahead, warm, scratch)?;
        scratch.clear();

        self.clear_pending_source();
        self.output_start_meta = Some(Self::meta_at_frame(chunk.meta, cue));
        self.resident_feed = Some(prefix_end);
        self.source_frames_admitted =
            u64::try_from(prefix_frames).map_err(|_| ElasticError::SampleCountOverflow)?;
        self.primed_source_debt = u64::try_from(activation.warm.source_frames())
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        self.active = true;
        Ok(())
    }

    fn activation_latency_frames(&self) -> Option<(usize, usize)> {
        if self.active || self.scratch.is_none() || self.rendered_source_end.is_none() {
            return None;
        }
        let latency = self.engine.as_ref()?.capabilities().latency();
        let history_frames = latency.source_frames();
        let output_frames = latency.output_frames();
        if history_frames == 0 || output_frames == 0 || self.residency.as_ref()?.end.is_none() {
            return None;
        }
        Some((history_frames, output_frames))
    }

    /// Select the next source span that fits the configured output quantum.
    /// A quantum at the renderer's own speed renders at most `output_limit`
    /// output frames; a projected quantum follows its plan.
    ///
    /// # Errors
    /// Returns pending activation or the geometry/engine admission error.
    pub fn prepare_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
        output_limit: usize,
    ) -> Result<FrameCount, crate::WarpRenderError> {
        if let Some(prepared) = self.prepared_quantum {
            return if prepared.source_start == meta.frame_offset {
                Ok(FrameCount::new(prepared.frames))
            } else {
                Err(crate::WarpRenderError::OutstandingQuantum)
            };
        }
        if self.projection.retired.is_some() || self.transition_pending() || self.engine_outdated()
        {
            return Err(crate::WarpRenderError::NeedsService);
        }
        if !self.requires_staging()
            && (self.plan.is_some()
                || self.projection.selected.is_some()
                || self.projection.active.is_some())
        {
            return Err(crate::WarpRenderError::UnsupportedProjection);
        }
        if let Some(prepared) = self.continue_resident_projection(meta, remaining)? {
            self.prepared_quantum = Some(prepared);
            return Ok(FrameCount::new(prepared.frames));
        }
        let remaining = match self.prepare_projection(meta, remaining) {
            Ok(ProjectionPreparation::Projected(prepared)) => {
                self.prepared_quantum = Some(prepared);
                return Ok(FrameCount::new(prepared.frames));
            }
            Ok(ProjectionPreparation::Service) => {
                return Err(crate::WarpRenderError::NeedsService);
            }
            Ok(ProjectionPreparation::Pending) => {
                return Err(crate::WarpRenderError::PendingActivation);
            }
            Ok(ProjectionPreparation::Preroll(frames)) => {
                return Err(crate::WarpRenderError::Preroll { frames });
            }
            Ok(ProjectionPreparation::Manual(remaining)) => remaining,
            Err(error) => {
                return Err(error.into());
            }
        };
        let rate = self.rate;
        let preview_frames = self
            .render_quantum_frames
            .map_or(remaining, NonZeroUsize::get)
            .max(1);
        let result = self
            .preview_speed(rate.speed(), preview_frames)
            .and_then(|speed| {
                self.prepared_activation(speed)
                    .map(|activation| (speed, activation))
            })
            .and_then(|(speed, activation)| {
                let prefix = activation.map_or(Ok(0), PreparedActivation::prefix_frames)?;
                let active_start = match activation {
                    Some(_) => self
                        .rendered_source_end
                        .ok_or(ElasticError::EnginePreparation(
                            "Warp renderer has no presented source frontier",
                        ))?
                        .0
                        .checked_add(
                            u64::try_from(prefix).map_err(|_| ElasticError::SampleCountOverflow)?,
                        )
                        .ok_or(ElasticError::SampleCountOverflow)?,
                    None => self.resident_feed.unwrap_or(meta.frame_offset),
                };
                let (active_frames, landing_frames) = self.source_frames_for_quantum(
                    Self::meta_at_frame(meta, active_start),
                    remaining,
                    speed,
                    output_limit,
                )?;
                let frames = usize::try_from(
                    active_start
                        .checked_add(
                            u64::try_from(active_frames)
                                .map_err(|_| ElasticError::SampleCountOverflow)?,
                        )
                        .ok_or(ElasticError::SampleCountOverflow)?
                        .saturating_sub(meta.frame_offset),
                )
                .map_err(|_| ElasticError::SampleCountOverflow)?;
                Ok(PreparedQuantum {
                    activation,
                    rate,
                    speed,
                    active_frames,
                    frames,
                    source_start: meta.frame_offset,
                    projection: None,
                    landing_frames,
                })
            });
        match result {
            Ok(prepared) => {
                self.prepared_quantum = Some(prepared);
                Ok(FrameCount::new(prepared.frames))
            }
            Err(error) => {
                self.prepared_quantum = None;
                Err(error.into())
            }
        }
    }

    /// Shrink a prepared source span at true EOF without sampling controls again.
    pub fn prepare_terminal_quantum(
        &mut self,
        meta: AudioChunkInfo,
        frames: usize,
    ) -> Option<FrameCount> {
        let mut prepared = self.prepared_quantum.take()?;
        if frames == 0 || frames > prepared.frames {
            return None;
        }
        if prepared.projection.is_some() {
            if self
                .residency
                .as_ref()
                .is_some_and(|resident| resident.prepared.is_some())
            {
                prepared.frames = frames;
                if let Some(projection) = prepared.projection.as_mut() {
                    projection.output_frames = 0;
                }
                self.prepared_quantum = Some(prepared);
                return Some(FrameCount::new(frames));
            }
            match self.resize_projected_quantum(prepared, meta, frames) {
                Ok(projected) => {
                    self.prepared_quantum = Some(projected);
                    return Some(FrameCount::new(projected.frames));
                }
                Err(error) => {
                    warn!(%error, "terminal projected source quantum sizing failed");
                    return None;
                }
            }
        }
        let shrink = prepared.frames - frames;
        if shrink > 0 {
            prepared.landing_frames = None;
        }
        prepared.frames = frames;
        if prepared.active_frames > shrink {
            prepared.active_frames -= shrink;
        } else {
            prepared.active_frames = frames;
            prepared.activation = None;
        }
        self.prepared_quantum = Some(prepared);
        Some(FrameCount::new(frames))
    }

    pub(super) fn prepared_activation(
        &self,
        speed: f32,
    ) -> Result<Option<PreparedActivation>, ElasticError> {
        if self.unity_passthrough(speed) {
            return Ok(None);
        }
        let Some((history_frames, output_frames)) = self.activation_latency_frames() else {
            return Ok(None);
        };
        let source_frames = output_frames
            .to_f64()
            .map(|frames| (frames * f64::from(speed)).round())
            .and_then(|frames| frames.to_usize())
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok(Some(PreparedActivation {
            history_frames,
            warm: ElasticRequest::new(source_frames, output_frames)?,
        }))
    }
}
