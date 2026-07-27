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
const FREQ_HZ: f64 = 783.99;
const STREAM_FRAMES: usize = 44_100 * 30;

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

async fn wait_for_duration(queue: &Queue, deadline: Duration) -> Option<f64> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Some(duration) = queue.playback_view().duration
            && duration > 0.0
        {
            return Some(duration);
        }
        time::sleep(Duration::from_millis(100)).await;
    }
    queue.playback_view().duration
}

/// LABA-417: seeking to the duration boundary must preserve a coherent
/// duration and must never publish a position beyond that duration.
#[kithara::test(tokio, timeout(Duration::from_secs(60)))]
async fn seek_to_duration_keeps_time_and_duration_consistent(temp_dir: TestTempDir) {
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
    wait_for_loaded(&mut rx, &queue, id, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("precondition: {error}"));

    let duration = wait_for_duration(&queue, Duration::from_secs(10))
        .await
        .expect("LABA-417 precondition: duration must be known before seeking");

    for target in [duration - 0.05, duration] {
        let outcome = queue.seek(target);
        assert!(
            outcome.is_ok(),
            "LABA-417: seek to {target:.3}s was rejected: {:?}",
            outcome.err()
        );
        time::sleep(Duration::from_millis(200)).await;

        let view = queue.playback_view();
        let after = view
            .duration
            .expect("duration must survive a seek to the end");
        assert!(
            (after - duration).abs() < 0.01,
            "LABA-417: duration changed across a seek to {target:.3}s \
             ({duration:.3}s -> {after:.3}s)"
        );

        let position = view.position.unwrap_or(0.0);
        assert!(
            position <= duration + 0.01,
            "LABA-417: position {position:.3}s exceeded duration {duration:.3}s \
             after seeking to {target:.3}s"
        );
    }

    tick_handle.abort();
}
