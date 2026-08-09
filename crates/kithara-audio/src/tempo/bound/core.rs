use kithara_bufpool::PcmPool;
use kithara_decode::{DecodeError, DecodeResult, PcmChunk, PcmMeta, PcmSpec};
use kithara_platform::sync::Arc;
use kithara_stretch::{
    ElasticCapabilities, ElasticCursor, ElasticEngine, ElasticSpan, ElasticSpanConfig,
    ElasticSpanPlan,
};
use num_traits::ToPrimitive;
use tracing::warn;

use super::BoundError;
use crate::{
    musical::{SessionBeat, SourceSchedule},
    traits::AudioEffect,
};

/// Exact-span tempo slot for a deck bound to the session grid.
///
/// The streaming slot is push-driven: a chunk goes in and whatever the backend
/// renders comes out. This slot is the inverse. It chooses the output span
/// itself, asks the schedule which source span that span is due to consume, and
/// renders exactly those frames through an [`ElasticEngine`]. Output frame `n`
/// therefore lands on the session frame the binding placed it at, with no
/// accumulated rounding: [`ElasticSpanPlan`] quantizes each block's endpoints
/// and carries the fractional remainder in its [`ElasticCursor`].
///
/// The slot is forward-only. It never asks for source behind what it has
/// already consumed, so it needs no retention behind the window it reads.
pub(crate) struct BoundRenderer<E> {
    schedule: Arc<SourceSchedule>,
    /// Session beat aligned with this deck's output frame zero.
    session_origin: SessionBeat,
    engine: E,
    capabilities: ElasticCapabilities,
    span_config: ElasticSpanConfig,
    cursor: Option<ElasticCursor>,
    /// Interleaved source frames admitted but not yet consumed by the engine.
    pending: Vec<f32>,
    /// Integer source frame of the first pending frame.
    pending_start: u64,
    /// Source frames consumed by the engine since the last reset.
    consumed: u64,
    /// Next output frame to plan.
    output_frame: u64,
    /// Session beats this deck has advanced since its start.
    ///
    /// The deck's own count, not a reading of the session clock: a tempo
    /// commit changes what the *next* block adds, and a pause moves the
    /// session's frame axis without moving this. Deriving the position from a
    /// frame pin instead would reinterpret frames already rendered.
    elapsed_beats: f64,
    /// Session beats per frame used at the last planned frontier.
    old_beats_per_frame: Option<f64>,
    /// Interleaved output accumulated across the blocks of one call.
    scratch: Vec<f32>,
    last_input_meta: Option<PcmMeta>,
    pool: PcmPool,
    spec: PcmSpec,
}

impl<E: ElasticEngine> BoundRenderer<E> {
    /// Output frames planned per block. A commit may split the block into two
    /// engine calls inside one exact-span plan.
    pub(crate) const BLOCK_FRAMES: u64 = 512;

    pub(crate) fn new(
        schedule: Arc<SourceSchedule>,
        session_origin: SessionBeat,
        engine: E,
        span_config: ElasticSpanConfig,
        spec: PcmSpec,
        pool: PcmPool,
    ) -> Result<Self, BoundError> {
        let capabilities = engine.capabilities();
        let old_beats_per_frame = Some(schedule.beats_per_frame()?);
        Ok(Self {
            capabilities,
            engine,
            pool,
            schedule,
            session_origin,
            span_config,
            spec,
            consumed: 0,
            cursor: None,
            last_input_meta: None,
            old_beats_per_frame,
            output_frame: 0,
            elapsed_beats: 0.0,
            pending: Vec::new(),
            pending_start: 0,
            scratch: Vec::new(),
        })
    }

    fn channels(&self) -> usize {
        usize::from(self.spec.channels.max(1))
    }

    fn pending_frames(&self) -> u64 {
        (self.pending.len() / self.channels())
            .to_u64()
            .unwrap_or(u64::MAX)
    }

    fn span(&self, start: f64, end: f64, output_frames: usize) -> Result<ElasticSpan, BoundError> {
        Ok(ElasticSpan::try_from((
            f64::from(self.schedule.source_after(start)?)
                ..f64::from(self.schedule.source_after(end)?),
            output_frames,
        ))?)
    }

    /// Quantized plan for the block starting at the current presentation frame.
    fn plan_block(&self) -> Result<(ElasticSpanPlan, f64, f64), BoundError> {
        let block = usize::try_from(Self::BLOCK_FRAMES).map_err(|_| BoundError::BlockOverflow)?;
        let block_frames = Self::BLOCK_FRAMES
            .to_f64()
            .ok_or(BoundError::BlockOverflow)?;
        let commit = self.schedule.commit(self.session_origin)?;
        let old_beats_per_frame = self
            .old_beats_per_frame
            .unwrap_or_else(|| commit.beats_per_frame());
        let old_next = self.elapsed_beats + old_beats_per_frame * block_frames;
        let commit_frame = if commit.elapsed_beats() > self.elapsed_beats {
            Some(
                ((commit.elapsed_beats() - self.elapsed_beats) / old_beats_per_frame)
                    .round()
                    .to_usize()
                    .ok_or(BoundError::BlockOverflow)?,
            )
        } else {
            None
        };
        let mut spans = [None, None];
        let (next, planned_beats_per_frame) =
            if commit.elapsed_beats() > self.elapsed_beats && commit.elapsed_beats() < old_next {
                let before = commit_frame.ok_or(BoundError::BlockOverflow)?;
                let after = block.checked_sub(before).ok_or(BoundError::BlockOverflow)?;
                let split = commit.elapsed_beats();
                if before == 0 {
                    let next = split + commit.beats_per_frame() * block_frames;
                    spans[0] = Some(self.span(split, next, block)?);
                    (next, commit.beats_per_frame())
                } else if after == 0 {
                    spans[0] = Some(self.span(self.elapsed_beats, split, block)?);
                    (split, commit.beats_per_frame())
                } else {
                    let after_advance = commit.beats_per_frame()
                        * after.to_f64().ok_or(BoundError::BlockOverflow)?;
                    let next = split + after_advance;
                    spans[0] = Some(self.span(self.elapsed_beats, split, before)?);
                    spans[1] = Some(self.span(split, next, after)?);
                    (next, commit.beats_per_frame())
                }
            } else {
                let beats_per_frame = if commit.elapsed_beats() <= self.elapsed_beats {
                    commit.beats_per_frame()
                } else {
                    old_beats_per_frame
                };
                let next = self.elapsed_beats + beats_per_frame * block_frames;
                spans[0] = Some(self.span(self.elapsed_beats, next, block)?);
                let frontier_beats_per_frame = if commit_frame == Some(block) {
                    commit.beats_per_frame()
                } else {
                    beats_per_frame
                };
                (next, frontier_beats_per_frame)
            };
        let plan = ElasticSpanPlan::new(
            spans.into_iter().flatten(),
            self.cursor,
            self.capabilities,
            self.span_config,
        )?;
        Ok((plan, next, planned_beats_per_frame))
    }

    /// Renders every block the pending source can cover, appending each to
    /// `scratch`. Stops without consuming anything when the next block's source
    /// has not arrived.
    pub(super) fn render_available(&mut self) -> Result<(), BoundError> {
        loop {
            let (plan, next_elapsed, planned_beats_per_frame) = self.plan_block()?;
            let segment = *plan.segments().first().ok_or(BoundError::EmptyPlan)?;
            let start =
                u64::try_from(segment.source_start()).map_err(|_| BoundError::BehindWindow {
                    requested: segment.source_start(),
                    available: self.pending_start,
                })?;
            if start < self.pending_start {
                return Err(BoundError::BehindWindow {
                    requested: segment.source_start(),
                    available: self.pending_start,
                });
            }
            let skip = usize::try_from(start - self.pending_start)
                .map_err(|_| BoundError::BlockOverflow)?;
            let source_frames = plan.segments().iter().try_fold(0usize, |total, segment| {
                total
                    .checked_add(segment.request().source_frames())
                    .ok_or(BoundError::BlockOverflow)
            })?;
            let needed = skip
                .checked_add(source_frames)
                .ok_or(BoundError::BlockOverflow)?;
            if self.pending_frames() < needed.to_u64().unwrap_or(u64::MAX) {
                return Ok(());
            }

            let channels = self.channels();
            let mut source_offset = skip;
            for segment in plan.segments() {
                let request = segment.request();
                let source_end = source_offset
                    .checked_add(request.source_frames())
                    .ok_or(BoundError::BlockOverflow)?;
                let written = self.scratch.len();
                let output_samples = request
                    .output_frames()
                    .checked_mul(channels)
                    .ok_or(BoundError::BlockOverflow)?;
                self.scratch.resize(written + output_samples, 0.0);
                let source = &self.pending[source_offset * channels..source_end * channels];
                self.engine
                    .process(request, source, &mut self.scratch[written..])?;
                source_offset = source_end;
            }

            self.pending.drain(..needed * channels);
            let consumed = source_frames.to_u64().unwrap_or(u64::MAX);
            self.pending_start = start.saturating_add(consumed);
            self.consumed = self.consumed.saturating_add(consumed);
            self.elapsed_beats = next_elapsed;
            self.old_beats_per_frame = Some(planned_beats_per_frame);
            self.cursor = Some(plan.cursor());
        }
    }

    fn emit(&mut self) -> Option<PcmChunk> {
        if self.scratch.is_empty() {
            return None;
        }
        let mut meta = self.last_input_meta.unwrap_or_default();
        meta.spec = self.spec;
        meta.frames = u32::try_from(self.scratch.len() / self.channels()).unwrap_or(u32::MAX);
        let mut pcm = self.pool.get();
        if pcm.ensure_len(self.scratch.len()).is_err() {
            warn!("PCM pool budget exhausted during bound rendering");
            return None;
        }
        pcm[..].copy_from_slice(&self.scratch);
        let emitted = (self.scratch.len() / self.channels())
            .to_u64()
            .unwrap_or(u64::MAX);
        self.output_frame = self.output_frame.saturating_add(emitted);
        Some(PcmChunk::new(meta, pcm))
    }

    #[cfg(test)]
    pub(super) const fn presentation_frame(&self) -> u64 {
        self.output_frame
    }

    #[cfg(test)]
    pub(super) const fn consumed_source_frames(&self) -> u64 {
        self.consumed
    }

    #[cfg(test)]
    pub(super) const fn elapsed_session_beats(&self) -> f64 {
        self.elapsed_beats
    }
}

impl<E: ElasticEngine + Send + 'static> AudioEffect for BoundRenderer<E> {
    fn flush(&mut self) -> Option<PcmChunk> {
        None
    }

    fn held_source_frames(&self) -> u64 {
        let latency = self
            .capabilities
            .latency()
            .source_frames()
            .to_u64()
            .unwrap_or(u64::MAX);
        self.pending_frames()
            .saturating_add(latency.min(self.consumed))
    }

    fn process(&mut self, chunk: PcmChunk) -> DecodeResult<Option<PcmChunk>> {
        if chunk.spec() != self.spec {
            self.spec = chunk.spec();
        }
        if self.pending.is_empty() {
            self.pending_start = chunk.meta.frame_offset;
        }
        self.last_input_meta = Some(chunk.meta);
        self.pending.extend_from_slice(&chunk.samples);
        self.scratch.clear();
        self.render_available()
            .map_err(|error| DecodeError::pcm_stream("bound tempo renderer", error))?;
        Ok(self.emit())
    }

    fn reset(&mut self) {
        if let Err(error) = self.engine.reset() {
            warn!(%error, "bound engine reset failed");
        }
        self.pending.clear();
        self.scratch.clear();
        self.cursor = None;
        self.consumed = 0;
        self.output_frame = 0;
        self.elapsed_beats = 0.0;
        self.old_beats_per_frame = None;
        self.pending_start = 0;
        self.last_input_meta = None;
    }
}
