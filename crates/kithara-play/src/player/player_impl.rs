use std::{num::NonZeroUsize, ops::Deref};

use delegate::delegate;
use kithara_abr::{AbrController, AbrSettings};
use kithara_bufpool::HasPool;
use kithara_command::mailbox;
use kithara_events::EventBus;
use kithara_platform::{
    CancelScope,
    sync::{Arc, ExclusiveGate, Mutex},
};
use kithara_warp::WarpConfigPatch;

use super::{
    PlayerView,
    command::{PlayerMailbox, PlayerPostbox},
    core::{PlayerCore, PlayerRuntime},
    lifecycle::PlayerLifecycle,
};
use crate::{
    EngineLoad,
    engine::{EngineConfig, EngineImpl},
    error::PlayError,
    player::{
        PlayerConfig, PlayerControl,
        config::TrackSettings,
        state::{CurrentItem, PlayerPhase, Tracks},
    },
};

/// Concrete Player implementation: one deck and the tracks it holds.
///
/// Its owner runs commands on it directly; its handles post them, and the
/// executor that holds it drains them. After each command it publishes what
/// its handles read.
#[derive(kithara_config::ConfigOwner)]
#[config_owner(PlayerConfig<S>, runtime.core.config)]
pub struct PlayerImpl<S> {
    pub(crate) runtime: Arc<PlayerRuntime<S>>,
    pub(in crate::player) view: PlayerView,
    pub(in crate::player) postbox: PlayerPostbox,
    pub(in crate::player) mailbox: PlayerMailbox,
}

impl<S> Deref for PlayerImpl<S> {
    type Target = PlayerRuntime<S>;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl<S: Send + Sync + 'static> PlayerImpl<S> {
    /// Create a new player with the given configuration.
    ///
    /// # Panics
    ///
    /// Panics if the supplied warp configuration violates its validated
    /// backend geometry invariant while applying the internal render quantum.
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

        let bus = config
            .bus
            .clone()
            .unwrap_or_else(|| EventBus::new(config.event_bus_capacity.get()));

        // Composed/standalone seam: `Some(parent)` → the player's master is a
        // child of it (so a passed cancel reaches the player but the player's
        // Drop never cancels the passed token); `None` → own root.
        let cancel = CancelScope::new(config.cancel.clone()).token();
        config.cancel = Some(cancel.clone());

        let engine_config = EngineConfig::builder()
            .grid_id(config.grid_id)
            .sample_rate(config.sample_rate)
            .eq_layout(config.eq_layout.clone())
            .maybe_response_budget_frames(config.response_budget_frames)
            .maybe_render_quantum_frames(config.warp.render_quantum_frames())
            .pools(pools)
            .maybe_session(config.session.clone())
            .cancel(cancel.clone())
            .build();
        let engine = EngineImpl::new(engine_config, bus.clone());
        if config.abr.is_none() {
            let abr_settings = AbrSettings::builder().cancel(cancel.clone()).build();
            config.abr = Some(AbrController::new(abr_settings));
        }

        let settings = TrackSettings::builder()
            .speed(config.default_rate())
            .keylock(config.warp.keylock())
            .backend(config.warp.backend());
        let tracks = Mutex::new(Tracks::new(settings.build()));
        let engine_load = Arc::new(EngineLoad::default());
        let core = PlayerCore {
            engine,
            config,
            engine_load: Arc::clone(&engine_load),
            status: Mutex::default(),
            start_position: Mutex::default(),
            current: CurrentItem::new(bus.clone()),
            tracks,
        };
        let runtime = PlayerRuntime {
            core,
            lifecycle: PlayerLifecycle::open(),
            operations: ExclusiveGate::default(),
            phase: Mutex::new(PlayerPhase::Idle),
        };
        let view = PlayerView::new(runtime.state(), engine_load, bus);
        let (postbox, mailbox) = mailbox();
        Self {
            runtime: Arc::new(runtime),
            view,
            postbox,
            mailbox,
        }
    }
}

impl<S> PlayerImpl<S> {
    /// Publishes what the player's handles read, as it stands now.
    pub(in crate::player) fn publish(&self) {
        self.view.publish(self.runtime.state());
    }

    /// What the player last published, for whoever reads it beside its owner.
    #[must_use]
    pub const fn view(&self) -> &PlayerView {
        &self.view
    }

    pub(in crate::player) fn make_control(&self) -> PlayerControl {
        PlayerControl::new(self.postbox.clone(), self.view.clone())
    }
}

/// Closes the player for every handle; posts it never drained read
/// unanswered.
impl<S> Drop for PlayerImpl<S> {
    fn drop(&mut self) {
        self.runtime.invalidate();
        self.view.publish(self.runtime.state());
    }
}

impl<S> crate::api::Equalizer for PlayerImpl<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
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
