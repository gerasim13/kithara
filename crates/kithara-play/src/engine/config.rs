use std::num::{NonZeroU32, NonZeroUsize};

use bon::Builder;
use firewheel::{
    dsp::filter::smoothing_filter::DEFAULT_SETTLE_RATIO, param::smoother::SmootherConfig,
};
use kithara_bufpool::PoolRegion;
use kithara_platform::CancelToken;
use kithara_warp::BeatGridId;

use crate::{
    effects::eq::{EqBandConfig, generate_log_spaced_bands},
    session::SessionBinding,
};

pub const DEFAULT_GATE_SMOOTHING: SmootherConfig = SmootherConfig {
    smooth_seconds: 0.005,
    settle_ratio: DEFAULT_SETTLE_RATIO,
};

/// Configuration for the audio engine.
#[kithara_config::config(builder = false)]
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
#[derive_where::derive_where(Clone)]
#[derive(derive_more::Debug)]
pub struct EngineConfig<S> {
    /// Stable synchronization identity of the owning player.
    #[config(skip = "player-owned synchronization identity")]
    #[debug(skip)]
    pub(crate) grid_id: BeatGridId,
    /// Initial output sample rate supplied by the owning player session.
    #[config(value)]
    pub(crate) sample_rate: NonZeroU32,
    /// Player-owned response contract used to validate session geometry.
    #[config(value)]
    pub(crate) response_budget_frames: NonZeroUsize,
    /// Master cancel token for the engine. The worker scheduler derives a
    /// `child()` so its produce-core's lock-free `is_cancelled()` read
    /// observes a master cancel.
    #[config(skip = "injected cancellation resource")]
    #[debug(skip)]
    pub(crate) cancel: Option<CancelToken>,
    /// Optional resident Warp render quantum supplied by the owning player.
    #[config(value)]
    pub(crate) render_quantum_frames: Option<NonZeroUsize>,
    /// Optional pre-bound session for isolated harnesses. Production engines
    /// receive theirs when the owning Player enters a Host.
    #[config(skip = "injected session binding")]
    #[debug(skip)]
    pub(crate) session: Option<SessionBinding<S>>,
    /// Typed pool facade for audio-thread scratch buffers.
    #[config(skip = "injected pooled scratch resource")]
    pub(crate) pools: PoolRegion<S>,
    /// EQ band layout per player. Default: 10-band log-spaced. Not a
    /// document key: every construction site in the workspace derives this
    /// from a generator (`generate_log_spaced_bands`), and a custom layout
    /// is installed at runtime through `PlayerImpl::set_eq_layout` rather
    /// than through config.
    #[config(skip = "layout moves to the live equalizer owner")]
    #[builder(default = generate_log_spaced_bands(10))]
    #[debug(skip)]
    pub(crate) eq_layout: Vec<EqBandConfig>,
    /// Render-pass slot gate smoothing. Default: 5 ms.
    #[config(value)]
    #[builder(default = DEFAULT_GATE_SMOOTHING)]
    pub(crate) gate_smoothing: SmootherConfig,
    /// Number of output channels. Default: 2 (stereo). Not a document key:
    /// the only reader is a startup log line, so a document value would
    /// change nothing the engine actually does.
    #[config(value)]
    #[builder(default = 2)]
    pub(crate) channels: u16,
    /// Maximum number of concurrent player slots. Default: 4.
    #[config(value)]
    #[builder(default = 4)]
    pub(crate) max_slots: usize,
}

#[cfg(test)]
mod tests {
    use kithara_config::Config as _;
    use kithara_test_utils::kithara;

    use super::{BeatGridId, DEFAULT_GATE_SMOOTHING, EngineConfig, NonZeroU32, NonZeroUsize};
    use crate::test_pools::{TestPools, pools};

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let config: EngineConfig<TestPools> = EngineConfig::builder()
            .grid_id(BeatGridId::allocate().expect("a grid identity"))
            .pools(pools())
            .sample_rate(NonZeroU32::new(48_000).expect("48000 is not zero"))
            .response_budget_frames(NonZeroUsize::new(448).expect("448 is not zero"))
            .build();

        assert_eq!(config.channels, 2);
        assert_eq!(config.max_slots, 4);
        assert_eq!(config.eq_layout.len(), 10);
        assert_eq!(config.gate_smoothing, DEFAULT_GATE_SMOOTHING);
    }

    #[kithara::test]
    fn retained_values_exclude_moved_layout_and_injected_resources() {
        let config: EngineConfig<TestPools> = EngineConfig::builder()
            .grid_id(BeatGridId::allocate().expect("a grid identity"))
            .pools(pools())
            .sample_rate(NonZeroU32::new(48_000).expect("48000 is not zero"))
            .response_budget_frames(NonZeroUsize::new(448).expect("448 is not zero"))
            .build();
        let values = config.values();
        assert_eq!(values.sample_rate.get(), 48_000);
        assert_eq!(values.response_budget_frames.get(), 448);
        assert_eq!(values.render_quantum_frames, None);
        assert_eq!(values.gate_smoothing, DEFAULT_GATE_SMOOTHING);
        assert_eq!(values.channels, 2);
        assert_eq!(values.max_slots, 4);
    }
}
