use delegate::delegate;
use kithara_abr::{AbrController, AbrSettings};
use kithara_audio::{EngineLoad, StretchControls};
use kithara_bufpool::{BytePool, PcmPool};
use kithara_decode::GaplessMode;
use kithara_platform::{
    CancelScope,
    sync::{Arc, Mutex},
};
use tracing::{debug, warn};

use super::{
    config::PlayerConfig,
    state::{ItemQueue, PlayerParams, PlayerPhase},
};
use crate::{
    api::{PlayerEvent, PlayerStatus},
    bridge::PlayerCmd,
    engine::{EngineConfig, EngineImpl},
    error::PlayError,
    resource::Resource,
};

/// Phase-neutral state shared across every player phase.
///
/// Field order is drop order: `items` (holding undelivered resources that
/// carry worker references) drops before `engine`, and `engine` (whose
/// `Drop` shuts the worker down) drops last.
pub(crate) struct PlayerCore {
    /// Serializes phase/slot transitions that may start or stop the engine.
    pub(crate) lifecycle: Mutex<()>,
    /// Live shared cost meter of the audio engine (decode + effects).
    /// Constructed once and kept address-stable for the player's lifetime.
    pub(crate) engine_load: Arc<EngineLoad>,

    pub(crate) timestretch: Arc<StretchControls>,
    pub(crate) byte_pool: BytePool,
    /// Engine drops last — worker shutdown happens after all tracks
    /// unregister and after `items` releases their resources.
    pub(crate) engine: EngineImpl,
    pub(crate) gapless_mode: GaplessMode,
    /// Items drop before engine — Audio tracks unregister from worker
    /// while it is still alive.
    pub(crate) items: ItemQueue,
    /// Status kept explicit (not derived from phase): `set_status` emits
    /// `StatusChanged` only on change and its values are not 1:1 with phase.
    pub(crate) status: Mutex<PlayerStatus>,
    pub(crate) params: PlayerParams,
}

/// Concrete Player implementation managing items queue.
///
/// Owns an [`EngineImpl`] and sends commands to the active slot's processor.
/// When `play()` is called, the engine is lazily started and a slot is
/// allocated. The current queue item is taken out of the queue, wrapped in
/// [`PlayerResource`](crate::rt::track::PlayerResource), and sent
/// to the processor via `PlayerCmd::LoadTrack`.
///
/// Internally the player is a phase-split typestate: `phase` is a typed
/// `Mutex<PlayerPhase>` carrying the slot / ABR handle / armed-next, while
/// `core` holds the phase-neutral fields. `phase` is declared first so it
/// drops before `core.engine`.
pub struct PlayerImpl {
    pub(crate) phase: Mutex<PlayerPhase>,
    pub(crate) core: PlayerCore,
}

impl PlayerImpl {
    /// Minimum playback rate to prevent stalling.
    pub(crate) const MIN_PLAYBACK_RATE: f32 = PlayerParams::MIN_PLAYBACK_RATE;

    /// Create a new player with the given configuration.
    #[must_use]
    pub fn new(mut config: PlayerConfig) -> Self {
        let resolved_pool = config.pcm_pool.clone();

        let bus = config.bus.clone().unwrap_or_default();

        // Composed/standalone seam: `Some(parent)` → the player's master is a
        // child of it (so a passed cancel reaches the player but the player's
        // Drop never cancels the passed token); `None` → own root.
        let cancel = CancelScope::new(config.cancel.clone()).token();
        config.cancel = Some(cancel.clone());

        let engine_config = EngineConfig::builder()
            .eq_layout(config.eq_layout.clone())
            .max_slots(config.max_slots)
            .sample_rate(config.sample_rate)
            .pcm_pool(resolved_pool)
            .maybe_session(config.session.clone())
            .cancel(cancel)
            .build();
        let engine = EngineImpl::new(engine_config, bus.clone());
        if config.abr.is_none() {
            config.abr = Some(AbrController::new(AbrSettings::default()));
        }

        // Seed the single speed source with the configured default rate.
        config.timestretch.set_speed(config.default_rate);
        let core = PlayerCore {
            lifecycle: Mutex::default(),
            engine,
            engine_load: Arc::new(EngineLoad::default()),
            params: PlayerParams::from(&config),
            timestretch: config.timestretch,
            gapless_mode: config.gapless_mode,
            byte_pool: config.byte_pool,
            status: Mutex::default(),
            items: ItemQueue::new(bus),
        };
        Self {
            core,
            phase: Mutex::new(PlayerPhase::Idle),
        }
    }

    delegate! {
        to self.core.items {
            /// Advance to the next item in the queue.
            ///
            /// Does nothing if the current item is already the last one.
            pub fn advance_to_next_item(&self);
            /// Sole publisher of `CurrentItemChanged`: emits only when `index` differs
            /// from the last announced item, so a `play()` resume of the same item
            /// stays quiet.
            pub(crate) fn announce_current_item(&self, index: usize);
            /// Drop the resource at `index` so the auto-advance prefetch path
            /// (`arm_next`) cannot plant it into the audio thread.
            ///
            /// Used by the queue when a previously-loaded track is cancelled by
            /// a later `select` — without this, a slow track whose loader
            /// raced ahead of the override stays in `items` and the next
            /// `TrackRequested` notification near EOF would arm it for
            /// handover, surfacing as a barge-in.
            pub fn clear_item(&self, index: usize);
            /// Insert a resource with optional queue-item identity metadata at a
            /// specific position, or append to the end.
            pub fn insert(
                &self,
                resource: Resource,
                item_id: Option<Arc<str>>,
                at_position: Option<usize>,
            );
            /// Replace a consumed (or existing) resource at the given index with item
            /// identity metadata.
            pub fn replace_item_tagged(&self, index: usize, resource: Resource, item_id: Option<Arc<str>>);
            /// Pre-allocate empty slots so `replace_item` can fill them by index.
            pub fn reserve_slots(&self, count: usize);
        }
    }

    /// Byte pool used for resources created by this player.
    #[must_use]
    pub const fn byte_pool(&self) -> &BytePool {
        &self.core.byte_pool
    }

    pub(crate) fn enqueue_to_processor(&self, index: usize) -> Option<(Arc<str>, f64)> {
        let item = self.core.items.take_for_load(
            index,
            self.core.timestretch.speed(),
            self.core.engine.master_sample_rate(),
            self.core.engine.pcm_pool(),
        )?;
        self.phase.lock().set_abr_handle(item.abr_handle);
        let src = Arc::clone(item.player_resource.src());
        let _ = self.send_to_slot(PlayerCmd::LoadTrack {
            item_id: item.item_id,
            resource: Box::new(item.player_resource),
        });
        Some((src, item.duration_seconds))
    }

    /// PCM pool used by this player's audio engine.
    #[must_use]
    pub fn pcm_pool(&self) -> &PcmPool {
        self.core.engine.pcm_pool()
    }

    /// Remove all items, release the active slot, and stop the engine.
    pub fn remove_all_items(&self) {
        let _lifecycle = self.core.lifecycle.lock();
        self.unarm_next();
        self.core.items.clear_all();
        self.set_status(PlayerStatus::Unknown);
        self.core.params.set_paused_rate();
        let slot = self.slot();
        let _ = self.send_to_slot(PlayerCmd::Clear);

        if self.core.engine.is_running() {
            if let Some(slot) = slot
                && let Err(error) = self.core.engine.release_slot(slot)
            {
                warn!(?slot, ?error, "failed to release player slot during stop");
            }
            if let Err(error) = self.core.engine.stop() {
                warn!(?error, "failed to stop player engine");
            }
        }

        self.enter_stopped();
        self.core
            .engine
            .bus()
            .publish(PlayerEvent::RateChanged { rate: 0.0 });
        debug!("all items removed");
    }

    /// Remove item at index. Returns the removed resource, or `None` if out of
    /// bounds or already consumed.
    pub fn remove_at(&self, index: usize) -> Option<Resource> {
        self.unarm_next();

        self.core
            .items
            .remove_at(index)
            .map(|queued| queued.resource)
    }

    /// Replace a consumed (or existing) resource at the given index.
    ///
    /// Use this to re-load a track that was previously played and consumed
    /// by `load_current_item`. Does nothing if `index` is out of bounds.
    pub fn replace_item(&self, index: usize, resource: Resource) {
        self.replace_item_tagged(index, resource, None);
    }

    /// Internal: set status and emit event if changed.
    pub(crate) fn set_status(&self, new_status: PlayerStatus) {
        let mut status = self.core.status.lock();
        if *status != new_status {
            *status = new_status;
            drop(status);
            self.core
                .engine
                .bus()
                .publish(PlayerEvent::StatusChanged { status: new_status });
        }
    }
}

impl Drop for PlayerImpl {
    fn drop(&mut self) {
        self.core.engine.cancel();
    }
}

impl crate::api::Equalizer for PlayerImpl {
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

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread};

    use kithara_assets::AssetStore;
    use kithara_audio::{ConsumerWakeMode, StretchControls, generate_log_spaced_bands};
    use kithara_bufpool::{BytePool, PcmPool};
    use kithara_decode::GaplessMode;
    use kithara_events::{Envelope, Event};
    use kithara_platform::{CancelToken, time::Duration};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        api::SlotId,
        bridge::PlayerCmd,
        session::{Cmd, Reply, SessionDispatcher, testing},
    };

    type SessionReply = Result<Reply, PlayError>;
    type SessionCommand = (Cmd, mpsc::Sender<SessionReply>);

    struct ThreadedTestSession {
        commands: mpsc::Sender<SessionCommand>,
    }

    impl Default for ThreadedTestSession {
        fn default() -> Self {
            let (commands, receiver) = mpsc::channel::<SessionCommand>();
            thread::spawn(move || {
                let session = testing::test_session();
                for (command, reply) in receiver {
                    let _ = reply.send(session.exec(command));
                }
            });
            Self { commands }
        }
    }

    impl SessionDispatcher for ThreadedTestSession {
        fn exec(&self, command: Cmd) -> SessionReply {
            let (reply, receiver) = mpsc::channel();
            self.commands
                .send((command, reply))
                .map_err(|_| PlayError::Internal("test session stopped".into()))?;
            receiver
                .recv()
                .map_err(|_| PlayError::Internal("test session dropped its reply".into()))?
        }

        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }
    }

    struct BlockingStopSession {
        inner: ThreadedTestSession,
        stop_entered: Mutex<Option<mpsc::Sender<()>>>,
        resume_stop: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl SessionDispatcher for BlockingStopSession {
        fn exec(&self, command: Cmd) -> SessionReply {
            if matches!(&command, Cmd::StopPlayer { .. }) {
                if let Some(entered) = self.stop_entered.lock().take() {
                    entered
                        .send(())
                        .map_err(|_| PlayError::Internal("stop observer dropped".into()))?;
                }
                if let Some(release) = self.resume_stop.lock().take() {
                    release
                        .recv()
                        .map_err(|_| PlayError::Internal("stop release dropped".into()))?;
                }
            }
            self.inner.exec(command)
        }

        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            self.inner.consumer_wake_mode()
        }
    }

    #[derive(Clone, Copy)]
    enum PlayerBasicScenario {
        AdvanceOnEmpty,
        EngineAccessor,
        QueueStartsEmpty,
        SendToSlotWithoutSlot,
        StartsPaused,
    }

    fn resource_config(input: &str) -> crate::resource::ResourceConfig {
        let src = crate::resource::ResourceConfig::parse_src(input)
            .expect("BUG: valid resource config source");
        crate::resource::ResourceConfig::for_src(src)
            .store(AssetStore::builder().build())
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .build()
    }

    #[kithara::test]
    fn prepare_config_applies_player_gapless_mode() {
        let player = PlayerImpl::new(
            PlayerConfig::test_builder()
                .gapless_mode(GaplessMode::Disabled)
                .build(),
        );
        let mut config = resource_config("https://example.com/song.mp3");

        config = player.prepare_config(config);

        assert_eq!(config.decoder.gapless_mode(), GaplessMode::Disabled);
        assert!(
            config.cancel.is_some(),
            "prepare_config must inject a per-track cancel child"
        );
        player.worker().shutdown();
    }

    #[kithara::test]
    fn prepare_config_per_track_cancel_is_child_of_player_master() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        let mut rc = resource_config("https://example.com/song.mp3");
        rc = player.prepare_config(rc);

        let track_cancel = rc.cancel.expect("prepare_config must populate cancel");
        let observer = track_cancel.child();
        assert!(!observer.is_cancelled());

        drop(player);
        assert!(
            observer.is_cancelled(),
            "dropping the player must cancel the per-track child via the master"
        );
    }

    #[kithara::test]
    fn prepare_config_preserves_caller_supplied_master() {
        let parent_master = CancelToken::never();
        let player = PlayerImpl::new(
            PlayerConfig::test_builder()
                .cancel(parent_master.clone())
                .build(),
        );
        let mut rc = resource_config("https://example.com/song.mp3");
        rc = player.prepare_config(rc);

        let track_cancel = rc.cancel.expect("prepare_config must populate cancel");
        let observer = track_cancel.child();
        assert!(!observer.is_cancelled());

        parent_master.cancel();
        assert!(observer.is_cancelled());
        player.worker().shutdown();
    }

    #[kithara::test]
    #[case(PlayerBasicScenario::StartsPaused)]
    #[case(PlayerBasicScenario::QueueStartsEmpty)]
    #[case(PlayerBasicScenario::AdvanceOnEmpty)]
    #[case(PlayerBasicScenario::EngineAccessor)]
    #[case(PlayerBasicScenario::SendToSlotWithoutSlot)]
    fn player_basic_behaviors(#[case] scenario: PlayerBasicScenario) {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        match scenario {
            PlayerBasicScenario::StartsPaused => {
                assert!((player.rate() - 0.0).abs() < f32::EPSILON);
                assert_eq!(player.status(), PlayerStatus::Unknown);
            }
            PlayerBasicScenario::QueueStartsEmpty => {
                assert_eq!(player.item_count(), 0);
            }
            PlayerBasicScenario::AdvanceOnEmpty => {
                player.advance_to_next_item();
                assert_eq!(player.current_index(), 0);
            }
            PlayerBasicScenario::EngineAccessor => {
                assert!(!player.engine().is_running());
            }
            PlayerBasicScenario::SendToSlotWithoutSlot => {
                let result = player.send_to_slot(PlayerCmd::SetPaused(true));
                assert!(result.is_err());
            }
        }
    }

    #[kithara::test]
    fn player_pause_sets_rate_zero() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        player.core.params.set_rate_value(1.0);
        player.pause();
        assert!((player.rate() - 0.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn player_volume_clamps() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        player.set_volume(2.0);
        assert!((player.volume() - 1.0).abs() < f32::EPSILON);
        player.set_volume(-1.0);
        assert!((player.volume() - 0.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn player_muted() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert!(!player.is_muted());
        player.set_muted(true);
        assert!(player.is_muted());
    }

    #[kithara::test]
    fn player_crossfade_duration() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert!((player.crossfade_duration() - 1.0).abs() < f32::EPSILON);
        player.set_crossfade_duration(3.0);
        assert!((player.crossfade_duration() - 3.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn player_prefetch_duration() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert!((player.prefetch_duration() - 3.5).abs() < f32::EPSILON);
        player.set_prefetch_duration(8.0);
        assert!((player.prefetch_duration() - 8.0).abs() < f32::EPSILON);
        player.set_prefetch_duration(-1.0);
        assert!((player.prefetch_duration() - 0.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn player_events_subscribe() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        let mut rx = player.subscribe();
        player.set_volume(0.5);
        let event = rx.try_recv();
        assert!(event.is_ok());
    }

    #[kithara::test]
    fn player_config_custom() {
        let config = PlayerConfig::test_builder()
            .crossfade_duration(2.0)
            .prefetch_duration(5.0)
            .default_rate(0.5)
            .eq_layout(generate_log_spaced_bands(5))
            .gapless_mode(GaplessMode::MediaOnly)
            .max_slots(2)
            .sample_rate(44_100)
            .timestretch(StretchControls::new(1.0))
            .build();
        let player = PlayerImpl::new(config);
        assert!((player.crossfade_duration() - 2.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn eq_band_count_tracks_a_replacement_layout_before_start() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .eq_layout(generate_log_spaced_bands(3))
                .byte_pool(BytePool::default())
                .pcm_pool(PcmPool::default())
                .build(),
        );
        assert_eq!(player.eq_band_count(), 3);

        player.set_eq_layout(generate_log_spaced_bands(4)).unwrap();
        assert_eq!(player.eq_band_count(), 4);
    }

    #[kithara::test]
    fn player_config_builder() {
        let config = PlayerConfig::test_builder()
            .max_slots(8)
            .default_rate(0.5)
            .crossfade_duration(2.5)
            .prefetch_duration(7.0)
            .eq_layout(generate_log_spaced_bands(5))
            .build();
        assert_eq!(config.max_slots, 8);
        assert!((config.default_rate - 0.5).abs() < f32::EPSILON);
        assert!((config.crossfade_duration - 2.5).abs() < f32::EPSILON);
        assert!((config.prefetch_duration - 7.0).abs() < f32::EPSILON);
        assert_eq!(config.eq_layout.len(), 5);
    }

    #[kithara::test]
    fn player_default_rate_getter_setter() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert!((player.default_rate() - 1.0).abs() < f32::EPSILON);
        player.set_default_rate(0.75);
        assert!((player.default_rate() - 0.75).abs() < f32::EPSILON);
    }

    #[kithara::test(tokio)]
    async fn player_multiple_events_in_order() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        let mut rx = player.subscribe();

        player.set_volume(0.5);
        player.set_muted(true);
        player.set_rate(2.0);

        let e1 = rx.try_recv();
        let e2 = rx.try_recv();
        let e3 = rx.try_recv();
        assert!(matches!(
            e1,
            Ok(Envelope {
                event: Event::Player(PlayerEvent::VolumeChanged { .. }),
                ..
            })
        ));
        assert!(matches!(
            e2,
            Ok(Envelope {
                event: Event::Player(PlayerEvent::MuteChanged { .. }),
                ..
            })
        ));
        assert!(matches!(
            e3,
            Ok(Envelope {
                event: Event::Player(PlayerEvent::RateChanged { .. }),
                ..
            })
        ));
    }

    #[kithara::test(tokio)]
    async fn player_negative_crossfade_duration_clamped() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        player.set_crossfade_duration(-5.0);
        assert!((player.crossfade_duration() - 0.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn set_rate_updates_shared_speed() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        player.set_rate(2.0);
        assert!((player.core.timestretch.speed() - 2.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn set_rate_clamps_invalid_values() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        player.set_rate(0.0);
        assert!(player.rate() >= 0.01);
        assert!(player.core.timestretch.speed() >= 0.01);

        player.set_rate(-1.0);
        assert!(player.rate() >= 0.01);
    }

    #[kithara::test]
    fn timestretch_is_address_stable_across_play_pause() {
        let player = PlayerImpl::new(
            PlayerConfig::test_builder()
                .session(testing::test_session())
                .build(),
        );
        let ptr_before = Arc::as_ptr(&player.core.timestretch);
        player.play();
        player.pause();
        player.play();
        let ptr_after = Arc::as_ptr(&player.core.timestretch);
        assert_eq!(
            ptr_before, ptr_after,
            "timestretch controls must stay address-stable across transitions"
        );
        player.worker().shutdown();
    }

    #[kithara::test]
    fn remove_all_items_stops_engine_and_restart_allocates_fresh_slot() {
        let player = PlayerImpl::new(
            PlayerConfig::test_builder()
                .session(testing::test_session())
                .build(),
        );
        player.play();
        let first_slot = player.slot().expect("play must allocate a slot");
        assert!(player.engine().is_running(), "setup must start the engine");

        player.remove_all_items();

        assert!(
            !player.engine().is_running(),
            "clearing must stop the engine"
        );
        assert!(
            player.slot().is_none(),
            "clearing must release slot ownership"
        );

        player.remove_all_items();
        player.play();
        let restarted_slot = player.slot().expect("restart must allocate a slot");
        assert_ne!(
            restarted_slot, first_slot,
            "restart must not reuse the slot invalidated by engine stop"
        );
        player.remove_all_items();
        player.worker().shutdown();
    }

    #[kithara::test]
    fn play_waits_for_stop_and_restarts_with_a_fresh_slot() {
        let (stop_entered, stop_observer) = mpsc::channel();
        let (resume_stop, stop_release) = mpsc::channel();
        let session: Arc<dyn SessionDispatcher> = Arc::new(BlockingStopSession {
            inner: ThreadedTestSession::default(),
            stop_entered: Mutex::new(Some(stop_entered)),
            resume_stop: Mutex::new(Some(stop_release)),
        });
        let player = Arc::new(PlayerImpl::new(
            PlayerConfig::test_builder().session(session).build(),
        ));
        player.play();
        let first_slot = player.slot().expect("play must allocate a slot");

        let stopping_player = Arc::clone(&player);
        let stop_thread = thread::spawn(move || stopping_player.remove_all_items());
        stop_observer
            .recv_timeout(Duration::from_secs(1))
            .expect("stop must reach the session boundary");

        let (play_started, play_observer) = mpsc::channel();
        let (play_finished, play_completion) = mpsc::channel();
        let restarting_player = Arc::clone(&player);
        let play_thread = thread::spawn(move || {
            play_started.send(()).expect("play observer must be alive");
            restarting_player.play();
            play_finished
                .send(())
                .expect("play completion observer must be alive");
        });
        play_observer
            .recv_timeout(Duration::from_secs(1))
            .expect("play thread must start");
        let completed_before_stop = play_completion
            .recv_timeout(Duration::from_millis(250))
            .is_ok();

        resume_stop.send(()).expect("stop thread must be waiting");
        stop_thread.join().expect("stop thread must finish");
        play_thread.join().expect("play thread must finish");

        assert!(
            !completed_before_stop,
            "play must wait until stop finishes invalidating its slot"
        );
        assert!(
            player.engine().is_running(),
            "serialized play must restart the engine"
        );
        assert_ne!(
            player.slot(),
            Some(first_slot),
            "serialized play must allocate a fresh slot"
        );
        player.remove_all_items();
        player.worker().shutdown();
    }

    #[kithara::test]
    fn pause_from_idle_is_noop() {
        use super::super::state::phase::PlayerPhaseKind;

        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert_eq!(player.phase_kind(), PlayerPhaseKind::Idle);
        player.pause();
        assert_eq!(
            player.phase_kind(),
            PlayerPhaseKind::Idle,
            "pause from Idle must not leak a phase transition"
        );
        assert!((player.rate() - 0.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn position_seconds_idle_is_none() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert!(player.position_seconds().is_none());
        assert!(player.duration_seconds().is_none());
        assert!(!player.is_playing());
        assert!(player.current_abr_handle().is_none());
        assert!(player.armed_next().is_none());
    }

    #[kithara::test]
    fn drop_player_releases_tracks_before_engine() {
        // Worker-registered tracks live in `engine`; undelivered resources in
        // `playlist` carry worker references. The phase (slot/abr/pending) holds
        // no worker-registered track directly. This pins that constructing,
        // arming a phase, and dropping does not panic / UAF: `phase` and
        // `core.items` must drop before `core.engine`.
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        *player.phase.lock() = PlayerPhase::Playing {
            slot: SlotId::new(0),
            abr_handle: None,
            pending: None,
        };
        player.worker().shutdown();
        drop(player);
    }

    #[kithara::test]
    fn set_rate_emits_rate_changed() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        let mut rx = player.subscribe();
        player.set_rate(2.0);
        let e = rx.try_recv();
        assert!(matches!(
            e,
            Ok(Envelope {
                event: Event::Player(PlayerEvent::RateChanged { .. }),
                ..
            })
        ));
    }

    #[kithara::test]
    fn player_exposes_worker() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        let _w = player.worker();
        let _w2 = player.worker().clone();
    }

    #[kithara::test]
    fn auto_advance_enabled_default_and_toggle() {
        let player = PlayerImpl::new(PlayerConfig::test_builder().build());
        assert!(player.auto_advance_enabled(), "default must be on");
        player.set_auto_advance_enabled(false);
        assert!(!player.auto_advance_enabled());
        player.set_auto_advance_enabled(true);
        assert!(player.auto_advance_enabled());
    }

    #[kithara::test]
    fn auto_advance_disabled_via_config() {
        let player = PlayerImpl::new(
            PlayerConfig::test_builder()
                .auto_advance_enabled(false)
                .build(),
        );
        assert!(!player.auto_advance_enabled());
    }
}
