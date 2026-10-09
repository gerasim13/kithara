use kithara_bufpool::HasPool;
use kithara_command::mailbox;
use kithara_events::{EventBus, TrackId};
use kithara_platform::{CancelScope, CancelToken, tokio::runtime::Handle as RuntimeHandle};
use kithara_play::{DeckSnapshot, PlayError, PlayerFactory, Position, TrackFactory};
use kithara_signal::{FrameCount, SessionFrame};

use super::{
    command::{QueueMailbox, QueuePostbox},
    slots::{Role, Slots},
    types::Target,
    view::QueueView,
};
use crate::{
    QueueConfig, QueueEvent, loader::Loader, navigation::NavigationState, track::Tracks,
};

/// Cloneable command capability and the queue's published state.
/// Commands answer after the owner accepts and sends them; executor effects
/// enter the published state on their receipts.
#[derive_where::derive_where(Clone)]
pub struct QueueControl<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) postbox: QueuePostbox<S>,
    pub(super) view: QueueView<S>,
    pub(super) bus: EventBus,
}

/// A deck whose track list, navigation and active tracks have one owner.
pub struct Queue<S, F = PlayerFactory>
where
    S: HasPool<u8> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    pub(super) config: QueueConfig<S, F>,
    pub(super) tracks: Tracks<S>,
    pub(super) navigation: NavigationState,
    pub(super) current: Option<TrackId>,
    pub(super) held_position: Option<Position>,
    pub(super) target: Option<Target>,
    pub(super) active: Slots<F::Track>,
    pub(super) postbox: QueuePostbox<S>,
    pub(super) mailbox: QueueMailbox<S>,
    pub(super) view: QueueView<S>,
    pub(super) bus: EventBus,
    pub(super) loader: Option<Loader<S>>,
    pub(super) shutdown: CancelToken,
    pub(super) events: Vec<QueueEvent>,
    pub(super) clock: Option<(SessionFrame, FrameCount)>,
    pub(super) deck: DeckSnapshot,
}

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    /// Builds the owner; its host checks registration before driving it.
    #[must_use]
    pub fn new(mut config: QueueConfig<S, F>) -> Self {
        let shutdown = CancelScope::new(config.cancel.clone()).token();
        if let Some(prep) = &mut config.prep {
            prep.cancel = Some(shutdown.clone());
        }
        let bus = config
            .prep
            .as_ref()
            .map_or_else(EventBus::default, |prep| prep.bus.clone());
        let (postbox, mailbox) = mailbox();
        let loader = match (&config.prep, &config.store) {
            (Some(prep), Some(store)) => Some(Loader::new(
                prep.clone(),
                store.clone(),
                config
                    .runtime
                    .clone()
                    .or_else(|| RuntimeHandle::try_current().ok()),
                postbox.clone(),
            )),
            _ => None,
        };
        let mut navigation = NavigationState::new(config.max_history_size);
        navigation.set_playback_order(config.playback_order, &[]);
        let tracks = Tracks::default();
        let view = QueueView::new(
            &tracks,
            &navigation,
            config.settings,
            config.track,
            config.action_at_item_end,
        );
        Self {
            active: Slots::new(config.mixer.slots().get()),
            config,
            tracks,
            navigation,
            current: None,
            held_position: None,
            target: None,
            postbox,
            mailbox,
            view,
            bus,
            loader,
            shutdown,
            events: Vec::new(),
            clock: None,
            deck: DeckSnapshot::default(),
        }
    }

    /// The handle reaches this owner's mailbox, never a track implementation.
    #[must_use]
    pub fn control(&self) -> QueueControl<S> {
        QueueControl {
            postbox: self.postbox.clone(),
            view: self.view.clone(),
            bus: self.bus.clone(),
        }
    }

    /// Every active track, including preloaded and outgoing tracks.
    pub fn tracks_mut(&mut self) -> impl Iterator<Item = &mut F::Track> {
        self.active.iter_mut().map(|active| &mut active.track)
    }

    /// Every active track, including both sides of an unfinished transition.
    pub fn tracks_active(&self) -> impl Iterator<Item = &F::Track> {
        self.active.iter().map(|active| &active.track)
    }

    /// The factory a track decorator configures for subsequent loads.
    pub fn factory_mut(&mut self) -> &mut F {
        &mut self.config.factory
    }

    /// The factory whose configuration subsequent tracks inherit.
    #[must_use]
    pub fn factory(&self) -> &F {
        &self.config.factory
    }

    /// The sounding track, chosen only when its transition applies.
    #[must_use]
    pub fn current_track(&self) -> Option<&F::Track> {
        self.active
            .iter()
            .find(|active| active.role == Role::Current)
            .map(|active| &active.track)
    }

    pub(super) fn active_current_index(&self) -> Option<usize> {
        self.active.position(|active| active.role == Role::Current)
    }

    pub(super) fn earliest(&self) -> Result<SessionFrame, PlayError> {
        self.clock
            .map(|(now, delivery)| now + delivery)
            .ok_or(PlayError::Untimed)
    }

    pub(super) fn ensure_open(&self) -> Result<(), PlayError> {
        if self.shutdown.is_cancelled() {
            Err(PlayError::Closed)
        } else {
            Ok(())
        }
    }

    /// Records the tracks' earlier changes before the owner's next event.
    pub(super) fn announce(&mut self, event: QueueEvent) {
        self.events.extend(self.tracks.drain_events());
        self.events.push(event);
    }

    /// Publishes before announcing, so an event's reader sees its state.
    pub(super) fn publish(&mut self) {
        self.events.extend(self.tracks.drain_events());
        let snapshot = self.queue_snapshot();
        self.view.publish(snapshot);
        for event in self.events.drain(..) {
            self.bus.publish(event);
        }
    }

    pub(super) fn track_ids(&self) -> Vec<TrackId> {
        self.tracks
            .records()
            .iter()
            .map(|record| record.id)
            .collect()
    }
}

impl<S, F> Drop for Queue<S, F>
where
    S: HasPool<u8> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.tracks.cancel_loads();
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
        mock::SessionMock,
        player::{Player, PlayerControlSource},
    };
    use kithara_test_utils::kithara;
    use kithara_warp::BeatGridId;

    use super::*;
    use crate::{
        consts,
        event::{QueueEvent, QueueRepeatMode, TrackStatus},
        navigation::{ActionAtItemEnd, PlaybackOrder, RepeatMode},
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

    /// A queue the mock session holds, seated the way a Host's insert seats it.
    /// A queue its Host has seated on a deck slot, with the mock that answers
    /// as the slot's audio thread.
    pub(in crate::queue) fn make_queue() -> (Queue<TestPools>, Arc<SessionMock>) {
        let mut queue = Queue::new(queue_config());
        let audio_thread = kithara_play::mock::insert(&mut queue);
        (queue, audio_thread)
    }

    pub(crate) fn test_session() -> SessionBinding {
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
        let (_queue, _audio_thread) = make_queue();
    }

    #[kithara::test]
    fn a_control_reads_closed_once_its_queue_is_dropped() {
        let (queue, _audio_thread) = make_queue();
        let control = queue.control();
        assert!(!control.is_closed(), "the queue still owns its mailbox");

        drop(queue);

        assert!(control.is_closed());
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
        assert_eq!(deck.grid_id, grid_id);
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
        let (mut queue, _audio_thread) = make_queue();
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
        let (mut queue, _audio_thread) = make_queue();
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
        let (mut queue, _audio_thread) = make_queue();

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
        let (mut queue, _audio_thread) = make_queue();
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

    /// A queue event is heard only with the view that shows it, in the order
    /// the queue made its changes: a handle that hears a track change reads
    /// that change.
    #[kithara::test]
    fn a_queue_event_is_heard_with_the_view_that_shows_it() {
        let (mut queue, _audio_thread) = make_queue();
        let control = queue.control();
        let id = queue
            .append("https://example.com/a.mp3")
            .expect("an open queue appends");
        let mut events = queue.subscribe::<QueueEvent>();

        queue.tracks.set_status(id, TrackStatus::Consumed);
        assert!(
            events.try_recv().is_err(),
            "a change is not heard before the queue publishes it"
        );
        queue.set_repeat(RepeatMode::All);

        let heard: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
            .map(|envelope| envelope.event)
            .collect();
        assert!(
            matches!(
                heard.as_slice(),
                [
                    QueueEvent::TrackStatusChanged {
                        id: changed,
                        status: TrackStatus::Consumed,
                    },
                    QueueEvent::RepeatModeChanged {
                        mode: QueueRepeatMode::All,
                    },
                ] if *changed == id
            ),
            "the publish announces both changes in order: {heard:?}"
        );
        assert_eq!(
            control.track(id).map(|track| track.status),
            Some(TrackStatus::Consumed)
        );
        assert_eq!(control.repeat_mode(), RepeatMode::All);
    }

    #[kithara::test]
    fn cached_position_unknown_after_construction() {
        let (queue, _audio_thread) = make_queue();
        assert_eq!(queue.position_seconds(), None);
        assert_eq!(queue.control().position_seconds(), None);
    }

    #[kithara::test]
    fn select_phase_idle_after_construction() {
        let (queue, _audio_thread) = make_queue();
        assert!(matches!(queue.pending_select, SelectPhase::Idle));
    }
}
