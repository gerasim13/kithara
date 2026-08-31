#![cfg(not(target_arch = "wasm32"))]

//! Two queue entries may name the same URL: a playlist that repeats a
//! track, or one asset reachable under a single address. A player event
//! must resolve to the entry that actually played. Resolving it by
//! source alone answers with whichever entry holds that URL first,
//! which is the wrong track as soon as the second copy is the one
//! playing.
use std::num::NonZero;

use kithara::{
    self,
    events::{Event, ItemRole, PlayerEvent, SlotId, TrackId, TrackRef, TrackStatus},
    platform::sync::Arc,
    queue::{Queue, QueueConfig, Transition, test_utils::QueueProbe},
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
const REPEATED_SRC: &str = "https://example.com/repeat.mp3";

fn make_fixture() -> (OfflinePlayerHarness, Queue<TestPools>) {
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

fn load(queue: &Queue<TestPools>, id: TrackId) {
    let spec = AudioSpec::new(
        CHANNELS,
        NonZero::new(SAMPLE_RATE).expect("sample rate is non-zero"),
    );
    queue.complete_load_for_test(
        id,
        resource_from_reader_with_src(
            TestPcmReader::with_value(spec, TRACK_SECS, LOUD),
            Arc::from(REPEATED_SRC),
        ),
    );
}

fn render_loop(queue: &Queue<TestPools>, harness: &OfflinePlayerHarness, block_budget: usize) {
    for _ in 0..block_budget {
        let _ = queue.tick();
        let _ = harness.render(BLOCK_FRAMES);
    }
}

fn status_of(queue: &Queue<TestPools>, id: TrackId) -> TrackStatus {
    queue
        .tracks()
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.status)
        .expect("the entry must still be in the queue")
}

/// The failing track is the *second* entry carrying this URL.
fn fixture_playing_the_second_copy() -> (OfflinePlayerHarness, Queue<TestPools>, TrackId, TrackId) {
    let (harness, queue) = make_fixture();
    let first = queue.append(REPEATED_SRC).expect("append first copy");
    let playing = queue.append(REPEATED_SRC).expect("append second copy");
    load(&queue, first);
    load(&queue, playing);

    queue
        .select(playing, Transition::None)
        .expect("select the second copy");
    render_loop(&queue, &harness, WARMUP_BLOCKS);

    (harness, queue, first, playing)
}

fn publish_leading_failure(harness: &OfflinePlayerHarness, id: TrackId) {
    harness
        .player()
        .bus()
        .publish(Event::Player(PlayerEvent::ItemDidFail {
            item: ItemRole::Leading(TrackRef::new(id, SlotId::new(0), Arc::from(REPEATED_SRC))),
        }));
}

#[kithara::test(tokio)]
async fn a_failure_flags_the_entry_that_played() {
    let (harness, queue, _first, playing) = fixture_playing_the_second_copy();

    publish_leading_failure(&harness, playing);
    render_loop(&queue, &harness, WARMUP_BLOCKS);

    let status = status_of(&queue, playing);
    assert!(
        matches!(status, TrackStatus::Failed(_)),
        "the entry that played must be the one flagged: {status:?}"
    );
}

#[kithara::test(tokio)]
async fn a_failure_spares_an_entry_that_only_shares_the_url() {
    let (harness, queue, first, playing) = fixture_playing_the_second_copy();

    publish_leading_failure(&harness, playing);
    render_loop(&queue, &harness, WARMUP_BLOCKS);

    let status = status_of(&queue, first);
    assert!(
        !matches!(status, TrackStatus::Failed(_)),
        "an entry that never played must not be flagged for sharing a URL: {status:?}"
    );
}
