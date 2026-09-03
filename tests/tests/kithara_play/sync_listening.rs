#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use kithara::platform::time::Duration;
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactRecording, AudioArtifactSet, audio_artifact_path},
    kithara,
};

use super::sync_product_matrix::{
    AMBIENT_TRIP_HOP_PROVIDER, AMBIENT_TRIP_HOP_SYNC, BLOCK_FRAMES, CHANNELS, CROSS_STYLE_PROVIDER,
    CROSS_STYLE_SYNC, DOWNTEMPO_HOUSE_PROVIDER, DOWNTEMPO_HOUSE_SYNC, ProductHarness, Provider,
    SEQUENTIAL_SYNC, SyncCase, TECHNO_BREAKBEAT_PROVIDER, TECHNO_BREAKBEAT_SYNC,
};

const CAPTURE_FRAMES: usize = 48_000 * 6;
const RIDE_STEPS: usize = 32;

async fn capture_solo(
    artifacts: &AudioArtifactSet,
    label: &str,
    case: SyncCase,
    provider: Provider,
    audible_deck: usize,
) -> (PathBuf, Vec<String>) {
    let mut recording = artifacts
        .recording(label, Some(CAPTURE_FRAMES as u64))
        .unwrap_or_else(|error| panic!("open {label} recording: {error}"));
    let mut harness = ProductHarness::new(case, provider, audible_deck).await;
    capture_frames(&mut harness, case, CAPTURE_FRAMES, &mut recording).await;
    let reader = AudioArtifactSet::finish(recording)
        .unwrap_or_else(|error| panic!("finish {label} recording: {error}"));
    let path = audio_artifact_path(&reader)
        .unwrap_or_else(|error| panic!("resolve {label} artifact path: {error}"));
    (path, harness.failures)
}

async fn capture_mix(
    artifacts: &AudioArtifactSet,
    label: &str,
    case: SyncCase,
    provider: Provider,
    target_bpm: Option<f64>,
) -> (PathBuf, Vec<String>) {
    let mut recording = artifacts
        .recording(label, Some(CAPTURE_FRAMES as u64))
        .unwrap_or_else(|error| panic!("open {label} recording: {error}"));
    let mut harness = ProductHarness::new(case, provider, 0).await;
    let volume = 1.0 / harness.decks.len() as f32;
    for deck in &harness.decks {
        deck.set_muted(false);
        deck.set_volume(volume);
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
#[case::synthetic_120("synthetic-120", SEQUENTIAL_SYNC, Provider::Synthetic, None)]
#[case::synthetic_127("synthetic-127", SEQUENTIAL_SYNC, Provider::Synthetic, Some(127.0))]
#[case::sweep_145("sweep-145", SEQUENTIAL_SYNC, Provider::Sweep, Some(145.0))]
#[case::ambient_trip_hop(
    "ambient-dub-62-trip-hop-74",
    AMBIENT_TRIP_HOP_SYNC,
    AMBIENT_TRIP_HOP_PROVIDER,
    None
)]
#[case::downtempo_house(
    "downtempo-96-house-124",
    DOWNTEMPO_HOUSE_SYNC,
    DOWNTEMPO_HOUSE_PROVIDER,
    None
)]
#[case::techno_breakbeat(
    "techno-132-breakbeat-140",
    TECHNO_BREAKBEAT_SYNC,
    TECHNO_BREAKBEAT_PROVIDER,
    None
)]
#[case::cross_style_four_deck(
    "ambient-62-downtempo-96-house-124-breakbeat-140",
    CROSS_STYLE_SYNC,
    CROSS_STYLE_PROVIDER,
    None
)]
async fn record_sync_listening_wavs(
    #[case] artifact_case: &str,
    #[case] case: SyncCase,
    #[case] provider: Provider,
    #[case] target_bpm: Option<f64>,
) {
    let artifacts = AudioArtifactSet::from_env(artifact_case, case.sample_rate, CHANNELS)
        .expect("configure sync listening artifacts")
        .unwrap_or_else(|| {
            panic!("KITHARA_AUDIO_ARTIFACT_DIR must be set for the listening recorder")
        });
    let mut paths = Vec::with_capacity(case.decks() + 1);
    let mut failures = Vec::new();
    for deck in 0..case.decks() {
        let label = format!("deck-{}", deck + 1);
        let (path, deck_failures) = capture_solo(&artifacts, &label, case, provider, deck).await;
        paths.push((label, path));
        failures.extend(deck_failures);
    }
    let (mix, mix_failures) = capture_mix(&artifacts, "mix", case, provider, target_bpm).await;
    paths.push(("mix".to_owned(), mix));
    failures.extend(mix_failures);

    let manifest = serde_json::json!({
        "case": case.id(),
        "fixture": artifact_case,
        "sample_rate": case.sample_rate,
        "channels": CHANNELS,
        "capture_frames": CAPTURE_FRAMES,
        "failures": failures,
        "artifacts": paths.iter().map(|(label, path)| {
            serde_json::json!({ "label": label, "path": path })
        }).collect::<Vec<_>>(),
    });
    let manifest = artifacts
        .write_manifest(&manifest)
        .expect("write sync listening manifest");
    let manifest_path =
        audio_artifact_path(&manifest).expect("resolve sync listening manifest path");

    for (label, path) in &paths {
        eprintln!("KITHARA_AUDIO_ARTIFACT {label}: {}", path.display());
    }
    eprintln!(
        "KITHARA_AUDIO_ARTIFACT manifest: {}",
        manifest_path.display()
    );
    assert!(
        failures.is_empty(),
        "{} listening capture failed:\n{}",
        case.id(),
        failures.join("\n"),
    );
}
