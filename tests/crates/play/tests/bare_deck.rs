#![cfg(not(target_arch = "wasm32"))]

//! A deck plays the item it was given and nothing else on its own: past the
//! item's end it still leads that item until its owner selects another.

use std::num::NonZeroU32;

use kithara::{
    audio::mock::TestPcmReader,
    events::TrackId,
    play::{Resource, SelectionPlayback},
    signal::AudioSpec,
};
use kithara_integration_tests::offline::{
    OfflinePlayer, OfflinePlayerOptions, resource_from_reader,
};

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
/// A tenth of a second, so the item ends a few blocks in.
const ITEM_FRAMES: usize = 4_410;
/// Several times the item, so the deck renders well past its end.
const BLOCKS: usize = 40;
const LEVEL: f32 = 0.25;

fn constant_item(value: f32) -> Resource {
    let spec = AudioSpec::new(2, NonZeroU32::new(SAMPLE_RATE).expect("test rate"));
    resource_from_reader(TestPcmReader::with_samples(spec, vec![value; ITEM_FRAMES]))
}

#[kithara::test(tokio)]
async fn a_bare_deck_keeps_its_item_past_the_end() {
    let harness = OfflinePlayer::with_sample_rate(
        OfflinePlayerOptions::builder()
            .block_on_underrun(true)
            .crossfade_duration(0.0)
            .build(),
        SAMPLE_RATE,
    )
    .await;
    let item = TrackId::allocate();
    harness
        .with_player(move |player| {
            player
                .select(item, Some(constant_item(LEVEL)), SelectionPlayback::Play)
                .expect("select the item");
        })
        .await;

    let mut peak = 0.0_f32;
    for _ in 0..BLOCKS {
        let block = harness.render(BLOCK_FRAMES).await;
        peak = block.iter().map(|sample| sample.abs()).fold(peak, f32::max);
        let _ = harness.tick_and_drain().await;
    }
    let current = harness.with_player(|player| player.current_item()).await;
    harness.close().await;

    assert!(
        peak > LEVEL * 0.5,
        "the selected item must be heard; peak={peak}"
    );
    assert_eq!(
        current,
        Some(item),
        "a deck without an owner must keep its item"
    );
}
