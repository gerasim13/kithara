#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use kithara::{
    audio::{
        NoResamplerBackend,
        analysis::{AnalysisWorker, AnalyzerBuilder},
    },
    bufpool::{BytePool, PcmPool},
    decode::PcmSpec,
    net::{HttpClient, NetOptions},
    platform::{CancelToken, sync::Arc, time::Duration, tokio},
    play::{PlayerConfig, PlayerImpl, ResourceConfig},
    queue::{Queue, QueueConfig, TrackSource},
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
/// the producer. The handle is left between `append` and `play`: `append` only
/// spawns the load, and nothing is awaited in between, so the load provably
/// has not run a step and finds the handle waiting.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn playback_feeds_the_pass_opened_for_the_track_it_plays() {
    let helper = TestServerHelper::new().await;
    let url = helper.asset("track.mp3");

    let temp = temp_dir();
    let store = kithara_integration_tests::disk_asset_store(temp.path());
    let player = Arc::new(PlayerImpl::new(
        PlayerConfig::builder()
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .session(OfflineSession::arc_auto())
            .build(),
    ));
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

    let rate = NonZeroU32::new(queue.sample_rate()).expect("the engine runs at some rate");
    let cancel = CancelToken::never();
    let worker = AnalysisWorker::new(
        &cancel,
        AnalyzerBuilder::<NoResamplerBackend>::default().with_waveform(64),
    );
    let (analysis, producer) = worker.analyze(
        stalled_reader(PcmSpec {
            channels: 2,
            sample_rate: rate,
        }),
        cancel.child(),
        "played-track".into(),
        rate,
    );

    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(NetOptions::default(), CancelToken::never()))
            .build(),
    );
    let cfg = ResourceConfig::for_src(
        ResourceConfig::parse_src(url.as_str()).expect("valid fixture URL"),
    )
    .byte_pool(BytePool::default())
    .pcm_pool(PcmPool::default())
    .downloader(downloader)
    .store(store)
    .build();

    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    queue.set_analysis(id, producer);
    queue.play();

    let covered = || {
        analysis
            .borrow()
            .as_ref()
            .map_or(0, |snapshot| snapshot.coverage().frames())
    };
    wait_until(Duration::from_secs(60), "analysis coverage", || {
        covered() > 0
    })
    .await
    .expect("the pass covered nothing, so the handle never reached the decode path");

    assert!(covered() > 0, "coverage must survive the wait");
    tick_handle.abort();
}
