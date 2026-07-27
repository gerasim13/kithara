#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{Event, EventReceiver, QueueEvent, TrackId, TrackStatus},
    net::{HttpClient, NetOptions, RetryPolicy},
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
const FREQ_HZ: f64 = 523.25;
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

#[derive(Debug)]
enum Terminal {
    Loaded,
    Failed(String),
    Timeout,
}

async fn wait_for_terminal(
    rx: &mut EventReceiver,
    queue: &Queue,
    id: TrackId,
    deadline: Duration,
) -> Terminal {
    if let Some(entry) = queue.track(id) {
        match &entry.status {
            TrackStatus::Loaded => return Terminal::Loaded,
            TrackStatus::Failed(error) => return Terminal::Failed(error.clone()),
            _ => {}
        }
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
                TrackStatus::Loaded => return Terminal::Loaded,
                TrackStatus::Failed(error) => return Terminal::Failed(error),
                _ => {}
            }
        }
    }
    Terminal::Timeout
}

/// LABA-428: a temporary network failure must not leave a track permanently
/// failed after connectivity returns and the same track is selected again.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn transient_failure_does_not_kill_the_track(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let spec = SignalSpec {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        length: SignalSpecLength::Frames(STREAM_FRAMES),
        format: SignalFormat::Mp3,
        bit_rate: None,
    };
    let url = helper.sine(&spec, FREQ_HZ).await;

    let net = NetOptions::builder()
        .inactivity_timeout(Duration::from_millis(500))
        .retry_policy(RetryPolicy::new(
            1,
            Duration::from_millis(10),
            Duration::from_millis(50),
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

    helper.set_network_online(false);
    let _ = queue.select(id, Transition::None);
    let first = wait_for_terminal(&mut rx, &queue, id, Duration::from_secs(30)).await;
    assert!(
        matches!(&first, Terminal::Failed(error) if !error.is_empty()),
        "precondition: the first attempt on a dead network must fail, got {first:?}"
    );

    helper.set_network_online(true);
    let _ = queue.select(id, Transition::None);
    let second = wait_for_terminal(&mut rx, &queue, id, Duration::from_secs(60)).await;

    assert!(
        matches!(second, Terminal::Loaded),
        "LABA-428: after the network returned the track must load again, got {second:?}"
    );

    tick_handle.abort();
}
