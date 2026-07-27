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
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    SignalFormat, SignalSpec, SignalSpecLength, TestServerHelper, TestTempDir, kithara,
    offline::OfflineSession, temp_dir,
};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const FREQ_HZ: f64 = 880.0;
const STREAM_FRAMES: usize = 44_100 * 30;
const SAVE_AFTER_SECS: f64 = 4.0;

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

fn new_queue() -> Arc<Queue> {
    let player = Arc::new(PlayerImpl::new(
        PlayerConfig::builder()
            .byte_pool(kithara::bufpool::BytePool::default())
            .pcm_pool(kithara::bufpool::PcmPool::default())
            .session(OfflineSession::arc_auto())
            .build(),
    ));
    Arc::new(Queue::new(QueueConfig::default().with_player(player)))
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

async fn wait_for_position(queue: &Queue, target: f64, deadline: Duration) -> Option<f64> {
    let start = Instant::now();
    let mut latest = queue.position_seconds();
    while latest.unwrap_or(0.0) < target && start.elapsed() < deadline {
        time::sleep(Duration::from_millis(100)).await;
        latest = queue.position_seconds();
    }
    latest
}

fn append_track(
    queue: &Queue,
    url: &str,
    downloader: &Downloader,
    temp_dir: &TestTempDir,
) -> TrackId {
    let cfg = ResourceConfig::for_src(url)
        .expect("valid URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader.clone())
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();
    queue.append(TrackSource::Config(Box::new(cfg)))
}

/// LABA-408: a position read while paused must remain a valid seek target for
/// a new player, and playback must not restart below that saved position.
#[kithara::test(tokio, timeout(Duration::from_secs(90)))]
async fn playback_starts_from_the_seeked_position(temp_dir: TestTempDir) {
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

    let first_queue = new_queue();
    let first_tick = spawn_ticker(Arc::clone(&first_queue));
    let mut first_rx = first_queue.subscribe();
    let first_id = append_track(&first_queue, url.as_str(), &downloader, &temp_dir);
    let _ = first_queue.select(first_id, Transition::None);
    wait_for_loaded(
        &mut first_rx,
        &first_queue,
        first_id,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|error| panic!("precondition: {error}"));
    let played = wait_for_position(&first_queue, SAVE_AFTER_SECS, Duration::from_secs(15))
        .await
        .expect("LABA-408 precondition: position must become known");
    assert!(
        played >= SAVE_AFTER_SECS,
        "LABA-408 precondition: playback reached only {played:.2}s"
    );
    first_queue.pause();
    let saved = first_queue.playback_view().position.unwrap_or(0.0);
    assert!(
        saved >= SAVE_AFTER_SECS,
        "LABA-408 precondition: paused position must remain non-zero, got {saved:.2}s"
    );
    first_tick.abort();

    let second_queue = new_queue();
    let second_tick = spawn_ticker(Arc::clone(&second_queue));
    let mut second_rx = second_queue.subscribe();
    let second_id = append_track(&second_queue, url.as_str(), &downloader, &temp_dir);
    let _ = second_queue.select(second_id, Transition::None);
    wait_for_loaded(
        &mut second_rx,
        &second_queue,
        second_id,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|error| panic!("precondition: {error}"));
    second_queue.pause();
    let outcome = second_queue.seek(saved);
    assert!(
        outcome.is_ok(),
        "LABA-408 precondition: seek to saved position {saved:.2}s failed: {:?}",
        outcome.err()
    );
    second_queue.play();

    let mut minimum = f64::MAX;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        time::sleep(Duration::from_millis(100)).await;
        let position = second_queue.playback_view().position.unwrap_or(0.0);
        if position > 0.0 {
            minimum = minimum.min(position);
        }
    }
    assert!(
        minimum >= saved - 1.0,
        "LABA-408: playback fell back to {minimum:.2}s after seeking to \
         saved position {saved:.2}s"
    );

    second_tick.abort();
}
