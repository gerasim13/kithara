use kithara_bufpool::HasPool;
use kithara_signal::{AudioChunkInfo, FrameCount, SampleCount};
use kithara_stretch::{ElasticError, ElasticRequest};
use num_traits::ToPrimitive;
use tracing::warn;

use super::renderer::WarpRenderer;

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    pub(super) fn append_pending_source(
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
        // Backends require a non-empty output. Keep a sub-frame source span
        // pending until its cumulative exact output reaches one full frame;
        // EOF rounds the final residual once.
        let output_frames = if exact < 1.0 { 0.0 } else { exact.round() };
        let output_frames = output_frames
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let emitted = output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok((output_frames, exact - emitted))
    }

    pub(super) fn render_terminal_pending(&mut self, channels: usize) -> Result<(), ElasticError> {
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
        if let Err(error) = engine.process(request, source, &mut scratch[start..end]) {
            scratch.truncate(start);
            return Err(error);
        }
        self.output_start_meta = self.pending_meta;
        self.pending_source
            .as_mut()
            .ok_or(ElasticError::PoolCapacity)?
            .clear();
        self.pending_meta = None;
        self.output_remainder = 0.0;
        self.active = true;
        Ok(())
    }

    pub(super) fn source_frames_for_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
        speed: f32,
    ) -> Result<usize, ElasticError> {
        if remaining == 0 {
            return Err(ElasticError::EmptySource);
        }
        if self.can_passthrough(speed) {
            return Ok(remaining);
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
        let output_limit = capabilities
            .max_output_frames()
            .min(self.render_quantum_frames.get());
        let source_limit =
            Self::source_block_limit(stretch, capabilities.max_source_frames(), output_limit)?;
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
        Ok(region_frames.min(available))
    }

    /// Return the next source span that fits the fixed output quantum.
    ///
    /// The playback scheduler uses this workspace-internal seam to prepare an
    /// owning pooled subchunk outside the checked render core.
    #[doc(hidden)]
    #[cfg_attr(feature = "perf", hotpath::measure)]
    pub fn prepare_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Option<FrameCount> {
        match self.scheduler_plan(meta, remaining) {
            Ok(mut prepared) => match Self::prepared_source_frames(&prepared) {
                Ok(frames) => {
                    prepared.bind(self.context.load());
                    self.prepared_quantum = Some(prepared);
                    Some(FrameCount::new(frames))
                }
                Err(error) => {
                    self.prepared_quantum = None;
                    warn!(%error, "time-stretch source quantum sizing failed");
                    None
                }
            },
            Err(error) => {
                self.prepared_quantum = None;
                warn!(%error, "time-stretch source quantum sizing failed");
                None
            }
        }
    }
}
