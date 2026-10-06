use core::ops::Deref;

use kithara_assets::{AssetStore, StorageBackend};
use kithara_bufpool::HasPool;
use kithara_command::{Mailbox, Postbox, mailbox};
use kithara_events::{EventBus, EventReceiver, TrackId};
use kithara_platform::{
    CancelScope, CancelToken,
    sync::{Arc, Mutex, MutexGuard},
    tokio::runtime::Handle as RuntimeHandle,
};
use kithara_play::{
    PlayError, PlayerImpl,
    player::{PlayerControl, PlayerControlSource},
};

use super::{
    command::QueueCommand,
    engine_events::PlayerBusEvent,
    types::{AtomicCachedPosition, CachedPosition, SelectPhase},
};
use crate::{
    config::QueueConfig,
    loader::Loader,
    navigation::NavigationState,
    track::{TrackRecord, Tracks},
};

/// What a queue and every [`QueueControl`] of it read: the tracks, the
/// navigation, the retained config, the cached position and the player's
/// published state. Only the queue writes them, on the executor that holds
/// it.
#[doc(hidden)]
pub struct QueueRuntime<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(super) player: PlayerControl<S>,
    pub(super) loader: Arc<Loader<S>>,
    pub(super) navigation: Arc<Mutex<NavigationState>>,
    /// Sole owner of the `Vec<TrackRecord>` (status, source, and live
    /// load attempt per track). Shared with [`Loader`] through
    /// `Arc<Tracks>`, whose attempts report to the queue instead of writing
    /// it; every status transition goes through
    /// [`Tracks::set_status`](crate::track::Tracks::set_status) so polling
    /// and the event stream stay in sync.
    pub(super) tracks: Arc<Tracks<S>>,
    /// Authoritative playback position updated on every `tick`. Filters
    /// transient 0.0 blips the engine reports on pause/resume —
    /// downstream UIs should read from this field rather than polling
    /// the engine directly. Read/written lock-free as a typed
    /// [`CachedPosition`] — [`CachedPosition::Unknown`] before the first
    /// stable sample.
    pub(super) cached_position: AtomicCachedPosition,
    /// Master cancel token for queue-owned loader work.
    pub(super) shutdown: CancelToken,
    pub(super) bus: EventBus,
    pub(super) config: Arc<QueueConfig<S>>,
}

/// Cloneable queue command capability without beat-grid identity or topology.
///
/// Every command is posted to the queue and waits for its answer, which the
/// executor holding the queue gives once it drains it; a command posted
/// before any executor holds the queue waits for one. Once the queue is
/// dropped a command fails with [`PlayError::Closed`].
#[derive_where::derive_where(Clone; S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static)]
pub struct QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(super) postbox: Postbox<QueueCommand<S>>,
    pub(super) runtime: Arc<QueueRuntime<S>>,
}

/// AVQueuePlayer-analogue orchestration facade.
///
/// Owns the resident player and the queue's state, and runs the commands its
/// [`QueueControl`]s post where an executor holds it. Publishes
/// [`QueueEvent`](crate::event::QueueEvent) on the shared [`EventBus`]
/// alongside player / audio / hls / file events so `subscribe` returns a
/// single unified stream.
pub struct Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(super) resident: PlayerImpl<S>,
    pub(super) runtime: Arc<QueueRuntime<S>>,
    pub(super) postbox: Postbox<QueueCommand<S>>,
    pub(super) mailbox: Mailbox<QueueCommand<S>>,
    pub(super) pending_select: SelectPhase,
    /// Track whose load completion starts playback: the first one appended
    /// while nothing is selected, when [`QueueConfig::should_autoplay`] is on.
    pub(super) autoplay_target: Option<TrackId>,
    /// Subscription to the shared bus; drained in `tick()` to convert
    /// engine events into queue-level side-effects (auto-advance / current
    /// track change forwarding).
    pub(super) player_rx: EventReceiver<PlayerBusEvent>,
}

impl<S> Deref for QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Target = QueueRuntime<S>;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl<S> Deref for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Target = QueueRuntime<S>;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Build a queue from a [`QueueConfig`].
    ///
    /// The queue takes ownership of the supplied [`PlayerImpl`]; all access to
    /// the decorated player then goes through this facade.
    #[must_use]
    pub fn new(mut config: QueueConfig<S>) -> Self {
        let player = config
            .player
            .take()
            .unwrap_or_else(|| unreachable!("QueueConfig builder requires a player"));
        let runtime = config.runtime.take();
        let store = config.store.take();
        let config_cancel = config.cancel.take();
        let max_concurrent_loads = config.max_concurrent_loads;
        let max_history_size = config.max_history_size;
        let playback_order = config.playback_order;
        let crossfade_settings = config.crossfade_settings();
        let cancel = CancelScope::new(config_cancel).token();
        let store = store.unwrap_or_else(|| {
            AssetStore::builder(player.pools().clone())
                .backend(StorageBackend::default())
                .cancel(cancel.child())
                .build()
        });
        player.set_crossfade_duration(crossfade_settings.duration);
        let bus = player.bus().clone();
        let player_control = player.control();
        let tracks = Arc::new(Tracks::new(bus.clone()));
        let (postbox, mailbox) = mailbox();
        let loader = Arc::new(Loader::new(
            player_control.clone(),
            runtime.or_else(|| RuntimeHandle::try_current().ok()),
            store,
            max_concurrent_loads,
            Arc::clone(&tracks),
            postbox.clone(),
            cancel.child(),
        ));
        let player_rx = player.subscribe();
        let mut navigation = NavigationState::new(max_history_size);
        navigation.set_playback_order(playback_order, &[]);
        let navigation = Arc::new(Mutex::new(navigation));
        config.navigation = Some(Arc::clone(&navigation));
        Self {
            resident: player,
            runtime: Arc::new(QueueRuntime {
                player: player_control,
                loader,
                tracks,
                bus,
                config: Arc::new(config),
                shutdown: cancel,
                navigation,
                cached_position: AtomicCachedPosition::unknown(),
            }),
            postbox,
            mailbox,
            pending_select: SelectPhase::Idle,
            autoplay_target: None,
            player_rx,
        }
    }

    pub(in crate::queue) fn command(&mut self, operation: impl FnOnce(&mut Self)) {
        if !self.is_closed() {
            operation(self);
        }
    }

    pub(in crate::queue) fn with_open<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, PlayError> {
        self.ensure_open()?;
        Ok(operation(self))
    }

    pub(in crate::queue) fn with_open_result<T, E>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<PlayError>,
    {
        self.ensure_open().map_err(E::from)?;
        operation(self)
    }
}

impl<S> QueueRuntime<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(in crate::queue) fn ensure_open(&self) -> Result<(), PlayError> {
        if self.is_closed() {
            Err(PlayError::Closed)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shutdown.is_cancelled() || self.player.is_closed()
    }

    delegate::delegate! {
        to self.tracks {
            #[call(lock)]
            pub(super) fn lock_tracks(&self) -> MutexGuard<'_, Vec<TrackRecord<S>>>;
            #[call(lock)]
            pub(super) fn lock_tracks_mut(&self) -> MutexGuard<'_, Vec<TrackRecord<S>>>;
            pub(super) fn set_status(&self, id: TrackId, status: crate::event::TrackStatus);
        }
        to self.cached_position {
            #[call(load)]
            pub(super) fn read_cached_position(&self) -> CachedPosition;
            #[call(store)]
            pub(super) fn write_cached_position(&self, pos: CachedPosition);
        }
        to self.navigation {
            #[call(lock)]
            pub(super) fn lock_navigation(&self) -> MutexGuard<'_, NavigationState>;
            #[call(lock)]
            pub(super) fn lock_navigation_mut(&self) -> MutexGuard<'_, NavigationState>;
        }
    }
}

impl<S> Drop for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::mpsc,
        task::{Wake, Waker},
    };

    use kithara_config::Config;
    use kithara_events::{Envelope, EventReceiver};
    use kithara_platform::{
        thread,
        time::{Duration, Instant, timeout},
        tokio::sync::mpsc::{UnboundedSender, unbounded_channel},
    };
    use kithara_play::{
        PlayError, PlayWorker, PlayWorkerConfig, PlayerConfig, SessionBinding,
        player::{Player, PlayerControlSource},
    };
    use kithara_test_utils::kithara;
    use kithara_warp::BeatGridId;

    use super::*;
    use crate::{
        consts,
        event::{QueueEvent, TrackStatus},
        navigation::{ActionAtItemEnd, PlaybackOrder},
        test_pools::{TestPools, pools},
    };

    /// No queue test ever streams bytes, so the store is here to be wired, not
    /// to hold anything. The default backend would map a file under the shared
    /// temp root, which every parallel test process also owns and which Miri
    /// cannot map at all.
    pub(in crate::queue) fn make_store() -> AssetStore<TestPools> {
        AssetStore::builder(pools())
            .backend(StorageBackend::Memory)
            .build()
    }

    pub(in crate::queue) fn make_queue() -> Queue<TestPools> {
        Queue::new(queue_config())
    }

    pub(crate) fn test_session() -> SessionBinding<TestPools> {
        kithara_play::mock::session()
    }

    fn queue_config() -> QueueConfig<TestPools> {
        let player = player();
        let store = AssetStore::builder(player.pools().clone())
            .backend(StorageBackend::Memory)
            .build();
        QueueConfig::builder().player(player).store(store).build()
    }

    fn player() -> PlayerImpl<TestPools> {
        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
        PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(consts::TEST_SAMPLE_RATE)
                .worker(worker)
                .session(test_session())
                .build(),
        )
    }

    pub(in crate::queue) async fn wait_for_queue_event<F>(
        rx: &mut EventReceiver<QueueEvent>,
        mut matches: F,
        timeout_ms: u64,
    ) -> bool
    where
        F: FnMut(&QueueEvent) -> bool,
    {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match timeout(remaining, rx.recv()).await {
                Ok(Ok(Envelope { event: ev, .. })) if matches(&ev) => return true,
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => return false,
            }
        }
    }

    #[kithara::test]
    fn queue_new_constructs_without_panic() {
        let _queue = make_queue();
    }

    #[kithara::test]
    fn queue_registers_its_resident_players_deck_with_the_session() {
        let grid_id = BeatGridId::allocate().expect("fixture grid id");
        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .grid_id(grid_id)
                .sample_rate(consts::TEST_SAMPLE_RATE)
                .worker(worker)
                .build(),
        );
        let mut queue = Queue::new(QueueConfig::builder().player(player).build());

        let deck = queue
            .attach_session(test_session())
            .expect("the queue binds its session");
        assert_eq!(deck, grid_id);
    }

    /// A holder's waker that reports each wake.
    struct Wakes(mpsc::Sender<()>);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    #[kithara::test]
    fn a_control_command_runs_when_the_holder_drains_the_queue() {
        let mut queue = make_queue();
        let control = queue.control();
        let (woke_tx, woke_rx) = mpsc::channel();
        Player::hold(&mut queue, Waker::from(Arc::new(Wakes(woke_tx))));

        let append = thread::spawn(move || {
            let appended = control.append("https://example.com/a.mp3");
            (control, appended)
        });
        woke_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a post wakes the holder");
        assert!(
            queue.is_empty(),
            "the command waits for the holder to drain"
        );

        Player::drain(&mut queue);
        let (control, appended) = append.join().expect("append thread must not panic");
        let id = appended.expect("an open queue appends");
        assert_eq!(queue.tracks().first().map(|track| track.id), Some(id));

        drop(queue);
        assert!(matches!(
            control.append("https://example.com/b.mp3"),
            Err(crate::QueueError::Play(PlayError::Closed))
        ));
    }

    /// A holder's waker that reports each wake to an async waiter.
    struct WakesTask(UnboundedSender<()>);

    impl Wake for WakesTask {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    /// A load's transitions reach its track on the queue's owner: whatever the
    /// load did meanwhile, the track keeps the status the owner left it with
    /// until the executor holding the queue drains it.
    #[kithara::test(tokio)]
    async fn a_load_reaches_its_track_only_when_the_holder_drains_the_queue() {
        let mut queue = make_queue();
        let (woke_tx, mut woke_rx) = unbounded_channel();
        Player::hold(&mut queue, Waker::from(Arc::new(WakesTask(woke_tx))));
        let id = queue
            .append("/kithara/missing-track.wav")
            .expect("an open queue appends");
        let status =
            |queue: &Queue<TestPools>| queue.track(id).expect("the track stays queued").status;

        loop {
            let left = status(&queue);
            timeout(Duration::from_secs(5), woke_rx.recv())
                .await
                .expect("the load reports to the holder");
            assert_eq!(
                status(&queue),
                left,
                "a load's report waits for the holder to drain the queue"
            );
            Player::drain(&mut queue);
            if matches!(status(&queue), TrackStatus::Failed(_)) {
                break;
            }
        }
    }

    #[kithara::test]
    fn a_closed_queue_rejects_mutation() {
        let mut queue = make_queue();

        Player::close(&mut queue).expect("unstarted fixture must close");

        assert!(queue.shutdown.is_cancelled());
        assert!(matches!(
            queue.append("https://example.com/a.mp3"),
            Err(crate::QueueError::Play(PlayError::Closed))
        ));
        assert!(queue.is_empty());
    }

    #[kithara::test]
    fn retained_config_follows_live_queue_controls() {
        let mut queue = make_queue();
        queue.set_action_at_item_end(ActionAtItemEnd::Pause);
        queue.set_playback_order(PlaybackOrder::Shuffle);
        let mut crossfade = queue.crossfade_settings();
        crossfade.duration = 2.0;
        queue
            .set_crossfade_settings(crossfade)
            .expect("valid crossfade settings");

        let values = queue.config.values();
        assert_eq!(values.action_at_item_end, ActionAtItemEnd::Pause);
        assert_eq!(values.playback_order, PlaybackOrder::Shuffle);
        assert_eq!(values.crossfade_settings, crossfade);
        assert_eq!(queue.player.crossfade_duration(), crossfade.duration);
    }

    #[kithara::test]
    fn cached_position_unknown_after_construction() {
        let queue = make_queue();
        assert_eq!(Option::<f64>::from(queue.read_cached_position()), None);
    }

    #[kithara::test]
    fn cached_position_round_trips_through_queue() {
        let queue = make_queue();
        queue.write_cached_position(CachedPosition::known(12.5));
        assert_eq!(
            Option::<f64>::from(queue.read_cached_position()),
            Some(12.5)
        );
    }

    #[kithara::test]
    fn select_phase_idle_after_construction() {
        let queue = make_queue();
        assert!(matches!(queue.pending_select, SelectPhase::Idle));
    }
}
