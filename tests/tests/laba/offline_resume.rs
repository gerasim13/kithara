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
const FREQ_HZ: f64 = 440.0;
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

/// LABA-419: loading must resume automatically after global network loss and
/// recovery instead of leaving the selected track permanently unavailable.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn playback_resumes_after_network_returns(temp_dir: TestTempDir) {
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
            3,
            Duration::from_millis(10),
            Duration::from_millis(200),
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
    let _ = queue.select(id, Transition::None);

    helper.set_network_online(false);
    time::sleep(Duration::from_secs(2)).await;
    helper.set_network_online(true);

    wait_for_loaded(&mut rx, &queue, id, Duration::from_secs(60))
        .await
        .unwrap_or_else(|error| {
            panic!("LABA-419: {error} -- loading never resumed after the network returned")
        });

    tick_handle.abort();
}
