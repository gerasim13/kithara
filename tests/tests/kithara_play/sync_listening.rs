#![cfg(not(target_arch = "wasm32"))]

use kithara::platform::time::Duration;
use kithara_integration_tests::{audio_artifact::write_audio_artifact, kithara};

use super::sync_product_matrix::{
    BLOCK_FRAMES, CHANNELS, ProductHarness, Provider, SEQUENTIAL_SYNC, SyncCase,
};

const CAPTURE_FRAMES: usize = 48_000 * 6;
const RIDE_STEPS: usize = 32;

async fn capture_solo(audible_deck: usize) -> (Vec<f32>, Vec<String>) {
    let mut harness = ProductHarness::new(SEQUENTIAL_SYNC, Provider::Synthetic, audible_deck).await;
    let pcm = harness
        .capture_frames(SEQUENTIAL_SYNC, CAPTURE_FRAMES, BLOCK_FRAMES)
        .await;
    (pcm, harness.failures)
}

async fn capture_mix(provider: Provider, target_bpm: Option<f64>) -> (Vec<f32>, Vec<String>) {
    let case = SEQUENTIAL_SYNC;
    let mut harness = ProductHarness::new(case, provider, 0).await;
    for deck in &harness.decks {
        deck.set_muted(false);
        deck.set_volume(0.5);
    }
    harness.request_sync(case).await;

    let pcm = if let Some(target_bpm) = target_bpm {
        capture_ride(&mut harness, case, target_bpm).await
    } else {
        harness
            .capture_frames(case, CAPTURE_FRAMES, BLOCK_FRAMES)
            .await
    };
    (pcm, harness.failures)
}

async fn capture_ride(harness: &mut ProductHarness, case: SyncCase, target_bpm: f64) -> Vec<f32> {
    let mut pcm = Vec::with_capacity(CAPTURE_FRAMES * usize::from(CHANNELS));
    let mut rendered = 0;
    for step in 1..=RIDE_STEPS {
        let progress = step as f64 / RIDE_STEPS as f64;
        harness.set_tempo(case, (target_bpm - 120.0).mul_add(progress, 120.0), false);
        let deadline = CAPTURE_FRAMES * step / RIDE_STEPS;
        pcm.extend(
            harness
                .capture_frames(case, deadline - rendered, BLOCK_FRAMES)
                .await,
        );
        rendered = deadline;
    }
    pcm
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(300))
)]
#[ignore = "writes opt-in listening WAVs; ignored-red until Warp alignment is implemented"]
async fn record_pr150_sync_listening_wavs() {
    let (deck_a, mut failures) = capture_solo(0).await;
    let (deck_b, deck_b_failures) = capture_solo(1).await;
    let (fixed_mix, fixed_failures) = capture_mix(Provider::Synthetic, None).await;
    let (ridden_mix, ridden_failures) = capture_mix(Provider::Synthetic, Some(127.0)).await;
    let (sweep_mix, sweep_failures) = capture_mix(Provider::Sweep, Some(145.0)).await;
    failures.extend(deck_b_failures);
    failures.extend(fixed_failures);
    failures.extend(ridden_failures);
    failures.extend(sweep_failures);

    let manifest = serde_json::json!({
        "case": "pr150-sync-listening",
        "sample_rate": SEQUENTIAL_SYNC.sample_rate,
        "channels": CHANNELS,
        "capture_frames": CAPTURE_FRAMES,
        "failures": failures,
    });
    let written = write_audio_artifact(
        "pr150-sync-listening",
        SEQUENTIAL_SYNC.sample_rate,
        CHANNELS,
        &[
            ("01-deck-a-120bpm", &deck_a),
            ("02-deck-b-120bpm", &deck_b),
            ("03-mix-on-120bpm-grid", &fixed_mix),
            ("04-mix-riding-120-to-127", &ridden_mix),
            ("05-sweep-mix-riding-120-to-145", &sweep_mix),
        ],
        &manifest,
    )
    .expect("write PR150 listening WAVs");
    assert!(
        written.is_some(),
        "KITHARA_AUDIO_ARTIFACT_DIR must be set for the listening recorder"
    );
    assert!(
        failures.is_empty(),
        "PR150 listening capture failed:\n{}",
        failures.join("\n"),
    );
}
