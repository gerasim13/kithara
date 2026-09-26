use std::num::NonZeroUsize;

use kithara_bufpool::HasPool;
use kithara_signal::AudioChunkInfo;
use kithara_stretch::ElasticError;
use num_traits::ToPrimitive;

use super::{renderer::WarpRenderer, renderer_projection::ProjectionPreparation};

impl<S: HasPool<f32>> WarpRenderer<S> {
    /// Places a decoded span against an entered activation before anything
    /// is rendered: audio before the activation source is history, and audio
    /// that lands after it, with no history admitted, can never present the
    /// activation's first frame.
    pub(super) fn entry_boundary(
        &self,
        activation: crate::WarpCursor,
        meta: AudioChunkInfo,
    ) -> Result<Option<ProjectionPreparation>, ElasticError> {
        let cue = activation.source();
        if meta.frame_offset < cue {
            let frames = usize::try_from(cue - meta.frame_offset)
                .map_err(|_| ElasticError::SampleCountOverflow)?;
            return Ok(NonZeroUsize::new(frames).map(ProjectionPreparation::Preroll));
        }
        let admitted = self
            .residency
            .as_ref()
            .is_some_and(|resident| resident.end == Some(meta.frame_offset));
        if meta.frame_offset > cue && !admitted {
            return Err(ElasticError::DiscontinuousSource {
                expected: cue.to_f64().ok_or(ElasticError::SampleCountOverflow)?,
                actual: meta
                    .frame_offset
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?,
            });
        }
        Ok(None)
    }

    /// The first source frame an entered renderer must receive: the plan's
    /// activation source less the history its engine needs, bounded by the
    /// start of the recording.
    #[must_use]
    pub fn entry_source(&self) -> Option<u64> {
        if !self.projection.entering {
            return None;
        }
        let cue = self.projection.active.as_ref()?.activation().source();
        let history = self
            .engine
            .as_ref()?
            .capabilities()
            .latency()
            .source_frames();
        Some(cue.saturating_sub(u64::try_from(history).ok()?))
    }

    /// Admits decoded history before an entered activation, keeping only
    /// what the engine's history window needs.
    ///
    /// # Errors
    /// Returns an error when the renderer entered no plan or the span does
    /// not continue the admitted history.
    pub fn admit_preroll(
        &mut self,
        meta: AudioChunkInfo,
        samples: &[f32],
    ) -> Result<(), crate::WarpRenderError> {
        let cue = self
            .projection
            .active
            .as_ref()
            .filter(|_| self.projection.entering && self.projection.cursor.is_none())
            .map(|plan| plan.activation().source())
            .ok_or(crate::WarpRenderError::UnsupportedProjection)?;
        let resident = self.residency.as_mut().ok_or(ElasticError::PoolCapacity)?;
        resident.retain_manual(meta, samples, Some(cue))?;
        Ok(())
    }
}
