#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use kithara::{
    analysis::{AnalysisWorker, AnalysisWorkerConfig, AnalyzerBuilder},
    bufpool::Region,
    events::TrackStatus,
    net::{HttpClient, NetOptions},
    platform::{CancelToken, sync::Arc, time::Duration, tokio},
    play::{PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl, ResourceConfig},
    queue::{Queue, QueueConfig, TrackSource},
    resampler::NoResamplerBackend,
    signal::AudioSpec,
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    TestServerHelper, analysis_pass::stalled_reader, kithara, offline::OfflineSession, temp_dir,
    waits::wait_until,
};

/// A handle left for a track must reach that track's decode path, so playing
/// it warms its own analysis instead of leaving the pass to decode the same
/// audio a second time.
///
/// The pass's own reader only stalls, so every covered frame arrived through
/// the producer. Attachment happens after the queue reports the resource
/// loaded, proving it reaches the decoder through the live relay rather than
/// relying on task scheduling during resource admission.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn playback_feeds_the_pass_opened_for_the_track_it_plays() {
    let helper = TestServerHelper::new().await;
    let url = helper.asset("track.mp3");

    let temp = temp_dir();
    let store = kithara_integration_tests::disk_asset_store(temp.path());
    let region = Region::default();
    let worker = PlayWorker::new(
        PlayWorkerConfig::for_pools(region.byte_pool(), region.sample_pool()).build(),
    );
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .worker(worker)
            .session(OfflineSession::arc_auto())
            .build(),
    );
    let queue = Arc::new(Queue::new(
        QueueConfig::builder()
            .player(player)
            .store(store.clone())
            .build(),
    ));
    let queue_for_tick = Arc::clone(&queue);
    let tick_handle = tokio::task::spawn(async move {
        loop {
            time::sleep(Duration::from_millis(50)).await;
            if queue_for_tick.tick().is_err() {
                break;
            }
        }
    });

    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(NetOptions::default(), CancelToken::never()))
            .build(),
    );
    let cfg = ResourceConfig::for_src(
        ResourceConfig::parse_src(url.as_str()).expect("valid fixture URL"),
    )
    .downloader(downloader)
    .store(store)
    .build();

    let id = queue
        .append(TrackSource::Config(Box::new(cfg)))
        .expect("analysis fixture track appends");
    wait_until(Duration::from_secs(60), "playback resource load", || {
        queue
            .track(id)
            .is_some_and(|entry| matches!(entry.status, TrackStatus::Loaded))
    })
    .await
    .expect("playback resource did not finish loading");

    let rate = NonZeroU32::new(queue.sample_rate()).expect("the engine runs at some rate");
    let cancel = CancelToken::never();
    let worker = AnalysisWorker::new(
        AnalysisWorkerConfig::for_builder(
            AnalyzerBuilder::<NoResamplerBackend>::new(region.sample_pool()).with_waveform(64),
        )
        .cancel(cancel)
        .build(),
    )
    .expect("analysis worker task is admitted");
    let (analysis, producer) = worker.analyze(
        stalled_reader(AudioSpec::new(2, rate)),
        "played-track".into(),
        rate,
    );
    queue.attach_observer(id, producer);
    queue.play();

    let covered = || {
        analysis
            .borrow()
            .as_ref()
            .map_or(0, |progress| progress.analysis().coverage().frames())
    };
    wait_until(Duration::from_secs(60), "analysis coverage", || {
        covered() > 0
    })
    .await
    .expect("the pass covered nothing, so the handle never reached the decode path");

    assert!(covered() > 0, "coverage must survive the wait");
    tick_handle.abort();
}
