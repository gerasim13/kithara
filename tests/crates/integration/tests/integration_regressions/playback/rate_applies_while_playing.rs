#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;

use kithara::{
    events::TrackId,
    platform::time::{self, Duration, WallInstant},
    play::PlayerEvent,
};
use kithara_integration_tests::offline::{OfflinePlayer, OfflinePlayerOptions};
use kithara_test_fixtures::assets;

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const WARMUP_BLOCKS: usize = 8;
const CLOCK_BLOCKS: usize = 32;
/// Wide enough for the rate-1.0 baseline to reach silence inside the window.
/// A window that clips the baseline makes both sides saturate at the cap and
/// the comparison below vacuous.
const MEASURE_BLOCKS: usize = 200;
const FAST_RATE: f32 = 2.0;

#[kithara::test(tokio)]
#[kithara_test_utils::kithara::hang_watchdog]
async fn fixed_rate_reader_keeps_source_and_player_clock_at_unity() {
    let oracle = loaded_harness(1.0).await;
    assert_eq!(oracle.player().rate(), 1.0);
    completed(oracle.with_queue(kithara::queue::QueueControl::pause)).await;
    assert_eq!(
        oracle.player().rate(),
        1.0,
        "the control thread must not publish pause before RT applies it"
    );
    let _ = completed(oracle.render(BLOCK_FRAMES)).await;
    let paused_rates = rate_events(completed(oracle.tick_and_drain()).await);
    assert_eq!(paused_rates, [0.0]);
    assert_eq!(oracle.player().rate(), 0.0);

    completed(oracle.with_queue(move |player| player.set_default_rate(FAST_RATE)))
        .await
        .expect("a finite rate is accepted");
    wait_for_default_rate(&oracle).await;
    assert_eq!(oracle.player().default_rate(), FAST_RATE);
    hang_reset!();
    completed(oracle.with_queue(kithara::queue::QueueControl::play)).await;
    assert_eq!(
        oracle.player().rate(),
        0.0,
        "the control thread must not publish the requested rate before RT applies it"
    );
    let _ = completed(oracle.render(BLOCK_FRAMES)).await;
    let resumed_rates = rate_events(completed(oracle.tick_and_drain()).await);
    assert_eq!(resumed_rates, [FAST_RATE]);
    assert_eq!(oracle.player().rate(), FAST_RATE);

    let baseline = blocks_until_silence(1.0).await;
    let requested_fast = blocks_until_silence(FAST_RATE).await;
    let baseline_advance = media_advance(1.0).await;
    let requested_fast_advance = media_advance(FAST_RATE).await;

    assert!(
        baseline < MEASURE_BLOCKS,
        "the rate-1.0 baseline must drain inside the measured window, \
         got {baseline} of {MEASURE_BLOCKS} blocks — the comparison below would \
         compare two saturated caps"
    );
    assert_eq!(
        requested_fast + WARMUP_BLOCKS - 1,
        (baseline + WARMUP_BLOCKS - 1).div_ceil(2),
        "a rate-2 lane must exhaust the same PCM in half the output blocks, \
         including warmup and excluding the first silent block"
    );
    assert_eq!(
        baseline_advance,
        u64::try_from(CLOCK_BLOCKS * BLOCK_FRAMES).expect("clock window fits u64"),
        "the unity media clock must advance by every rendered source frame"
    );
    assert_eq!(
        requested_fast_advance,
        baseline_advance * 2,
        "the lane's media clock must follow its PCM speed exactly"
    );
    completed(oracle.close()).await;
}

#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn completed<Output>(operation: impl Future<Output = Output>) -> Output {
    let mut operation = std::pin::pin!(operation);
    loop {
        hang_tick!();
        if let Ok(output) = time::timeout(Duration::from_millis(10), operation.as_mut()).await {
            hang_reset!();
            return output;
        }
    }
}

#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn wait_for_default_rate(oracle: &OfflinePlayer) {
    let deadline = WallInstant::now() + Duration::from_secs(5);
    while oracle.player().default_rate() != FAST_RATE && WallInstant::now() < deadline {
        hang_tick!();
        let _ = completed(oracle.render(BLOCK_FRAMES)).await;
        completed(oracle.tick_and_drain()).await;
        time::sleep(Duration::from_millis(5)).await;
    }
    if oracle.player().default_rate() == FAST_RATE {
        hang_reset!();
    }
}

fn rate_events(events: Vec<PlayerEvent>) -> Vec<f32> {
    events
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::RateChanged { rate } => Some(rate),
            PlayerEvent::StatusChanged { .. }
            | PlayerEvent::TimeControlStatusChanged { .. }
            | PlayerEvent::PlaybackStarted { .. }
            | PlayerEvent::VolumeChanged { .. }
            | PlayerEvent::MuteChanged { .. }
            | PlayerEvent::CurrentItemChanged { .. }
            | PlayerEvent::PrerollCompleted { .. }
            | PlayerEvent::ItemDidPlayToEnd { .. }
            | PlayerEvent::ItemDidFail { .. } => None,
        })
        .collect()
}

#[kithara::flash(io)]
#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn loaded_harness(rate: f32) -> OfflinePlayer {
    let harness = completed(OfflinePlayer::with_sample_rate(
        OfflinePlayerOptions::builder().build(),
        SAMPLE_RATE,
    ))
    .await;
    let deck_source = assets::constant_half_wav_default()
        .path()
        .expect("prepared float WAV has a store path")
        .to_str()
        .expect("UTF-8 fixture path")
        .to_owned();
    completed(harness.with_queue(move |player| {
        player.set_default_rate(rate).expect("initial lane speed");
        let deck_id = TrackId::allocate();
        player
            .append_with_id(deck_id, deck_source)
            .expect("append PCM deck");
        player
            .select(deck_id, kithara::queue::Transition::None)
            .expect("select the item");
    }))
    .await;

    let deadline = WallInstant::now() + Duration::from_secs(5);
    while !harness
        .player()
        .current()
        .is_some_and(|track| track.status == kithara::queue::TrackStatus::Consumed)
    {
        hang_tick!();
        assert!(WallInstant::now() < deadline, "WAV lane becomes loaded");
        let _ = completed(harness.render(BLOCK_FRAMES)).await;
        let _ = completed(harness.tick_and_drain()).await;
        time::sleep(Duration::from_millis(5)).await;
    }
    hang_reset!();
    completed(harness.with_queue(kithara::queue::QueueControl::play)).await;
    for _ in 0..WARMUP_BLOCKS {
        hang_tick!();
        let _ = completed(harness.render(BLOCK_FRAMES)).await;
        let _ = completed(harness.tick_and_drain()).await;
        hang_reset!();
    }
    harness
}

#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn blocks_until_silence(rate: f32) -> usize {
    let harness = loaded_harness(rate).await;

    let mut blocks = 0usize;
    for _ in 0..MEASURE_BLOCKS {
        hang_tick!();
        let block = completed(harness.render(BLOCK_FRAMES)).await;
        let _ = completed(harness.tick_and_drain()).await;
        blocks = blocks.saturating_add(1);
        hang_reset!();
        if block.iter().all(|sample| sample.abs() == 0.0) {
            break;
        }
    }
    completed(harness.close()).await;
    blocks
}

#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn media_advance(rate: f32) -> u64 {
    let harness = loaded_harness(rate).await;
    let start = media_frame(&harness);
    for _ in 0..CLOCK_BLOCKS {
        hang_tick!();
        let _ = completed(harness.render(BLOCK_FRAMES)).await;
        let _ = completed(harness.tick_and_drain()).await;
        hang_reset!();
    }
    let advance = media_frame(&harness) - start;
    completed(harness.close()).await;
    advance
}

fn media_frame(harness: &OfflinePlayer) -> u64 {
    let snapshot = harness.deck_snapshot();
    let mut playing = snapshot.slots.iter().filter(|slot| slot.mark.is_some());
    let slot = playing.next().expect("the PCM track has a mixer mark");
    assert!(playing.next().is_none(), "the fixture plays one track");
    let mark = slot.mark.expect("marked slot");
    let source = slot.source.expect("the mixer observed source PCM");
    assert_eq!(mark.lane.segment, source.segment);
    let offset = mark
        .lane
        .frame
        .checked_sub(source.lane_frame)
        .expect("the mark lies inside its source chunk");
    let (numerator, denominator) = source
        .source_geometry()
        .expect("source PCM carries exact frame geometry")
        .source_ratio_at(offset)
        .expect("the mark lies within the source geometry");
    assert_eq!(
        numerator % denominator.get(),
        0,
        "integer playback rates must consume whole source frames"
    );
    let frame = u32::try_from(numerator / denominator.get()).expect("fixture frame fits u32");
    let position = Duration::from_secs_f64(f64::from(frame) / f64::from(SAMPLE_RATE));
    assert_eq!(
        harness.player().position_seconds(),
        Some(position.as_secs_f64()),
        "the published media clock must represent the exact source frame at nanosecond precision"
    );
    u64::from(frame)
}
