#![cfg(not(target_arch = "wasm32"))]

//! A deck plays the item it was given and nothing else on its own: the next
//! item reaches it only when its owner arms or selects it.

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
/// A tenth of a second per item, so the first one ends a few blocks in.
const ITEM_FRAMES: usize = 4_410;
/// Several times the first item, so a self-advancing deck would be heard.
const BLOCKS: usize = 40;
const QUIET: f32 = 0.25;
const LOUD: f32 = 0.75;

fn constant_item(value: f32) -> Resource {
    let spec = AudioSpec::new(2, NonZeroU32::new(SAMPLE_RATE).expect("test rate"));
    resource_from_reader(TestPcmReader::with_samples(spec, vec![value; ITEM_FRAMES]))
}

#[kithara::test(tokio)]
async fn a_bare_deck_stops_at_the_end_of_its_item() {
    let harness = OfflinePlayer::with_sample_rate(
        OfflinePlayerOptions::builder()
            .block_on_underrun(true)
            .crossfade_duration(0.0)
            .build(),
        SAMPLE_RATE,
    )
    .await;
    harness
        .with_player(|player| {
            player.insert(constant_item(QUIET), TrackId::allocate(), None);
            player.insert(constant_item(LOUD), TrackId::allocate(), None);
            player
                .select_item(0, SelectionPlayback::Play)
                .expect("select the first item");
        })
        .await;

    let mut peak = 0.0_f32;
    for _ in 0..BLOCKS {
        let block = harness.render(BLOCK_FRAMES).await;
        peak = block.iter().map(|sample| sample.abs()).fold(peak, f32::max);
        let _ = harness.tick_and_drain().await;
    }
    let current = harness.with_player(|player| player.current_index()).await;
    harness.close().await;

    assert!(
        peak > QUIET * 0.5,
        "the selected item must be heard; peak={peak}"
    );
    assert!(
        peak < (QUIET + LOUD) / 2.0,
        "a deck without an owner must not play the next item; peak={peak}"
    );
    assert_eq!(current, 0, "a deck without an owner must keep its item");
}
