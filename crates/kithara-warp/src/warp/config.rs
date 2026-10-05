use std::num::NonZeroUsize;

use kithara_config::Config;
use kithara_derive::Patch;
use kithara_platform::sync::Arc;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
))]
use kithara_stretch::{
    ElasticBackendConfig, ElasticBackendConfigPatch, ElasticBackendConfigPatchError,
};

use crate::{StretchControls, WarpPlan, WarpPlanSlot, consts};

/// Fixed resources used to construct one resident [`super::Warp`].
///
/// [`WarpConfigPatch`] is what a configuration document may say about it.
#[derive(Clone, Debug, Patch, Config)]
#[config(builder(state_mod(vis = "pub")), patch(fallible), fields(value))]
#[non_exhaustive]
pub struct WarpConfig {
    /// Explicit projected selection prepared by the musical policy owner.
    #[config(skip = "shared projected warp plan handle", builder(default = Arc::new(WarpPlanSlot::default())), get(ref), patch(skip))]
    plan: Arc<WarpPlanSlot>,
    /// Live temporal controls consumed by the resident Warp lane. Not a
    /// document key: this is the handle the UI and the deck already share, so
    /// a document naming a stretch ratio would be overwritten by the first
    /// gesture.
    #[config(skip = "shared live temporal control handle", builder(default = StretchControls::new(1.0)), get(ref), patch(skip))]
    stretch: Arc<StretchControls>,
    /// Preparation geometry each compiled stretch backend is built with. Not
    /// the backend selection: which engine runs is a live control on
    /// [`StretchControls`], while this is the geometry the selected engine is
    /// prepared with, read again on every rebuild. Only a build that compiles
    /// a stretch backend has it, so a document naming it under a build that
    /// has none is refused rather than silently ignored.
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    #[config(nested, builder(default), get(copy), patch(nested, fallible))]
    backends: ElasticBackendConfig,
    /// Maximum source frames admitted to one elastic render operation.
    #[config(builder(default = consts::DEFAULT_SOURCE_BLOCK_FRAMES), get(copy))]
    source_block_frames: NonZeroUsize,
    /// Output-frame window used to smooth live rate changes.
    #[config(builder(default = NonZeroUsize::MIN), get(copy))]
    rate_smooth_frames: NonZeroUsize,
    /// Optional output-frame cap between samples of live temporal controls.
    /// Without a cap, Warp consumes the complete source span accepted by its backend.
    #[config(get(copy))]
    render_quantum_frames: Option<NonZeroUsize>,
    /// Whether a renderer built from this configuration enters its plan at
    /// the plan's activation rather than waiting for a presented output to
    /// reach it. Only [`WarpConfig::entering`] sets it.
    #[config(skip = "staged renderer activation state", builder(skip), patch(skip))]
    entering: bool,
}

impl WarpConfig {
    /// A copy that renders `plan` from its activation on through a plan slot
    /// of its own: a staged lane prepares audio the plan will present, while
    /// the lane that sounds now keeps its slot and its selection.
    #[must_use]
    pub fn entering(&self, plan: Arc<WarpPlan>) -> Self {
        let slot = WarpPlanSlot::default();
        slot.install(Some(plan));
        Self {
            plan: Arc::new(slot),
            entering: true,
            ..self.clone()
        }
    }

    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    pub(crate) const fn enters_plan(&self) -> bool {
        self.entering
    }
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

    /// Backend geometry merges one engine at a time: a patch naming only
    /// Signalsmith must leave Bungee's built value standing, or a document
    /// tuning one engine would reset the other.
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    #[kithara::test]
    fn a_patch_naming_one_backend_leaves_the_other_standing() {
        use kithara_stretch::{BungeeConfig, ElasticBackendConfig, SignalsmithConfig};

        let mut config = WarpConfig::builder()
            .backends(
                ElasticBackendConfig::builder()
                    .bungee(
                        BungeeConfig::builder()
                            .log2_synthesis_hop_adjust(-2)
                            .build(),
                    )
                    .build(),
            )
            .build();
        let mut patch = WarpConfigPatch::default();
        patch.backends.signalsmith.block_frames = NonZeroUsize::new(512);
        patch.backends.signalsmith.interval_frames = NonZeroUsize::new(16);

        config.apply(patch).expect("valid backend geometry patch");

        let backends = config.backends();
        assert_eq!(
            *backends.signalsmith(),
            SignalsmithConfig::builder()
                .block_frames(NonZeroUsize::new(512).expect("fixture block is non-zero"))
                .interval_frames(NonZeroUsize::new(16).expect("fixture interval is non-zero"))
                .build()
                .expect("valid Signalsmith geometry")
        );
        assert_eq!(
            backends.bungee().log2_synthesis_hop_adjust(),
            -2,
            "a patch that never names Bungee must not reset its geometry"
        );
    }

    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    #[kithara::test]
    fn rejected_backend_geometry_keeps_the_entire_warp_config() {
        let mut config = WarpConfig::builder().build();
        let previous_source_limit = config.source_block_frames();
        let mut patch = WarpConfigPatch {
            source_block_frames: NonZeroUsize::new(64),
            ..WarpConfigPatch::default()
        };
        patch.backends.signalsmith.block_frames = NonZeroUsize::new(16);
        patch.backends.signalsmith.interval_frames = NonZeroUsize::new(32);

        assert!(matches!(
            config.apply(patch),
            Err(WarpConfigPatchError::Backends(_))
        ));
        assert_eq!(config.source_block_frames(), previous_source_limit);
        assert_eq!(config.backends().signalsmith().block_frames(), None);
        assert_eq!(config.backends().signalsmith().interval_frames(), None);
    }
}
