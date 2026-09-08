use std::{
    convert::Infallible,
    num::{NonZeroU32, NonZeroUsize},
};

use kithara::{
    analysis::{
        AnalysisFingerprint, AnalysisProgress, AnalysisToken, BeatArtifact, BeatSnapshot,
        BeatState, Coverage, FrameRange, Waveform,
    },
    assets::StorageBackend,
    events::TrackId,
    host::HostConfig,
    net::{HttpClient, NetOptions},
    platform::{
        CancelToken,
        sync::Mutex,
        time::Duration,
        tokio::{
            runtime::Handle,
            sync::{mpsc, oneshot, watch},
            task,
        },
    },
    play::{PlayWorkerConfig, PlayerConfig, PlayerImpl, policy::DomainKeyPolicy},
    queue::QueueConfig,
    stream::dl::{Downloader, DownloaderConfig},
    worker::{DispatcherConfig, TaskConfig, Worker, WorkerConfig},
};
use kithara_test_fixtures::{asset::Asset, assets};
use kithara_test_utils::off_thread::OffThread;
use url::Url;

use super::{Entry, Request};
use crate::{
    config::{AppConfig, AppDrm},
    pools::{
        self, AppHost, AppQueue, AppQueueControl, AppStore, AppTrackSource, AppWorker, Pools,
        PoolsSection,
    },
    sources::build_resource_config,
    state::UiState,
    wave_cache::{AnalysisPersistence, AnalysisTarget, persistence::AnalysisPersistenceConfig},
    waveform::TrackAnalysis,
};

pub(crate) fn chunk_seconds() -> NonZeroU32 {
    NonZeroU32::new(16).expect("fixture chunk duration is non-zero")
}

pub(crate) fn test_pools() -> Pools {
    pools::build(&PoolsSection::default()).expect("valid app pool policy")
}

pub(crate) fn axis() -> NonZeroU32 {
    NonZeroU32::new(44_100).expect("fixture rate is non-zero")
}

pub(crate) fn other_axis() -> NonZeroU32 {
    NonZeroU32::new(48_000).expect("fixture rate is non-zero")
}

pub(crate) fn fingerprint() -> AnalysisFingerprint {
    AnalysisFingerprint::new(None, Some("wave:test:v1"))
}

pub(crate) fn progress(analysis: TrackAnalysis) -> AnalysisProgress {
    AnalysisProgress::try_from(analysis).expect("settled fixture is valid progress")
}

pub(crate) fn one_bucket_wave() -> Waveform {
    // version 1 + one bucket of three 0.5 band heights (0.5 = 0x3F000000).
    Waveform::try_from([1, 0, 0, 0, 0, 0, 0, 63, 0, 0, 0, 63, 0, 0, 0, 63].as_slice())
        .expect("hand-built blob is valid")
}

pub(crate) fn grid() -> BeatSnapshot {
    BeatSnapshot::new(
        BeatArtifact::new(
            128.0,
            vec![(0, Some(0.9)), (500, None)],
            vec![(0, Some(0.9))],
        ),
        BeatState::Final,
        Vec::new(),
    )
}

pub(crate) fn snapshot(
    token: AnalysisToken,
    revision: u64,
    covered: u64,
    fingerprint: AnalysisFingerprint,
    beat: Option<BeatSnapshot>,
) -> TrackAnalysis {
    let mut coverage = Coverage::default();
    coverage.insert(FrameRange::new(0, covered));
    TrackAnalysis::builder()
        .token(token)
        .revision(revision)
        .source_sample_rate(axis())
        .extent(1_000)
        .settled(true)
        .coverage(coverage)
        .fingerprint(fingerprint)
        .waveform(one_bucket_wave())
        .maybe_beat(beat)
        .build()
}

pub(crate) fn analysis() -> TrackAnalysis {
    snapshot("test-track".into(), 1, 1_000, fingerprint(), None)
}

pub(crate) fn revision_of(revision: u64) -> TrackAnalysis {
    snapshot("test-track".into(), revision, 1_000, fingerprint(), None)
}

pub(crate) fn revision_held(rx: &watch::Receiver<Option<AnalysisProgress>>) -> Option<u64> {
    rx.borrow().as_ref().map(|p| p.analysis().revision())
}

pub(crate) fn queue() -> (AppHost, AppQueueControl) {
    let worker = AppWorker::new(PlayWorkerConfig::builder(test_pools()).build());
    let mut host = AppHost::new(HostConfig::builder().build()).expect("test host");
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .worker(worker)
            .sample_rate(host.requested_sample_rate())
            .build(),
    );
    let queue = AppQueue::new(QueueConfig::builder().player(player).build());
    let queue = host.insert(queue).expect("host accepts queue");
    let control = queue.control().clone();
    (host, control)
}

pub(crate) async fn queue_off() -> (OffThread<(AppHost, AppQueueControl)>, AppQueueControl) {
    queue_off_named("app-host").await
}

pub(crate) async fn queue_off_named(
    name: &'static str,
) -> (OffThread<(AppHost, AppQueueControl)>, AppQueueControl) {
    let host = OffThread::spawn(name, || Ok::<_, Infallible>(queue()))
        .await
        .expect("app host fixture is infallible");
    let control = host.call(|(_, control)| control.clone()).await;
    (host, control)
}

/// Appends a track from the host owner thread, as the app would.
pub(crate) async fn track(
    host: &OffThread<(AppHost, AppQueueControl)>,
    id: u64,
    url: &str,
) -> (TrackId, AppTrackSource) {
    let track_id = TrackId::from(id);
    let url = url.to_owned();
    host.call(move |(_, queue)| {
        queue
            .append_with_id(track_id, url)
            .expect("append test track");
        let source = queue.track_source(track_id).expect("track has a source");
        (track_id, source)
    })
    .await
}

pub(crate) fn memory_store() -> AppStore {
    AppStore::builder(test_pools())
        .backend(StorageBackend::Memory)
        .build()
}

pub(crate) fn app_config(cancel: &CancelToken, store: AppStore) -> AppConfig {
    let pools = test_pools();
    let worker = AppWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
    AppConfig::builder()
        .drm(AppDrm::new(DomainKeyPolicy::new(Vec::new())))
        .downloader(Downloader::new(
            DownloaderConfig::for_client(HttpClient::new(
                NetOptions::builder().build(),
                pools,
                cancel.child(),
            ))
            .build(),
        ))
        .shutdown(cancel.child())
        .worker(worker)
        .store(store)
        .build()
}

pub(crate) fn persistence(cancel: &CancelToken, pools: Pools) -> AnalysisPersistence {
    let worker = Worker::new(
        WorkerConfig::new()
            .with_cancel(cancel.child())
            .with_runtime(Handle::current()),
    );
    AnalysisPersistence::new(AnalysisPersistenceConfig::new(
        worker,
        pools,
        NonZeroUsize::MIN,
        Duration::from_secs(u64::from(chunk_seconds().get())),
        DispatcherConfig::builder()
            .name("analysis-service-test")
            .build(),
        TaskConfig::new(),
    ))
    .expect("persistence fixture starts")
}

fn asset_url(asset: Asset) -> String {
    let path = asset.path().expect("fixture is stored on disk");
    assert!(path.is_file(), "fixture file exists: {}", path.display());
    Url::from_file_path(path)
        .expect("fixture path is absolute")
        .into()
}

#[kithara::fixture]
pub(crate) fn tone_mp3() -> String {
    asset_url(assets::sine_mp3_a440_2s())
}

#[kithara::fixture]
pub(crate) fn rhythm_a_mp3() -> String {
    asset_url(assets::rhythm_mp3_deck_a_120bpm_48k())
}

#[kithara::fixture]
pub(crate) fn rhythm_b_mp3() -> String {
    asset_url(assets::rhythm_mp3_deck_b_120bpm_48k())
}

#[kithara::fixture]
pub(crate) fn short_wav() -> String {
    asset_url(assets::sine_wav_a440_2s())
}

#[kithara::fixture]
pub(crate) fn long_wav() -> String {
    asset_url(assets::sine_wav_a440_12s())
}

pub(crate) async fn next_subscribe(
    requests: &mut mpsc::Receiver<Request>,
) -> (
    TrackId,
    oneshot::Sender<watch::Receiver<Option<AnalysisProgress>>>,
) {
    loop {
        match requests.recv().await {
            Some(Request::Subscribe {
                track_id, reply, ..
            }) => return (track_id, reply),
            Some(Request::Warm { .. }) => {}
            None => panic!("the deck subscribes"),
        }
    }
}

pub(crate) async fn answer_subscribe(
    requests: &mut mpsc::Receiver<Request>,
    expected: TrackId,
) -> watch::Sender<Option<AnalysisProgress>> {
    let (track_id, reply) = next_subscribe(requests).await;
    assert_eq!(track_id, expected, "for the track its queue holds");
    let (tx, rx) = watch::channel(None);
    assert!(reply.send(rx).is_ok(), "the deck waits for the reply");
    tx
}

pub(crate) async fn wait_for_revision(state: &Mutex<UiState>, revision: u64) {
    for _ in 0..2_000 {
        if state.lock().analysis.as_ref().map(TrackAnalysis::revision) == Some(revision) {
            return;
        }
        task::yield_now().await;
    }
    panic!("revision {revision} never reached the deck");
}

pub(crate) fn entry(
    config: &AppConfig,
    queue: AppQueueControl,
    track_id: TrackId,
    source: AppTrackSource,
) -> Entry {
    let AppTrackSource::Uri(url) = source else {
        panic!("fixture tracks are appended by URL");
    };
    let config = build_resource_config(&url, config).expect("source yields a resource");
    let target = AnalysisTarget::for_config(&config).expect("source has an analysis target");
    Entry::new(target, config, queue, track_id)
}
