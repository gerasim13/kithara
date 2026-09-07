#![cfg(not(target_arch = "wasm32"))]

use kithara::platform::time::Duration;
use kithara_integration_tests::{
    cochlea::{CochleaReport, mix_loudness_failures},
    kithara,
};

use super::sync_product_matrix::{
    BLOCK_FRAMES, CHANNELS, DOWNTEMPO_HOUSE_PROVIDER, DOWNTEMPO_HOUSE_SYNC, ProductHarness,
    Provider, SyncCase,
};

const CAPTURE_FRAMES: usize = 48_000 * 6;
const LOUDNESS_TOLERANCE_LU: f64 = 0.5;

struct Capture {
    pcm: Vec<f32>,
}

async fn render_solo(case: SyncCase, provider: Provider, audible_deck: usize) -> Capture {
    let mut harness = ProductHarness::new(case, provider, audible_deck).await;
    let pcm = render_frames(&mut harness, case, CAPTURE_FRAMES).await;
    Capture { pcm }
}

async fn render_mix(case: SyncCase, provider: Provider) -> Capture {
    let mut harness = ProductHarness::new(case, provider, 0).await;
    for deck in &harness.decks {
        deck.set_muted(false);
    }
    harness.request_sync(case).await;

    let pcm = render_frames(&mut harness, case, CAPTURE_FRAMES).await;
    Capture { pcm }
}

async fn render_frames(harness: &mut ProductHarness, case: SyncCase, frames: usize) -> Vec<f32> {
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
async fn sync_listening_mix_is_not_quieter_than_a_solo_deck() {
    let case = DOWNTEMPO_HOUSE_SYNC;
    let provider = DOWNTEMPO_HOUSE_PROVIDER;
    let mut decks = Vec::with_capacity(case.decks());
    for deck in 0..case.decks() {
        let capture = render_solo(case, provider, deck).await;
        decks.push(CochleaReport::measure(
            &capture.pcm,
            CHANNELS,
            case.sample_rate,
        ));
    }
    let mix = render_mix(case, provider).await;
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
