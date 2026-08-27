#![cfg(not(target_arch = "wasm32"))]

//! The failure twin of [`background_track_eof`]: `ItemDidFail` is
//! published for whichever slot aborted, and `PlayerImpl::
//! process_notifications` walks every active slot. A background slot
//! whose decoder gives up must not skip the track being heard, and must
//! not mark a queue entry failed on its behalf.
//!
//! [`background_track_eof`]: super::background_track_eof

use std::num::NonZero;

use kithara::{
    self,
    decode::PcmSpec,
    events::{Event, PlayerEvent, TrackId, TrackStatus},
    platform::sync::Arc,
    queue::{Queue, QueueConfig, Transition, test_utils::QueueProbe},
};
use kithara_integration_tests::{
    audio_mock::TestPcmReader,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions, resource_from_reader_with_src},
};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
/// ≈ 0.74 s of rendered audio — far short of `TRACK_SECS`.
const WARMUP_BLOCKS: usize = 64;
const TRACK_SECS: f64 = 30.0;
const LOUD: f32 = 0.80;
const QUIET: f32 = 0.10;

fn make_fixture() -> (OfflinePlayerHarness, Queue) {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .build(),
        SAMPLE_RATE,
    );
    let config = QueueConfig::builder()
        .player(harness.take_player())
        .should_autoplay(false)
        .build();
    (harness, Queue::new(config))
}

fn loaded_track(queue: &Queue, value: f32) -> (TrackId, Arc<str>) {
    let id = queue.register_for_test();
    let src: Arc<str> = Arc::from(format!("test://memory/{}", id.as_u64()));
    let spec = PcmSpec {
        channels: CHANNELS,
        sample_rate: NonZero::new(SAMPLE_RATE).unwrap(),
    };
    queue.complete_load_for_test(
        id,
        resource_from_reader_with_src(
            TestPcmReader::with_value(spec, TRACK_SECS, value),
            Arc::clone(&src),
        ),
    );
    (id, src)
}

fn mean_abs(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    pcm.iter().map(|s| s.abs()).sum::<f32>() / pcm.len() as f32
}

fn render_loop(queue: &Queue, harness: &OfflinePlayerHarness, block_budget: usize) -> Vec<f32> {
    let mut pcm = Vec::new();
    for _ in 0..block_budget {
        let _ = queue.tick();
        pcm.extend(harness.render(BLOCK_FRAMES));
    }
    pcm
}

/// Three loaded tracks, the middle one playing and warmed up. The first
/// — never selected, standing in for the background slot — is the one
/// that will report the failure.
fn fixture_playing_the_middle_track() -> (OfflinePlayerHarness, Queue, TrackId, Arc<str>) {
    let (harness, queue) = make_fixture();
    let (stale, stale_src) = loaded_track(&queue, QUIET);
    let (current, _) = loaded_track(&queue, LOUD);
    let (_next, _) = loaded_track(&queue, QUIET);

    queue
        .select(current, Transition::None)
        .expect("select the current track");
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    (harness, queue, stale, stale_src)
}

fn publish_background_failure(harness: &OfflinePlayerHarness, src: Arc<str>) {
    harness
        .player()
        .bus()
        .publish(Event::Player(PlayerEvent::ItemDidFail {
            src,
            item_id: None,
            from_current_item: false,
        }));
}

#[kithara::test(tokio)]
async fn background_track_failure_does_not_advance_the_queue() {
    let (harness, queue, _stale, stale_src) = fixture_playing_the_middle_track();

    publish_background_failure(&harness, stale_src);
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    assert_eq!(
        queue.current_index(),
        Some(1),
        "a failure from a track that is not current must leave the current track selected"
    );
}

#[kithara::test(tokio)]
async fn background_track_failure_does_not_cut_the_current_track_audio() {
    let (harness, queue) = make_fixture();
    let (_stale, stale_src) = loaded_track(&queue, QUIET);
    let (current, _) = loaded_track(&queue, LOUD);
    let (_next, _) = loaded_track(&queue, QUIET);

    queue
        .select(current, Transition::None)
        .expect("select the current track");
    let before_pcm = render_loop(&queue, &harness, WARMUP_BLOCKS);
    let before = mean_abs(&before_pcm[before_pcm.len() / 2..]);
    assert!(
        before > 0.005,
        "the current track must be audible before the background failure: mean={before}"
    );

    publish_background_failure(&harness, stale_src);
    let after_pcm = render_loop(&queue, &harness, WARMUP_BLOCKS);
    let after = mean_abs(&after_pcm[after_pcm.len() / 2..]);

    assert!(
        after > before / 2.0,
        "the current track must keep sounding through a background track's failure — \
         the quieter successor took over instead: before={before}, after={after}"
    );
}

/// Acting on a background failure also flags a queue entry, taking it out
/// of selection for the rest of the session — on the strength of a slot
/// nobody is listening to.
#[kithara::test(tokio)]
async fn background_track_failure_does_not_flag_a_queue_entry() {
    let (harness, queue, stale, stale_src) = fixture_playing_the_middle_track();

    publish_background_failure(&harness, stale_src);
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    let status = queue
        .tracks()
        .into_iter()
        .find(|entry| entry.id == stale)
        .map(|entry| entry.status)
        .expect("the background entry must still be in the queue");
    assert!(
        !matches!(status, TrackStatus::Failed(_)),
        "a background track's failure must not mark the entry failed: {status:?}"
    );
}
