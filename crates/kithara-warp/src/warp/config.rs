use std::num::NonZeroUsize;

use bon::Builder;
use kithara_platform::sync::Arc;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
))]
use kithara_stretch::ElasticBackendConfig;

use crate::StretchControls;

const DEFAULT_SOURCE_BLOCK_FRAMES: NonZeroUsize = match NonZeroUsize::new(8192) {
    Some(frames) => frames,
    None => unreachable!(),
};

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
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    #[builder(default)]
    #[field(get, copy)]
    backends: ElasticBackendConfig,
    /// Maximum source frames admitted to one elastic render operation.
    #[builder(default = DEFAULT_SOURCE_BLOCK_FRAMES)]
    #[field(get, copy)]
    source_block_frames: NonZeroUsize,
    /// Output-frame window used to smooth live rate changes.
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
