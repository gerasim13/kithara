use std::num::NonZeroUsize;

use bon::Builder;
use kithara_platform::sync::Arc;

use crate::StretchControls;

const DEFAULT_RENDER_QUANTUM_FRAMES: NonZeroUsize = match NonZeroUsize::new(32) {
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
    /// Maximum output frames planned before live temporal controls are sampled again.
    #[builder(default = DEFAULT_RENDER_QUANTUM_FRAMES)]
    #[field(get, copy)]
    render_quantum_frames: NonZeroUsize,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    #[case::default(None, 32)]
    #[case::configured(Some(64), 64)]
    fn render_quantum_is_configurable_in_frames(
        #[case] configured: Option<usize>,
        #[case] expected: usize,
    ) {
        let config = WarpConfig::builder()
            .maybe_render_quantum_frames(
                configured
                    .map(|frames| NonZeroUsize::new(frames).expect("fixture quantum is non-zero")),
            )
            .build();

        assert_eq!(config.render_quantum_frames().get(), expected);
    }
}
