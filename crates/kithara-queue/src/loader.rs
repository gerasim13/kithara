use std::{error::Error as StdError, io::Error, num::NonZeroUsize, task::Waker};

use kithara_assets::AssetStore;
use kithara_bufpool::HasPool;
use kithara_command::{Batch, ChannelConfig, Outcome, Rejection, Sender, Seq, When, channel};
use kithara_download::DownloaderEvent;
use kithara_events::{Envelope, EventReceiver, RecvError, ScopeLabel, TrackId};
use kithara_net::NetError;
use kithara_platform::{
    CancelToken,
    sync::Arc,
    tokio,
    tokio::{runtime::Handle as RuntimeHandle, task::spawn_on},
};
use kithara_play::{
    ArtifactLoadError, Cover, DispatcherProtocol, LoadRefusal, PlayWorker, Resource,
    ResourceConfig, ResourceLoad, ResourceSrc, dispatch, player::PlayerView,
};
use kithara_test_utils::kithara;
use tracing::{debug, warn};

use crate::{
    error::QueueError,
    event::TrackStatus,
    loading::{LoadClass, LoadReport, OpenFailure, TrackLoad},
    queue::{QueueCommand, QueuePostbox},
    track::{TrackSource, Tracks},
};

/// What the queue's dispatcher speaks: one track's open per batch.
type Loads<S> = DispatcherProtocol<ResourceLoad<S>>;

/// Track loader: `ResourceConfig` -> `Resource`, opened by the worker's
/// dispatcher in two isolated lanes, one live load per track. The queue owns
/// it and settles what the dispatcher answers on its own thread; a task
/// beside a load reports a slow transfer or a cover to the queue's mailbox.
pub(crate) struct Loader<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// `None` when the queue was built with no runtime: nothing can open.
    dispatcher: Option<Dispatcher<S>>,
    /// User-selection lane: one slot, isolated from prefetch.
    interactive: Lane,
    /// Background prefetch lane (`max_concurrent_loads` slots).
    prefetch: Lane,
    /// Where a task beside a load reports to the queue.
    postbox: QueuePostbox<S>,
    store: AssetStore<S>,
    /// What the player published: its bus, and the shape a config is
    /// prepared to.
    player: PlayerView,
    /// The worker every prepared config plays on.
    worker: PlayWorker<S>,
}

/// The dispatcher the queue's tracks open on, and the runtime it and the
/// tasks beside each load run on.
struct Dispatcher<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    sender: Sender<Loads<S>>,
    runtime: RuntimeHandle,
}

/// One loader lane: how many opens it keeps in flight, and the ones it has.
struct Lane {
    limit: usize,
    running: Vec<Seq>,
}

impl Lane {
    const fn new(limit: usize) -> Self {
        Self {
            limit,
            running: Vec::new(),
        }
    }

    fn has_room(&self) -> bool {
        self.running.len() < self.limit
    }

    /// The open sent as `seq` was answered; its slot is free when it was this
    /// lane's.
    fn finish(&mut self, seq: Seq) {
        self.running.retain(|running| *running != seq);
    }
}

impl<S> Loader<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// A loader whose dispatcher runs on `runtime`, holding both lanes'
    /// opens in flight at once.
    pub(crate) fn new(
        player: PlayerView,
        worker: PlayWorker<S>,
        runtime: Option<RuntimeHandle>,
        store: AssetStore<S>,
        max_concurrent_loads: NonZeroUsize,
        postbox: QueuePostbox<S>,
    ) -> Self {
        let dispatcher = runtime.map(|runtime| {
            let (sender, inbox) = channel(
                ChannelConfig::builder()
                    .capacity(max_concurrent_loads.saturating_add(1))
                    .build(),
            );
            drop(spawn_on(&runtime, dispatch(inbox)));
            Dispatcher { sender, runtime }
        });
        Self {
            dispatcher,
            interactive: Lane::new(1),
            prefetch: Lane::new(max_concurrent_loads.get()),
            postbox,
            store,
            player,
            worker,
        }
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
    /// Both paths finish with the player's prepare so worker /
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
        self.player
            .prepare_config(config, self.worker.clone())
            .map_err(QueueError::from)
    }

    /// The queue closed: cancel every live load, then let the dispatcher go,
    /// which drops the opens still running once their tokens read cancelled.
    pub(crate) fn close(&mut self, tracks: &mut Tracks<S>) {
        tracks.cancel_loads();
        self.dispatcher = None;
    }

    /// Wake `waker` with each open the dispatcher answers, so the executor
    /// holding the queue settles it.
    pub(crate) fn hold(&mut self, waker: Waker) {
        if let Some(dispatcher) = &mut self.dispatcher {
            dispatcher.sender.hold(waker);
        }
    }

    /// What a load needs before it begins: its prepared config, and the
    /// runtime the tasks beside it run on.
    fn prepare(
        &self,
        id: TrackId,
        source: TrackSource<S>,
        class: LoadClass,
    ) -> Result<(TrackLoad<S>, RuntimeHandle), QueueError> {
        let load = TrackLoad::new(self.build_config(id, source)?, class)?;
        let dispatcher = self.dispatcher.as_ref().ok_or(QueueError::NoRuntime)?;
        Ok((load, dispatcher.runtime.clone()))
    }

    /// The selection reached `id`, which is still loading: its load is
    /// wanted. A live load keeps going, moved to the interactive lane while
    /// it still waits for one; a track with none gets a fresh interactive
    /// load.
    pub(crate) fn promote_load(
        &mut self,
        tracks: &mut Tracks<S>,
        id: TrackId,
        source: TrackSource<S>,
    ) {
        if tracks.promote_load(id) {
            self.pump(tracks);
        } else {
            self.spawn_load(tracks, id, source, LoadClass::Interactive);
        }
    }

    /// Send the loads waiting for a lane to the dispatcher while their lanes
    /// have room, the interactive lane first.
    pub(crate) fn pump(&mut self, tracks: &mut Tracks<S>) {
        let Some(dispatcher) = &mut self.dispatcher else {
            return;
        };
        for (class, lane) in [
            (LoadClass::Interactive, &mut self.interactive),
            (LoadClass::Prefetch, &mut self.prefetch),
        ] {
            while lane.has_room()
                && let Some((id, load)) = tracks.next_unsent(class)
            {
                let batch = Batch {
                    basis: Vec::new(),
                    commands: vec![load],
                };
                match dispatcher.sender.send(When::Next, batch) {
                    Ok(seq) => {
                        kithara::probe_event!(admission_started, track_id = id.as_u64());
                        lane.running.push(seq);
                        tracks.mark_sent(id, seq);
                    }
                    Err(error) => tracks.fail(id, &QueueError::Resource(error.to_string())),
                }
            }
        }
    }

    /// Read the track's cover beside its audio, over the load's transport and
    /// the track's token, and report it to the queue, which places it while
    /// that token lives. The audio never waits for the cover, and a cover
    /// that never arrives leaves the load untouched.
    fn read_cover(&self, runtime: &RuntimeHandle, id: TrackId, load: &TrackLoad<S>) {
        let Some(cover) = load.config.artwork().cloned() else {
            return;
        };
        let config = load.config.clone();
        let token = load.token().clone();
        let postbox = self.postbox.clone();
        drop(spawn_on(runtime, async move {
            match config.artifact_fetch().load::<Cover>(&cover).await {
                Ok(cover) => report(
                    &postbox,
                    LoadReport::Cover {
                        id,
                        load: token,
                        cover: Arc::new(cover.into()),
                    },
                ),
                Err(ArtifactLoadError::Cancelled { .. }) => {}
                Err(error) => warn!(?id, %error, "the track's cover never arrived"),
            }
        }));
    }

    /// Stop waking the executor that held the queue.
    pub(crate) fn release(&mut self) {
        if let Some(dispatcher) = &mut self.dispatcher {
            dispatcher.sender.release();
        }
    }

    /// The next open the dispatcher answered, with its lane slot freed;
    /// `None` once every answer is read. The queue settles it on the track
    /// that sent it, then [`Self::pump`]s the loads waiting for the slots.
    pub(crate) fn answered(&mut self) -> Option<(Seq, Result<Resource, OpenFailure>)> {
        let receipt = self.dispatcher.as_mut()?.sender.receipts().next()?;
        let seq = receipt.seq();
        self.interactive.finish(seq);
        self.prefetch.finish(seq);
        let (outcome, _batch) = receipt.into();
        Some((seq, settled(outcome)))
    }

    /// Begin loading `id` from `source` in `class`'s lane, unless a live load
    /// already runs for it - one track never holds two lane slots. A load
    /// that cannot begin fails its track at once, never reaching `Loading`.
    pub(crate) fn spawn_load(
        &mut self,
        tracks: &mut Tracks<S>,
        id: TrackId,
        source: TrackSource<S>,
        class: LoadClass,
    ) {
        let (load, runtime) = match self.prepare(id, source, class) {
            Ok(prepared) => prepared,
            Err(error) => {
                tracks.set_status(id, TrackStatus::Failed(error.to_string()));
                return;
            }
        };
        if let Some(load) = tracks.begin_load(id, load) {
            self.read_cover(&runtime, id, load);
            self.watch_for_slow_transfer(&runtime, id, load);
            self.watch_for_cancel(&runtime, id, load);
        }
        self.pump(tracks);
    }

    /// Watch the load's bus for its first slow transfer. The watch
    /// subscribes here, before the load is sent, so no `LoadSlow` slips past
    /// it, and ends with the load.
    fn watch_for_slow_transfer(&self, runtime: &RuntimeHandle, id: TrackId, load: &TrackLoad<S>) {
        let Some(bus) = load.config.bus() else {
            return;
        };
        drop(spawn_on(
            runtime,
            slow_transfer(
                id,
                load.watch().clone(),
                bus.subscribe(),
                self.postbox.clone(),
            ),
        ));
    }

    /// Watch the load for its track's cancel, which can come from outside
    /// the queue: a load still waiting for its lane is never sent, so only
    /// this report ends it.
    fn watch_for_cancel(&self, runtime: &RuntimeHandle, id: TrackId, load: &TrackLoad<S>) {
        drop(spawn_on(
            runtime,
            report_cancel(
                id,
                load.token().clone(),
                load.watch().clone(),
                self.postbox.clone(),
            ),
        ));
    }
}

/// Report [`LoadReport::Cancelled`] to the queue once `watch` ends, when the
/// track's `token` is what ended it rather than the load's own end.
async fn report_cancel<S>(
    id: TrackId,
    token: CancelToken,
    watch: CancelToken,
    postbox: QueuePostbox<S>,
) where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    watch.cancelled().await;
    if token.is_cancelled() {
        report(&postbox, LoadReport::Cancelled { id });
    }
}

/// Post `report` to the queue. A queue that is gone has no track left to
/// report on.
fn report<S>(postbox: &QueuePostbox<S>, report: LoadReport)
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    if postbox.post(QueueCommand::Load(report)).is_err() {
        debug!("the queue is gone: dropping a load's report");
    }
}

/// What became of an open, as the queue answers it: a refusal a later ask
/// can answer is told apart from one that is final.
fn settled<S>(outcome: Outcome<Loads<S>>) -> Result<Resource, OpenFailure>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    match outcome {
        Outcome::Applied { data, .. } => Ok(data),
        Outcome::Rejected(Rejection::Refused(LoadRefusal::Open(error))) => {
            let later = can_answer_later(&error);
            let error = QueueError::Resource(error.to_string());
            Err(if later {
                OpenFailure::AskAgain(error)
            } else {
                OpenFailure::Final(error)
            })
        }
        Outcome::Rejected(Rejection::Refused(refusal)) => Err(OpenFailure::Final(
            QueueError::Resource(refusal.to_string()),
        )),
        Outcome::Rejected(rejection) => Err(OpenFailure::Final(QueueError::Resource(format!(
            "the load ended without an open: {rejection:?}"
        )))),
    }
}

/// Report the first [`DownloaderEvent::LoadSlow`] `events` delivers to the
/// queue, which turns the track [`TrackStatus::Slow`], unless `watch` ends
/// first. A `Lagged` bus dropped the oldest envelopes and keeps delivering,
/// so the watch survives the gap; only `Closed` ends it.
async fn slow_transfer<S>(
    id: TrackId,
    watch: CancelToken,
    mut events: EventReceiver<DownloaderEvent>,
    postbox: QueuePostbox<S>,
) where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    let slow = async {
        loop {
            match events.recv().await {
                Ok(Envelope {
                    event: DownloaderEvent::LoadSlow { .. },
                    ..
                }) => return true,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return false,
            }
        }
    };
    tokio::select! {
        biased;
        () = watch.cancelled() => {}
        slow = slow => if slow {
            report(&postbox, LoadReport::Slow { id, watch: watch.clone() });
        },
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
        future::Future,
        num::{NonZeroU16, NonZeroU32, NonZeroU64},
        pin::pin,
        task::{Context, Poll, Wake},
    };

    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_command::{Mailbox, mailbox};
    use kithara_download::RequestId;
    use kithara_events::EventBus;
    use kithara_platform::{
        time::{self, Duration},
        tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    };
    use kithara_play::{
        ArtifactSource, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl, StreamShape, mock,
    };
    use kithara_test_utils::{TestTempDir, kithara, temp_dir};
    use kithara_warp::WarpConfig;
    use kithara_waveform::Waveform;

    use super::*;
    use crate::{
        consts,
        event::QueueEvent,
        loading::tests::config,
        test_pools::{TestPools, pools},
        track::TrackRecord,
    };

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
        /// What the loader's dispatcher runs on.
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

    /// A runtime nothing drives: the loader sends to a dispatcher that never
    /// opens anything, so every load it sends stays in flight.
    fn idle_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime for the dispatcher")
    }

    fn queued(tracks: &mut Tracks<TestPools>, id: TrackId, url: &str) -> TrackSource<TestPools> {
        let source = TrackSource::Uri(url.into());
        tracks
            .records_mut()
            .push(TrackRecord::new(id, String::new(), source.clone()));
        source
    }

    fn status_of(tracks: &Tracks<TestPools>, id: TrackId) -> TrackStatus {
        tracks
            .records()
            .iter()
            .find(|record| record.id == id)
            .map(|record| record.status.clone())
            .expect("the track stays queued")
    }

    /// A runtime that goes away drops the dispatcher with the opens it
    /// never ran. Each still comes back answered, so the queue never waits
    /// on a load nothing runs any more.
    #[kithara::test]
    fn a_load_its_runtime_dropped_fails_its_track() {
        let runtime = idle_runtime();
        let mut fixture = LoaderFixtureSpec::default()
            .with_runtime(Some(runtime.handle().clone()))
            .build();
        let id = TrackId::allocate();
        let source = queued(&mut fixture.tracks, id, "https://example.com/abandoned.mp3");
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, source, LoadClass::Prefetch);
        assert_eq!(status_of(&fixture.tracks, id), TrackStatus::Loading);

        drop(runtime);
        assert_eq!(settle(&mut fixture.loader, &mut fixture.tracks), 0);

        assert!(
            matches!(status_of(&fixture.tracks, id), TrackStatus::Failed(_)),
            "the track waits on a load nothing runs: {:?}",
            status_of(&fixture.tracks, id)
        );
    }

    /// A load sent after its runtime went away comes back unsent: the track
    /// fails instead of loading on a dispatcher that is gone.
    #[kithara::test]
    fn a_load_sent_after_its_runtime_dropped_fails_its_track() {
        let runtime = idle_runtime();
        let mut fixture = LoaderFixtureSpec::default()
            .with_runtime(Some(runtime.handle().clone()))
            .build();
        drop(runtime);

        let id = TrackId::allocate();
        let source = queued(&mut fixture.tracks, id, "https://example.com/late.mp3");
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, source, LoadClass::Prefetch);
        settle(&mut fixture.loader, &mut fixture.tracks);

        assert!(
            matches!(status_of(&fixture.tracks, id), TrackStatus::Failed(_)),
            "the track waits on a dispatcher that is gone: {:?}",
            status_of(&fixture.tracks, id)
        );
    }

    /// Each lane keeps its own count of opens in flight: prefetch never holds
    /// more than its cap, the loads past it wait in queue order, and a
    /// selection still opens beside a full prefetch lane.
    #[kithara::test]
    fn each_lane_caps_its_own_opens_in_flight() {
        let runtime = idle_runtime();
        let mut fixture = LoaderFixtureSpec::default()
            .with_cap(NonZeroUsize::new(2).expect("two prefetch slots"))
            .with_runtime(Some(runtime.handle().clone()))
            .build();
        let prefetched: Vec<TrackId> = (0..4).map(|_| TrackId::allocate()).collect();
        for (index, id) in prefetched.iter().enumerate() {
            let source = queued(
                &mut fixture.tracks,
                *id,
                &format!("https://example.com/prefetch-{index}.mp3"),
            );
            fixture
                .loader
                .spawn_load(&mut fixture.tracks, *id, source, LoadClass::Prefetch);
        }
        let selected = TrackId::allocate();
        let source = queued(
            &mut fixture.tracks,
            selected,
            "https://example.com/selected.mp3",
        );
        fixture.loader.spawn_load(
            &mut fixture.tracks,
            selected,
            source,
            LoadClass::Interactive,
        );

        let statuses: Vec<TrackStatus> = prefetched
            .iter()
            .map(|id| status_of(&fixture.tracks, *id))
            .collect();
        assert_eq!(
            statuses,
            [
                TrackStatus::Loading,
                TrackStatus::Loading,
                TrackStatus::Pending,
                TrackStatus::Pending
            ],
            "prefetch opens past its cap"
        );
        assert_eq!(
            status_of(&fixture.tracks, selected),
            TrackStatus::Loading,
            "a full prefetch lane starves the selection"
        );
    }

    /// A closed queue wants nothing it was loading: every live load is
    /// cancelled, so its open ends instead of fetching for nobody.
    #[kithara::test]
    fn closing_cancels_every_live_load() {
        let runtime = idle_runtime();
        let mut fixture = LoaderFixtureSpec::default()
            .with_runtime(Some(runtime.handle().clone()))
            .build();
        let id = TrackId::allocate();
        let source = queued(&mut fixture.tracks, id, "https://example.com/closing.mp3");
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, source, LoadClass::Prefetch);
        let token = fixture.tracks.records()[0]
            .load
            .as_ref()
            .map(|load| load.token().clone())
            .expect("the track loads");

        fixture.loader.close(&mut fixture.tracks);

        assert!(token.is_cancelled(), "a closed queue's load keeps running");
        assert_eq!(status_of(&fixture.tracks, id), TrackStatus::Cancelled);
    }

    /// A load whose caller cancelled its track while it waited for its lane
    /// is never sent; it ends cancelled instead of leaving its track pending.
    #[kithara::test(native, tokio)]
    async fn a_load_cancelled_while_it_waits_for_its_lane_ends_cancelled(temp_dir: TestTempDir) {
        let mut fixture = LoaderFixtureSpec::default().build();
        let path = |name: &str| temp_dir.path().join(name).to_string_lossy().into_owned();
        let held = TrackId::allocate();
        let source = queued(&mut fixture.tracks, held, &path("held.wav"));
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, held, source, LoadClass::Interactive);
        let waiting = TrackId::allocate();
        let master = CancelToken::never().child();
        let source = TrackSource::from(config(&path("waiting.wav"), master.clone()));
        fixture
            .tracks
            .records_mut()
            .push(TrackRecord::new(waiting, String::new(), source.clone()));
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, waiting, source, LoadClass::Interactive);
        assert_eq!(
            status_of(&fixture.tracks, waiting),
            TrackStatus::Pending,
            "the selection lane holds one open"
        );

        master.cancel();
        fixture
            .run_until(|tracks| status_of(tracks, waiting) == TrackStatus::Cancelled)
            .await;
    }

    /// The first track's live load, begun from a config the test owns.
    fn loading(tracks: &mut Tracks<TestPools>) -> TrackId {
        let id = TrackId::allocate();
        queued(tracks, id, "https://example.com/slow.mp3");
        let load = TrackLoad::new(
            config("https://example.com/slow.mp3", CancelToken::never().child()),
            LoadClass::Prefetch,
        )
        .expect("the config carries its token");
        assert!(tracks.begin_load(id, load).is_some());
        id
    }

    fn watch(tracks: &Tracks<TestPools>) -> CancelToken {
        tracks.records()[0]
            .load
            .as_ref()
            .map(|load| load.watch().clone())
            .expect("the track loads")
    }

    /// The bus drops the oldest envelopes under a burst and keeps
    /// delivering, so the slow watch has to survive the gap. The burst
    /// below is longer than the bus capacity with nothing reading, which
    /// makes the drop certain, and the `LoadSlow` behind it still has to
    /// reach the watch.
    #[kithara::test]
    fn a_slow_watch_survives_a_bus_that_dropped_a_burst() {
        const CAPACITY: usize = 4;

        let bus = EventBus::new(CAPACITY);
        let mut tracks = Tracks::<TestPools>::default();
        let id = loading(&mut tracks);
        let (postbox, mut mailbox) = mailbox();
        let events = bus.subscribe();

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

        let mut slow = pin!(slow_transfer(id, watch(&tracks), events, postbox));
        assert!(
            slow.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready(),
            "the watch ends once it reported the slow transfer"
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

    /// A slow transfer is news only while its load runs. Once the selection
    /// moved on and cancelled that load, a `LoadSlow` its download reports
    /// afterwards leaves the track `Cancelled`.
    #[kithara::test]
    fn a_slow_transfer_after_its_load_was_cancelled_leaves_the_track_cancelled() {
        let bus = EventBus::new(4);
        let mut tracks = Tracks::<TestPools>::default();
        let id = loading(&mut tracks);
        let (postbox, mut mailbox) = mailbox();
        let mut slow = pin!(slow_transfer(id, watch(&tracks), bus.subscribe(), postbox));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(slow.as_mut().poll(&mut cx).is_pending());

        tracks.set_status(id, TrackStatus::Cancelled);
        bus.publish(DownloaderEvent::LoadSlow {
            request_id: RequestId::new(NonZeroU64::MIN),
            elapsed: Duration::ZERO,
        });
        assert!(
            matches!(slow.as_mut().poll(&mut cx), Poll::Ready(())),
            "the watch ends with its load"
        );
        for report in posted(&mut mailbox) {
            tracks.apply_report(report);
        }

        assert_eq!(
            tracks.records()[0].status,
            TrackStatus::Cancelled,
            "a cancelled load's slow transfer must not revive its track"
        );
    }

    /// The reports the tasks beside a load posted since the last drain, in
    /// post order.
    fn posted(
        mailbox: &mut Mailbox<QueueCommand<TestPools>, QueueError>,
    ) -> impl Iterator<Item = LoadReport> + use<> {
        mailbox.drain().map(|post| {
            let QueueCommand::Load(report) = post.command else {
                panic!("a load's tasks post only their reports");
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
    /// loads into (so tests can seed entries), the root [`EventBus`] (so
    /// tests can subscribe for assertions), and the mailbox the tasks beside
    /// its loads report to, held as a queue would hold both.
    struct LoaderFixture {
        loader: Loader<TestPools>,
        tracks: Tracks<TestPools>,
        bus: EventBus,
        mailbox: Mailbox<QueueCommand<TestPools>, QueueError>,
        woke: UnboundedReceiver<()>,
        _player: PlayerImpl<TestPools>,
    }

    /// Settle every open the dispatcher answered as the queue does, admitting
    /// what opened; returns how many opened.
    fn settle(loader: &mut Loader<TestPools>, tracks: &mut Tracks<TestPools>) -> usize {
        let mut opened = 0;
        while let Some((seq, outcome)) = loader.answered() {
            if let Some((id, resource)) = tracks.settle_load(seq, outcome) {
                tracks.admit(id, resource);
                opened += 1;
            }
        }
        loader.pump(tracks);
        opened
    }

    impl LoaderFixture {
        /// Settle what the dispatcher answered and apply what the tasks
        /// reported, as the queue does when its holder wakes it, until
        /// `done` holds.
        async fn run_until(&mut self, done: impl Fn(&Tracks<TestPools>) -> bool) {
            loop {
                settle(&mut self.loader, &mut self.tracks);
                for report in posted(&mut self.mailbox) {
                    self.tracks.apply_report(report);
                }
                if done(&self.tracks) {
                    return;
                }
                time::timeout(Duration::from_secs(2), self.woke.recv())
                    .await
                    .expect("the loader answers")
                    .expect("the fixture holds the waker");
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
            let waker = Waker::from(Arc::new(Wakes(woke_tx)));
            mailbox.hold(waker.clone());
            let mut loader = Loader::new(
                player.view().clone(),
                player.worker().clone(),
                self.runtime,
                store,
                self.cap,
                postbox,
            );
            loader.hold(waker);
            LoaderFixture {
                loader,
                tracks,
                bus,
                mailbox,
                woke,
                _player: player,
            }
        }
    }

    /// A load reads its track's cover beside the audio and places it while
    /// that load is current; a superseded load's cover never lands.
    #[kithara::test(native, tokio)]
    async fn only_the_current_load_places_its_cover(temp_dir: TestTempDir) {
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
        fixture
            .loader
            .spawn_load(&mut fixture.tracks, id, current, LoadClass::Prefetch);
        fixture
            .run_until(|tracks| matches!(tracks.records()[0].status, TrackStatus::Failed(_)))
            .await;
        fixture
            .run_until(|tracks| tracks.records()[0].entry().metadata().artwork.is_some())
            .await;

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
        assert_eq!(changes, 1, "the superseded load's cover landed");
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

    #[kithara::test(tokio)]
    async fn spawn_load_bad_url_emits_failed_status() {
        let mut fx = LoaderFixtureSpec::default().build();
        fx.tracks.records_mut().push(TrackRecord::new(
            TrackId(42),
            String::new(),
            TrackSource::Uri("not-a-url".into()),
        ));

        fx.loader.spawn_load(
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
    type LoaderParts = (PlayerImpl<TestPools>, Tracks<TestPools>, Loader<TestPools>);

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
        let (postbox, _no_load_reports) = mailbox();
        let loader = Loader::new(
            player.view().clone(),
            player.worker().clone(),
            None,
            AssetStore::builder(player.pools().clone()).build(),
            NonZeroUsize::MIN,
            postbox,
        );
        (player, tracks, loader)
    }

    /// A queue built with no runtime has nowhere to run a load: the track
    /// fails, and the load never reaches for whatever runtime the calling
    /// thread has. A Host's session thread, which ticks the queue, has none.
    #[kithara::test]
    fn a_load_without_a_runtime_fails_its_track() {
        let (_player, mut tracks, mut loader) = loader_without_runtime();
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
        let (_player, mut tracks, mut loader) = loader_without_runtime();
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
