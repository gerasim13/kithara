use kithara_bufpool::PcmPool;
use kithara_decode::{PcmChunk, PcmSpec};
use kithara_platform::sync::Arc;

use crate::StretchControls;

/// Identity renderer for targets without elastic DSP.
/// It preserves PCM exactly and keeps playback-rate capability disabled.
#[non_exhaustive]
pub struct WarpRenderer;

impl WarpRenderer {
    pub(crate) fn new(_controls: Arc<StretchControls>, _spec: PcmSpec, _pool: PcmPool) -> Self {
        Self
    }

    #[doc(hidden)]
    pub const fn prepare(&mut self, _spec: PcmSpec) {}

    #[doc(hidden)]
    pub const fn flush(&mut self) -> Option<PcmChunk> {
        None
    }

    #[doc(hidden)]
    pub const fn render(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        Some(chunk)
    }

    #[doc(hidden)]
    pub const fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_decode::PcmMeta;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn renderer_preserves_pcm_exactly() {
        let pool = PcmPool::default();
        let spec = PcmSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let mut meta = PcmMeta::default();
        meta.spec = spec;
        meta.frames = 1;
        let input = PcmChunk::new(meta, pool.attach(vec![0.25, -0.5]));
        let input_ptr = input.samples.as_ptr();
        let mut renderer = WarpRenderer::new(StretchControls::new(1.5), spec, pool);

        let output = renderer.render(input).expect("identity output");

        assert_eq!(output.samples.as_ptr(), input_ptr);
        assert_eq!(output.samples.as_ref(), &[0.25, -0.5]);
        assert!(renderer.flush().is_none());
        assert!(!crate::supports_playback_rate());
    }
}
