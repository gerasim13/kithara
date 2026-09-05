#![cfg(not(target_arch = "wasm32"))]

//! An end is a track finishing only if the media says so, and both halves of
//! that are pinned here: a body that stopped early must not be heard as the
//! track's own end, and a body that arrived whole must still be.

use std::path::Path;

use kithara::{
    events::{AdvanceReason, Event, QueueEvent, TrackId},
    platform::{
        sync::Arc,
        time::{self, Duration},
    },
    play::{Resource, ResourceConfig, ResourceSrc, player::PlayerControl},
    queue::{Queue, QueueConfig, QueueControl, Transition, test_utils::QueueProbe},
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, TestServerHelper, TestTempDir, kithara,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
};
use kithara_test_fixtures::assets;

use crate::bufpool_ext::TestPools;

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const CROSSFADE_SECS: f32 = 1.0;
const NO_CROSSFADE_SECS: f32 = 0.0;
/// Two tracks plus slack; the loop leaves as soon as the queue ends.
const BLOCK_BUDGET: usize = 3_000;
/// How much of the first track's body arrives before it stops, as a fraction
/// of the whole: far enough in that the header and seconds of audio are there,
/// far enough from the end that no crossfade could reach it.
const DELIVERED_NUMERATOR: usize = 2;
const DELIVERED_DENOMINATOR: usize = 5;

async fn open_resource(
    player: &PlayerControl<TestPools>,
    src: ResourceSrc,
    cache_dir: &Path,
) -> Resource {
    let config = ResourceConfig::<TestPools>::for_src(src)
        .store(kithara_integration_tests::disk_asset_store(cache_dir))
        .build();
    let config = player.prepare_config(config).expect("prepare resource");
    let mut resource = Resource::new(config).await.expect("open resource");
    let _ = resource.preload().await;
    resource
}

/// What the queue did over the whole playthrough.
#[derive(Default)]
struct QueueLog {
    advances: Vec<(Option<TrackId>, AdvanceReason)>,
    crossfades: usize,
    load_failures: Vec<String>,
    ended: bool,
}

impl QueueLog {
    /// Why the queue moved onto `id` - for the successor, the ways it left the
    /// track before it.
    fn advances_onto(&self, id: TrackId) -> Vec<AdvanceReason> {
        self.advances
            .iter()
            .filter_map(|&(target, reason)| (target == Some(id)).then_some(reason))
            .collect()
    }
}

/// A whole body served over HTTP, delivered in full or cut short.
fn track_src(
    server: &TestServerHelper,
    bytes: &'static [u8],
    content_type: &'static str,
    name: &'static str,
    delivery: Delivery,
) -> ResourceSrc {
    let handle = server.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(bytes.to_vec()),
            content_type: Some(content_type),
        },
        delivery,
    });
    ResourceSrc::parse(handle.child_url(name).as_str()).expect("valid track URL")
}

/// Play a two-track queue from the first track to the end of the queue, and
/// report what the queue did and which track it moved onto.
async fn play_queue(
    srcs: [ResourceSrc; 2],
    crossfade: f32,
    temp_dir: &TestTempDir,
) -> (QueueLog, TrackId) {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(crossfade)
            .block_on_underrun(true)
            .build(),
        SAMPLE_RATE,
    );
    let mut config = QueueConfig::builder().player(harness.take_player()).build();
    config.should_autoplay = false;
    let queue: QueueControl<TestPools> = harness.insert_control(Queue::new(config));

    let mut tracks = Vec::with_capacity(2);
    for (index, src) in srcs.into_iter().enumerate() {
        let resource = open_resource(
            harness.player(),
            src,
            &temp_dir.path().join(format!("track{index}")),
        )
        .await;
        tracks.push(queue.insert_loaded_for_test(resource));
    }
    let (first, second) = (tracks[0], tracks[1]);
    let transition = if crossfade > 0.0 {
        Transition::Crossfade
    } else {
        Transition::None
    };
    queue
        .select(first, transition)
        .expect("select the first track");

    let mut receiver = queue.subscribe();
    let mut log = QueueLog::default();
    for _ in 0..BLOCK_BUDGET {
        let _ = queue.tick();
        let _ = harness.render(BLOCK_FRAMES);
        while let Ok(envelope) = receiver.try_recv() {
            match envelope.event {
                Event::Queue(QueueEvent::CurrentTrackAdvance { id, reason }) => {
                    log.advances.push((id, reason));
                }
                Event::Queue(QueueEvent::CrossfadeStarted { .. }) => log.crossfades += 1,
                Event::Queue(QueueEvent::TrackLoadFailed { reason, .. }) => {
                    log.load_failures.push(reason);
                }
                Event::Queue(QueueEvent::QueueEnded) => log.ended = true,
                _ => {}
            }
        }
        if log.ended {
            break;
        }
        time::sleep(Duration::from_millis(1)).await;
    }

    (log, second)
}

/// A body that stops early must not be heard as the track's own end.
///
/// A `200` with no `Content-Length` names no total, so a body that stops after
/// two fifths is framed exactly as a complete one: the net layer reads a clean
/// end, the file layer commits what it wrote as the whole file, and the reader
/// announces an end two fifths in. Taken at face value that announcement arms
/// the crossfade a fade before it and advances the queue after it, which is the
/// fade a listener hears in the middle of a track. The one number a lost body
/// cannot move is the length the media itself declares.
#[kithara::test(
    native,
    tokio,
    timeout(Duration::from_secs(180)),
    hang_timeout_secs(30)
)]
async fn a_truncated_body_does_not_advance_the_queue_as_a_natural_end(temp_dir: TestTempDir) {
    let server = TestServerHelper::new().await;
    let flac = assets::signal_flac_saw_6s().bytes();
    let truncated = track_src(
        &server,
        flac,
        "audio/flac",
        "track.flac",
        Delivery::UnsizedEarlyClose {
            after_bytes: flac.len() * DELIVERED_NUMERATOR / DELIVERED_DENOMINATOR,
        },
    );
    let successor = track_src(&server, flac, "audio/flac", "track.flac", Delivery::Range);

    let (log, second) = play_queue([truncated, successor], CROSSFADE_SECS, &temp_dir).await;
    let left_by = log.advances_onto(second);

    assert!(
        !left_by.is_empty(),
        "the queue must leave a track whose body stopped, not sit on it: \
         advances={:?} ended={}",
        log.advances,
        log.ended
    );
    assert!(
        !left_by.contains(&AdvanceReason::NaturalEof),
        "a body that stopped at {DELIVERED_NUMERATOR}/{DELIVERED_DENOMINATOR} of \
         the track must not advance the queue as a track that played to its \
         end: left_by={left_by:?}"
    );
    assert_eq!(
        log.crossfades, 0,
        "a body that stopped must not cross-fade into the next track: \
         left_by={left_by:?}"
    );
}

/// A body that arrived whole must still be heard as the track's own end.
///
/// The refusal above weighs an announced end against the length the media
/// declares, and MPEG is where those two are most likely to disagree on their
/// own: the length comes from the Xing frame count, and nothing reconciles it
/// with the audio that decodes. A track that ends a little short of its own
/// header must still end, not fail.
async fn a_whole_body_still_ends_the_track(crossfade: f32, temp_dir: &TestTempDir) {
    let server = TestServerHelper::new().await;
    let mp3 = assets::signal_mp3_saw_2s().bytes();
    let first = track_src(&server, mp3, "audio/mpeg", "track.mp3", Delivery::Range);
    let successor = track_src(&server, mp3, "audio/mpeg", "track.mp3", Delivery::Range);

    let (log, second) = play_queue([first, successor], crossfade, temp_dir).await;
    let left_by = log.advances_onto(second);

    assert!(
        !left_by.is_empty(),
        "the queue must move onto the successor: advances={:?} ended={}",
        log.advances,
        log.ended
    );
    assert!(
        !left_by.contains(&AdvanceReason::TrackFailed),
        "a track whose body arrived whole must not be reported as a failure: \
         left_by={left_by:?}"
    );
    assert!(
        log.load_failures.is_empty(),
        "a track whose body arrived whole must reach its end without a load \
         failure: {:?}",
        log.load_failures
    );
}

/// Without a crossfade the distance the refusal allows shrinks to a single
/// block, which is the configuration a legitimate end would fail in first.
#[kithara::test(
    native,
    tokio,
    timeout(Duration::from_secs(180)),
    hang_timeout_secs(30)
)]
async fn a_whole_body_still_ends_the_track_without_a_crossfade(temp_dir: TestTempDir) {
    a_whole_body_still_ends_the_track(NO_CROSSFADE_SECS, &temp_dir).await;
}

#[kithara::test(
    native,
    tokio,
    timeout(Duration::from_secs(180)),
    hang_timeout_secs(30)
)]
async fn a_whole_body_still_ends_the_track_with_a_crossfade(temp_dir: TestTempDir) {
    a_whole_body_still_ends_the_track(CROSSFADE_SECS, &temp_dir).await;
}
