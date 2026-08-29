use std::num::NonZeroU32;

use kithara_bufpool::SamplePool;
use kithara_platform::sync::Arc;
use kithara_signal::{AudioChunk, AudioSpec};

use crate::StretchControls;

/// Identity renderer for targets without elastic DSP.
/// It preserves decoded samples exactly and keeps playback-rate capability disabled.
#[non_exhaustive]
pub struct WarpRenderer {
    rendered_source_end: Option<(u64, NonZeroU32)>,
}

impl WarpRenderer {
    pub(crate) fn new(
        _controls: Arc<StretchControls>,
        _spec: AudioSpec,
        _sample_pool: SamplePool,
    ) -> Self {
        Self {
            rendered_source_end: None,
        }
    }

    #[doc(hidden)]
    pub const fn prepare(&mut self, _spec: AudioSpec) {}

    #[doc(hidden)]
    pub const fn flush(&mut self) -> Option<AudioChunk> {
        None
    }

    #[doc(hidden)]
    pub const fn accepts_input(&self) -> bool {
        true
    }

    #[doc(hidden)]
    pub fn prepare_quantum(
        &mut self,
        _meta: kithara_signal::AudioChunkInfo,
        remaining: usize,
    ) -> Option<kithara_signal::FrameCount> {
        (remaining > 0).then(|| kithara_signal::FrameCount::new(remaining))
    }

    #[doc(hidden)]
    pub fn render(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        self.rendered_source_end = Some((
            chunk
                .meta
                .frame_offset
                .saturating_add(u64::from(chunk.meta.frames)),
            chunk.meta.spec.sample_rate,
        ));
        Some(chunk)
    }

    #[doc(hidden)]
    pub fn render_quantum(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        self.render(chunk)
    }

    #[doc(hidden)]
    pub const fn rendered_source_end(&self) -> Option<(u64, NonZeroU32)> {
        self.rendered_source_end
    }

    #[doc(hidden)]
    pub const fn reset(&mut self) {
        self.rendered_source_end = None;
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_signal::AudioChunkInfo;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn renderer_preserves_samples_exactly() {
        let sample_pool = SamplePool::default();
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let mut meta = AudioChunkInfo::default();
        meta.spec = spec;
        meta.frames = 1;
        meta.frame_offset = 41;
        let input = AudioChunk::new(meta, sample_pool.attach(vec![0.25, -0.5]));
        let input_ptr = input.samples.as_ptr();
        let mut renderer = WarpRenderer::new(StretchControls::new(1.5), spec, sample_pool);

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
