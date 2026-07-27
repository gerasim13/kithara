#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashSet;

use kithara::{
    events::{AudioEvent, DownloaderEvent, Event, QueueEvent, RequestId, RequestMethod},
    net::{HttpClient, NetOptions},
    platform::{CancelToken, sync::Arc, time::Duration},
    play::{PlayerConfig, PlayerImpl, ResourceConfig, SessionDispatcher},
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    BehaviorHandle, Content, Delivery, FixtureBehavior, TestServerHelper, TestTempDir,
    audio_fixture::EmbeddedAudio, kithara, offline::OfflineSession, temp_dir,
    waits::wait_for_loader_done_event,
};

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const TRACK_COUNT: usize = 3;
const STORM_ROUNDS: usize = 12;
const APPLY_BLOCKS: usize = 1_024;
const SEEK_BLOCKS: usize = 1_024;
const PAUSE_BLOCKS: usize = 128;
const SEEK_TARGET_SECS: f64 = 5.0;
const SEEK_TOLERANCE_SECS: f64 = 1.0;

fn render_and_tick(session: &OfflineSession, queue: &Queue) {
    let _ = session.render(BLOCK_FRAMES);
    queue.tick().expect("tick queue");
}

fn resource_config(
    handle: &BehaviorHandle,
    downloader: &Downloader,
    temp_dir: &TestTempDir,
    index: usize,
) -> ResourceConfig {
    let url = handle.child_url(&format!("storm-{index}.mp3"));
    ResourceConfig::for_src(url.as_str())
        .expect("valid fixture URL")
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .downloader(downloader.clone())
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build()
}

fn drain_active_gets(rx: &mut kithara::events::EventReceiver, active: &mut HashSet<RequestId>) {
    while let Ok(envelope) = rx.try_recv() {
        match envelope.event {
            Event::Downloader(DownloaderEvent::RequestEnqueued {
                request_id,
                method: RequestMethod::Get,
                ..
            }) => {
                active.insert(request_id);
            }
            Event::Downloader(DownloaderEvent::RequestCompleted { request_id, .. })
            | Event::Downloader(DownloaderEvent::RequestFailed { request_id, .. })
            | Event::Downloader(DownloaderEvent::RetryExhausted { request_id, .. })
            | Event::Downloader(DownloaderEvent::RequestCancelled { request_id, .. }) => {
                active.remove(&request_id);
            }
            _ => {}
        }
    }
}

fn drain_seek_progress(
    rx: &mut kithara::events::EventReceiver,
    target: f64,
    landed: &mut Option<f64>,
) {
    while let Ok(envelope) = rx.try_recv() {
        match envelope.event {
            Event::Audio(AudioEvent::SeekComplete { position, .. }) => {
                *landed = Some(position.as_secs_f64());
            }
            Event::Audio(AudioEvent::PlaybackProgress { position_ms, .. }) => {
                let position = position_ms as f64 / 1000.0;
                if (position - target).abs() < SEEK_TOLERANCE_SECS {
                    *landed = Some(position);
                }
            }
            _ => {}
        }
    }
}

fn drain_post_pause_progress(rx: &mut kithara::events::EventReceiver, latest: &mut Option<f64>) {
    while let Ok(envelope) = rx.try_recv() {
        if let Event::Audio(AudioEvent::PlaybackProgress { position_ms, .. }) = envelope.event {
            *latest = Some(position_ms as f64 / 1000.0);
        }
    }
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(120)))]
async fn commands_still_work_after_a_switch_storm(temp_dir: TestTempDir) {
    let helper = TestServerHelper::new().await;
    let handles: Vec<_> = (0..TRACK_COUNT)
        .map(|_| {
            helper.register_behavior(FixtureBehavior {
                content: Content::StaticBytes {
                    bytes: Arc::new(EmbeddedAudio::TEST_MP3_BYTES.to_vec()),
                    content_type: Some("audio/mpeg"),
                },
                delivery: Delivery::Throttle {
                    chunk: 4 * 1024,
                    delay_ms: 20,
                },
            })
        })
        .collect();
    let downloader = Downloader::new(
        DownloaderConfig::builder()
            .client(HttpClient::new(NetOptions::default(), CancelToken::never()))
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
    let mut status_rx = queue.subscribe();
    let mut probe_rx = queue.subscribe();

    let mut ids = Vec::with_capacity(TRACK_COUNT);
    for (index, handle) in handles.iter().enumerate() {
        let id = queue.append(TrackSource::Config(Box::new(resource_config(
            handle,
            &downloader,
            &temp_dir,
            index,
        ))));
        wait_for_loader_done_event(&mut status_rx, &queue, id, Duration::from_secs(30))
            .await
            .unwrap_or_else(|error| panic!("LABA-425 precondition: track {index}: {error}"));
        ids.push(id);
    }

    let mut active_gets = HashSet::new();
    drain_active_gets(&mut probe_rx, &mut active_gets);
    assert!(
        !active_gets.is_empty(),
        "LABA-425 precondition: all throttled GETs completed before the switch storm; \
         the in-flight scenario did not happen"
    );
    assert!(
        handles.iter().all(|handle| handle.request_count() > 0),
        "LABA-425 precondition: at least one throttled fixture was never requested"
    );

    let mut applied_switches = 0usize;
    for round in 0..STORM_ROUNDS {
        let target = ids[round % ids.len()];
        queue
            .select(target, Transition::None)
            .unwrap_or_else(|error| panic!("LABA-425: switch {round} was rejected: {error}"));
        let mut current_changed = false;
        for _ in 0..APPLY_BLOCKS {
            render_and_tick(&session, &queue);
            while let Ok(envelope) = status_rx.try_recv() {
                if matches!(
                    envelope.event,
                    Event::Queue(QueueEvent::CurrentTrackChanged { id: Some(id) }) if id == target
                ) {
                    current_changed = true;
                }
            }
            if queue.current().is_some_and(|entry| entry.id == target) {
                current_changed = true;
                break;
            }
        }
        assert!(
            current_changed,
            "LABA-425: switch {round} never made track {target:?} current"
        );
        applied_switches += 1;

        let seek_step = u32::try_from(round).expect("storm round fits u32");
        queue
            .seek(f64::from(seek_step) * 0.25)
            .unwrap_or_else(|error| panic!("LABA-425: storm seek {round} failed: {error}"));
        queue.pause();
        queue.play();
        render_and_tick(&session, &queue);
        drain_active_gets(&mut probe_rx, &mut active_gets);
    }
    assert_eq!(
        applied_switches, STORM_ROUNDS,
        "LABA-425 precondition: switch storm did not complete"
    );

    while status_rx.try_recv().is_ok() {}
    queue
        .seek(SEEK_TARGET_SECS)
        .unwrap_or_else(|error| panic!("LABA-425: final seek failed: {error}"));
    let mut landed = None;
    for _ in 0..SEEK_BLOCKS {
        render_and_tick(&session, &queue);
        drain_seek_progress(&mut status_rx, SEEK_TARGET_SECS, &mut landed);
        if landed.is_some() {
            break;
        }
    }
    let landed = landed.unwrap_or_else(|| {
        panic!(
            "LABA-425: no sink-truth seek completion reached {SEEK_TARGET_SECS:.1}s \
             after the switch storm"
        )
    });
    assert!(
        (landed - SEEK_TARGET_SECS).abs() < SEEK_TOLERANCE_SECS,
        "LABA-425: seek to {SEEK_TARGET_SECS:.1}s landed at {landed:.3}s after the switch storm"
    );

    while status_rx.try_recv().is_ok() {}
    queue.pause();
    let mut post_pause = None;
    for _ in 0..PAUSE_BLOCKS {
        render_and_tick(&session, &queue);
        drain_post_pause_progress(&mut status_rx, &mut post_pause);
    }
    let post_pause = post_pause.unwrap_or(landed);
    assert!(
        post_pause <= landed + 0.05,
        "LABA-425: sink progress kept moving after pause following the switch storm \
         ({landed:.3}s -> {post_pause:.3}s); the command was swallowed"
    );

    queue.clear();
}
