#![cfg(not(target_arch = "wasm32"))]

use std::{fs, path::PathBuf};

use kithara::{
    assets::{AssetStore, StorageBackend},
    events::{AudioEvent, DownloaderEvent, Event},
    hls::AbrMode,
    net::{HttpClient, NetOptions, RetryPolicy},
    platform::{
        CancelToken,
        sync::Arc,
        time::{self, Duration},
        tokio,
    },
    play::{PlayerConfig, PlayerImpl, ResourceConfig},
    queue::{Queue, QueueConfig, TrackSource, Transition},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, PrivateTestServer, TestTempDir, kithara,
    offline::OfflineSession,
    temp_dir,
    waits::{wait_for_event, wait_for_loader_done_event, wait_for_position_event},
};

/// Playback failing to resume once connectivity returns does not reproduce in
/// the core: with connectivity restored the engine resumes on its own. This
/// stays as the contract that keeps it that way, and as the alibi placing the
/// reported bug above the Rust layer.
/// Verified to have teeth — leaving the server offline pins playback at the
/// starvation point instead of advancing.
///
/// The packaged `hls/` fixture is a 37-segment, 222 s variant whose slq
/// segments are ~50 KiB. A track far longer than the look-ahead window is
/// what makes the outage observable at all: with a short fixture the
/// downloader finishes the whole variant over loopback before playback
/// reaches its first second, and taking the network down afterwards is a
/// no-op no product could react to.
///
/// The window is deliberately narrow: the outage only becomes observable once
/// the cached bytes drain, and draining is paced by real playback. At 256 KiB
/// the drain outlasted the wait on roughly one run in five.
const LOOK_AHEAD_BYTES: u64 = 64 * 1024;
/// Wide enough that an outage catches several fetches mid-body, which is
/// what a device does and what the narrow window above deliberately avoids.
const PACED_LOOK_AHEAD_BYTES: u64 = 1024 * 1024;
/// ~12 KiB/s: the slq variant carries ~50 KiB per 5 s of audio, so the
/// downloader gains on playback slowly instead of racing to the end.
const SEGMENT_CHUNK_BYTES: usize = 3 * 1024;
const SEGMENT_CHUNK_DELAY_MS: u64 = 250;
const PLAY_BEFORE_OUTAGE_SECS: f64 = 1.0;
const MIN_RESUME_PROGRESS_SECS: f64 = 1.0;

struct NetworkRestore<'a>(&'a PrivateTestServer);

impl Drop for NetworkRestore<'_> {
    fn drop(&mut self) {
        self.0.set_network_online(true);
    }
}

fn spawn_ticker(queue: Arc<Queue>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        loop {
            time::sleep(Duration::from_millis(20)).await;
            if queue.tick().is_err() {
                break;
            }
        }
    })
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(120)))]
async fn playback_resumes_after_network_returns(temp_dir: TestTempDir) {
    // A private server: this test takes the network down, and the switch covers
    // every data route on whichever server it runs against.
    let server = PrivateTestServer::start().await;
    let url = server.helper().asset("hls/master.m3u8").to_string();
    resumes_after_outage(temp_dir, &server, url, LOOK_AHEAD_BYTES).await;
}

/// The same contract, with the segments arriving at the rate they are played
/// and a look-ahead wide enough to keep several of them in flight.
///
/// [`playback_resumes_after_network_returns`] serves them from loopback into a
/// 64 KiB window, so at most one fetch is open when the network goes: it fails,
/// and the next attempt happens after connectivity is back. On a device the
/// window is wider and the segments take real time to arrive, so an outage
/// catches a handful of part-written bodies at once — the case that stranded
/// the iOS lane, where every one of them was written off permanently and
/// playback stopped at the first of the gaps they left.
#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(180)))]
async fn playback_resumes_after_network_returns_with_paced_segments(temp_dir: TestTempDir) {
    let server = PrivateTestServer::start().await;
    let url = paced_master(&server);
    resumes_after_outage(temp_dir, &server, url, PACED_LOOK_AHEAD_BYTES).await;
}

/// Re-serve the packaged variant with every media segment throttled to roughly
/// the rate it is consumed (~50 KiB of audio per 5 s). The playlists stay
/// immediate: pacing those would only delay the start.
fn paced_master(server: &PrivateTestServer) -> String {
    const VARIANT: &str = "index-slq-a1.m3u8";
    const INITIALIZATION: &str = "init-slq-a1.mp4";
    const PLAYLIST_TYPE: Option<&'static str> = Some("application/vnd.apple.mpegurl");

    let hls = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root from tests/")
        .join("assets/hls");
    let playlist = fs::read_to_string(hls.join(VARIANT)).expect("read the packaged variant");

    let mut rewritten = Vec::new();
    for line in playlist.split('\n') {
        if line.starts_with("#EXT-X-MAP:") {
            let init = server.helper().asset(&format!("hls/{INITIALIZATION}"));
            rewritten.push(line.replace(INITIALIZATION, init.as_str()));
        } else if !line.is_empty() && !line.starts_with('#') {
            let bytes = fs::read(hls.join(line)).expect("read a packaged segment");
            let handle = server.helper().register_behavior(FixtureBehavior {
                content: Content::StaticBytes {
                    bytes: Arc::new(bytes),
                    content_type: None,
                },
                delivery: Delivery::Throttle {
                    chunk: SEGMENT_CHUNK_BYTES,
                    delay_ms: SEGMENT_CHUNK_DELAY_MS,
                },
            });
            rewritten.push(handle.child_url(line).to_string());
        } else {
            rewritten.push(line.to_string());
        }
    }

    let media = server.helper().register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(rewritten.join("\n").into_bytes()),
            content_type: PLAYLIST_TYPE,
        },
        delivery: Delivery::Normal,
    });
    let master = format!(
        "#EXTM3U\n#EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=66005,CODECS=\"mp4a.40.2\"\n{}\n",
        media.child_url(VARIANT)
    );
    let handle = server.helper().register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(master.into_bytes()),
            content_type: PLAYLIST_TYPE,
        },
        delivery: Delivery::Normal,
    });
    handle.child_url("master.m3u8").to_string()
}

async fn resumes_after_outage(
    temp_dir: TestTempDir,
    server: &PrivateTestServer,
    url: String,
    look_ahead_bytes: u64,
) {
    let region = kithara::bufpool::Region::default();
    let byte_pool = region.byte_pool();
    let net = NetOptions::builder()
        .inactivity_timeout(Duration::from_millis(500))
        .retry_policy(
            RetryPolicy::builder()
                .max_retries(3)
                .base_delay(Duration::from_millis(10))
                .max_delay(Duration::from_millis(200))
                .build(),
        )
        .byte_pool(byte_pool.clone())
        .build();
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(net, CancelToken::never())).build(),
    );
    let store = AssetStore::builder()
        .backend(StorageBackend::Disk {
            root: temp_dir.path().into(),
        })
        .pool(byte_pool.clone())
        .build();
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .worker(kithara::play::PlayWorker::new(
                kithara::play::PlayWorkerConfig::for_pools(byte_pool, region.sample_pool()).build(),
            ))
            .session(OfflineSession::arc_auto())
            .build(),
    );
    let queue = Arc::new(Queue::new(
        QueueConfig::builder()
            .player(player)
            .store(store.clone())
            .build(),
    ));
    let cfg =
        ResourceConfig::for_src(ResourceConfig::parse_src(url.as_str()).expect("valid HLS URL"))
            .downloader(downloader)
            .initial_abr_mode(AbrMode::manual(0))
            .look_ahead_bytes(look_ahead_bytes)
            .store(store)
            .build();

    let ticker = spawn_ticker(Arc::clone(&queue));
    let mut rx = queue.subscribe();
    let id = queue
        .append(TrackSource::Config(Box::new(cfg)))
        .expect("append offline-resume track");
    queue
        .select(id, Transition::None)
        .expect("select HLS track");
    wait_for_loader_done_event(&mut rx, &queue, id, Duration::from_secs(30))
        .await
        .unwrap_or_else(|error| panic!("precondition: {error}"));
    queue.play();

    let before_outage = wait_for_position_event(
        &mut rx,
        &queue,
        PLAY_BEFORE_OUTAGE_SECS,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|error| panic!("precondition: {error}"));

    server.set_network_online(false);
    let _network_restore = NetworkRestore(&server);
    wait_for_event(
        &mut rx,
        "a segment fetch observing the offline server",
        |event| {
            matches!(
                event,
                Event::Downloader(
                    DownloaderEvent::FirstByte { status: 503, .. }
                        | DownloaderEvent::RequestFailed { .. }
                        | DownloaderEvent::RetryExhausted { .. }
                )
            )
        },
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "precondition: {error}; the look-ahead window never needed a further \
             segment, so no outage was ever observed"
        )
    });

    // Resumption is only meaningful once playback has genuinely run dry.
    // Without this the buffered look-ahead carries the position past the
    // resume target on its own and the assertion below passes vacuously.
    let mut starved_at = 0.0;
    wait_for_event(
        &mut rx,
        "playback starving on the exhausted buffer",
        |event| {
            let Event::Audio(AudioEvent::UnderrunStarted { position_ms, .. }) = event else {
                return false;
            };
            starved_at = *position_ms as f64 / 1000.0;
            true
        },
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "precondition: {error}; the buffer never ran dry while the network was \
             down, so there was nothing for the recovery to resume from"
        )
    });
    server.set_network_online(true);

    let resume_target = starved_at + MIN_RESUME_PROGRESS_SECS;
    let resumed_at =
        wait_for_position_event(&mut rx, &queue, resume_target, Duration::from_secs(30))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "connectivity returned after starving at {starved_at:.3}s (outage began at {before_outage:.3}s), but \
                 playback never reached {resume_target:.3}s: {error}"
                )
            });
    assert!(
        resumed_at >= resume_target,
        "playback stayed at {resumed_at:.3}s after the network returned \
         (starved at {starved_at:.3}s, outage began at {before_outage:.3}s)"
    );

    queue.clear();
    ticker.abort();
    let _ = ticker.await;
}
