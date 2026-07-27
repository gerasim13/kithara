#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{AudioEvent, DownloaderEvent, Event},
    net::{HttpClient, NetOptions, RetryPolicy},
    platform::{CancelToken, sync::Arc, time::Duration},
    play::{PlayerConfig, PlayerImpl, ResourceConfig},
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, TestServerHelper, TestTempDir,
    audio_fixture::EmbeddedAudio, kithara, offline::OfflineSession, temp_dir,
};

const STALL_AFTER_BYTES: usize = EmbeddedAudio::TEST_MP3_BYTES.len() / 2;
const MIN_DOWNLOADED_FRACTION_PERCENT: u64 = 45;
const MAX_POSITION_FRACTION_PERCENT: u64 = 25;
const MIN_BUFFERED_FRACTION_PERCENT: u64 = 40;

#[derive(Clone, Copy, Debug)]
struct Progress {
    buffered_ms: u64,
    position_ms: u64,
    total_ms: u64,
}

async fn observe_partial_window(
    rx: &mut kithara::events::EventReceiver,
    deadline: Duration,
) -> Result<((u64, u64), Progress), String> {
    let mut stalled = None;
    let mut progress = None;
    kithara::platform::time::timeout(deadline, async {
        loop {
            let event = rx
                .recv()
                .await
                .map_err(|error| format!("event stream closed: {error}"))?
                .event;
            match event {
                Event::Downloader(DownloaderEvent::BodyStalled {
                    consumed,
                    expected: Some(expected),
                    ..
                }) => {
                    stalled = Some((consumed, expected));
                }
                Event::Audio(AudioEvent::PlaybackProgress {
                    position_ms,
                    total_ms: Some(total_ms),
                    buffered_ms,
                    ..
                }) => {
                    progress = Some(Progress {
                        buffered_ms: buffered_ms.unwrap_or(0),
                        position_ms,
                        total_ms,
                    });
                }
                _ => {}
            }
            if let (Some(stalled), Some(progress)) = (stalled, progress) {
                return Ok((stalled, progress));
            }
        }
    })
    .await
    .map_err(|_| format!("partial-transfer facts not observed within {deadline:?}"))?
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(60)))]
async fn progressive_download_fills_the_buffer_bar(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let handle = helper.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(EmbeddedAudio::TEST_MP3_BYTES.to_vec()),
            content_type: Some("audio/mpeg"),
        },
        delivery: Delivery::StallAfter {
            after_bytes: STALL_AFTER_BYTES,
        },
    });
    let url = handle.child_url("partial.mp3");

    let net = NetOptions::builder()
        .inactivity_timeout(Duration::from_millis(500))
        .retry_policy(RetryPolicy::new(
            0,
            Duration::from_millis(1),
            Duration::from_millis(1),
        ))
        .build();
    let downloader = Downloader::new(
        DownloaderConfig::builder()
            .client(HttpClient::new(net, CancelToken::never()))
            .build(),
    );
    let player = Arc::new(PlayerImpl::new(
        PlayerConfig::builder()
            .byte_pool(kithara::bufpool::BytePool::default())
            .pcm_pool(kithara::bufpool::PcmPool::default())
            .session(OfflineSession::arc_auto())
            .build(),
    ));
    let queue = Queue::new(QueueConfig::default().with_player(player));
    let cfg = ResourceConfig::for_src(url.as_str())
        .expect("valid fixture URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader)
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();

    let mut rx = queue.subscribe();
    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    queue
        .select(id, Transition::None)
        .expect("select progressive track");

    // Without this the engine stays paused and no playback progress is ever
    // published, so the partial-transfer window below can never be observed.
    queue.play();

    let ((consumed, expected), progress) = observe_partial_window(&mut rx, Duration::from_secs(15))
        .await
        .unwrap_or_else(|error| panic!("LABA-430 precondition: {error}"));
    assert!(
        consumed > 0 && consumed < expected,
        "LABA-430 precondition: transfer was not partial (consumed {consumed} of {expected} bytes)"
    );
    assert!(
        consumed.saturating_mul(100) >= expected.saturating_mul(MIN_DOWNLOADED_FRACTION_PERCENT),
        "LABA-430 precondition: stalled prefix was too small to expose an absolute range \
         (consumed {consumed} of {expected} bytes)"
    );
    assert!(
        progress.position_ms.saturating_mul(100)
            < progress
                .total_ms
                .saturating_mul(MAX_POSITION_FRACTION_PERCENT),
        "LABA-430 precondition: playback had already reached {}ms of {}ms before the partial \
         buffer was observed",
        progress.position_ms,
        progress.total_ms
    );

    assert!(
        progress.buffered_ms.saturating_mul(100)
            >= progress
                .total_ms
                .saturating_mul(MIN_BUFFERED_FRACTION_PERCENT),
        "LABA-430: half the progressive body was downloaded, but the absolute buffer frontier \
         reached only {}ms of {}ms at position {}ms; it still tracks decoded-ahead playback",
        progress.buffered_ms,
        progress.total_ms,
        progress.position_ms
    );

    queue.clear();
}
