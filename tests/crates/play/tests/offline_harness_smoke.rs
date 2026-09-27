#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    self,
    events::TrackId,
    platform::sync::Arc,
    play::Resource,
    warp::{StretchControls, StretchKind, WarpConfig},
};
use kithara_integration_tests::{
    offline::{OfflinePlayerHarness, OfflinePlayerOptions, resource_from_reader},
    test_defaults::consts,
};
use kithara_test_fixtures::integration_fixtures::constant_half;

const BLOCK_FRAMES: usize = 512;
/// 100 ms of stereo audio at 44.1 kHz: `4_410` frames × 2 channels.
const TARGET_SAMPLES: usize = 8_820;
const MAX_RENDERED_FRAMES: usize = 9_000;

fn make_resource(constant_half: &'static [u8], duration_secs: f64) -> Resource {
    resource_from_reader(
        kithara_integration_tests::audio_mock::TestPcmReader::from_pcm(
            consts::AUDIO_SPEC,
            duration_secs,
            constant_half,
        ),
    )
}

async fn render_target(harness: &OfflinePlayerHarness) -> Vec<f32> {
    let mut rendered: Vec<f32> = Vec::new();
    let mut total_frames: usize = 0;
    while rendered.len() < TARGET_SAMPLES && total_frames < MAX_RENDERED_FRAMES {
        let block = harness.render(BLOCK_FRAMES).await;
        rendered.extend_from_slice(&block);
        total_frames = total_frames.saturating_add(BLOCK_FRAMES);
        let _ = harness.tick_and_drain().await;
    }
    rendered
}

#[kithara::test(tokio)]
async fn offline_harness_smoke(constant_half: &'static [u8]) {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder().build(),
        consts::SAMPLE_RATE,
    )
    .await;
    harness
        .with_player(move |player| {
            player.insert(make_resource(constant_half, 0.1), TrackId::allocate(), None);
            player.insert(make_resource(constant_half, 0.1), TrackId::allocate(), None);
            player
                .select_item(0, kithara::play::SelectionPlayback::Play)
                .expect("select first queue item");
        })
        .await;

    let rendered = render_target(&harness).await;

    assert!(!rendered.is_empty());
    assert!(rendered.iter().any(|sample| sample.abs() > 0.0));
    harness.close().await;
}

/// Glide at speed 1.25 renders through the anti-alias filter on the audio
/// thread; `just test rtsan` runs it under the realtime sanitizer.
#[kithara::test(tokio)]
async fn offline_harness_glide_varispeed(constant_half: &'static [u8]) {
    let stretch = StretchControls::new(1.25);
    stretch.set_backend(StretchKind::Glide);
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .warp(WarpConfig::builder().stretch(Arc::clone(&stretch)).build())
            .build(),
        consts::SAMPLE_RATE,
    )
    .await;
    harness
        .with_player(move |player| {
            player.insert(make_resource(constant_half, 0.2), TrackId::allocate(), None);
            player
                .select_item(0, kithara::play::SelectionPlayback::Play)
                .expect("select first queue item");
        })
        .await;

    let rendered = render_target(&harness).await;

    assert!(rendered.iter().any(|sample| sample.abs() > 0.0));
    harness.close().await;
}
