#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{Event, EventReceiver, QueueEvent, TrackId, TrackStatus},
    net::{HttpClient, NetOptions},
    platform::{
        CancelToken,
        sync::Arc,
        time::{self, Duration, Instant, timeout},
        tokio,
    },
    play::{PlayerConfig, PlayerImpl, ResourceConfig},
    queue::{PlaybackView, Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    SignalFormat, SignalSpec, SignalSpecLength, TestServerHelper, TestTempDir, kithara,
    offline::OfflineSession, temp_dir,
};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const FREQ_HZ: f64 = 659.25;
const STREAM_FRAMES: usize = 44_100 * 30;
const BUFFER_MUST_REACH_FRACTION: f64 = 0.9;

fn spawn_ticker(queue: Arc<Queue>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        loop {
            time::sleep(Duration::from_millis(50)).await;
            if queue.tick().is_err() {
                break;
            }
        }
    })
}

async fn wait_for_loaded(
    rx: &mut EventReceiver,
    queue: &Queue,
    id: TrackId,
    deadline: Duration,
) -> Result<(), String> {
    if let Some(entry) = queue.track(id)
        && matches!(entry.status, TrackStatus::Loaded)
    {
        return Ok(());
    }
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Ok(Ok(Event::Queue(QueueEvent::TrackStatusChanged { id: tid, status }))) =
            timeout(Duration::from_millis(500), rx.recv())
                .await
                .map(|result| result.map(|envelope| envelope.event))
            && tid == id
        {
            match status {
                TrackStatus::Loaded => return Ok(()),
                TrackStatus::Failed(error) => return Err(format!("track failed: {error}")),
                _ => {}
            }
        }
    }
    Err("track never reached Loaded".into())
}

async fn play_to_first_third(queue: &Queue, deadline: Duration) -> PlaybackView {
    let start = Instant::now();
    let mut latest = PlaybackView::default();
    while start.elapsed() < deadline {
        time::sleep(Duration::from_millis(100)).await;
        latest = queue.playback_view();
        if let (Some(position), Some(duration)) = (latest.position, latest.duration)
            && position >= duration / 3.0
        {
            return latest;
        }
    }
    latest
}

/// LABA-430: once a progressive track has downloaded, its buffer bar must
/// reach the track end instead of remaining tied to the decoded-ahead window.
#[kithara::test(tokio, timeout(Duration::from_secs(90)))]
async fn progressive_download_fills_the_buffer_bar(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let spec = SignalSpec {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        length: SignalSpecLength::Frames(STREAM_FRAMES),
        format: SignalFormat::Mp3,
        bit_rate: None,
    };
    let url = helper.sine(&spec, FREQ_HZ).await;

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
    let tick_handle = spawn_ticker(Arc::clone(&queue));

    let cfg = ResourceConfig::for_src(url.as_str())
        .expect("valid URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader)
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();

    let mut rx = queue.subscribe();
    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    let _ = queue.select(id, Transition::None);
    wait_for_loaded(&mut rx, &queue, id, Duration::from_secs(60))
        .await
        .unwrap_or_else(|error| panic!("precondition: {error}"));

    let third = play_to_first_third(&queue, Duration::from_secs(30)).await;
    let third_position = third
        .position
        .expect("precondition: playback position must become known");
    let third_duration = third
        .duration
        .expect("precondition: duration must become known");
    assert!(
        third_position >= third_duration / 3.0,
        "precondition: playback reached only {third_position:.2}s of {third_duration:.2}s"
    );
    queue.pause();

    let mut best = queue.playback_view();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        time::sleep(Duration::from_millis(250)).await;
        let view = queue.playback_view();
        if view.buffered.unwrap_or(0.0) > best.buffered.unwrap_or(0.0) {
            best = view;
        }
        if let (Some(buffered), Some(duration)) = (best.buffered, best.duration)
            && buffered >= duration * BUFFER_MUST_REACH_FRACTION
        {
            break;
        }
    }

    let duration = best
        .duration
        .expect("precondition: duration must be known before judging the buffer bar");
    let buffered = best.buffered.unwrap_or(0.0);
    let position = best.position.unwrap_or(0.0);
    assert!(
        buffered >= duration * BUFFER_MUST_REACH_FRACTION,
        "LABA-430: on a fully-downloaded progressive track the buffer bar \
         reached only {buffered:.2}s of {duration:.2}s (position {position:.2}s); \
         the decoded-ahead frontier cannot represent download progress"
    );

    tick_handle.abort();
}
