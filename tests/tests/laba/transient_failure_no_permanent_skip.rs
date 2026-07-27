#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{AudioEvent, DownloaderEvent, Event, QueueEvent, TrackId},
    net::{HttpClient, NetOptions, RetryPolicy},
    platform::{CancelToken, sync::Arc, time::Duration},
    play::{PlayerConfig, PlayerImpl, ResourceConfig, SessionDispatcher},
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, HlsFixtureBuilder, TestServerHelper, TestTempDir,
    audio_fixture::EmbeddedAudio, kithara, offline::OfflineSession, temp_dir,
    waits::wait_for_loader_done_event,
};

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const SEGMENT_COUNT: usize = 6;
const SEGMENT_DURATION_SECS: f64 = 2.0;
const RELEASE_SEGMENT: usize = 2;
const MIN_PLAYED_BEFORE_FAILURE_SECS: f64 = 2.0;
const MIN_RECOVERY_PROGRESS_SECS: f64 = 1.0;
const SETUP_BLOCKS: usize = 2_048;
const FAILURE_BLOCKS: usize = 2_048;
const RECOVERY_BLOCKS: usize = 2_048;

struct NetworkRestore<'a>(&'a TestServerHelper);

impl Drop for NetworkRestore<'_> {
    fn drop(&mut self) {
        self.0.set_network_online(true);
    }
}

#[derive(Default)]
struct Observation {
    auto_skipped: bool,
    latest_position: Option<f64>,
    saw_offline_failure: bool,
}

fn render_and_tick(session: &OfflineSession, queue: &Queue) {
    let _ = session.render(BLOCK_FRAMES);
    queue.tick().expect("tick queue");
}

fn drain_events(
    rx: &mut kithara::events::EventReceiver,
    target: TrackId,
    fallback: TrackId,
    observation: &mut Observation,
) {
    while let Ok(envelope) = rx.try_recv() {
        match envelope.event {
            Event::Audio(AudioEvent::PlaybackProgress { position_ms, .. }) => {
                observation.latest_position = Some(position_ms as f64 / 1000.0);
            }
            Event::Downloader(DownloaderEvent::FirstByte { status: 503, .. })
            | Event::Downloader(DownloaderEvent::RequestFailed { .. })
            | Event::Downloader(DownloaderEvent::RetryExhausted { .. }) => {
                observation.saw_offline_failure = true;
            }
            Event::Queue(QueueEvent::TrackLoadFailed {
                id,
                auto_skipped: true,
                ..
            }) if id == target => {
                observation.auto_skipped = true;
            }
            Event::Queue(QueueEvent::CurrentTrackChanged { id: Some(id) }) if id == fallback => {
                observation.auto_skipped = true;
            }
            _ => {}
        }
    }
}

fn resource_config(
    url: &url::Url,
    downloader: &Downloader,
    temp_dir: &TestTempDir,
) -> ResourceConfig {
    ResourceConfig::for_src(url.as_str())
        .expect("valid fixture URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader.clone())
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build()
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(120)))]
async fn transient_failure_does_not_kill_the_track(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let target_fixture = helper
        .create_hls(
            HlsFixtureBuilder::new()
                .variant_count(1)
                .segments_per_variant(SEGMENT_COUNT)
                .segment_duration_secs(SEGMENT_DURATION_SECS)
                .packaged_audio_aac_lc(SAMPLE_RATE, 2),
        )
        .await
        .expect("create HLS fixture");
    let release_gate = helper.register_segment_gate(target_fixture.token(), 0, RELEASE_SEGMENT);
    let fallback_fixture = helper.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(EmbeddedAudio::TEST_MP3_BYTES.to_vec()),
            content_type: Some("audio/mpeg"),
        },
        delivery: Delivery::Range,
    });
    let fallback_url = fallback_fixture.child_url("fallback.mp3");

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
    let mut rx = queue.subscribe();

    let target = queue.append(TrackSource::Config(Box::new(resource_config(
        &target_fixture.master_url(),
        &downloader,
        &temp_dir,
    ))));
    let fallback = queue.append(TrackSource::Config(Box::new(resource_config(
        &fallback_url,
        &downloader,
        &temp_dir,
    ))));
    wait_for_loader_done_event(&mut rx, &queue, fallback, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("LABA-428 precondition: fallback load failed: {error}"));
    queue
        .select(target, Transition::None)
        .expect("select target track");
    wait_for_loader_done_event(&mut rx, &queue, target, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("LABA-428 precondition: target load failed: {error}"));

    // Without this the engine stays paused, `render` returns silence, and the
    // setup below pulls audio that never becomes playback progress.
    queue.play();

    let mut observation = Observation::default();
    for _ in 0..SETUP_BLOCKS {
        render_and_tick(&session, &queue);
        drain_events(&mut rx, target, fallback, &mut observation);
        if release_gate.requested() > 0
            && observation
                .latest_position
                .is_some_and(|position| position >= MIN_PLAYED_BEFORE_FAILURE_SECS)
        {
            break;
        }
    }
    let before_failure = observation.latest_position.unwrap_or(0.0);
    assert!(
        before_failure >= MIN_PLAYED_BEFORE_FAILURE_SECS,
        "LABA-428 precondition: target played only {before_failure:.3}s before fault injection"
    );
    assert!(
        release_gate.requested() > 0,
        "LABA-428 precondition: segment {RELEASE_SEGMENT} never reached the withhold gate; \
         no mid-stream fault window was established"
    );
    assert!(
        !observation.auto_skipped,
        "LABA-428 precondition: target was skipped before the injected failure"
    );

    helper.set_network_online(false);
    let _network_restore = NetworkRestore(&helper);
    release_gate.release();
    for _ in 0..FAILURE_BLOCKS {
        render_and_tick(&session, &queue);
        drain_events(&mut rx, target, fallback, &mut observation);
        if observation.saw_offline_failure {
            break;
        }
    }
    helper.set_network_online(true);
    let failure_position = observation.latest_position.unwrap_or(before_failure);
    assert!(
        observation.saw_offline_failure,
        "LABA-428 precondition: no later segment fetch observed the offline server; \
         the failure did not land mid-stream"
    );
    assert!(
        failure_position >= MIN_PLAYED_BEFORE_FAILURE_SECS,
        "LABA-428 precondition: fetch failed before two seconds of playback \
         ({failure_position:.3}s)"
    );

    let recovery_target = failure_position + MIN_RECOVERY_PROGRESS_SECS;
    for _ in 0..RECOVERY_BLOCKS {
        render_and_tick(&session, &queue);
        drain_events(&mut rx, target, fallback, &mut observation);
        if observation.auto_skipped
            || observation
                .latest_position
                .is_some_and(|position| position >= recovery_target)
        {
            break;
        }
    }
    let recovered_at = observation.latest_position.unwrap_or(failure_position);
    assert!(
        !observation.auto_skipped,
        "LABA-428: the current track was permanently failed and auto-skipped after a \
         mid-stream transient failure at {failure_position:.3}s"
    );
    assert!(
        recovered_at >= recovery_target,
        "LABA-428: connectivity returned after a mid-stream failure, but target playback \
         stayed at {recovered_at:.3}s instead of reaching {recovery_target:.3}s"
    );

    queue.clear();
}
