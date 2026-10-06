use std::{error::Error as StdError, io::Error, num::NonZeroUsize};

use kithara_assets::AssetStore;
use kithara_audio::AudioObserverSlot;
use kithara_bufpool::HasPool;
use kithara_command::Postbox;
use kithara_download::DownloaderEvent;
use kithara_events::{Envelope, EventBus, RecvError, ScopeLabel, TrackId};
use kithara_net::NetError;
use kithara_platform::{
    CancelGroup, CancelToken,
    sync::Arc,
    time::Duration,
    tokio,
    tokio::{runtime::Handle as RuntimeHandle, sync::Semaphore, task::spawn_on},
};
use kithara_play::{
    ArtifactLoadError, Cover, Resource, ResourceConfig, ResourceSrc, player::PlayerControl,
};
use kithara_test_utils::kithara;
use tracing::{debug, warn};

use crate::{
    attempts::{AttemptReport, LoadClass, Ticket},
    error::QueueError,
    event::TrackStatus,
    queue::QueueCommand,
    track::{TrackSource, Tracks},
};

/// Async track loader: `ResourceConfig` -> `Resource`, run in two
/// isolated permit lanes with one abortable attempt per track. The queue
/// starts each attempt; the attempt's task posts what happens to it back to
/// the queue, which applies it.
pub(crate) struct Loader<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// User-selection lane: one dedicated permit, isolated from prefetch.
    interactive_lane: Arc<Semaphore>,
    /// Background prefetch lane (`max_concurrent_loads` permits).
    prefetch_lane: Arc<Semaphore>,
    /// Where an attempt's task reports to the queue.
    postbox: Postbox<QueueCommand<S>>,
    store: AssetStore<S>,
    cancel: CancelToken,
    runtime: Option<RuntimeHandle>,
    player: PlayerControl<S>,
}

impl<S> Loader<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Repeated asks with nothing to show for them: the downloader's own budget.
    const HANG_TIMEOUT: Duration = Duration::from_secs(60);

    pub(crate) fn new(
        player: PlayerControl<S>,
        runtime: Option<RuntimeHandle>,
        store: AssetStore<S>,
        max_concurrent_loads: NonZeroUsize,
        postbox: Postbox<QueueCommand<S>>,
        cancel: CancelToken,
    ) -> Self {
        Self {
            cancel,
            player,
            runtime,
            postbox,
            store,
            interactive_lane: Arc::new(Semaphore::new(1)),
            prefetch_lane: Arc::new(Semaphore::new(max_concurrent_loads.get())),
        }
    }

    /// What a load attempt needs before it begins: its config, its
    /// per-track cancel, and the runtime it runs on.
    fn attempt_config(
        &self,
        id: TrackId,
        source: TrackSource<S>,
    ) -> Result<(ResourceConfig<S>, CancelToken, RuntimeHandle), QueueError> {
        let config = self.build_config(id, source)?;
        let Some(cancel) = config.cancel().cloned() else {
            return Err(QueueError::Resource(format!(
                "track {id:?}: resource config missing per-track cancel"
            )));
        };
        Ok((config, cancel, self.runtime()?.clone()))
    }

    /// Build a [`ResourceConfig`] for the given [`TrackSource`].
    ///
    /// - [`TrackSource::Uri`] uses the queue store and player pools; other
    ///   resource options keep their defaults. Callers wanting custom
    ///   behavior build a configured [`ResourceConfig`] and pass it via
    ///   [`TrackSource::Config`].
    /// - [`TrackSource::Config`] is passed through untouched (DRM keys,
    ///   headers, format hints preserved).
    ///
    /// Both paths finish with `PlayerImpl::prepare_config` so worker /
    /// sample-rate / runtime / default bus are injected.
    pub(crate) fn build_config(
        &self,
        id: TrackId,
        source: TrackSource<S>,
    ) -> Result<ResourceConfig<S>, QueueError> {
        let mut config = match source {
            TrackSource::Uri(url) => {
                let src = ResourceSrc::parse(&url)
                    .map_err(|e| QueueError::InvalidUrl(format!("{url}: {e}")))?;
                ResourceConfig::for_src(src)
                    .store(self.store.clone())
                    .build()
            }
            TrackSource::Config(boxed) => *boxed,
        };
        if config.bus().is_none() {
            config.set_bus(self.player.bus().scoped_labeled(ScopeLabel {
                track: Some(id),
                ..ScopeLabel::default()
            }));
        }
        self.player.prepare_config(config).map_err(QueueError::from)
    }

    /// Load a [`Resource`] from a prepared config, attaching the observer
    /// left in the track's `observer` slot when there is one. Caller is responsible
    /// for admitting it into the track list and emitting [`TrackStatus::Loaded`].
    ///
    /// A load that failed on something the network can answer later is not a
    /// verdict on the track: the attempt reports the failure and asks again,
    /// so a track chosen during an outage plays when connectivity returns
    /// instead of waiting to be chosen a second time. An HLS segment already
    /// gets exactly this — a transient failure returns its slot to the pool
    /// and the next dispatch asks again. Whether anyone waits for the track
    /// is the queue's call, not the attempt's: the queue owns the selection.
    ///
    /// Nothing here polls for the network's state: each ask spends the
    /// downloader's own retry budget before returning, which is what paces the
    /// repeat, and the per-track cancel ends it the moment the selection moves
    /// on, or the moment the queue answers that nobody selected the attempt,
    /// so it never holds its lane permit against a network that is not
    /// answering.
    #[kithara::hang_watchdog(timeout = Self::HANG_TIMEOUT)]
    async fn load(
        &self,
        ticket: Ticket,
        config: ResourceConfig<S>,
        observer: &AudioObserverSlot,
    ) -> Result<Resource, QueueError> {
        let id = ticket.id;
        let slow_watcher =
            Self::watch_for_slow_status(ticket, config.bus().cloned(), self.postbox.clone());
        tokio::pin!(slow_watcher);
        loop {
            let relay = observer.relay();
            let attempt = async { Resource::new_observed(config.clone(), Box::new(relay)).await };
            let result = tokio::select! {
                biased;
                result = attempt => result,
                never = &mut slow_watcher => match never {},
            };
            let err = match result {
                Ok(resource) => return Ok(resource),
                Err(err) => err,
            };
            let error = QueueError::Resource(format!("{err}"));
            if !can_answer_later(&err) {
                return Err(error);
            }
            Self::report(&self.postbox, AttemptReport::Stalled { ticket, error });
            hang_tick!();
            debug!(?id, error = %err, "load failed on a cause a later ask can answer; asking again");
        }
    }

    /// Move a track's pending load into the interactive lane.
    pub(crate) fn promote_load(
        self: &Arc<Self>,
        tracks: &mut Tracks<S>,
        id: TrackId,
        source: TrackSource<S>,
    ) {
        let (config, cancel, runtime) = match self.attempt_config(id, source) {
            Ok(attempt) => attempt,
            Err(err) => {
                tracks.set_status(id, TrackStatus::Failed(err.to_string()));
                return;
            }
        };
        if let Some(ticket) = tracks.promote_attempt(id, cancel.clone()) {
            let attempt = AttemptTask {
                ticket,
                config,
                cancel,
                observer: tracks.observer_slot(id),
            };
            self.spawn_attempt(&runtime, attempt, LoadClass::Interactive);
        }
    }

    /// Post `report` to the queue. A queue that is gone has no track left to
    /// report on.
    fn report(postbox: &Postbox<QueueCommand<S>>, report: AttemptReport) {
        if postbox.post(QueueCommand::Attempt(report)).is_err() {
            debug!("the queue is gone: dropping a load attempt's report");
        }
    }

    /// Read the track's cover beside its audio, over the attempt's transport
    /// and cancel token, and report it to the queue, which places it while
    /// that attempt's token lives. The audio never waits for the cover, and a
    /// cover that never arrives leaves the load untouched.
    fn read_cover(
        &self,
        runtime: &RuntimeHandle,
        id: TrackId,
        config: &ResourceConfig<S>,
        attempt: &CancelToken,
    ) {
        let Some(cover) = config.artwork().cloned() else {
            return;
        };
        let config = config.clone();
        let attempt = attempt.clone();
        let postbox = self.postbox.clone();
        drop(spawn_on(runtime, async move {
            match config.artifact_fetch().load::<Cover>(&cover).await {
                Ok(cover) => Self::report(
                    &postbox,
                    AttemptReport::Cover {
                        id,
                        attempt,
                        cover: Arc::new(cover.into()),
                    },
                ),
                Err(ArtifactLoadError::Cancelled { .. }) => {}
                Err(error) => warn!(?id, %error, "the track's cover never arrived"),
            }
        }));
    }

    /// Run one load attempt: wait for a permit in `class`'s lane, report the
    /// start, and load. A cancel before the permit ends it without loading.
    async fn run_attempt(
        &self,
        attempt: AttemptTask<S>,
        class: LoadClass,
    ) -> Result<Resource, QueueError> {
        let AttemptTask {
            ticket,
            config,
            cancel: track_cancel,
            observer,
        } = attempt;
        let track_cancel = &track_cancel;
        let id = ticket.id;
        let cancel = CancelGroup::new(vec![track_cancel.clone(), self.cancel.clone()]);
        let lane = match class {
            LoadClass::Interactive => &self.interactive_lane,
            LoadClass::Prefetch => &self.prefetch_lane,
        };
        kithara::probe_event!(admission_started, track_id = id.as_u64());
        let permit = tokio::select! {
            biased;
            _ = Self::wait_and_cancel_track(&cancel, track_cancel) => {
                return Err(QueueError::Cancelled(id));
            }
            permit = Arc::clone(lane).acquire_owned() => permit
                .map_err(|e| QueueError::Resource(format!("semaphore closed: {e}")))?,
        };
        Self::report(&self.postbox, AttemptReport::Started(ticket));

        let result = tokio::select! {
            biased;
            _ = Self::wait_and_cancel_track(&cancel, track_cancel) =>
                Err(QueueError::Cancelled(id)),
            result = self.load(ticket, config, &observer) => result,
        };
        drop(permit);
        result
    }

    fn spawn_attempt(
        self: &Arc<Self>,
        runtime: &RuntimeHandle,
        attempt: AttemptTask<S>,
        class: LoadClass,
    ) {
        let ticket = attempt.ticket;
        self.read_cover(runtime, ticket.id, &attempt.config, &attempt.cancel);
        let this = Arc::clone(self);
        let finish = Finish {
            ticket,
            postbox: self.postbox.clone(),
            outcome: None,
        };
        drop(spawn_on(runtime, async move {
            finish.settle(this.run_attempt(attempt, class).await.map(Box::new));
        }));
    }

    /// The runtime the queue was built with. The queue never borrows the
    /// calling thread's: a Host ticks it from a thread that has none.
    fn runtime(&self) -> Result<&RuntimeHandle, QueueError> {
        self.runtime.as_ref().ok_or(QueueError::NoRuntime)
    }

    /// Spawn a fresh async load in the given lane, unless a live attempt
    /// already exists - one track never occupies two permits.
    pub(crate) fn spawn_load(
        self: &Arc<Self>,
        tracks: &mut Tracks<S>,
        id: TrackId,
        source: TrackSource<S>,
        class: LoadClass,
    ) {
        let (config, cancel, runtime) = match self.attempt_config(id, source) {
            Ok(attempt) => attempt,
            Err(err) => {
                tracks.set_status(id, TrackStatus::Failed(err.to_string()));
                return;
            }
        };
        if let Some(ticket) =
            tracks.begin_attempt(id, cancel.clone(), class == LoadClass::Interactive)
        {
            let attempt = AttemptTask {
                ticket,
                config,
                cancel,
                observer: tracks.observer_slot(id),
            };
            self.spawn_attempt(&runtime, attempt, class);
        }
    }

    async fn wait_and_cancel_track(cancel: &CancelGroup, track_cancel: &CancelToken) {
        cancel.cancelled().await;
        track_cancel.cancel();
    }

    /// Watches the [`EventBus`] for the first
    /// [`DownloaderEvent::LoadSlow`] and reports it to the queue, which
    /// flips a track the attempt still loads to [`TrackStatus::Slow`].
    /// Returns a never-completing future:
    /// the caller `select!`s it against `Resource::new`, so the
    /// completion side always belongs to the resource future.
    /// A `Lagged` bus dropped the oldest envelopes and keeps
    /// delivering, so the watch survives the gap and only `Closed`
    /// ends it.
    async fn watch_for_slow_status(
        ticket: Ticket,
        bus: Option<EventBus>,
        postbox: Postbox<QueueCommand<S>>,
    ) -> std::convert::Infallible {
        let mut rx = match bus {
            Some(b) => b.subscribe::<DownloaderEvent>(),
            None => return std::future::pending().await,
        };
        let mut marked = false;
        loop {
            match rx.recv().await {
                Ok(Envelope { event: ev, .. }) => {
                    if !marked && matches!(ev, DownloaderEvent::LoadSlow { .. }) {
                        Self::report(&postbox, AttemptReport::Slow(ticket));
                        marked = true;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
        std::future::pending().await
    }
}

/// One load attempt as its task runs it: the ticket it reports under, the
/// config it loads, its per-track cancel, and the slot that reaches the
/// track's decoder.
struct AttemptTask<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    ticket: Ticket,
    config: ResourceConfig<S>,
    cancel: CancelToken,
    observer: AudioObserverSlot,
}

/// Reports its attempt's `Finished` to the queue however the attempt's task
/// ends: with the outcome the attempt returned, or with a failure when a
/// panic or a runtime going away dropped the task first, so the queue never
/// waits on an attempt nothing runs.
struct Finish<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    ticket: Ticket,
    postbox: Postbox<QueueCommand<S>>,
    /// `None` until the attempt returns.
    outcome: Option<Result<Box<Resource>, QueueError>>,
}

impl<S> Finish<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// The attempt returned `outcome`: report it.
    fn settle(mut self, outcome: Result<Box<Resource>, QueueError>) {
        self.outcome = Some(outcome);
    }
}

impl<S> Drop for Finish<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        let ticket = self.ticket;
        let outcome = self.outcome.take().unwrap_or_else(|| {
            Err(QueueError::Resource(format!(
                "track {:?}: its load ended without an outcome",
                ticket.id
            )))
        });
        Loader::report(&self.postbox, AttemptReport::Finished { ticket, outcome });
    }
}

/// Whether a failed load is worth asking for again as it stands.
///
/// That is [`NetError::can_answer_later`]'s question — the same one an HLS segment
/// slot asks about its own re-dispatch. It is read off the typed `NetError`
/// the load carries down its source chain: never a message match, and never a
/// verdict read back off the bus, which another task publishes and so is not
/// there yet when the load returns. A failure with no network cause at all
/// (an unparseable container, a codec the build does not carry) is never
/// asked again — connectivity does not change that answer — and neither is a
/// transfer that stopped delivering, which is the verdict
/// `stalled_master_playlist_fails_load` pins.
fn can_answer_later(error: &(dyn StdError + 'static)) -> bool {
    net_cause(error).is_some_and(NetError::can_answer_later)
}

/// The network failure behind a load error, if the load failed on the network at
/// all.
///
/// [`io::Error`] hides its payload from [`StdError::source`] — it reports the
/// payload's *own* source instead — so a plain chain walk steps straight over a
/// wrapped `NetError`. This looks inside one explicitly.
fn net_cause<'e>(error: &'e (dyn StdError + 'static)) -> Option<&'e NetError> {
    let mut current = Some(error);
    while let Some(err) = current {
        if let Some(net) = err.downcast_ref::<NetError>() {
            return Some(net);
        }
        if let Some(net) = err
            .downcast_ref::<Error>()
            .and_then(Error::get_ref)
            .and_then(|payload| net_cause(payload))
        {
            return Some(net);
        }
        current = err.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        future::{self, Future},
        num::{NonZeroU16, NonZeroU32, NonZeroU64},
        pin::pin,
        task::{Context, Wake, Waker},
    };

    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_command::{Mailbox, mailbox};
    use kithara_download::RequestId;
    use kithara_events::EventBus;
    use kithara_platform::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{self, Duration},
        tokio::{
            sync::{
                mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
                oneshot,
            },
            task::spawn,
        },
    };
    use kithara_play::{
        ArtifactSource, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl, StreamShape, mock,
        player::PlayerControlSource,
    };
    use kithara_test_utils::{TestTempDir, cancel_token, kithara, temp_dir};
    use kithara_warp::WarpConfig;
    use kithara_waveform::Waveform;

    use super::*;
    use crate::{
        consts,
        event::QueueEvent,
        test_pools::{TestPools, pools},
        track::TrackRecord,
    };

    struct CancelDropProbe {
        state: Arc<AtomicUsize>,
        cancel: CancelToken,
    }

    impl Drop for CancelDropProbe {
        fn drop(&mut self) {
            self.state
                .store(usize::from(self.cancel.is_cancelled()), Ordering::SeqCst);
        }
    }

    /// A spent budget over a refusal keeps the resource askable: the load is
    /// repeated while the selection wants it, which is how a track chosen during
    /// an outage starts once connectivity returns.
    #[kithara::test]
    fn a_refused_host_can_answer_later() {
        let refused = NetError::RetryExhausted {
            max_retries: 3,
            source: Box::new(NetError::Status {
                status: NonZeroU16::new(503).expect("503 is not zero"),
                url: None,
                body: Some("network offline".to_string()),
            }),
        };
        // `io::Error` hides its payload from the source chain; the classifier looks inside.
        assert!(can_answer_later(&Error::other(refused)));
    }

    /// A vanished transport is the same answer: nothing was reached, so the whole
    /// load is worth asking for again.
    #[kithara::test]
    fn a_vanished_host_can_answer_later() {
        let gone = NetError::Network("connection closed".to_string());
        assert!(can_answer_later(&Error::other(gone)));
    }

    /// A transfer that established and then stopped delivering is the net layer's
    /// own verdict: repeating it would spin instead of telling the user, the
    /// contract `stalled_master_playlist_fails_load` pins.
    #[kithara::test]
    fn a_stalled_transfer_is_not_asked_again() {
        let stalled = NetError::RetryExhausted {
            max_retries: 1,
            source: Box::new(NetError::Timeout),
        };
        assert!(!can_answer_later(&Error::other(stalled)));
    }

    /// A missing resource answers the same however long one waits.
    #[kithara::test]
    fn a_missing_resource_is_not_asked_again() {
        let missing = NetError::Status {
            status: NonZeroU16::new(404).expect("404 is not zero"),
            url: None,
            body: None,
        };
        assert!(!can_answer_later(&Error::other(missing)));
    }

    /// A failure the network had no part in — an unparseable container, a codec
    /// the build does not carry — is not a connectivity question.
    #[kithara::test]
    fn a_failure_with_no_network_cause_is_not_asked_again() {
        let local = Error::other("unsupported container");
        assert!(!can_answer_later(&local));
    }

    /// Builder for test [`Loader`] fixtures. Defaults cover most tests;
    /// override via setters when a specific concurrency cap matters.
    #[derive(fieldwork::Fieldwork)]
    #[fieldwork(with, vis = "")]
    struct LoaderFixtureSpec {
        cap: NonZeroUsize,
        /// What the loader runs its attempts on.
        runtime: Option<RuntimeHandle>,
    }

    impl Default for LoaderFixtureSpec {
        fn default() -> Self {
            const CAP_3: NonZeroUsize = match NonZeroUsize::new(3) {
                Some(n) => n,
                None => unreachable!(),
            };
            Self {
                cap: CAP_3,
                runtime: RuntimeHandle::try_current().ok(),
            }
        }
    }

    /// A runtime that goes away drops the tasks it never ran to the end.
    /// An attempt's task still reports how it ended, so the queue never
    /// waits on an attempt nothing runs any more.
    #[kithara::test]
    fn an_attempt_its_runtime_dropped_fails_its_track() {
        let attempts = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime for the attempt");
        let mut fixture = LoaderFixtureSpec::default()
            .with_runtime(Some(attempts.handle().clone()))
            .build();
        let id = TrackId::allocate();
        let source = TrackSource::Uri("https://example.com/abandoned.mp3".into());
        fixture
            .tracks
            .records_mut()
            .push(TrackRecord::new(id, "abandoned".into(), source.clone()));
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, source, LoadClass::Prefetch);

        drop(attempts);
        for report in posted(&mut fixture.mailbox) {
            fixture.tracks.apply_report(report);
        }

        assert!(
            matches!(fixture.tracks.records()[0].status, TrackStatus::Failed(_)),
            "the track waits on an attempt nothing runs: {:?}",
            fixture.tracks.records()[0].status
        );
    }

    #[kithara::test(tokio)]
    async fn cancellation_precedes_in_flight_future_drop() {
        let owner = CancelToken::root();
        let queue_cancel = owner.child();
        let track_cancel = owner.child();
        let group = CancelGroup::new(vec![queue_cancel.clone(), track_cancel.clone()]);
        let state = Arc::new(AtomicUsize::new(0));
        let probe_state = Arc::clone(&state);
        let probe_cancel = track_cancel.clone();
        let (started_tx, started_rx) = oneshot::channel();
        let in_flight = async move {
            let _probe = CancelDropProbe {
                cancel: probe_cancel,
                state: probe_state,
            };
            let _ = started_tx.send(());
            future::pending::<()>().await;
        };
        let canceller = spawn(async move {
            started_rx.await.expect("in-flight future must start");
            queue_cancel.cancel();
        });

        tokio::select! {
            biased;
            _ = Loader::<TestPools>::wait_and_cancel_track(&group, &track_cancel) => {}
            () = in_flight => panic!("in-flight future must stay pending"),
        }
        canceller.await.expect("canceller task must not panic");

        assert_eq!(state.load(Ordering::SeqCst), 1);
    }

    #[kithara::test(tokio)]
    async fn cancellation_wakes_an_attempt_waiting_for_admission() {
        let mut fixture = LoaderFixtureSpec::default()
            .with_cap(NonZeroUsize::MIN)
            .build();
        let permit = Arc::clone(&fixture.loader.prefetch_lane)
            .acquire_owned()
            .await
            .expect("loader keeps the prefetch semaphore open");
        let id = TrackId::allocate();
        let source = TrackSource::Uri("https://example.com/pending.mp3".into());
        fixture
            .tracks
            .records_mut()
            .push(TrackRecord::new(id, "pending".into(), source.clone()));
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, source, LoadClass::Prefetch);
        assert!(fixture.tracks.records().iter().any(|track| {
            track.id == id && track.load.as_ref().is_some_and(|attempt| attempt.waiting)
        }));

        fixture.loader.cancel.cancel();

        assert!(matches!(
            fixture.report().await,
            AttemptReport::Finished {
                outcome: Err(QueueError::Cancelled(cancelled)),
                ..
            } if cancelled == id
        ));
        drop(permit);
    }

    /// The bus drops the oldest envelopes under a burst and keeps
    /// delivering, so the slow watch has to survive the gap. The burst
    /// below is longer than the bus capacity with nothing reading, which
    /// makes the drop certain, and the `LoadSlow` behind it still has to
    /// reach the watch.
    #[kithara::test(native)]
    fn a_slow_watch_survives_a_bus_that_dropped_a_burst(cancel_token: CancelToken) {
        const CAPACITY: usize = 4;

        let bus = EventBus::new(CAPACITY);
        let mut tracks = Tracks::<TestPools>::default();
        let id = TrackId::allocate();
        tracks.records_mut().push(TrackRecord::new(
            id,
            "slow".into(),
            TrackSource::Uri("https://example.com/slow.mp3".into()),
        ));
        let ticket = tracks
            .begin_attempt(id, cancel_token, false)
            .expect("a fresh track starts one load attempt");
        let (postbox, mut mailbox) = mailbox();

        let mut watch = pin!(Loader::watch_for_slow_status(
            ticket,
            Some(bus.clone()),
            postbox
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            watch.as_mut().poll(&mut cx).is_pending(),
            "the watch must subscribe before the burst it has to survive"
        );

        let request_id = RequestId::new(NonZeroU64::MIN);
        for _ in 0..=CAPACITY {
            bus.publish(DownloaderEvent::RequestStarted {
                request_id,
                wait_in_queue: Duration::ZERO,
            });
        }
        bus.publish(DownloaderEvent::LoadSlow {
            request_id,
            elapsed: Duration::ZERO,
        });

        assert!(
            watch.as_mut().poll(&mut cx).is_pending(),
            "the watch never completes: it ends only with the resource it races"
        );
        for report in posted(&mut mailbox) {
            tracks.apply_report(report);
        }
        assert_eq!(
            tracks.records()[0].status,
            TrackStatus::Slow,
            "a dropped burst must not deafen the watch to the `LoadSlow` behind it"
        );
    }

    /// A slow transfer is news only while its attempt still loads the track.
    /// Once the selection moved on and cancelled that attempt, a `LoadSlow`
    /// its download reports afterwards leaves the track `Cancelled`.
    #[kithara::test(native)]
    fn a_slow_transfer_after_its_attempt_was_cancelled_leaves_the_track_cancelled(
        cancel_token: CancelToken,
    ) {
        let bus = EventBus::new(4);
        let mut tracks = Tracks::<TestPools>::default();
        let id = TrackId::allocate();
        tracks.records_mut().push(TrackRecord::new(
            id,
            "slow".into(),
            TrackSource::Uri("https://example.com/slow.mp3".into()),
        ));
        let ticket = tracks
            .begin_attempt(id, cancel_token, false)
            .expect("a fresh track starts one load attempt");
        let (postbox, mut mailbox) = mailbox();

        let mut watch = pin!(Loader::watch_for_slow_status(
            ticket,
            Some(bus.clone()),
            postbox
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            watch.as_mut().poll(&mut cx).is_pending(),
            "the watch must subscribe before the attempt is cancelled"
        );

        tracks.set_status(id, TrackStatus::Cancelled);
        bus.publish(DownloaderEvent::LoadSlow {
            request_id: RequestId::new(NonZeroU64::MIN),
            elapsed: Duration::ZERO,
        });
        assert!(watch.as_mut().poll(&mut cx).is_pending());
        for report in posted(&mut mailbox) {
            tracks.apply_report(report);
        }

        assert_eq!(
            tracks.records()[0].status,
            TrackStatus::Cancelled,
            "a cancelled attempt's slow transfer must not revive its track"
        );
    }

    /// The reports a loader posted since the last drain, in post order.
    fn posted(
        mailbox: &mut Mailbox<QueueCommand<TestPools>>,
    ) -> impl Iterator<Item = AttemptReport> + use<> {
        mailbox.drain().map(|command| {
            let QueueCommand::Attempt(report) = command else {
                panic!("a loader posts only its attempts' reports");
            };
            report
        })
    }

    /// A holder's waker that reports each wake to an async waiter.
    struct Wakes(UnboundedSender<()>);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    /// Test fixture: the [`Loader`] under test, the [`Tracks`] store it
    /// starts attempts on (so tests can seed entries), the root
    /// [`EventBus`] (so tests can subscribe for assertions), and the
    /// mailbox its attempts report to, held as a queue would hold it.
    struct LoaderFixture {
        loader: Arc<Loader<TestPools>>,
        tracks: Tracks<TestPools>,
        bus: EventBus,
        mailbox: Mailbox<QueueCommand<TestPools>>,
        woke: UnboundedReceiver<()>,
        reports: VecDeque<AttemptReport>,
        _player: PlayerImpl<TestPools>,
    }

    impl LoaderFixture {
        /// The next report the loader's attempts posted, in post order.
        async fn report(&mut self) -> AttemptReport {
            loop {
                if let Some(report) = self.reports.pop_front() {
                    return report;
                }
                self.reports.extend(posted(&mut self.mailbox));
                if self.reports.is_empty() {
                    time::timeout(Duration::from_secs(2), self.woke.recv())
                        .await
                        .expect("a load attempt reports")
                        .expect("the fixture holds the waker");
                }
            }
        }

        /// Apply the loader's reports as the queue would, through the next
        /// attempt's finish.
        async fn apply_through_finish(&mut self) {
            loop {
                let report = self.report().await;
                let finished = matches!(report, AttemptReport::Finished { .. });
                self.tracks.apply_report(report);
                if finished {
                    return;
                }
            }
        }
    }

    impl LoaderFixtureSpec {
        fn build(self) -> LoaderFixture {
            let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
            let player = PlayerImpl::new(
                PlayerConfig::builder()
                    .sample_rate(consts::TEST_SAMPLE_RATE)
                    .worker(worker)
                    .session(crate::queue::test_session())
                    .build(),
            );
            let bus = player.bus().clone();
            let tracks = Tracks::default();
            let store = AssetStore::builder(player.pools().clone()).build();
            let (postbox, mut mailbox) = mailbox();
            let (woke_tx, woke) = unbounded_channel();
            mailbox.hold(Waker::from(Arc::new(Wakes(woke_tx))));
            let loader = Arc::new(Loader::new(
                player.control(),
                self.runtime,
                store,
                self.cap,
                postbox,
                CancelToken::root(),
            ));
            LoaderFixture {
                loader,
                tracks,
                bus,
                mailbox,
                woke,
                reports: VecDeque::new(),
                _player: player,
            }
        }
    }

    /// A load attempt reads its track's cover beside the audio and places it
    /// while that attempt is current; a superseded attempt's cover never lands.
    #[kithara::test(native, tokio)]
    async fn only_the_current_attempt_places_its_cover(temp_dir: TestTempDir) {
        let mut fixture = LoaderFixtureSpec::default().build();
        let id = TrackId::allocate();
        let audio = ResourceSrc::Path(temp_dir.path().join("missing.mp3"));
        let covered = |name: &str, cover: &[u8]| -> TrackSource<TestPools> {
            ResourceConfig::for_src(audio.clone())
                .store(fixture.loader.store.clone())
                .artwork(ResourceSrc::Path(temp_dir.write(name, cover)))
                .build()
                .into()
        };
        let superseded = covered("superseded.jpg", b"superseded cover");
        let current = covered("current.jpg", b"current cover");
        fixture
            .tracks
            .records_mut()
            .push(TrackRecord::new(id, "covered".into(), current.clone()));

        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, superseded, LoadClass::Prefetch);
        fixture.tracks.set_status(id, TrackStatus::Cancelled);
        fixture.apply_through_finish().await;
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, current, LoadClass::Prefetch);
        fixture.apply_through_finish().await;
        assert!(
            matches!(fixture.tracks.records()[0].status, TrackStatus::Failed(_)),
            "the audio is missing"
        );

        while fixture.tracks.records()[0]
            .entry()
            .metadata()
            .artwork
            .is_none()
        {
            let report = fixture.report().await;
            fixture.tracks.apply_report(report);
        }
        let entry = fixture.tracks.records()[0].entry();
        assert_eq!(
            entry.metadata().artwork.as_deref().map(Vec::as_slice),
            Some(b"current cover".as_slice())
        );
        let mut changes = 0;
        for event in fixture.tracks.drain_events() {
            if let QueueEvent::TrackMetadataChanged { id: changed } = event {
                assert_eq!(changed, id);
                changes += 1;
            }
        }
        assert_eq!(changes, 1, "the superseded attempt's cover landed");
    }

    #[kithara::test(tokio)]
    async fn build_config_preserves_caller_supplied_config() {
        let fixture = LoaderFixtureSpec::default().build();
        let loader = &fixture.loader;
        let supplied_store = AssetStore::builder(pools())
            .backend(StorageBackend::Memory)
            .build();
        let Ok(src) = ResourceSrc::parse("https://example.com/a.mp3") else {
            panic!("valid url");
        };
        let given = ResourceConfig::for_src(src)
            .store(supplied_store.clone())
            .preferred_peak_bitrate(321.0)
            .build();
        let Ok(returned) = loader.build_config(TrackId(1), TrackSource::Config(Box::new(given)))
        else {
            panic!("build_config should succeed");
        };
        assert!(
            (returned.preferred_peak_bitrate() - 321.0).abs() < f64::EPSILON,
            "caller-set fields must be preserved"
        );
        assert!(returned.store().is_same(&supplied_store));
        assert!(!returned.store().is_same(&loader.store));
    }

    #[kithara::test(tokio)]
    async fn build_config_forwards_a_prepared_artifact() {
        let fixture = LoaderFixtureSpec::default().build();
        let Ok(src) = ResourceSrc::parse("https://example.com/a.mp3") else {
            panic!("valid url");
        };
        let Ok(grid) = ResourceSrc::parse("https://example.com/a.grid") else {
            panic!("valid artifact url");
        };
        let given = ResourceConfig::for_src(src)
            .store(AssetStore::builder(pools()).build())
            .beat_grid(ArtifactSource::from(grid.clone()))
            .waveform(ArtifactSource::Value(Arc::new(Waveform::default())))
            .build();
        let Ok(returned) = fixture
            .loader
            .build_config(TrackId(7), TrackSource::Config(Box::new(given)))
        else {
            panic!("build_config should succeed");
        };
        assert!(
            matches!(returned.beat_grid(), Some(ArtifactSource::Source(src)) if *src == grid),
            "a grid source must reach the resource untouched"
        );
        assert!(
            matches!(returned.waveform(), Some(ArtifactSource::Value(_))),
            "a caller-held waveform must reach the resource untouched"
        );
    }

    #[kithara::test(tokio)]
    async fn build_config_labels_default_bus_with_track_id() {
        let fixture = LoaderFixtureSpec::default().build();
        let mut rx = fixture.bus.subscribe::<QueueEvent>();
        let Ok(config) = fixture.loader.build_config(
            TrackId(42),
            TrackSource::Uri("https://example.com/a.mp3".into()),
        ) else {
            panic!("build_config should succeed");
        };
        let Some(bus) = config.bus() else {
            panic!("build_config must inject a per-track bus");
        };
        assert!(config.store().is_same(&fixture.loader.store));
        bus.publish(QueueEvent::QueueEnded);
        let Ok(envelope) = rx.try_recv() else {
            panic!("scoped publish must reach the root subscriber");
        };
        assert_eq!(envelope.meta.track, Some(TrackId(42)));
    }

    #[kithara::test(tokio)]
    async fn build_config_invalid_uri_errors() {
        let fixture = LoaderFixtureSpec::default().build();
        let loader = &fixture.loader;
        let Err(err) = loader.build_config(TrackId(1), TrackSource::Uri("not-a-url".into())) else {
            panic!("should reject relative path");
        };
        assert!(matches!(err, QueueError::InvalidUrl(_)));
    }

    #[kithara::test(tokio, multi_thread)]
    async fn prefetch_lane_caps_concurrent_loads() {
        let cap = NonZeroUsize::new(2).expect("BUG: 2 > 0 is mathematically guaranteed");
        let fixture = LoaderFixtureSpec::default().with_cap(cap).build();
        let loader = &fixture.loader;

        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..6 {
            let sem = Arc::clone(&loader.prefetch_lane);
            let in_flight = Arc::clone(&in_flight);
            let max_seen = Arc::clone(&max_seen);
            handles.push(spawn(async move {
                let _permit = sem
                    .acquire_owned()
                    .await
                    .expect("BUG: semaphore not closed in test");
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(cur, Ordering::SeqCst);
                time::sleep(Duration::from_millis(50)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("BUG: spawned task panicked");
        }
        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "concurrency exceeded cap: {}",
            max_seen.load(Ordering::SeqCst)
        );
    }

    #[kithara::test(tokio, multi_thread)]
    async fn spawn_load_bad_url_emits_failed_status() {
        let mut fx = LoaderFixtureSpec::default().build();
        fx.tracks.records_mut().push(TrackRecord::new(
            TrackId(42),
            String::new(),
            TrackSource::Uri("not-a-url".into()),
        ));
        let loader = fx.loader;

        loader.spawn_load(
            &mut fx.tracks,
            TrackId(42),
            TrackSource::Uri("not-a-url".into()),
            LoadClass::Prefetch,
        );
        let status = fx.tracks.records()[0].status.clone();
        assert!(matches!(&status, TrackStatus::Failed(_)));

        // Invalid config fails synchronously without ever loading: the
        // track goes straight to Failed, no fictional Loading first.
        let changes: Vec<_> = fx.tracks.drain_events().collect();
        assert!(
            matches!(
                changes.as_slice(),
                [QueueEvent::TrackStatusChanged {
                    id: TrackId(42),
                    status: changed,
                }] if *changed == status
            ),
            "invalid config records only its Failed status: {changes:?}"
        );
    }

    /// A player on a mock session, its track records, and a loader over them.
    type LoaderParts = (
        PlayerImpl<TestPools>,
        Tracks<TestPools>,
        Arc<Loader<TestPools>>,
    );

    /// A loader built with no runtime, over a player on a mock session.
    fn loader_without_runtime() -> LoaderParts {
        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker)
                .session(mock::session_with_shape(Some(StreamShape::new(
                    NonZeroU32::new(128).expect("fixture output block is non-zero"),
                    mock::SAMPLE_RATE,
                ))))
                .warp(
                    WarpConfig::builder()
                        .render_quantum_frames(
                            NonZeroUsize::new(64).expect("fixture quantum is non-zero"),
                        )
                        .build(),
                )
                .build(),
        );
        let tracks = Tracks::default();
        let (postbox, _no_attempt_reports) = mailbox();
        let loader = Arc::new(Loader::new(
            player.control(),
            None,
            AssetStore::builder(player.pools().clone()).build(),
            NonZeroUsize::MIN,
            postbox,
            CancelToken::root(),
        ));
        (player, tracks, loader)
    }

    /// A queue built with no runtime has nowhere to run a load: the track
    /// fails, and the load never reaches for whatever runtime the calling
    /// thread has. A Host's deck thread, which ticks the queue, has none.
    #[kithara::test]
    fn a_load_without_a_runtime_fails_its_track() {
        let (_player, mut tracks, loader) = loader_without_runtime();
        let id = TrackId(42);
        let source = TrackSource::Uri("/kithara/a-track.wav".into());
        tracks
            .records_mut()
            .push(TrackRecord::new(id, String::new(), source.clone()));

        loader.spawn_load(&mut tracks, id, source, LoadClass::Prefetch);
        assert_eq!(
            tracks.records()[0].status,
            TrackStatus::Failed(QueueError::NoRuntime.to_string()),
            "a load with no runtime fails its track"
        );
    }

    #[kithara::test]
    fn config_failure_without_runtime_updates_tracks_synchronously() {
        let (_player, mut tracks, loader) = loader_without_runtime();
        let source = TrackSource::Uri("not a url".into());
        let spawn_id = TrackId(42);
        let promote_id = TrackId(43);
        tracks.records_mut().extend([
            TrackRecord::new(spawn_id, String::new(), source.clone()),
            TrackRecord::new(promote_id, String::new(), source.clone()),
        ]);
        let Err(expected) = loader.build_config(spawn_id, source.clone()) else {
            panic!("fixture source must be rejected");
        };
        assert!(matches!(expected, QueueError::InvalidUrl(_)));
        let reason = expected.to_string();

        loader.spawn_load(&mut tracks, spawn_id, source.clone(), LoadClass::Prefetch);
        assert_eq!(
            tracks.records()[0].status,
            TrackStatus::Failed(reason.clone())
        );
        assert!(matches!(
            tracks.drain_events().collect::<Vec<_>>().as_slice(),
            [QueueEvent::TrackStatusChanged { id, status }]
                if *id == spawn_id && *status == TrackStatus::Failed(reason.clone())
        ));

        loader.promote_load(&mut tracks, promote_id, source);
        assert_eq!(
            tracks.records()[1].status,
            TrackStatus::Failed(reason.clone())
        );
        assert!(
            matches!(
                tracks.drain_events().collect::<Vec<_>>().as_slice(),
                [QueueEvent::TrackStatusChanged { id, status }]
                    if *id == promote_id && *status == TrackStatus::Failed(reason)
            ),
            "config failure records only its Failed status, never Loading"
        );
    }
}
