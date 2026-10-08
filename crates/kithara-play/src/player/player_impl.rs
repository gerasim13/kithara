use std::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Deref,
};

use delegate::delegate;
use kithara_abr::{AbrController, AbrSettings};
use kithara_bufpool::HasPool;
use kithara_events::EventBus;
use kithara_platform::{
    CancelScope,
    sync::{Arc, ExclusiveGate, Mutex},
};
use kithara_warp::{BeatGridId, WarpConfigPatch};

use super::{
    core::{PlayerCore, PlayerRuntime},
    lifecycle::PlayerLifecycle,
};
use crate::{
    engine::{EngineConfig, EngineImpl},
    error::PlayError,
    player::{
        PlayerConfig, PlayerControl,
        config::TrackSettings,
        staging::SyncStaging,
        state::{ItemQueue, PlayerPhase, TrackGrid, TrackLanes},
    },
    worker::EngineLoad,
};

/// Concrete Player implementation managing items queue.
#[derive(kithara_config::ConfigOwner)]
#[config_owner(PlayerConfig<S>, runtime.core.config)]
pub struct PlayerImpl<S> {
    pub(crate) runtime: Arc<PlayerRuntime<S>>,
    /// Identity of the synchronization group the player's owner builds.
    pub(super) grid_id: BeatGridId,
    pub(super) sample_rate: NonZeroU32,
}

impl<S> Deref for PlayerImpl<S> {
    type Target = PlayerRuntime<S>;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl<S: Send + Sync + 'static> PlayerImpl<S> {
    /// Submit a crossfade duration while this player is open.
    ///
    /// # Errors
    /// Returns a closed-owner or slot command admission error.
    pub fn try_set_crossfade_duration(&self, seconds: f32) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.try_set_crossfade_duration(seconds))
    }

    /// Create a player with one lifetime track grid and its own cancellation subtree.
    /// Loads, replacements and releases revise that grid without changing group topology.
    /// A supplied cancel token is a parent; absence creates a standalone root. Drop leaves the parent live.
    /// Web sessions cannot stage Send receipts.
    ///
    /// # Panics
    /// Panics if internal render-quantum admission violates validated Warp backend geometry.
    #[must_use]
    pub fn new(mut config: PlayerConfig<S>) -> Self {
        config.normalize_live_values();
        if config.response_budget_frames.is_some() && config.warp.render_quantum_frames().is_none()
        {
            let mut patch = WarpConfigPatch::default();
            patch.render_quantum_frames = NonZeroUsize::new(32);
            config
                .warp
                .apply(patch)
                .expect("only a nonzero render quantum changes on valid backend geometry");
        }
        let pools = config.worker.pools().clone();
        let track_grid = TrackGrid::new(config.track_grid_id, config.sample_rate);

        let bus = config
            .bus
            .clone()
            .unwrap_or_else(|| EventBus::new(config.event_bus_capacity.get()));

        let cancel = CancelScope::new(config.cancel.clone()).token();
        config.cancel = Some(cancel.clone());

        let engine_config = EngineConfig::builder()
            .grid_id(config.grid_id)
            .sample_rate(config.sample_rate)
            .max_slots(config.max_slots)
            .eq_layout(config.eq_layout.clone())
            .maybe_response_budget_frames(config.response_budget_frames)
            .maybe_render_quantum_frames(config.warp.render_quantum_frames())
            .pools(pools)
            .maybe_session(config.session.clone())
            .cancel(cancel.clone())
            .build();
        let engine = EngineImpl::new(engine_config, bus.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let owner: Option<Arc<dyn kithara_sync::ReceiptSink>> =
            Some(Arc::new(engine.session().clone()));
        #[cfg(target_arch = "wasm32")]
        let owner = None;
        let staging = SyncStaging::new(config.track_grid_id, owner, cancel.clone());
        if config.abr.is_none() {
            let abr_settings = AbrSettings::builder().cancel(cancel.clone()).build();
            config.abr = Some(AbrController::new(abr_settings));
        }

        let settings = TrackSettings::builder()
            .speed(config.default_rate())
            .keylock(config.warp.keylock())
            .backend(config.warp.backend());
        let lanes = TrackLanes::new(settings.build());
        let grid_id = config.grid_id;
        let sample_rate = config.sample_rate;
        let core = PlayerCore {
            engine,
            config,
            staging,
            engine_load: Arc::new(EngineLoad::default()),
            status: Mutex::default(),
            start_position: Mutex::default(),
            items: ItemQueue::new(bus),
            track_grid,
            lanes,
        };
        Self {
            grid_id,
            sample_rate,
            runtime: Arc::new(PlayerRuntime {
                core,
                lifecycle: PlayerLifecycle::open(),
                operations: ExclusiveGate::default(),
                phase: Mutex::new(PlayerPhase::Idle),
            }),
        }
    }

    pub(in crate::player) fn make_control(&self) -> PlayerControl<S>
    where
        S: HasPool<f32>,
    {
        PlayerControl::new(Arc::clone(&self.runtime))
    }
}

impl<S> Drop for PlayerImpl<S> {
    fn drop(&mut self) {
        self.runtime.invalidate();
    }
}

impl<S> crate::api::Equalizer for PlayerImpl<S>
where
    S: Send + Sync + 'static,
{
    delegate! {
        to self {
            #[call(eq_band_count)]
            fn band_count(&self) -> usize;
            #[call(eq_gain)]
            fn gain(&self, band: usize) -> Option<f32>;
            #[call(reset_eq)]
            fn reset(&self) -> Result<(), PlayError>;
            #[call(set_eq_gain)]
            fn set_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError>;
        }
    }
}
