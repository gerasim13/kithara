#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use kithara::platform::time::Duration;
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactRecording, AudioArtifactSet, audio_artifact_path},
    kithara,
};

use super::sync_product_matrix::{
    BLOCK_FRAMES, CHANNELS, ProductHarness, Provider, SEQUENTIAL_SYNC, SyncCase,
};

const CAPTURE_FRAMES: usize = 48_000 * 6;
const RIDE_STEPS: usize = 32;

async fn capture_solo(
    artifacts: &AudioArtifactSet,
    label: &str,
    audible_deck: usize,
) -> (PathBuf, Vec<String>) {
    let mut recording = artifacts
        .recording(label, Some(CAPTURE_FRAMES as u64))
        .unwrap_or_else(|error| panic!("open {label} recording: {error}"));
    let mut harness = ProductHarness::new(SEQUENTIAL_SYNC, Provider::Synthetic, audible_deck).await;
    capture_frames(
        &mut harness,
        SEQUENTIAL_SYNC,
        CAPTURE_FRAMES,
        &mut recording,
    )
    .await;
    let reader = AudioArtifactSet::finish(recording)
        .unwrap_or_else(|error| panic!("finish {label} recording: {error}"));
    let path = audio_artifact_path(&reader)
        .unwrap_or_else(|error| panic!("resolve {label} artifact path: {error}"));
    (path, harness.failures)
}

async fn capture_mix(
    artifacts: &AudioArtifactSet,
    label: &str,
    provider: Provider,
    target_bpm: Option<f64>,
) -> (PathBuf, Vec<String>) {
    let case = SEQUENTIAL_SYNC;
    let mut recording = artifacts
        .recording(label, Some(CAPTURE_FRAMES as u64))
        .unwrap_or_else(|error| panic!("open {label} recording: {error}"));
    let mut harness = ProductHarness::new(case, provider, 0).await;
    for deck in &harness.decks {
        deck.set_muted(false);
        deck.set_volume(0.5);
    }
    harness.request_sync(case).await;

    if let Some(target_bpm) = target_bpm {
        capture_ride(&mut harness, case, target_bpm, &mut recording).await;
    } else {
        capture_frames(&mut harness, case, CAPTURE_FRAMES, &mut recording).await;
    }
    let reader = AudioArtifactSet::finish(recording)
        .unwrap_or_else(|error| panic!("finish {label} recording: {error}"));
    let path = audio_artifact_path(&reader)
        .unwrap_or_else(|error| panic!("resolve {label} artifact path: {error}"));
    (path, harness.failures)
}

async fn capture_frames(
    harness: &mut ProductHarness,
    case: SyncCase,
    frames: usize,
    recording: &mut AudioArtifactRecording,
) {
    let mut rendered = 0;
    while rendered < frames {
        let block_frames = (frames - rendered).min(BLOCK_FRAMES);
        let block = harness.render(case, block_frames).await;
        assert_eq!(
            block.len(),
            block_frames * usize::from(CHANNELS),
            "offline renderer must return the requested complete block",
        );
        recording
            .push(&block)
            .unwrap_or_else(|error| panic!("record listening block: {error}"));
        rendered += block_frames;
    }
}

async fn capture_ride(
    harness: &mut ProductHarness,
    case: SyncCase,
    target_bpm: f64,
    recording: &mut AudioArtifactRecording,
) {
    let mut rendered = 0;
    for step in 1..=RIDE_STEPS {
        let progress = step as f64 / RIDE_STEPS as f64;
        harness.set_tempo(case, (target_bpm - 120.0).mul_add(progress, 120.0), false);
        let deadline = CAPTURE_FRAMES * step / RIDE_STEPS;
        capture_frames(harness, case, deadline - rendered, recording).await;
        rendered = deadline;
    }
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
    let artifacts = AudioArtifactSet::from_env(
        "pr150-sync-listening",
        SEQUENTIAL_SYNC.sample_rate,
        CHANNELS,
    )
    .expect("configure PR150 listening artifacts")
    .unwrap_or_else(|| panic!("KITHARA_AUDIO_ARTIFACT_DIR must be set for the listening recorder"));

    let (deck_a, mut failures) = capture_solo(&artifacts, "01-deck-a-120bpm", 0).await;
    let (deck_b, deck_b_failures) = capture_solo(&artifacts, "02-deck-b-120bpm", 1).await;
    let (fixed_mix, fixed_failures) = capture_mix(
        &artifacts,
        "03-mix-on-120bpm-grid",
        Provider::Synthetic,
        None,
    )
    .await;
    let (ridden_mix, ridden_failures) = capture_mix(
        &artifacts,
        "04-mix-riding-120-to-127",
        Provider::Synthetic,
        Some(127.0),
    )
    .await;
    let (sweep_mix, sweep_failures) = capture_mix(
        &artifacts,
        "05-sweep-mix-riding-120-to-145",
        Provider::Sweep,
        Some(145.0),
    )
    .await;
    failures.extend(deck_b_failures);
    failures.extend(fixed_failures);
    failures.extend(ridden_failures);
    failures.extend(sweep_failures);

    let paths = [
        ("01-deck-a-120bpm", deck_a),
        ("02-deck-b-120bpm", deck_b),
        ("03-mix-on-120bpm-grid", fixed_mix),
        ("04-mix-riding-120-to-127", ridden_mix),
        ("05-sweep-mix-riding-120-to-145", sweep_mix),
    ];
    let manifest = serde_json::json!({
        "case": "pr150-sync-listening",
        "sample_rate": SEQUENTIAL_SYNC.sample_rate,
        "channels": CHANNELS,
        "capture_frames": CAPTURE_FRAMES,
        "failures": failures,
        "artifacts": paths.iter().map(|(label, path)| {
            serde_json::json!({ "label": label, "path": path })
        }).collect::<Vec<_>>(),
    });
    let manifest = artifacts
        .write_manifest(&manifest)
        .expect("write PR150 listening manifest");
    let manifest_path = audio_artifact_path(&manifest).expect("resolve listening manifest path");

    for (label, path) in &paths {
        eprintln!("KITHARA_AUDIO_ARTIFACT {label}: {}", path.display());
    }
    eprintln!(
        "KITHARA_AUDIO_ARTIFACT manifest: {}",
        manifest_path.display()
    );
    assert!(
        failures.is_empty(),
        "PR150 listening capture failed:\n{}",
        failures.join("\n"),
    );
}
