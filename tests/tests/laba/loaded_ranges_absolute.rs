#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{AudioEvent, DownloaderEvent, Event},
    net::{HttpClient, NetOptions},
    platform::{
        CancelToken,
        sync::Arc,
        time::{self, Duration},
        tokio,
    },
    play::{PlayerConfig, PlayerImpl, ResourceConfig},
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, TestServerHelper, TestTempDir,
    audio_fixture::EmbeddedAudio, kithara, offline::OfflineSession, temp_dir,
    waits::wait_for_event,
};

/// The whole body must have landed before the buffer bar is judged. Loopback
/// delivers the 3 MB fixture long before playback leaves its first seconds,
/// so the gap between "downloaded" and "played" is wide and unambiguous.
const MIN_TRANSFERRED_FRACTION_PERCENT: u64 = 90;
/// Playback must still be near the start, otherwise a frontier that merely
/// tracks the playhead could satisfy the property by accident.
const MAX_POSITION_FRACTION_PERCENT: u64 = 25;
const MIN_BUFFERED_FRACTION_PERCENT: u64 = 80;

fn spawn_ticker(queue: Arc<Queue>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        loop {
            time::sleep(Duration::from_millis(20)).await;
            if queue.tick().is_err() {
                break;
            }
        }
    })
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(60)))]
async fn progressive_download_fills_the_buffer_bar(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let handle = helper.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(EmbeddedAudio::TEST_MP3_BYTES.to_vec()),
            content_type: Some("audio/mpeg"),
        },
        delivery: Delivery::Range,
    });
    let url = handle.child_url("progressive.mp3");
    let body_len = EmbeddedAudio::TEST_MP3_BYTES.len() as u64;

    let downloader = Downloader::new(
        DownloaderConfig::builder()
            .client(HttpClient::new(NetOptions::default(), CancelToken::never()))
            .build(),
    );
    let player = Arc::new(PlayerImpl::new(
        PlayerConfig::builder()
            .byte_pool(kithara::bufpool::BytePool::default())
            .pcm_pool(kithara::bufpool::PcmPool::default())
            .session(OfflineSession::arc_auto())
            .build(),
    ));
    let queue = Arc::new(Queue::new(QueueConfig::default().with_player(player)));
    let cfg = ResourceConfig::for_src(url.as_str())
        .expect("valid fixture URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader)
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();

    let ticker = spawn_ticker(Arc::clone(&queue));
    let mut rx = queue.subscribe();
    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    queue
        .select(id, Transition::None)
        .expect("select progressive track");
    queue.play();

    let mut transferred = 0;
    wait_for_event(
        &mut rx,
        "the progressive body finishing its transfer",
        |event| {
            let Event::Downloader(DownloaderEvent::RequestCompleted {
                bytes_transferred, ..
            }) = event
            else {
                return false;
            };
            transferred = transferred.max(*bytes_transferred);
            transferred.saturating_mul(100)
                >= body_len.saturating_mul(MIN_TRANSFERRED_FRACTION_PERCENT)
        },
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "LABA-430 precondition: {error}; only {transferred} of {body_len} bytes were \
             transferred, so there is no downloaded-but-unplayed span to report"
        )
    });

    let mut position_ms = 0;
    let mut buffered_ms = 0;
    let mut total_ms = 0;
    wait_for_event(
        &mut rx,
        "playback progress carrying a known duration",
        |event| {
            let Event::Audio(AudioEvent::PlaybackProgress {
                position_ms: position,
                total_ms: Some(total),
                buffered_ms: buffered,
                ..
            }) = event
            else {
                return false;
            };
            position_ms = *position;
            buffered_ms = buffered.unwrap_or(0);
            total_ms = *total;
            true
        },
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|error| panic!("LABA-430 precondition: {error}"));

    assert!(
        position_ms.saturating_mul(100) < total_ms.saturating_mul(MAX_POSITION_FRACTION_PERCENT),
        "LABA-430 precondition: playback had already reached {position_ms}ms of {total_ms}ms, \
         too far in to distinguish a download frontier from the playhead"
    );
    assert!(
        buffered_ms.saturating_mul(100) >= total_ms.saturating_mul(MIN_BUFFERED_FRACTION_PERCENT),
        "LABA-430: the whole {body_len}-byte body is downloaded, but the buffer frontier \
         reports only {buffered_ms}ms of {total_ms}ms at position {position_ms}ms — it tracks \
         decoded-ahead playback instead of what is actually available"
    );

    queue.clear();
    ticker.abort();
    let _ = ticker.await;
}
