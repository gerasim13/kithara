#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{AudioEvent, DownloaderEvent, Event},
    net::{HttpClient, NetOptions, RetryPolicy},
    platform::{CancelToken, sync::Arc, time::Duration},
    play::{PlayerConfig, PlayerImpl, ResourceConfig, SessionDispatcher},
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    HlsFixtureBuilder, TestServerHelper, TestTempDir, kithara, offline::OfflineSession, temp_dir,
    waits::wait_for_loader_done_event,
};

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const SEGMENT_COUNT: usize = 6;
const SEGMENT_DURATION_SECS: f64 = 2.0;
const OUTAGE_RELEASE_SEGMENT: usize = 2;
const RECOVERY_GATED_SEGMENT: usize = SEGMENT_COUNT - 1;
const WARMUP_BLOCKS: usize = 2_048;
const OUTAGE_BLOCKS: usize = 2_048;
const RECOVERY_BLOCKS: usize = 2_048;
const PAUSE_BLOCKS: usize = 64;
const MIN_PROGRESS_SECS: f64 = 0.5;

struct NetworkRestore<'a>(&'a TestServerHelper);

impl Drop for NetworkRestore<'_> {
    fn drop(&mut self) {
        self.0.set_network_online(true);
    }
}

fn render_and_tick(session: &OfflineSession, queue: &Queue) {
    let _ = session.render(BLOCK_FRAMES);
    queue.tick().expect("tick queue");
}

fn drain_progress(rx: &mut kithara::events::EventReceiver, latest: &mut Option<f64>) -> bool {
    let mut saw_offline_failure = false;
    while let Ok(envelope) = rx.try_recv() {
        match envelope.event {
            Event::Audio(AudioEvent::PlaybackProgress { position_ms, .. }) => {
                *latest = Some(position_ms as f64 / 1000.0);
            }
            Event::Downloader(DownloaderEvent::FirstByte { status: 503, .. })
            | Event::Downloader(DownloaderEvent::RequestFailed { .. })
            | Event::Downloader(DownloaderEvent::RetryExhausted { .. }) => {
                saw_offline_failure = true;
            }
            _ => {}
        }
    }
    saw_offline_failure
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(120)))]
async fn playback_resumes_after_network_returns(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let fixture = helper
        .create_hls(
            HlsFixtureBuilder::new()
                .variant_count(1)
                .segments_per_variant(SEGMENT_COUNT)
                .segment_duration_secs(SEGMENT_DURATION_SECS)
                .packaged_audio_aac_lc(SAMPLE_RATE, 2),
        )
        .await
        .expect("create HLS fixture");
    let outage_gate = helper.register_segment_gate(fixture.token(), 0, OUTAGE_RELEASE_SEGMENT);
    let recovery_gate = helper.register_segment_gate(fixture.token(), 0, RECOVERY_GATED_SEGMENT);

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
    let session = Arc::new(OfflineSession::new_manual());
    let player = Arc::new(PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(SAMPLE_RATE)
            .byte_pool(kithara::bufpool::BytePool::default())
            .pcm_pool(kithara::bufpool::PcmPool::default())
            .session(Arc::clone(&session) as Arc<dyn SessionDispatcher>)
            .build(),
    ));
    let queue = Queue::new(QueueConfig::default().with_player(player));
    let cfg = ResourceConfig::for_src(fixture.master_url().as_str())
        .expect("valid HLS URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader)
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();

    let mut rx = queue.subscribe();
    let id = queue.append(TrackSource::Config(Box::new(cfg)));
    queue
        .select(id, Transition::None)
        .expect("select HLS track");
    wait_for_loader_done_event(&mut rx, &queue, id, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("LABA-419 precondition: {error}"));

    let mut latest = None;
    for _ in 0..WARMUP_BLOCKS {
        render_and_tick(&session, &queue);
        let _ = drain_progress(&mut rx, &mut latest);
        if latest.is_some_and(|position| position >= MIN_PROGRESS_SECS)
            && outage_gate.requested() > 0
        {
            break;
        }
    }
    let before_outage = latest.unwrap_or(0.0);
    assert!(
        before_outage >= MIN_PROGRESS_SECS,
        "LABA-419 precondition: playback produced only {before_outage:.3}s before the outage"
    );
    assert!(
        outage_gate.requested() > 0,
        "LABA-419 precondition: segment {OUTAGE_RELEASE_SEGMENT} never reached the withhold gate; \
         no download-in-flight window was established"
    );
    assert_eq!(
        recovery_gate.requested(),
        0,
        "LABA-419 precondition: recovery segment {RECOVERY_GATED_SEGMENT} was already requested \
         before the outage; no later fetch remained to observe the offline server"
    );

    helper.set_network_online(false);
    let _network_restore = NetworkRestore(&helper);
    outage_gate.release();
    let mut saw_offline_failure = false;
    for _ in 0..OUTAGE_BLOCKS {
        render_and_tick(&session, &queue);
        saw_offline_failure |= drain_progress(&mut rx, &mut latest);
        if saw_offline_failure {
            break;
        }
    }
    helper.set_network_online(true);
    assert!(
        saw_offline_failure,
        "LABA-419 precondition: no fetch observed the offline server after playback began; \
         the outage scenario did not happen"
    );

    let mut request_resumed = false;
    for _ in 0..RECOVERY_BLOCKS {
        render_and_tick(&session, &queue);
        let _ = drain_progress(&mut rx, &mut latest);
        if recovery_gate.requested() > 0 {
            request_resumed = true;
            break;
        }
    }
    let recovery_anchor = latest.unwrap_or(before_outage);
    recovery_gate.release();
    assert!(
        request_resumed,
        "LABA-419: no request reached future segment {RECOVERY_GATED_SEGMENT} after the network \
         returned; the downloader did not resume"
    );

    let mut resumed_at = recovery_anchor;
    for _ in 0..RECOVERY_BLOCKS {
        render_and_tick(&session, &queue);
        let _ = drain_progress(&mut rx, &mut latest);
        resumed_at = latest.unwrap_or(resumed_at);
        if resumed_at >= recovery_anchor + MIN_PROGRESS_SECS {
            break;
        }
    }
    assert!(
        resumed_at >= recovery_anchor + MIN_PROGRESS_SECS,
        "LABA-419: bytes resumed after the network returned but playback stayed at \
         {resumed_at:.3}s (recovery anchor {recovery_anchor:.3}s)"
    );

    while rx.try_recv().is_ok() {}
    queue.pause();
    let paused_at = resumed_at;
    let mut after_pause = paused_at;
    for _ in 0..PAUSE_BLOCKS {
        render_and_tick(&session, &queue);
        let _ = drain_progress(&mut rx, &mut latest);
        after_pause = latest.unwrap_or(after_pause);
    }
    assert!(
        after_pause <= paused_at + 0.05,
        "LABA-419: pause was ineffective after recovery; sink progress moved from \
         {paused_at:.3}s to {after_pause:.3}s"
    );

    queue.play();
    let mut after_play = after_pause;
    for _ in 0..RECOVERY_BLOCKS {
        render_and_tick(&session, &queue);
        let _ = drain_progress(&mut rx, &mut latest);
        after_play = latest.unwrap_or(after_play);
        if after_play >= after_pause + MIN_PROGRESS_SECS {
            break;
        }
    }
    assert!(
        after_play >= after_pause + MIN_PROGRESS_SECS,
        "LABA-419: play was ineffective after recovery; sink progress stayed at \
         {after_play:.3}s"
    );

    queue.clear();
}
