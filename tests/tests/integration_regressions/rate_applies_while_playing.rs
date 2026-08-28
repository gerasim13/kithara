#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{events::PlayerEvent, play::Resource, signal::AudioSpec};
use kithara_integration_tests::offline::{
    OfflinePlayerHarness, OfflinePlayerOptions, resource_from_reader,
};

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const WARMUP_BLOCKS: usize = 8;
const CLOCK_BLOCKS: usize = 32;
/// Wide enough for the rate-1.0 baseline to reach silence inside the window.
/// A window that clips the baseline makes both sides saturate at the cap and
/// the comparison below vacuous.
const MEASURE_BLOCKS: usize = 200;
const FAST_RATE: f32 = 2.0;

fn make_resource(duration_secs: f64) -> Resource {
    resource_from_reader(kithara_integration_tests::audio_mock::TestPcmReader::new(
        AudioSpec::new(2, NonZeroU32::new(SAMPLE_RATE).expect("test rate")),
        duration_secs,
    ))
}

#[kithara::test]
fn fixed_rate_reader_keeps_source_and_player_clock_at_unity() {
    let oracle = loaded_harness();
    assert_eq!(oracle.player().rate(), 1.0);
    oracle.player().pause();
    assert_eq!(
        oracle.player().rate(),
        1.0,
        "the control thread must not publish pause before RT applies it"
    );
    let _ = oracle.render(BLOCK_FRAMES);
    let paused_rates = rate_events(oracle.tick_and_drain());
    assert_eq!(paused_rates, [0.0]);
    assert_eq!(oracle.player().rate(), 0.0);

    oracle.player().set_default_rate(FAST_RATE);
    assert_eq!(oracle.player().default_rate(), FAST_RATE);
    oracle.player().play();
    assert_eq!(
        oracle.player().rate(),
        0.0,
        "the control thread must not publish the requested rate before RT applies it"
    );
    let _ = oracle.render(BLOCK_FRAMES);
    let resumed_rates = rate_events(oracle.tick_and_drain());
    assert_eq!(resumed_rates, [1.0]);
    assert_eq!(oracle.player().rate(), 1.0);

    let baseline = blocks_until_silence(1.0);
    let requested_fast = blocks_until_silence(FAST_RATE);
    let baseline_advance = media_advance(1.0);
    let requested_fast_advance = media_advance(FAST_RATE);

    assert!(
        baseline < MEASURE_BLOCKS,
        "the rate-1.0 baseline must drain inside the measured window, \
         got {baseline} of {MEASURE_BLOCKS} blocks — the comparison below would \
         compare two saturated caps"
    );
    assert_eq!(
        requested_fast, baseline,
        "a reader without a Warp control must stay fixed-rate instead of \
         consuming source frames at the requested rate"
    );
    assert!(
        (requested_fast_advance - baseline_advance).abs() < f64::EPSILON,
        "a reader without a Warp control must not report a media clock that \
         its PCM cannot follow: {requested_fast_advance}s vs {baseline_advance}s"
    );
}

fn rate_events(events: Vec<PlayerEvent>) -> Vec<f32> {
    events
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::RateChanged { rate } => Some(rate),
            _ => None,
        })
        .collect()
}

fn loaded_harness() -> OfflinePlayerHarness {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder().build(),
        SAMPLE_RATE,
    );
    harness.with_player(|player| {
        player.insert(make_resource(1.0), None, None);
        player
            .select_item(0, true)
            .expect("select first queue item");
    });

    for _ in 0..WARMUP_BLOCKS {
        let _ = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
    }
    harness
}

fn blocks_until_silence(rate: f32) -> usize {
    let harness = loaded_harness();
    harness.player().set_default_rate(rate);

    let mut blocks = 0usize;
    for _ in 0..MEASURE_BLOCKS {
        let block = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        blocks = blocks.saturating_add(1);
        if block.iter().all(|sample| sample.abs() == 0.0) {
            break;
        }
    }
    blocks
}

fn media_advance(rate: f32) -> f64 {
    let harness = loaded_harness();
    let start = harness.player().position_seconds().unwrap_or(0.0);
    harness.player().set_default_rate(rate);
    for _ in 0..CLOCK_BLOCKS {
        let _ = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
    }
    harness.player().position_seconds().unwrap_or(0.0) - start
}
