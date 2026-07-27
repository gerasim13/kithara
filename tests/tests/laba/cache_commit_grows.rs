#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

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
const STREAM_FRAMES: usize = 44_100 * 10;
const FREQUENCIES_HZ: [f64; 2] = [440.0, 554.37];

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
    cache_root: &Path,
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
        .store(kithara_integration_tests::disk_asset_store(cache_root))
        .build();
    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    let _ = queue.select(id, Transition::None);
    wait_for_loaded(rx, queue, id, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("precondition: {error}"));
    id
}

async fn wait_for_cache_growth(root: &Path, previous: u64, deadline: Duration) -> u64 {
    let start = Instant::now();
    let mut largest = dir_size_bytes(root);
    while largest <= previous && start.elapsed() < deadline {
        time::sleep(Duration::from_millis(100)).await;
        largest = largest.max(dir_size_bytes(root));
    }
    largest
}

fn dir_size_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path()));
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    total
}

/// LABA-413: playing distinct tracks through a disk-backed store must commit
/// bytes below the configured cache root, and the second track must grow it.
#[kithara::test(tokio, timeout(Duration::from_secs(90)))]
async fn played_tracks_land_in_the_disk_cache(temp_dir: TestTempDir) {
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

    let before = dir_size_bytes(temp_dir.path());
    let _first = append_and_load(&helper, &queue, &mut rx, &downloader, temp_dir.path(), 0).await;
    let after_first = wait_for_cache_growth(temp_dir.path(), before, Duration::from_secs(20)).await;
    assert!(
        after_first > before,
        "LABA-413: cache root did not grow after playing a track \
         ({before} -> {after_first} bytes, root: {})",
        temp_dir.path().display()
    );

    let _second = append_and_load(&helper, &queue, &mut rx, &downloader, temp_dir.path(), 1).await;
    let after_second =
        wait_for_cache_growth(temp_dir.path(), after_first, Duration::from_secs(20)).await;
    assert!(
        after_second > after_first,
        "LABA-413: cache did not grow with the second track \
         ({after_first} -> {after_second} bytes)"
    );

    tick_handle.abort();
}
