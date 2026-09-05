use std::num::NonZeroUsize;

use bon::Builder;
use kithara_platform::sync::Arc;
#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use kithara_stretch::ElasticBackendConfig;

use crate::StretchControls;

/// Fixed resources used to construct one resident [`super::Warp`].
#[derive(Clone, Debug, Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct WarpConfig {
    /// Live temporal controls consumed by the resident Warp lane.
    #[builder(default = StretchControls::new(1.0))]
    #[field(get, deref = false)]
    stretch: Arc<StretchControls>,
    /// Preparation parameters for the compiled elastic backends.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[builder(default)]
    #[field(get, copy)]
    backends: ElasticBackendConfig,
    /// Time-stretch rate smoothing window in output frames.
    #[builder(default = NonZeroUsize::MIN)]
    #[field(get, copy)]
    rate_smooth_frames: NonZeroUsize,
    /// Optional output-frame cap between samples of live temporal controls.
    /// Without a cap, Warp consumes the complete source span accepted by its backend.
    #[field(get, copy)]
    render_quantum_frames: Option<NonZeroUsize>,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    #[case::default(None, None)]
    #[case::configured(Some(64), Some(64))]
    fn render_quantum_is_configurable_in_frames(
        #[case] configured: Option<usize>,
        #[case] expected: Option<usize>,
    ) {
        let config = WarpConfig::builder()
            .maybe_render_quantum_frames(
                configured
                    .map(|frames| NonZeroUsize::new(frames).expect("fixture quantum is non-zero")),
            )
            .build();

        assert_eq!(
            config.render_quantum_frames().map(NonZeroUsize::get),
            expected
        );
    }
}
