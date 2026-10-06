//! A deck renders on the audio thread, which must never allocate: each block measured here
//! renders inside `assert_no_alloc`, so an allocation on the render path aborts the test.
#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::{NonZeroU32, NonZeroUsize};

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use firewheel::node::ProcBuffers;
use kithara_audio::mock::TestPcmReader;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_play::{CrossfadeSettings, Resource, SharedEq};
use kithara_render::{
    bridge::{DeckPart, SlotControl, TrackTransition, slot_channels},
    rt::{DeckMixer, DeckMixerConfig, StreamShape, track::PlayerResource},
};
use kithara_signal::{AudioSpec, SessionFrame};
use kithara_test_fixtures::integration_fixtures::constant_half;
use kithara_test_utils::{bufpool::pools, kithara};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const SAMPLE_RATE: NonZeroU32 = NonZeroU32::MIN.saturating_add(47_999);
const BLOCK: NonZeroU32 = NonZeroU32::MIN.saturating_add(127);
const ABSENT_FADES: usize = 4;

fn send(control: &mut SlotControl, part: DeckPart) {
    control.send(part).expect("the deck channel has room");
}

fn fade(transition: TrackTransition) -> DeckPart {
    DeckPart::Fade(transition)
}

#[kithara::test]
fn a_deck_applies_fades_for_tracks_it_does_not_hold_without_allocating(
    constant_half: &'static [u8],
) {
    let (inputs, mut control) = slot_channels(SharedEq::new(0));
    let shape = StreamShape {
        sample_rate: SAMPLE_RATE,
        max_block_frames: BLOCK,
    };
    let config = DeckMixerConfig::builder().slots(NonZeroUsize::MIN).build();
    let mut deck = DeckMixer::new(inputs, shape, &pools(), config);
    let settings = CrossfadeSettings::default();
    let spec = AudioSpec::new(2, SAMPLE_RATE);
    let resource = PlayerResource::new(
        Resource::from_reader(TestPcmReader::with_pcm(spec, 60.0, constant_half), None).into(),
        Arc::from("held.mp3"),
        &pools(),
    )
    .expect("player resource fits the test pool budget");
    let held = TrackId::allocate();

    let frames = usize::try_from(BLOCK.get()).expect("a block fits in memory");
    let mut out_l = vec![0.0f32; frames];
    let mut out_r = vec![0.0f32; frames];
    let mut render = |deck: &mut DeckMixer| {
        let inputs: [&[f32]; 0] = [];
        let mut outputs = [&mut out_l[..], &mut out_r[..]];
        let mut buffers = ProcBuffers {
            inputs: &inputs,
            outputs: &mut outputs,
        };
        deck.render_block(SessionFrame::default(), &mut buffers, frames);
    };

    send(
        &mut control,
        DeckPart::Attach {
            resource: Box::new(resource),
            item_id: held,
        },
    );
    send(
        &mut control,
        fade(TrackTransition::FadeIn {
            item_id: held,
            settings,
            epoch: 0,
        }),
    );
    render(&mut deck);

    send(
        &mut control,
        fade(TrackTransition::FadeIn {
            item_id: TrackId::allocate(),
            settings,
            epoch: 0,
        }),
    );
    for _ in 0..ABSENT_FADES {
        send(
            &mut control,
            fade(TrackTransition::FadeOut {
                item_id: TrackId::allocate(),
                settings,
            }),
        );
    }
    assert_no_alloc(|| render(&mut deck));
}
