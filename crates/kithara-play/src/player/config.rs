use std::{fmt, num::NonZeroU32};

use bon::Builder;
use kithara_abr::AbrController;
use kithara_decode::GaplessMode;
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::{BeatGridId, StretchControls};

use crate::{
    PlayWorker,
    effects::eq::{EqBandConfig, generate_log_spaced_bands},
    session::SessionDispatcher,
};

fn allocate_grid_id() -> BeatGridId {
    let Ok(id) = BeatGridId::allocate() else {
        panic!("process-wide beat-grid identity space is exhausted");
    };
    id
}

/// Configuration for the player.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct PlayerConfig<S> {
    /// Stable synchronization-group identity owned by this player.
    #[builder(default = allocate_grid_id())]
    pub(crate) grid_id: BeatGridId,
    /// Per-deck time-stretch control handle, shared with the UI and the
    /// worker Warp chain (see `kithara_warp::StretchControls`).
    #[builder(default = StretchControls::new(1.0))]
    pub(crate) timestretch: Arc<StretchControls>,
    /// Explicit shared playback worker. Its pools and cancellation lifetime
    /// are configured once in [`crate::PlayWorkerConfig`].
    pub(crate) worker: PlayWorker<S>,
    /// How resources created for this player trim leading/trailing audio.
    #[builder(default)]
    pub(crate) gapless_mode: GaplessMode,
    /// Make audio-thread reads block on a producer-ring underrun instead of
    /// zero-filling the block. Offline (faster-than-real-time) harnesses opt
    /// in so rendered output never stretches with inserted silence while the
    /// decode worker catches up. Real-time hosts must keep the default
    /// (`false`): the audio callback can never block.
    #[builder(default)]
    pub(crate) block_on_underrun: bool,
    /// Shared ABR controller. When `None`, a default one is created.
    pub(crate) abr: Option<Arc<AbrController>>,
    /// Root event bus for this player.
    pub(crate) bus: Option<EventBus>,
    /// Master cancel token for this player.
    pub(crate) cancel: Option<CancelToken>,
    /// Optional pre-bound session for isolated harnesses. Production players
    /// are constructed unbound and attached exactly once by their Host.
    pub(crate) session: Option<Arc<dyn SessionDispatcher<S>>>,
    /// EQ band layout. Default: 10-band log-spaced.
    #[builder(default = generate_log_spaced_bands(10))]
    pub(crate) eq_layout: Vec<EqBandConfig>,
    /// Built-in auto-advance handler. Default: `true`.
    #[builder(default = true)]
    pub(crate) auto_advance_enabled: bool,
    /// Crossfade duration in seconds. Default: 1.0.
    #[builder(default = 1.0)]
    pub(crate) crossfade_duration: f32,
    /// Default playback-rate target (1.0 = normal). Default: 1.0.
    #[builder(default = 1.0)]
    pub(crate) default_rate: f32,
    /// Secondary lead time before EOF at which the next queued item is loaded.
    #[builder(default = 3.5)]
    pub(crate) prefetch_duration: f32,
    /// Initial output sample rate supplied by the owning session.
    pub(crate) sample_rate: NonZeroU32,
    /// Maximum concurrent slots in the engine. Default: 4.
    #[builder(default = 4)]
    pub(crate) max_slots: usize,
}

impl<S> Clone for PlayerConfig<S> {
    fn clone(&self) -> Self {
        Self {
            grid_id: self.grid_id,
            timestretch: Arc::clone(&self.timestretch),
            worker: self.worker.clone(),
            gapless_mode: self.gapless_mode,
            block_on_underrun: self.block_on_underrun,
            abr: self.abr.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            session: self.session.clone(),
            eq_layout: self.eq_layout.clone(),
            auto_advance_enabled: self.auto_advance_enabled,
            crossfade_duration: self.crossfade_duration,
            default_rate: self.default_rate,
            prefetch_duration: self.prefetch_duration,
            sample_rate: self.sample_rate,
            max_slots: self.max_slots,
        }
    }
}

impl<S> fmt::Debug for PlayerConfig<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlayerConfig")
            .field("gapless_mode", &self.gapless_mode)
            .field("eq_layout", &self.eq_layout)
            .field("auto_advance_enabled", &self.auto_advance_enabled)
            .field("crossfade_duration", &self.crossfade_duration)
            .field("default_rate", &self.default_rate)
            .field("prefetch_duration", &self.prefetch_duration)
            .field("max_slots", &self.max_slots)
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}
