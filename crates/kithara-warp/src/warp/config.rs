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
    fn defaults_express_the_frame_contract() {
        let config = WarpConfig::builder().build();

        assert_eq!(config.render_quantum_frames().get(), 32);
    }
}
