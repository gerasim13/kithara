#![cfg(not(target_arch = "wasm32"))]

//! `ItemDidPlayToEnd` is published for whichever track in the player's
//! arena hit EOF, not for the one being heard:
//! `PlayerImpl::process_notifications` walks every active slot, and a slot
//! holds more than one track. An orphaned slot decoding ahead, or the
//! outgoing half of a crossfade, reaches its own end while the current
//! track has minutes left. The player names the role in `item`; only
//! `ItemRole::Leading` may advance the queue.
use std::num::NonZero;

use kithara::{
    self,
    events::{Event, ItemRole, PlayerEvent, SlotId, TrackId, TrackRef},
    platform::sync::Arc,
    queue::{Queue, QueueConfig, QueueControl, Transition, test_utils::QueueProbe},
    signal::AudioSpec,
};
use kithara_integration_tests::{
    audio_mock::TestPcmReader,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions, resource_from_reader_with_src},
};

use crate::bufpool_ext::TestPools;

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
/// ≈ 0.74 s of rendered audio — far short of `TRACK_SECS`.
const WARMUP_BLOCKS: usize = 64;
const TRACK_SECS: f64 = 30.0;
const LOUD: f32 = 0.80;
const QUIET: f32 = 0.10;

fn make_fixture() -> (OfflinePlayerHarness, QueueControl<TestPools>) {
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
    let queue = harness.insert_control(Queue::new(config));
    (harness, queue)
}

/// Load a track whose player-side `src` is its queue URI, the way a real
/// source arrives.
fn loaded_track(queue: &QueueControl<TestPools>, value: f32) -> (TrackId, Arc<str>) {
    let id = queue.register_for_test();
    let src: Arc<str> = Arc::from(format!("test://memory/{}", id.as_u64()));
    let spec = AudioSpec {
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

fn render_loop(
    queue: &QueueControl<TestPools>,
    harness: &OfflinePlayerHarness,
    block_budget: usize,
) -> Vec<f32> {
    let mut pcm = Vec::new();
    for _ in 0..block_budget {
        let _ = queue.tick();
        pcm.extend(harness.render(BLOCK_FRAMES));
    }
    pcm
}

/// Three loaded tracks, the middle one playing. The first track — never
/// selected, standing in for the background slot — reports natural EOF.
fn fixture_with_background_eof() -> (
    OfflinePlayerHarness,
    QueueControl<TestPools>,
    TrackRef,
    TrackId,
) {
    let (harness, queue) = make_fixture();
    let (stale, stale_src) = loaded_track(&queue, QUIET);
    let (current, _) = loaded_track(&queue, LOUD);
    let (_next, _) = loaded_track(&queue, QUIET);
    (
        harness,
        queue,
        TrackRef::new(stale, SlotId::new(0), stale_src),
        current,
    )
}

/// Field log, 2026-08-26: a background HLS slot hit EOF 5 s after the
/// current track started and the queue advanced on it, cutting a track
/// with minutes left. The queue must key the advance on the track that
/// ended being the current one.
#[kithara::test(tokio)]
async fn background_track_eof_does_not_advance_the_queue() {
    let (harness, queue, stale, current) = fixture_with_background_eof();

    queue
        .select(current, Transition::None)
        .expect("select the current track");
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    harness
        .player()
        .bus()
        .publish(Event::Player(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Background(stale),
        }));
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    assert_eq!(
        queue.current_index(),
        Some(1),
        "EOF from a track that is not current must leave the current track selected"
    );
}

/// The audible half of the same defect: the listener hears the current
/// track handed over to the successor while it is still playing.
#[kithara::test(tokio)]
async fn background_track_eof_does_not_cut_the_current_track_audio() {
    let (harness, queue, stale, current) = fixture_with_background_eof();

    queue
        .select(current, Transition::None)
        .expect("select the current track");
    let before_pcm = render_loop(&queue, &harness, WARMUP_BLOCKS);
    let before = mean_abs(&before_pcm[before_pcm.len() / 2..]);
    assert!(
        before > 0.005,
        "the current track must be audible before the background EOF: mean={before}"
    );

    harness
        .player()
        .bus()
        .publish(Event::Player(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Background(stale),
        }));
    let after_pcm = render_loop(&queue, &harness, WARMUP_BLOCKS);
    let after = mean_abs(&after_pcm[after_pcm.len() / 2..]);

    assert!(
        after > before / 2.0,
        "the current track must keep sounding through a background track's EOF — \
         the quieter successor took over instead: before={before}, after={after}"
    );
}

/// The other non-leading role: `commit_next` promotes the successor inside
/// the *current* slot, so the faded-out track ends there — its own slot is
/// still the held one. The queue has already moved on with the pre-arm;
/// answering that end again would skip a track the listener just started.
#[kithara::test(tokio)]
async fn outgoing_crossfade_half_eof_does_not_advance_the_queue() {
    let (harness, queue, outgoing, current) = fixture_with_background_eof();

    queue
        .select(current, Transition::None)
        .expect("select the current track");
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    harness
        .player()
        .bus()
        .publish(Event::Player(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Outgoing(outgoing),
        }));
    let _ = render_loop(&queue, &harness, WARMUP_BLOCKS);

    assert_eq!(
        queue.current_index(),
        Some(1),
        "the end of a crossfade's outgoing half must leave the promoted track selected"
    );
}
