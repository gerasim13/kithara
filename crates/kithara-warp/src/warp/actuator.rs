use kithara_platform::sync::Arc;
#[cfg(feature = "render")]
use {kithara_bufpool::SamplePool, kithara_signal::AudioSpec};

use super::WarpConfig;
#[cfg(feature = "render")]
use super::WarpRenderer;
use crate::StretchControls;

/// Resident warp actuator around one decoded-audio source.
///
/// The wrapper remains present in identity and future synchronized modes. It
/// owns the live temporal controls that the playback layer composes into its
/// resident DSP path.
#[derive(Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct Warp<S> {
    #[field(get, deref = false)]
    stretch: Arc<StretchControls>,
    #[field(get, get_mut)]
    source: S,
}

impl<S> Warp<S> {
    /// Wraps `source` with the configured live temporal controls.
    #[must_use]
    pub fn new(source: S, config: &WarpConfig) -> Self {
        Self {
            source,
            stretch: Arc::clone(config.stretch()),
        }
    }

    /// Creates the worker-side renderer paired with this Warp facade.
    #[cfg(feature = "render")]
    #[must_use]
    pub fn renderer(&self, spec: AudioSpec, sample_pool: SamplePool) -> WarpRenderer {
        WarpRenderer::new(Arc::clone(&self.stretch), spec, sample_pool)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    #[kithara::test]
    fn source_access_delegates_to_the_resident_value() {
        let config = WarpConfig::builder().build();
        let mut warp = Warp::new(vec![1_u8], &config);

        warp.source_mut().push(2);

        assert_eq!(warp.source(), &[1, 2]);
    }

    #[kithara::test]
    fn controls_are_shared_with_the_resident_lane() {
        let stretch = StretchControls::new(1.0);
        let config = WarpConfig::builder().stretch(Arc::clone(&stretch)).build();
        let warp = Warp::new((), &config);

        warp.stretch().set_speed(1.25);

        assert!((stretch.speed() - 1.25).abs() < f32::EPSILON);
    }
}
