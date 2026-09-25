#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use kithara::platform::time::Duration;
use kithara_integration_tests::{
    cochlea::{CochleaReport, mix_loudness_failures},
    kithara,
};

use super::sync_product_matrix::{
    BLOCK_FRAMES, CHANNELS, PreparedSources, ProductHarness, SyncCase, TUNNEL_FOUR_DECK_SYNC,
    TUNNEL_SYNC, tunnel_sources,
};

const CAPTURE_FRAMES: usize = 48_000 * 6;
const LOUDNESS_TOLERANCE_LU: f64 = 0.5;
const RIDE_STEPS: usize = 32;

struct Capture {
    pcm: Vec<f32>,
    failures: Vec<String>,
}

async fn render_solo(case: SyncCase, provider: &PreparedSources, audible_deck: usize) -> Capture {
    let mut harness = ProductHarness::new(case, provider, audible_deck).await;
    let pcm = render_frames(&mut harness, case, CAPTURE_FRAMES).await;
    Capture {
        pcm,
        failures: harness.failures,
    }
}

async fn render_mix(
    case: SyncCase,
    provider: &PreparedSources,
    target_bpm: Option<f64>,
) -> Capture {
    let mut harness = ProductHarness::new(case, provider, 0).await;
    harness.mark("mix: every deck audible");
    for deck in &harness.decks {
        let control = deck.control().clone();
        harness.host.run(move || control.set_muted(false)).await;
    }
    harness.request_sync(case).await;

    let pcm = if let Some(target_bpm) = target_bpm {
        let mut pcm = Vec::with_capacity(CAPTURE_FRAMES * usize::from(CHANNELS));
        let mut rendered = 0;
        for step in 1..=RIDE_STEPS {
            let progress = step as f64 / RIDE_STEPS as f64;
            harness
                .set_tempo(case, (target_bpm - 120.0).mul_add(progress, 120.0), false)
                .await;
            let deadline = CAPTURE_FRAMES * step / RIDE_STEPS;
            pcm.extend(render_frames(&mut harness, case, deadline - rendered).await);
            rendered = deadline;
        }
        pcm
    } else {
        render_frames(&mut harness, case, CAPTURE_FRAMES).await
    };
    Capture {
        pcm,
        failures: harness.failures,
    }
}

pub(super) async fn render_frames(
    harness: &mut ProductHarness,
    case: SyncCase,
    frames: usize,
) -> Vec<f32> {
    let mut pcm = Vec::with_capacity(frames * usize::from(CHANNELS));
    let mut rendered = 0;
    while rendered < frames {
        let block_frames = (frames - rendered).min(BLOCK_FRAMES);
        let block = harness.render(case, block_frames).await;
        assert_eq!(
            block.len(),
            block_frames * usize::from(CHANNELS),
            "offline renderer must return the requested complete block",
        );
        pcm.extend_from_slice(&block);
        rendered += block_frames;
    }
    pcm
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(60))
)]
async fn sync_listening_mix_is_not_quieter_than_a_solo_deck(
    #[future(awt)] tunnel_sources: PreparedSources,
) {
    let case = TUNNEL_SYNC;
    let provider = tunnel_sources;
    let mut decks = Vec::with_capacity(case.decks());
    for deck in 0..case.decks() {
        let capture = render_solo(case, &provider, deck).await;
        decks.push(CochleaReport::measure(
            &capture.pcm,
            CHANNELS,
            case.sample_rate,
        ));
    }
    let mix = render_mix(case, &provider, None).await;
    let mix = CochleaReport::measure(&mix.pcm, CHANNELS, case.sample_rate);
    let mut failures = mix_loudness_failures(case.id(), &mix, &decks, LOUDNESS_TOLERANCE_LU);
    if mix.clipped_samples > 0 || mix.true_peak_over_0dbtp {
        failures.push(format!("{}: mix clips: {mix:?}", case.id()));
    }
    assert!(
        failures.is_empty(),
        "sync listening loudness failed:\n{}\ndecks={decks:?}\nmix={mix:?}",
        failures.join("\n"),
    );
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
#[case::host_120(TUNNEL_SYNC, None)]
#[case::ride_to_96(TUNNEL_SYNC, Some(96.0))]
#[case::ride_to_127(TUNNEL_SYNC, Some(127.0))]
#[case::ride_to_145(TUNNEL_SYNC, Some(145.0))]
#[case::four_deck_host_120(TUNNEL_FOUR_DECK_SYNC, None)]
async fn record_sync_listening_wavs(
    #[case] case: SyncCase,
    #[case] target_bpm: Option<f64>,
    #[future(awt)] tunnel_sources: PreparedSources,
) {
    assert!(
        std::env::var_os("KITHARA_AUDIO_ARTIFACT_DIR").is_some(),
        "KITHARA_AUDIO_ARTIFACT_DIR must be set for the listening recorder"
    );
    let mut deck_reports = Vec::with_capacity(case.decks());
    let mut failures = Vec::new();
    for deck in 0..case.decks() {
        let capture = render_solo(case, &tunnel_sources, deck).await;
        deck_reports.push(CochleaReport::measure(
            &capture.pcm,
            CHANNELS,
            case.sample_rate,
        ));
        failures.extend(capture.failures);
    }
    let mix = render_mix(case, &tunnel_sources, target_bpm).await;
    let mix_report = CochleaReport::measure(&mix.pcm, CHANNELS, case.sample_rate);
    failures.extend(mix.failures);
    failures.extend(mix_loudness_failures(
        case.id(),
        &mix_report,
        &deck_reports,
        LOUDNESS_TOLERANCE_LU,
    ));
    if mix_report.clipped_samples > 0 || mix_report.true_peak_over_0dbtp {
        failures.push(format!("{}: mix clips: {mix_report:?}", case.id()));
    }
    assert!(
        failures.is_empty(),
        "{} listening capture failed:\n{}\ndecks={deck_reports:?}\nmix={mix_report:?}",
        case.id(),
        failures.join("\n"),
    );
}
