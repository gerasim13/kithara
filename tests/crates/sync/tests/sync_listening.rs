#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use kithara::platform::time::Duration;
use kithara_integration_tests::{
    cochlea::{CochleaReport, mix_loudness_failures},
    grid::Start,
    kithara,
};

use super::sync_product_matrix::{
    Audible, BLOCK_FRAMES, CHANNELS, NEWTECHNO_PHRASE, PreparedSources, ProductHarness,
    REAL_TRACK_FOUR_DECK_SYNC, REAL_TRACK_SYNC, SyncCase, newtechno_sources, tunnel_sources,
};

const CAPTURE_FRAMES: usize = 48_000 * 6;
const LOUDNESS_TOLERANCE_LU: f64 = 0.5;
const RIDE_STEPS: usize = 32;
/// An entry on the second beat of the first bar.
const WEAK_BEAT: Start = Start::Bar { bar: 0, beat: 1 };

struct Capture {
    pcm: Vec<f32>,
    failures: Vec<String>,
}

async fn render_solo(
    case: SyncCase,
    provider: &PreparedSources,
    start: Start,
    audible_deck: usize,
) -> Capture {
    let mut harness = ProductHarness::new(case, provider, start, Audible::Deck(audible_deck)).await;
    harness.seek_staggered(case).await;
    let pcm = render_frames(&mut harness, case, CAPTURE_FRAMES).await;
    Capture {
        pcm,
        failures: harness.failures,
    }
}

async fn render_mix(
    case: SyncCase,
    provider: &PreparedSources,
    start: Start,
    target_bpm: Option<f64>,
) -> Capture {
    let mut harness = ProductHarness::new(case, provider, start, Audible::Mix).await;
    harness.seek_staggered(case).await;
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
#[case::tunnel(tunnel_sources().await, Start::bar(0))]
#[case::newtechno(newtechno_sources().await, NEWTECHNO_PHRASE)]
#[case::tunnel_weak_beat(tunnel_sources().await, WEAK_BEAT)]
async fn sync_listening_mix_is_not_quieter_than_a_solo_deck(
    #[case] provider: PreparedSources,
    #[case] start: Start,
) {
    let case = REAL_TRACK_SYNC;
    let mut decks = Vec::with_capacity(case.decks());
    for deck in 0..case.decks() {
        let capture = render_solo(case, &provider, start, deck).await;
        decks.push(CochleaReport::measure(
            &capture.pcm,
            CHANNELS,
            case.sample_rate,
        ));
    }
    let mix = render_mix(case, &provider, start, None).await;
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
#[ignore = "writes opt-in listening WAVs; run through `just test audio-artifacts`"]
#[case::tunnel_host_120(tunnel_sources().await, Start::bar(0), REAL_TRACK_SYNC, None)]
#[case::tunnel_ride_to_96(tunnel_sources().await, Start::bar(0), REAL_TRACK_SYNC, Some(96.0))]
#[case::tunnel_ride_to_127(tunnel_sources().await, Start::bar(0), REAL_TRACK_SYNC, Some(127.0))]
#[case::tunnel_ride_to_145(tunnel_sources().await, Start::bar(0), REAL_TRACK_SYNC, Some(145.0))]
#[case::tunnel_four_deck_host_120(tunnel_sources().await, Start::bar(0), REAL_TRACK_FOUR_DECK_SYNC, None)]
#[case::newtechno_host_120(newtechno_sources().await, NEWTECHNO_PHRASE, REAL_TRACK_SYNC, None)]
#[case::newtechno_ride_to_96(newtechno_sources().await, NEWTECHNO_PHRASE, REAL_TRACK_SYNC, Some(96.0))]
#[case::newtechno_ride_to_127(newtechno_sources().await, NEWTECHNO_PHRASE, REAL_TRACK_SYNC, Some(127.0))]
#[case::newtechno_ride_to_145(newtechno_sources().await, NEWTECHNO_PHRASE, REAL_TRACK_SYNC, Some(145.0))]
#[case::newtechno_four_deck_host_120(newtechno_sources().await, NEWTECHNO_PHRASE, REAL_TRACK_FOUR_DECK_SYNC, None)]
async fn record_sync_listening_wavs(
    #[case] provider: PreparedSources,
    #[case] start: Start,
    #[case] case: SyncCase,
    #[case] target_bpm: Option<f64>,
) {
    assert!(
        std::env::var_os("KITHARA_AUDIO_ARTIFACT_DIR").is_some(),
        "KITHARA_AUDIO_ARTIFACT_DIR must be set for the listening recorder"
    );
    let mut deck_reports = Vec::with_capacity(case.decks());
    let mut failures = Vec::new();
    for deck in 0..case.decks() {
        let capture = render_solo(case, &provider, start, deck).await;
        deck_reports.push(CochleaReport::measure(
            &capture.pcm,
            CHANNELS,
            case.sample_rate,
        ));
        failures.extend(capture.failures);
    }
    let mix = render_mix(case, &provider, start, target_bpm).await;
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
