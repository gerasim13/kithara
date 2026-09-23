use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_signal::{AudioChunkInfo, SessionFrame};
use kithara_stretch::{ElasticError, ElasticLatency, ElasticRequest};
use num_traits::ToPrimitive;

use super::{
    renderer::{PreparedQuantum, WarpRenderer},
    renderer_projection::ProjectedQuantum,
};
use crate::WarpPlan;

#[derive(Clone, Copy)]
pub(super) struct ResidentRequest {
    pub(super) source_start: u64,
    pub(super) source_end: u64,
    pub(super) prime: Option<(u64, usize, ElasticRequest)>,
    pub(super) projection: ProjectedQuantum,
    pub(super) rate: crate::temporal::RateTarget,
}

/// A bounded decoded-source window, shared by activation and backend replacement.
/// It contains history and lookahead, never independently scheduled output.
pub(super) struct SourceResidency {
    pub(super) samples: SampleBuffer,
    pub(super) replacement: SampleBuffer,
    pub(super) replacement_offset: usize,
    pub(super) history_frames: usize,
    pub(super) start: i64,
    pub(super) offset: usize,
    pub(super) end: Option<u64>,
    pub(super) primed: bool,
    pub(super) prepared: Option<ResidentRequest>,
}

impl SourceResidency {
    pub(super) fn prepare<S: HasPool<f32>>(
        pools: &PoolRegion<S>,
        reusable: Option<Self>,
        history_frames: usize,
        resident_frames: usize,
        replacement_frames: usize,
        channels: usize,
    ) -> Result<Self, ElasticError> {
        let mut residency = reusable.unwrap_or_else(|| Self {
            samples: pools.get::<f32>(),
            replacement: pools.get::<f32>(),
            replacement_offset: 0,
            history_frames,
            start: 0,
            offset: 0,
            end: None,
            primed: false,
            prepared: None,
        });
        for (buffer, frames) in [
            (&mut residency.samples, resident_frames),
            (&mut residency.replacement, replacement_frames),
        ] {
            let length = buffer.len();
            buffer
                .ensure_len(
                    frames
                        .checked_mul(channels)
                        .ok_or(ElasticError::SampleCountOverflow)?,
                )
                .map_err(|_| ElasticError::PoolCapacity)?;
            buffer.truncate(length);
        }
        residency.history_frames = history_frames;
        Ok(residency)
    }

    pub(super) fn pad_to(&mut self, end: u64, channels: usize) -> Result<(), ElasticError> {
        let stored_frames = self.samples.len().saturating_sub(self.offset) / channels;
        let stored_end = self
            .start
            .checked_add(
                i64::try_from(stored_frames).map_err(|_| ElasticError::SampleCountOverflow)?,
            )
            .and_then(|end| u64::try_from(end).ok())
            .ok_or(ElasticError::SampleCountOverflow)?;
        let pad = usize::try_from(end.saturating_sub(stored_end))
            .map_err(|_| ElasticError::SampleCountOverflow)?
            .checked_mul(channels)
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.make_room(pad)?;
        let length = self.samples.len();
        self.samples
            .ensure_len(length + pad)
            .map_err(|_| ElasticError::PoolCapacity)?;
        self.samples[length..].fill(0.0);
        Ok(())
    }

    fn make_room(&mut self, samples: usize) -> Result<(), ElasticError> {
        if self.samples.len() + samples > self.samples.capacity() && self.offset > 0 {
            self.samples.copy_within(self.offset.., 0);
            self.samples.truncate(self.samples.len() - self.offset);
            self.offset = 0;
        }
        if self.samples.len() + samples > self.samples.capacity() {
            return Err(ElasticError::PoolCapacity);
        }
        Ok(())
    }

    pub(super) fn retain_manual(
        &mut self,
        mut meta: AudioChunkInfo,
        input: &[f32],
        frontier: Option<u64>,
    ) -> Result<(), ElasticError> {
        let channels = usize::from(meta.spec.channels.max(1));
        if let Some(frontier) = frontier {
            self.retain_from(frontier, channels);
        }
        let frames = input.len() / channels;
        let skip = frames.saturating_sub(self.samples.capacity() / channels);
        if skip > 0 || self.end.is_some_and(|end| end != meta.frame_offset) {
            self.samples.clear();
            self.offset = 0;
            meta.frame_offset = meta
                .frame_offset
                .saturating_add(u64::try_from(skip).unwrap_or(u64::MAX));
            self.start =
                i64::try_from(meta.frame_offset).map_err(|_| ElasticError::SampleCountOverflow)?;
            self.end = Some(meta.frame_offset);
        }
        self.append(meta, &input[skip * channels..])
    }

    pub(super) fn append(
        &mut self,
        meta: AudioChunkInfo,
        input: &[f32],
    ) -> Result<(), ElasticError> {
        let channels = usize::from(meta.spec.channels.max(1));
        let frames = input.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        if self.end.is_none() {
            let prefix = self
                .history_frames
                .min(usize::try_from(meta.frame_offset).unwrap_or(usize::MAX));
            self.start =
                i64::try_from(meta.frame_offset).map_err(|_| ElasticError::SampleCountOverflow)?;
            // Only the portion before the physical recording is known silence.
            if meta.frame_offset == 0 {
                let history = self
                    .history_frames
                    .min(self.samples.capacity() / channels - frames);
                self.start =
                    -i64::try_from(history).map_err(|_| ElasticError::SampleCountOverflow)?;
                self.samples
                    .ensure_len(history * channels)
                    .map_err(|_| ElasticError::PoolCapacity)?;
                self.samples.fill(0.0);
            } else if prefix == 0 {
                self.samples.clear();
            }
            self.end = Some(meta.frame_offset);
        }
        let expected = self.end.ok_or(ElasticError::EmptySource)?;
        if expected != meta.frame_offset {
            return Err(ElasticError::DiscontinuousSource {
                expected: expected.to_f64().ok_or(ElasticError::SampleCountOverflow)?,
                actual: meta
                    .frame_offset
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
            });
        }
        self.make_room(input.len())?;
        self.samples
            .try_extend_from_slice(input)
            .map_err(|_| ElasticError::PoolCapacity)?;
        self.end = Some(
            expected
                .checked_add(u64::try_from(frames).map_err(|_| ElasticError::SampleCountOverflow)?)
                .ok_or(ElasticError::SampleCountOverflow)?,
        );
        Ok(())
    }

    pub(super) fn retain_from(&mut self, source: u64, channels: usize) {
        let first = i64::try_from(source)
            .unwrap_or(i64::MAX)
            .saturating_sub(i64::try_from(self.history_frames).unwrap_or(i64::MAX));
        let remove = usize::try_from(first.saturating_sub(self.start).max(0))
            .unwrap_or(usize::MAX)
            .saturating_mul(channels)
            .min(self.samples.len().saturating_sub(self.offset));
        self.offset += remove;
        self.start = self
            .start
            .saturating_add(i64::try_from(remove / channels).unwrap_or(i64::MAX));
    }

    pub(super) fn range(
        &self,
        start: i64,
        end: u64,
        channels: usize,
    ) -> Result<std::ops::Range<usize>, ElasticError> {
        let first = start
            .checked_sub(self.start)
            .and_then(|frames| usize::try_from(frames).ok())
            .and_then(|frames| frames.checked_mul(channels))
            .ok_or(ElasticError::EnginePreparation(
                "required source history is no longer resident",
            ))?;
        let first = first
            .checked_add(self.offset)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let last = i64::try_from(end)
            .ok()
            .and_then(|end| end.checked_sub(self.start))
            .and_then(|frames| usize::try_from(frames).ok())
            .and_then(|frames| frames.checked_mul(channels))
            .ok_or(ElasticError::SampleCountOverflow)?;
        let last = last
            .checked_add(self.offset)
            .ok_or(ElasticError::SampleCountOverflow)?;
        if last > self.samples.len() || first > last {
            return Err(ElasticError::EnginePreparation(
                "required projected source is not resident",
            ));
        }
        Ok(first..last)
    }

    pub(super) fn clear(&mut self) {
        self.samples.clear();
        self.replacement.clear();
        self.replacement_offset = 0;
        self.start = 0;
        self.offset = 0;
        self.end = None;
        self.primed = false;
        self.prepared = None;
    }
}

impl<S: HasPool<f32>> WarpRenderer<S> {
    pub(super) fn continue_resident_projection(
        &self,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Result<Option<PreparedQuantum>, ElasticError> {
        let Some(resident) = self.residency.as_ref() else {
            return Ok(None);
        };
        let Some(request) = resident.prepared else {
            return Ok(None);
        };
        let end = resident.end.unwrap_or(meta.frame_offset);
        if end != meta.frame_offset {
            return Err(ElasticError::EnginePreparation(
                "resident continuation has a different decoder origin",
            ));
        }
        let needed = usize::try_from(request.source_end.saturating_sub(end))
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        let frames = needed
            .min(remaining.max(1))
            .min(self.source_block_frames.get());
        let active_frames = usize::try_from(request.source_end - request.source_start)
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        let speed = (active_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?
            / request
                .projection
                .output_frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?)
        .to_f32()
        .ok_or(ElasticError::SampleCountOverflow)?;
        let mut projection = request.projection;
        if frames < needed {
            projection.output_frames = 0;
        }
        Ok(Some(PreparedQuantum {
            source_start: meta.frame_offset,
            activation: None,
            projection: Some(projection),
            rate: request.rate,
            speed,
            active_frames,
            frames,
        }))
    }

    pub(super) fn prepare_resident_projection(
        &mut self,
        plan: &WarpPlan,
        start: SessionFrame,
        meta: AudioChunkInfo,
        remaining: usize,
        pending_activation: Option<SessionFrame>,
        output_offset: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let capabilities = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation(
                "projected engine is unavailable",
            ))?
            .capabilities();
        let latency = capabilities.latency();
        let axis = plan.output_axis().ok_or(ElasticError::EnginePreparation(
            "projected output axis is unavailable",
        ))?;
        let endpoint = |frames: usize| -> Result<SessionFrame, ElasticError> {
            let frames = frames.to_f64().ok_or(ElasticError::SampleCountOverflow)?
                * f64::from(axis.sample_rate().get())
                / f64::from(self.spec.sample_rate.get());
            Ok(SessionFrame::new(
                i64::from(plan.activation().output())
                    .checked_add(
                        frames
                            .round()
                            .to_i64()
                            .ok_or(ElasticError::SampleCountOverflow)?,
                    )
                    .ok_or(ElasticError::SampleCountOverflow)?,
            ))
        };
        let source_at = |at, label| {
            Self::projected_source(plan, at).map_err(|error| match error {
                ElasticError::EnginePreparation(_) => ElasticError::EnginePreparation(label),
                error => error,
            })
        };
        let audible_start = source_at(start, "projected audible start is uncovered")?;
        let advanced_offset = output_offset
            .checked_add(latency.output_frames())
            .ok_or(ElasticError::SampleCountOverflow)?;
        let advanced_start = endpoint(advanced_offset)?;
        let Ok(delayed_start) = source_at(advanced_start, "projected admitted start is uncovered")
        else {
            return self.prepare_finite_resident_projection(
                plan,
                start,
                meta,
                remaining,
                pending_activation,
                output_offset,
            );
        };
        let input_start = delayed_start
            .checked_add(
                u64::try_from(latency.source_frames())
                    .map_err(|_| ElasticError::SampleCountOverflow)?,
            )
            .ok_or(ElasticError::SampleCountOverflow)?;
        let resident = self.residency.as_ref().ok_or(ElasticError::PoolCapacity)?;
        let resident_end = resident.end.unwrap_or(meta.frame_offset);
        if resident_end != meta.frame_offset {
            return Err(ElasticError::DiscontinuousSource {
                expected: resident_end
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
                actual: meta
                    .frame_offset
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
            });
        }
        let mut virtual_meta = meta;
        virtual_meta.frame_offset = delayed_start;
        // Latency is expressed once at the engine boundary. The map itself
        // already converts source frames per session output frame.
        let prepared = match self.projected_span(
            plan,
            advanced_start,
            virtual_meta,
            remaining.max(1),
            pending_activation.map(|at| {
                SessionFrame::new(
                    i64::from(at)
                        .saturating_add(i64::from(advanced_start).saturating_sub(i64::from(start))),
                )
            }),
            advanced_offset,
        ) {
            Ok(prepared) => prepared,
            Err(ElasticError::EmptyOutput) => {
                return self.prepare_finite_resident_projection(
                    plan,
                    start,
                    meta,
                    remaining,
                    pending_activation,
                    output_offset,
                );
            }
            Err(error) => return Err(error),
        };
        let mut projection = prepared.projection.ok_or(ElasticError::EmptyOutput)?;
        let source_end = projection
            .end
            .source()
            .checked_add(
                u64::try_from(latency.source_frames())
                    .map_err(|_| ElasticError::SampleCountOverflow)?,
            )
            .ok_or(ElasticError::SampleCountOverflow)?;
        let audible_end = endpoint(
            output_offset
                .checked_add(projection.output_frames)
                .ok_or(ElasticError::SampleCountOverflow)?,
        )?;
        projection.output_offset = output_offset;
        projection.output_start = start;
        projection.end = plan.map().reanchor(
            Self::projected_endpoint(
                plan,
                audible_end,
                meta.frame_offset
                    .checked_add(
                        u64::try_from(remaining).map_err(|_| ElasticError::SampleCountOverflow)?,
                    )
                    .ok_or(ElasticError::SampleCountOverflow)?,
            )?,
            audible_end,
        );
        let prime = self.resident_prime(plan, audible_start, delayed_start, latency)?;
        self.prepare_resident_request(
            ResidentRequest {
                source_start: input_start,
                source_end,
                prime,
                projection,
                rate: prepared.rate,
            },
            meta,
            remaining,
        )
    }

    fn resident_prime(
        &self,
        plan: &WarpPlan,
        audible_start: u64,
        warm_end: u64,
        latency: ElasticLatency,
    ) -> Result<Option<(u64, usize, ElasticRequest)>, ElasticError> {
        let resident = self.residency.as_ref().ok_or(ElasticError::PoolCapacity)?;
        let same_map = self
            .projection
            .active
            .as_ref()
            .is_some_and(|active| std::ptr::eq(active.as_ref(), plan));
        if (resident.primed && same_map) || latency.output_frames() == 0 {
            return Ok(None);
        }
        if audible_start > 0 {
            let history_start = i64::try_from(audible_start)
                .ok()
                .and_then(|cue| {
                    i64::try_from(latency.source_frames())
                        .ok()
                        .and_then(|history| cue.checked_sub(history))
                })
                .ok_or(ElasticError::SampleCountOverflow)?;
            resident.range(
                history_start,
                audible_start,
                usize::from(self.spec.channels.max(1)),
            )?;
        }
        let warm_frames = warm_end
            .checked_sub(audible_start)
            .and_then(|frames| usize::try_from(frames).ok())
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok(Some((
            audible_start,
            latency.source_frames(),
            ElasticRequest::new(warm_frames, latency.output_frames())?,
        )))
    }

    fn prepare_resident_request(
        &mut self,
        request: ResidentRequest,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let resident = self.residency.as_mut().ok_or(ElasticError::PoolCapacity)?;
        let end = resident.end.unwrap_or(meta.frame_offset);
        if end != meta.frame_offset {
            return Err(ElasticError::EnginePreparation(
                "resident request has a different decoder origin",
            ));
        }
        let needed = usize::try_from(request.source_end.saturating_sub(end))
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        let frames = needed
            .min(self.source_block_frames.get())
            .min(remaining.max(1));
        let active_frames = usize::try_from(
            request
                .source_end
                .checked_sub(request.source_start)
                .ok_or(ElasticError::SampleCountOverflow)?,
        )
        .map_err(|_| ElasticError::SampleCountOverflow)?;
        let speed = (active_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?
            / request
                .projection
                .output_frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?)
        .to_f32()
        .ok_or(ElasticError::SampleCountOverflow)?;
        resident.prepared = Some(request);
        let mut projection = request.projection;
        if frames < needed {
            projection.output_frames = 0;
        }
        Ok(PreparedQuantum {
            source_start: meta.frame_offset,
            activation: None,
            projection: Some(projection),
            rate: request.rate,
            speed,
            active_frames,
            frames,
        })
    }

    fn prepare_finite_resident_projection(
        &mut self,
        plan: &WarpPlan,
        start: SessionFrame,
        meta: AudioChunkInfo,
        remaining: usize,
        pending_activation: Option<SessionFrame>,
        output_offset: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let uncovered = ElasticError::EnginePreparation("projected terminal extent is uncovered");
        let Some(crate::MapAxis::Asset(axis)) = plan.source_axis() else {
            return Err(uncovered);
        };
        let crate::AssetExtent::Bounded(end) = axis.extent() else {
            return Err(uncovered);
        };
        let source_end =
            crate::AssetFrame::new(end.to_f64().ok_or(ElasticError::SampleCountOverflow)?)
                .map_err(|_| ElasticError::SampleCountOverflow)?;
        let crate::BeatGridQuery::Resolved(output_end) = plan.map().output_at(source_end) else {
            return Err(uncovered);
        };
        let latency = self
            .engine
            .as_ref()
            .ok_or(ElasticError::PoolCapacity)?
            .capabilities()
            .latency();
        let terminal_offset = self.projected_output_offset(plan, output_end)?;
        if terminal_offset > output_offset.saturating_add(latency.output_frames()) {
            return Err(uncovered);
        }
        let audible_start = Self::projected_source(plan, start)?;
        let covered_source = end
            .checked_sub(audible_start)
            .ok_or(ElasticError::EmptySource)?;
        let covered_output = terminal_offset
            .checked_sub(output_offset)
            .filter(|frames| *frames > 0)
            .ok_or(ElasticError::EmptyOutput)?;
        // This chord sizes terminal DSP silence; it does not extend the map.
        // Padding is materialized only by flush after the decoder reports EOF.
        let warm_frames = (covered_source
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?
            * latency
                .output_frames()
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?
            / covered_output
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?)
        .round()
        .to_u64()
        .ok_or(ElasticError::SampleCountOverflow)?;
        let warm_end = audible_start
            .checked_add(warm_frames)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let input_start = warm_end
            .checked_add(
                u64::try_from(latency.source_frames())
                    .map_err(|_| ElasticError::SampleCountOverflow)?,
            )
            .ok_or(ElasticError::SampleCountOverflow)?;
        let mut audible_meta = meta;
        audible_meta.frame_offset = audible_start;
        let prepared = self.projected_span(
            plan,
            start,
            audible_meta,
            remaining,
            pending_activation,
            output_offset,
        )?;
        let projection = prepared.projection.ok_or(ElasticError::EmptyOutput)?;
        let input_end = input_start
            .checked_add(
                projection
                    .end
                    .source()
                    .checked_sub(audible_start)
                    .ok_or(ElasticError::SampleCountOverflow)?,
            )
            .ok_or(ElasticError::SampleCountOverflow)?;
        let request = ResidentRequest {
            source_start: input_start,
            source_end: input_end,
            prime: self.resident_prime(plan, audible_start, warm_end, latency)?,
            projection,
            rate: prepared.rate,
        };
        self.prepare_resident_request(request, meta, remaining)
    }
}
