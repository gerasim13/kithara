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
const STREAM_FRAMES: usize = 44_100 * 30;
const FREQUENCIES_HZ: [f64; 3] = [440.0, 554.37, 659.25];
const STORM_ROUNDS: usize = 12;
const SEEK_TARGET_SECS: f64 = 5.0;

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

async fn append_and_load(
    helper: &TestServerHelper,
    queue: &Queue,
    rx: &mut EventReceiver,
    downloader: &Downloader,
    temp_dir: &TestTempDir,
    track_index: usize,
) -> TrackId {
    let spec = SignalSpec {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        length: SignalSpecLength::Frames(STREAM_FRAMES),
        format: SignalFormat::Mp3,
        bit_rate: None,
    };
    let url = helper.sine(&spec, FREQUENCIES_HZ[track_index]).await;
    let cfg = ResourceConfig::for_src(url.as_str())
        .expect("valid URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader.clone())
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();
    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    let _ = queue.select(id, Transition::None);
    wait_for_loaded(rx, queue, id, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("precondition: {error}"));
    id
}

/// LABA-425: pause and seek must remain effective after repeated track
/// selections, seeks, and transport toggles.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn commands_still_work_after_a_switch_storm(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
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
    let mut rx = queue.subscribe();

    let mut ids = Vec::with_capacity(FREQUENCIES_HZ.len());
    for track_index in 0..FREQUENCIES_HZ.len() {
        ids.push(
            append_and_load(
                &helper,
                &queue,
                &mut rx,
                &downloader,
                &temp_dir,
                track_index,
            )
            .await,
        );
    }

    for round in 0..STORM_ROUNDS {
        let target = round % ids.len();
        let _ = queue.select(ids[target], Transition::None);
        let seek_step = u32::try_from(round).expect("storm round fits u32");
        let _ = queue.seek(f64::from(seek_step) * 0.5);
        queue.pause();
        queue.play();
        time::sleep(Duration::from_millis(80)).await;
    }

    let _ = queue.seek(SEEK_TARGET_SECS);
    time::sleep(Duration::from_millis(500)).await;
    let after_seek = queue.playback_view().position.unwrap_or(0.0);
    assert!(
        (after_seek - SEEK_TARGET_SECS).abs() < 2.0,
        "LABA-425: seek to {SEEK_TARGET_SECS:.1}s landed at {after_seek:.2}s \
         after a switch storm"
    );

    queue.pause();
    time::sleep(Duration::from_millis(300)).await;
    let paused_at = queue.playback_view().position.unwrap_or(0.0);
    time::sleep(Duration::from_millis(700)).await;
    let still_at = queue.playback_view().position.unwrap_or(0.0);
    assert!(
        (still_at - paused_at).abs() < 0.05,
        "LABA-425: position kept moving after pause() \
         ({paused_at:.3}s -> {still_at:.3}s) -- the command was swallowed"
    );

    tick_handle.abort();
}
