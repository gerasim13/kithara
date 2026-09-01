use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::sync::Arc;
use kithara_signal::{AudioChunk, AudioSpec};

use crate::StretchControls;

/// Identity renderer for targets without elastic DSP.
/// It preserves decoded samples exactly and keeps playback-rate capability disabled.
#[non_exhaustive]
pub struct WarpRenderer<S> {
    rendered_source_end: Option<(u64, NonZeroU32)>,
    schema: PhantomData<fn() -> S>,
}

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    pub(crate) fn new(
        _controls: Arc<StretchControls>,
        _spec: AudioSpec,
        _pools: PoolRegion<S>,
    ) -> Self {
        Self {
            rendered_source_end: None,
            schema: PhantomData,
        }
    }

    #[doc(hidden)]
    pub const fn accepts_input(&self) -> bool {
        true
    }

    #[doc(hidden)]
    pub const fn flush(&mut self) -> Option<AudioChunk> {
        None
    }

    #[doc(hidden)]
    pub const fn prepare(&mut self, _spec: AudioSpec) {}

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
    pub const fn rendered_source_end(&self) -> Option<(u64, NonZeroU32)> {
        self.rendered_source_end
    }

    #[doc(hidden)]
    pub const fn reset(&mut self) {
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
        let mut renderer = WarpRenderer::new(StretchControls::new(1.5), spec, pools);

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
