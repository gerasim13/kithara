use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::{HasPool, PoolRegion};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, FrameCount};
use kithara_test_macros as kithara;

use crate::{RenderReader, RenderSnapshot, WarpConfig};

/// Identity renderer for targets without elastic DSP.
/// It preserves decoded samples exactly and keeps playback-rate capability disabled.
#[non_exhaustive]
pub struct WarpRenderer<S> {
    context: RenderReader,
    committed: Option<RenderSnapshot>,
    prepared: Option<usize>,
    rendered_source_end: Option<(u64, NonZeroU32)>,
    schema: PhantomData<fn() -> S>,
}

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    #[kithara::probe(
        session_epoch = u64::from(committed.context().session_epoch()),
        transport_revision = committed.context().transport_revision().map_or(0, u64::from),
        output_start,
        output_end = i64::from(committed.frontier().output()),
        source_start,
        source_end = committed.frontier().source()
    )]
    fn render_committed(
        &mut self,
        committed: RenderSnapshot,
        source_start: u64,
        output_start: i64,
    ) {
        self.committed = Some(committed);
    }

    pub(crate) fn new(
        _config: &WarpConfig,
        context: RenderReader,
        _spec: AudioSpec,
        _pools: PoolRegion<S>,
    ) -> Self {
        Self {
            context,
            committed: None,
            prepared: None,
            rendered_source_end: None,
            schema: PhantomData,
        }
    }

    #[doc(hidden)]
    pub const fn accepts_input(&self) -> bool {
        true
    }

    #[doc(hidden)]
    pub fn prepare_quantum(
        &mut self,
        _meta: AudioChunkInfo,
        remaining: usize,
    ) -> Option<FrameCount> {
        self.prepared = (remaining > 0).then_some(remaining);
        self.prepared.map(FrameCount::new)
    }

    #[doc(hidden)]
    pub fn prepare_terminal_quantum(
        &mut self,
        _meta: AudioChunkInfo,
        frames: usize,
    ) -> Option<FrameCount> {
        let prepared = self.prepared.take()?;
        if frames == 0 || frames > prepared {
            return None;
        }
        self.prepared = Some(frames);
        Some(FrameCount::new(frames))
    }

    #[doc(hidden)]
    pub const fn requires_staging(&self) -> bool {
        false
    }

    #[doc(hidden)]
    pub const fn flush(&mut self) -> Option<AudioChunk> {
        None
    }

    #[doc(hidden)]
    pub const fn prepare(&mut self, _spec: AudioSpec) {}

    #[doc(hidden)]
    pub fn render(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        self.prepared = None;
        self.render_prepared(chunk)
    }

    #[doc(hidden)]
    pub fn render_quantum(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        let frames = self.prepared.take()?;
        (chunk.frames() == frames).then(|| self.render_prepared(chunk))?
    }

    fn render_prepared(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        let snapshot = self.context.load();
        self.rendered_source_end = Some((
            chunk
                .meta
                .frame_offset
                .saturating_add(u64::from(chunk.meta.frames)),
            chunk.meta.spec.sample_rate,
        ));
        if let Some(snapshot) = snapshot
            && self.context.is_current(&snapshot)
            && let Some((source, _)) = self.rendered_source_end
        {
            let source_start = snapshot.frontier().source();
            let output_start = i64::from(snapshot.frontier().output());
            if let Some(committed) =
                snapshot.advance(self.committed.as_ref(), source, chunk.frames())
            {
                self.render_committed(committed, source_start, output_start);
            }
        }
        Some(chunk)
    }

    /// Last context and frontier committed by a successful worker render.
    #[doc(hidden)]
    #[must_use]
    pub fn render_snapshot(&self) -> Option<&RenderSnapshot> {
        self.committed.as_ref()
    }

    #[doc(hidden)]
    pub const fn rendered_source_end(&self) -> Option<(u64, NonZeroU32)> {
        self.rendered_source_end
    }

    #[doc(hidden)]
    pub const fn reset(&mut self) {
        self.committed = None;
        self.prepared = None;
        self.rendered_source_end = None;
    }

    #[doc(hidden)]
    pub const fn transition_pending(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_signal::AudioChunkInfo;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::test_pools::{pools, sample_buffer};

    #[kithara::test]
    fn renderer_preserves_samples_exactly() {
        let pools = pools();
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let mut meta = AudioChunkInfo::default();
        meta.spec = spec;
        meta.frames = 1;
        meta.frame_offset = 41;
        let input = AudioChunk::new(meta, sample_buffer(&pools, &[0.25, -0.5]));
        let input_ptr = input.samples.as_ptr();
        let config = WarpConfig::builder()
            .stretch(StretchControls::new(1.5))
            .build();
        let mut renderer = WarpRenderer::new(
            &config,
            crate::RenderPublisher::default().reader(),
            spec,
            pools,
        );

        assert_eq!(renderer.rendered_source_end(), None);
        let output = renderer.render(input).expect("identity output");

        assert_eq!(output.samples.as_ptr(), input_ptr);
        assert_eq!(output.samples.as_ref(), &[0.25, -0.5]);
        assert_eq!(renderer.rendered_source_end(), Some((42, spec.sample_rate)));
        renderer.reset();
        assert_eq!(renderer.rendered_source_end(), None);
        assert!(renderer.flush().is_none());
        assert!(!crate::supports_playback_rate());
    }
}
