use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;
use kithara_signal::AudioChunkInfo;
use kithara_stretch::ElasticError;
use num_traits::ToPrimitive;

use super::renderer::{PreparedQuantum, WarpRenderer};
use crate::{AssetFrame, BeatGridQuery, MapAxis, SessionAxis, SessionFrame, WarpPlan};

#[derive(Clone, Copy)]
pub(super) struct ProjectedQuantum {
    pub(super) output_start: SessionFrame,
    pub(super) end: crate::WarpCursor,
    pub(super) output_frames: usize,
    pub(super) output_offset: usize,
}

#[derive(Default)]
pub(super) struct ProjectionState {
    pub(super) active: Option<Arc<WarpPlan>>,
    pub(super) cursor: Option<crate::WarpCursor>,
    pub(super) prepared: Option<Arc<WarpPlan>>,
    pub(super) retired: Option<Arc<WarpPlan>>,
    pub(super) selected: Option<Arc<WarpPlan>>,
    pub(super) output_frames: usize,
}

impl ProjectionState {
    pub(super) fn new(config: &crate::WarpConfig) -> Self {
        let selected = config.plan().load();
        Self {
            active: selected
                .clone()
                .filter(|plan| plan.activation().output() == SessionFrame::new(0)),
            selected,
            ..Self::default()
        }
    }
}

pub(super) enum ProjectionPreparation {
    Pending,
    Service,
    Manual(usize),
    Projected(PreparedQuantum),
}

impl<S: HasPool<f32>> WarpRenderer<S> {
    pub(super) fn prepare_projection(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Result<ProjectionPreparation, ElasticError> {
        self.projection.prepared = None;
        let selected = self.projection.selected.clone();
        let Some(selected) = selected else {
            return Ok(ProjectionPreparation::Manual(remaining));
        };
        let same = self
            .projection
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &selected));
        let activation = selected.activation();
        let output = self
            .projection
            .cursor
            .map(|cursor| cursor.output())
            .or_else(|| {
                self.committed
                    .as_ref()
                    .map(|snapshot| snapshot.frontier().output())
            })
            .or_else(|| {
                self.context
                    .load()
                    .map(|snapshot| snapshot.frontier().output())
            });
        let reached = output.map_or_else(
            || activation.output() == SessionFrame::new(0),
            |output| output >= activation.output(),
        );
        let mut plan = if same || !reached {
            let Some(active) = self.projection.active.as_ref() else {
                let until_activation = activation
                    .source()
                    .checked_sub(meta.frame_offset)
                    .and_then(|frames| usize::try_from(frames).ok())
                    .ok_or(ElasticError::EnginePreparation(
                        "source has passed an unapplied projection",
                    ))?;
                if until_activation == 0 {
                    return Ok(ProjectionPreparation::Pending);
                }
                return Ok(ProjectionPreparation::Manual(
                    remaining.min(until_activation),
                ));
            };
            Arc::clone(active)
        } else {
            selected
        };
        let mut start = if same || !reached {
            self.projection
                .cursor
                .map_or_else(|| plan.activation().output(), |cursor| cursor.output())
        } else {
            activation.output()
        };
        let prior_start = start;
        if !same && reached && self.projection.active.is_none() {
            if let Some(output) = output {
                start = output;
            } else if self.rendered_source_end == Some((meta.frame_offset, meta.spec.sample_rate)) {
                let source = AssetFrame::new(
                    meta.frame_offset
                        .to_f64()
                        .ok_or(ElasticError::SampleCountOverflow)?,
                )
                .map_err(|_| ElasticError::SampleCountOverflow)?;
                let BeatGridQuery::Resolved(output) = plan.map().output_at(source) else {
                    return Err(ElasticError::EnginePreparation(
                        "presented source is outside projection",
                    ));
                };
                start = output;
            }
        }
        let until_activation = (!same && !reached).then_some(activation.output());
        let output_offset = if start != prior_start {
            self.projected_output_offset(&plan, start)?
        } else if same || !reached {
            self.projection.output_frames
        } else {
            0
        };
        if self.pending_frames(usize::from(self.spec.channels.max(1))) != 0 {
            return Err(ElasticError::EnginePreparation(
                "previous source span awaits output before projection",
            ));
        }
        let prepared = match self.prepare_resident_projection(
            &plan,
            start,
            meta,
            remaining,
            until_activation,
            output_offset,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let Some(active) = self
                    .projection
                    .active
                    .clone()
                    .filter(|active| !Arc::ptr_eq(active, &plan))
                else {
                    return Err(error);
                };
                let start = self
                    .projection
                    .cursor
                    .map_or_else(|| active.activation().output(), |cursor| cursor.output());
                let prepared = self.prepare_resident_projection(
                    &active,
                    start,
                    meta,
                    remaining,
                    None,
                    self.projection.output_frames,
                )?;
                plan = active;
                prepared
            }
        };
        let replacing = self.active
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.capabilities().latency().output_frames() > 0)
            && self
                .projection
                .active
                .as_ref()
                .is_none_or(|active| !Arc::ptr_eq(active, &plan));
        if replacing {
            self.projection.prepared = Some(plan);
            self.backend_transition_pending = true;
            return Ok(ProjectionPreparation::Service);
        }
        self.projection.prepared = Some(plan);
        Ok(ProjectionPreparation::Projected(prepared))
    }
}

impl<S: HasPool<f32>> WarpRenderer<S> {
    /// A finite source endpoint can map between session frames. Invert that
    /// covered endpoint before rounding to the engine lattice; do not extend
    /// geometry or manufacture a rate beyond it.
    pub(super) fn projected_endpoint(
        plan: &WarpPlan,
        output: SessionFrame,
        source_limit: u64,
    ) -> Result<u64, ElasticError> {
        match plan.source_at(output) {
            BeatGridQuery::Resolved(source) => Self::source_frame(source),
            BeatGridQuery::OutsideDomain => {
                let source = AssetFrame::new(
                    source_limit
                        .to_f64()
                        .ok_or(ElasticError::SampleCountOverflow)?,
                )
                .map_err(|_| ElasticError::SampleCountOverflow)?;
                match plan.map().output_at(source) {
                    BeatGridQuery::Resolved(end) if end == output => Ok(source_limit),
                    _ => Err(ElasticError::EmptyOutput),
                }
            }
            _ => Err(ElasticError::EnginePreparation(
                "projected source endpoint is uncovered",
            )),
        }
    }

    pub(super) fn projected_output_offset(
        &self,
        plan: &WarpPlan,
        output: SessionFrame,
    ) -> Result<usize, ElasticError> {
        let axis = plan.output_axis().ok_or(ElasticError::EnginePreparation(
            "projected output axis is unavailable",
        ))?;
        let frames = i64::from(output)
            .checked_sub(i64::from(plan.activation().output()))
            .and_then(|frames| frames.to_f64())
            .ok_or(ElasticError::SampleCountOverflow)?;
        (frames * f64::from(self.spec.sample_rate.get()) / f64::from(axis.sample_rate().get()))
            .round()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)
    }

    pub(super) fn projected_source(
        plan: &WarpPlan,
        output: SessionFrame,
    ) -> Result<u64, ElasticError> {
        match plan.source_at(output) {
            BeatGridQuery::Resolved(source) => Self::source_frame(source),
            _ => Err(ElasticError::EnginePreparation(
                "projected source endpoint is uncovered",
            )),
        }
    }

    pub(super) fn projected_span(
        &self,
        plan: &WarpPlan,
        start: SessionFrame,
        meta: AudioChunkInfo,
        remaining: usize,
        pending_activation: Option<SessionFrame>,
        output_offset: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let source_axis = plan.source_axis().ok_or(ElasticError::EnginePreparation(
            "projected source axis is unavailable",
        ))?;
        let output_axis = plan.output_axis().ok_or(ElasticError::EnginePreparation(
            "projected output axis is unavailable",
        ))?;
        if source_axis.sample_rate() != meta.spec.sample_rate {
            return Err(ElasticError::EnginePreparation(
                "projected source sample rate differs from decoded audio",
            ));
        }
        if let Some(snapshot) = self.context.load() {
            let output = snapshot.context().output();
            let expected = MapAxis::Session(SessionAxis::new(
                output.sample_rate(),
                output.session_epoch(),
            ));
            if output_axis != expected {
                return Err(ElasticError::EnginePreparation(
                    "projected session axis differs from the render context",
                ));
            }
        }
        let capabilities = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation(
                "projected engine is unavailable",
            ))?
            .capabilities();
        let source_start = Self::projected_source(plan, start)?;
        if source_start != meta.frame_offset {
            return Err(ElasticError::DiscontinuousSource {
                expected: source_start
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
                actual: meta
                    .frame_offset
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
            });
        }
        let mut output_frames = self
            .render_quantum_frames
            .map_or_else(
                || capabilities.max_output_frames(),
                std::num::NonZeroUsize::get,
            )
            .min(capabilities.max_output_frames());
        let session_per_engine =
            f64::from(output_axis.sample_rate().get()) / f64::from(self.spec.sample_rate.get());
        let endpoint = |frames: usize| -> Result<SessionFrame, ElasticError> {
            let total_frames = output_offset
                .checked_add(frames)
                .ok_or(ElasticError::SampleCountOverflow)?;
            let session_frames = (total_frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?
                * session_per_engine)
                .round()
                .to_i64()
                .ok_or(ElasticError::SampleCountOverflow)?;
            let end = SessionFrame::new(
                i64::from(plan.activation().output())
                    .checked_add(session_frames)
                    .ok_or(ElasticError::SampleCountOverflow)?,
            );
            Ok(end)
        };
        // A decoder suffix may be shorter than one mapped output frame.
        // The worker retains it and assembles the exact request across chunks.
        let available_end = source_start
            .checked_add(u64::try_from(remaining).map_err(|_| ElasticError::SampleCountOverflow)?)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let first_source_end = Self::projected_endpoint(plan, endpoint(1)?, available_end)?;
        let first_span = first_source_end
            .checked_sub(source_start)
            .and_then(|frames| usize::try_from(frames).ok())
            .ok_or(ElasticError::SampleCountOverflow)?;
        let limit = remaining
            .max(first_span)
            .min(capabilities.max_source_frames())
            .min(self.source_block_frames.get());
        let source_limit = source_start
            .checked_add(u64::try_from(limit).map_err(|_| ElasticError::SampleCountOverflow)?)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let mut low = 0;
        let mut high = output_frames;
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            let end = endpoint(middle)?;
            let fits = if pending_activation.is_some_and(|activation| end > activation) {
                false
            } else {
                match Self::projected_endpoint(plan, end, source_limit) {
                    Ok(source) => source <= source_limit,
                    Err(ElasticError::EmptyOutput) => false,
                    _ => {
                        return Err(ElasticError::EnginePreparation(
                            "projected source endpoint is uncovered",
                        ));
                    }
                }
            };
            if fits {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        output_frames = low;
        if output_frames == 0 {
            return Err(ElasticError::EmptyOutput);
        }
        let end = endpoint(output_frames)?;
        let source_end = Self::projected_endpoint(plan, end, source_limit)?;
        let frames = source_end
            .checked_sub(source_start)
            .and_then(|frames| usize::try_from(frames).ok())
            .ok_or(ElasticError::SampleCountOverflow)?;
        if frames == 0 {
            return Err(ElasticError::EmptySource);
        }
        let rate = frames.to_f64().ok_or(ElasticError::SampleCountOverflow)?
            / output_frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?;
        if !capabilities.rate_envelope().contains_rate(rate) {
            return Err(ElasticError::RateOutsideEnvelope {
                output_frames,
                source_frames: frames,
            });
        }
        let speed = rate.to_f32().ok_or(ElasticError::SampleCountOverflow)?;
        Ok(PreparedQuantum {
            source_start,
            activation: None,
            projection: Some(ProjectedQuantum {
                output_offset,
                output_frames,
                output_start: start,
                end: plan.map().reanchor(source_end, end),
            }),
            rate: self.controls.rate_target(),
            speed,
            active_frames: frames,
            frames,
        })
    }

    pub(super) fn projected_tail_cursor(&self, output_frames: usize) -> Option<ProjectedQuantum> {
        let plan = self.projection.active.as_ref()?;
        let previous = self.projection.cursor?.output();
        let sample_rate = plan.output_axis()?.sample_rate();
        let total_frames = self.projection.output_frames.checked_add(output_frames)?;
        let frames = (total_frames.to_f64()? * f64::from(sample_rate.get())
            / f64::from(self.spec.sample_rate.get()))
        .round()
        .to_i64()?;
        let output = SessionFrame::new(i64::from(plan.activation().output()).checked_add(frames)?);
        let (source, _) = self.rendered_source_end?;
        Some(ProjectedQuantum {
            output_frames,
            output_offset: self.projection.output_frames,
            output_start: previous,
            end: plan.map().reanchor(source, output),
        })
    }

    pub(super) fn resize_projected_quantum(
        &self,
        mut prepared: PreparedQuantum,
        meta: AudioChunkInfo,
        frames: usize,
    ) -> Result<PreparedQuantum, ElasticError> {
        let projection = prepared.projection.ok_or(ElasticError::EnginePreparation(
            "projected span has no prepared geometry",
        ))?;
        let plan = self
            .projection
            .prepared
            .as_ref()
            .or(self.projection.active.as_ref())
            .ok_or(ElasticError::EnginePreparation(
                "projected span has no resident map",
            ))?;
        if plan.map().revision() != projection.end.revision()
            || Self::projected_source(plan, projection.output_start)? != meta.frame_offset
        {
            return Err(ElasticError::EnginePreparation(
                "projected resize differs from its prepared source origin",
            ));
        }
        let source_end = meta
            .frame_offset
            .checked_add(u64::try_from(frames).map_err(|_| ElasticError::SampleCountOverflow)?)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let source = AssetFrame::new(
            source_end
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?,
        )
        .map_err(|_| ElasticError::SampleCountOverflow)?;
        let BeatGridQuery::Resolved(end) = plan.map().output_at(source) else {
            return Err(ElasticError::EnginePreparation(
                "projected source endpoint is uncovered",
            ));
        };
        let total_output = self.projected_output_offset(plan, end)?;
        let output_frames = total_output
            .checked_sub(projection.output_offset)
            .filter(|frames| *frames > 0)
            .ok_or(ElasticError::EmptyOutput)?;
        let rate = frames.to_f64().ok_or(ElasticError::SampleCountOverflow)?
            / output_frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?;
        let capabilities = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation(
                "projected engine is unavailable",
            ))?
            .capabilities();
        let source_limit = capabilities
            .max_source_frames()
            .min(self.source_block_frames.get());
        if frames == 0 {
            return Err(ElasticError::EmptySource);
        }
        if frames > source_limit {
            return Err(ElasticError::SourceFrameLimit {
                frames,
                limit: source_limit,
            });
        }
        if output_frames > capabilities.max_output_frames() {
            return Err(ElasticError::OutputFrameLimit {
                frames: output_frames,
                limit: capabilities.max_output_frames(),
            });
        }
        if !capabilities.rate_envelope().contains_rate(rate) {
            return Err(ElasticError::RateOutsideEnvelope {
                output_frames,
                source_frames: frames,
            });
        }
        prepared.frames = frames;
        prepared.active_frames = frames;
        prepared.speed = rate.to_f32().ok_or(ElasticError::SampleCountOverflow)?;
        prepared.projection = Some(ProjectedQuantum {
            output_frames,
            output_offset: projection.output_offset,
            output_start: projection.output_start,
            end: plan.map().reanchor(source_end, end),
        });
        Ok(prepared)
    }
    fn source_frame(source: AssetFrame) -> Result<u64, ElasticError> {
        f64::from(source)
            .round()
            .to_u64()
            .ok_or(ElasticError::SampleCountOverflow)
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU32, NonZeroUsize};

    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        Warp, WarpConfig, test_grids,
        test_pools::{TestPools, pools},
    };

    fn renderer() -> (WarpRenderer<TestPools>, AudioChunkInfo) {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("sample rate"));
        let config = WarpConfig::builder()
            .render_quantum_frames(NonZeroUsize::new(128).expect("quantum"))
            .build();
        (
            Warp::new((), &config).renderer(spec, pools()),
            AudioChunkInfo {
                spec,
                ..Default::default()
            },
        )
    }

    #[kithara::test]
    fn a_short_decoder_suffix_requests_the_next_mapped_source_frame() {
        let (renderer, meta) = renderer();
        let plan = test_grids::projected_plan(120.0, 240.0, meta.spec.sample_rate);
        let prepared = renderer
            .projected_span(&plan, SessionFrame::new(0), meta, 1, None, 0)
            .expect("one mapped output frame spans decoder chunks");
        assert_eq!(prepared.frames, 2);
        assert_eq!(
            prepared
                .projection
                .expect("projected request")
                .output_frames,
            1
        );
    }

    #[kithara::test]
    fn finite_projection_clips_a_quantum_before_resolving_past_the_recording() {
        let (renderer, mut meta) = renderer();
        let plan = test_grids::projected_plan(120.0, 120.0, meta.spec.sample_rate);
        // The existing 400-beat fixture ends at frame 8,820,000 at this rate.
        let start = 8_819_968;
        meta.frame_offset = start;
        let prepared = renderer
            .projected_span(
                &plan,
                SessionFrame::new(i64::try_from(start).expect("frame")),
                meta,
                32,
                None,
                usize::try_from(start).expect("frame"),
            )
            .expect("a covered final quantum survives an out-of-domain search midpoint");
        assert_eq!(prepared.frames, 32);
        let projection = prepared.projection.expect("projected quantum");
        assert_eq!(projection.output_frames, 32);
        assert_eq!(projection.end.source(), 8_820_000);
        assert_eq!(projection.end.output(), SessionFrame::new(8_820_000));
    }

    #[kithara::test]
    fn terminal_projection_recomputes_both_endpoints_and_keeps_the_selected_rate() {
        let (mut renderer, meta) = renderer();
        let plan = Arc::new(test_grids::projected_plan(
            120.0,
            180.0,
            meta.spec.sample_rate,
        ));
        let prepared = renderer
            .projected_span(&plan, SessionFrame::new(0), meta, 4096, None, 0)
            .expect("initial quantum resolves");
        let rate = prepared.rate;
        assert_eq!(prepared.frames, 192);
        renderer.projection.active = Some(plan);
        renderer.prepared_quantum = Some(prepared);
        renderer.controls.set_speed(2.0);
        assert_eq!(
            renderer
                .prepare_terminal_quantum(meta, 96)
                .map(|frames| frames.get()),
            Some(96)
        );
        let terminal = renderer
            .prepared_quantum
            .expect("terminal quantum is retained");
        assert_eq!(terminal.rate.revision(), rate.revision());
        assert_eq!(terminal.rate.speed(), rate.speed());
        assert_eq!(terminal.speed, 1.5);
        assert_eq!(terminal.active_frames, 96);
        let projection = terminal
            .projection
            .expect("terminal projection is retained");
        assert_eq!(projection.output_start, SessionFrame::new(0));
        assert_eq!(projection.output_frames, 64);
        assert_eq!(projection.end.source(), 96);
        assert_eq!(projection.end.output(), SessionFrame::new(64));
    }
}
