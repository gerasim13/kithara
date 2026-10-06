#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    reason = "test fixture values are small positive integers/floats"
)]

use std::{
    num::{NonZeroU32, NonZeroUsize},
    sync::atomic::Ordering as AtomicOrdering,
};

use firewheel::node::ProcBuffers;
use kithara_audio::mock::{MockReader, TestPcmReader};
use kithara_events::TrackId;
use kithara_platform::{
    sync::{Arc, Mutex},
    time::Duration,
};
use kithara_play::{
    PlayerNotification, Resource, SharedEq, TrackState, TrackTransition,
    bridge::{DeckPart, SlotControl, slot_channels},
    rt::{DeckMixer, DeckMixerConfig, StreamShape, track::PlayerResource},
};
use kithara_signal::SessionFrame;
use kithara_test_fixtures::integration_fixtures::constant_half;
use kithara_test_utils::{bufpool::pools, kithara};
use ringbuf::traits::Consumer;

use crate::support::{AUDIO_SPEC, SAMPLE_RATE};

#[derive(Clone, Copy)]
enum TrackCommandScenario {
    DuplicateLoad,
    LoadOnly,
    LoadThenUnload,
}

const MAX_BLOCK_FRAMES: u32 = 1024;

fn stream_shape(sample_rate: NonZeroU32) -> StreamShape {
    StreamShape {
        sample_rate,
        max_block_frames: NonZeroU32::new(MAX_BLOCK_FRAMES).expect("BUG: non-zero"),
    }
}

fn make_processor() -> (DeckMixer, SlotControl) {
    let (inputs, control) = slot_channels(SharedEq::new(0));
    let processor = DeckMixer::new(
        inputs,
        stream_shape(SAMPLE_RATE),
        &pools(),
        DeckMixerConfig::default(),
    );
    (processor, control)
}

/// Renders one block of `frames` on a clock that stands still, so every part sent so far
/// applies at its start. Returns whether a track was read and both channels.
fn render(processor: &mut DeckMixer, frames: usize) -> (bool, Vec<f32>, Vec<f32>) {
    let mut out_l = vec![99.0f32; frames];
    let mut out_r = vec![99.0f32; frames];
    let inputs: [&[f32]; 0] = [];
    let mut outputs = [&mut out_l[..], &mut out_r[..]];
    let mut buffers = ProcBuffers {
        inputs: &inputs,
        outputs: &mut outputs,
    };
    let rendered = processor.render_block(SessionFrame::default(), &mut buffers, frames);
    (rendered, out_l, out_r)
}

/// Applies every part sent so far through one block of the deck.
fn block(processor: &mut DeckMixer) {
    render(processor, MAX_BLOCK_FRAMES as usize);
}

fn create_mock_player_resource(constant_half: &'static [u8], src: &str) -> Box<PlayerResource> {
    create_mock_player_resource_with_duration(constant_half, src, 60.0)
}

fn create_mock_player_resource_with_duration(
    constant_half: &'static [u8],
    src: &str,
    duration_secs: f64,
) -> Box<PlayerResource> {
    let reader = TestPcmReader::with_pcm(AUDIO_SPEC, duration_secs, constant_half);
    let resource = Resource::from_reader(reader, None);
    Box::new(
        PlayerResource::new(resource, Arc::from(src), &pools())
            .expect("player resource fits the test pool budget"),
    )
}

fn create_duration_player_resource(src: &str, duration: Duration) -> Box<PlayerResource> {
    let (reader, _recorded) = MockReader::sample_rate_tracking_with_duration(AUDIO_SPEC, duration);
    let resource = Resource::from_reader(reader, None);
    Box::new(
        PlayerResource::new(resource, Arc::from(src), &pools())
            .expect("player resource fits the test pool budget"),
    )
}

fn create_tracking_player_resource(
    src: &str,
    seek_log: Arc<Mutex<Vec<u64>>>,
) -> Box<PlayerResource> {
    let resource = Resource::from_reader(MockReader::seek_tracking(seek_log), None);
    Box::new(
        PlayerResource::new(resource, Arc::from(src), &pools())
            .expect("player resource fits the test pool budget"),
    )
}

#[kithara::test(tokio)]
async fn load_track_propagates_host_sample_rate() {
    let host_rate = 88_200u32;
    let (reader, recorded) = MockReader::sample_rate_tracking(AUDIO_SPEC);
    let resource = Resource::from_reader(reader, None);
    let player_resource = Box::new(
        PlayerResource::new(resource, Arc::from("track.mp3"), &pools())
            .expect("player resource fits the test pool budget"),
    );

    let (inputs, mut control) = slot_channels(SharedEq::new(0));
    let sample_rate = NonZeroU32::new(host_rate).expect("BUG: non-zero");
    let mut processor = DeckMixer::new(
        inputs,
        stream_shape(sample_rate),
        &pools(),
        DeckMixerConfig::default(),
    );

    control
        .send(DeckPart::Attach {
            resource: player_resource,
            item_id: TrackId::allocate(),
        })
        .ok();
    block(&mut processor);

    assert_eq!(recorded.load(AtomicOrdering::Relaxed), host_rate);
}

#[kithara::test]
fn processor_renders_silence_when_no_tracks() {
    let (processor, _control) = make_processor();
    assert_eq!(processor.track_count(), 0);
}

#[kithara::test]
fn processor_seek_without_tracks_does_not_panic() {
    let (mut processor, mut control) = make_processor();
    control
        .send(DeckPart::Seek {
            seconds: 30.0,
            seek_epoch: 1,
        })
        .ok();
    block(&mut processor);
}

/// An empty deck stops itself on its next block, so the deck holds a track.
#[kithara::test(tokio)]
async fn start_and_stop_switch_a_loaded_deck() {
    let (mut processor, mut control) = make_processor();
    control
        .send(DeckPart::Attach {
            resource: create_duration_player_resource("track.mp3", Duration::from_secs(60)),
            item_id: TrackId::allocate(),
        })
        .ok();

    control.send(DeckPart::StartAll).ok();
    block(&mut processor);
    assert!(processor.playback().playing.load(AtomicOrdering::SeqCst));

    control.send(DeckPart::StopAll).ok();
    block(&mut processor);
    assert!(!processor.playback().playing.load(AtomicOrdering::SeqCst));
}

#[kithara::test(tokio)]
async fn processor_clear_unloads_tracks_and_resets_snapshot() {
    let (mut processor, mut control) = make_processor();
    let item_id = TrackId::allocate();

    control
        .send(DeckPart::Attach {
            resource: create_duration_player_resource("track.mp3", Duration::from_secs(60)),
            item_id,
        })
        .ok();
    control
        .send(DeckPart::Fade(TrackTransition::FadeIn {
            item_id,
            settings: kithara_play::CrossfadeSettings::default(),
            epoch: 0,
        }))
        .ok();
    block(&mut processor);
    assert_eq!(processor.track_count(), 1);

    processor
        .playback()
        .playing
        .store(true, AtomicOrdering::SeqCst);
    assert_eq!(processor.playback().snapshot().duration(), 60.0);

    control.send(DeckPart::Clear).ok();
    block(&mut processor);

    assert_eq!(
        processor.track_count(),
        0,
        "arena must be empty after Clear"
    );
    assert_eq!(processor.playback().snapshot().position(), 0.0);
    assert_eq!(processor.playback().snapshot().duration(), 0.0);
    assert!(!processor.playback().playing.load(AtomicOrdering::SeqCst));
}

#[kithara::test(tokio)]
async fn a_fade_in_makes_its_track_leading_and_a_preload_does_not() {
    let (mut processor, mut control) = make_processor();
    let first_src: Arc<str> = Arc::from("first.mp3");
    let second_src: Arc<str> = Arc::from("second.mp3");
    let first_id = TrackId::allocate();
    let second_id = TrackId::allocate();

    control
        .send(DeckPart::Attach {
            resource: create_duration_player_resource(&first_src, Duration::from_secs(64)),
            item_id: first_id,
        })
        .ok();
    control
        .send(DeckPart::Fade(TrackTransition::FadeIn {
            item_id: first_id,
            settings: kithara_play::CrossfadeSettings::default(),
            epoch: 0,
        }))
        .ok();
    block(&mut processor);

    assert_eq!(processor.playback().snapshot().duration(), 64.0);

    control
        .send(DeckPart::Attach {
            resource: create_duration_player_resource(&second_src, Duration::from_secs(162)),
            item_id: second_id,
        })
        .ok();
    block(&mut processor);

    assert_eq!(
        processor.playback().snapshot().duration(),
        64.0,
        "preload must not publish the next track duration"
    );

    control
        .send(DeckPart::Fade(TrackTransition::FadeIn {
            item_id: second_id,
            settings: kithara_play::CrossfadeSettings::default(),
            epoch: 0,
        }))
        .ok();
    block(&mut processor);

    assert_eq!(processor.playback().snapshot().position(), 0.0);
    assert_eq!(processor.playback().snapshot().duration(), 162.0);
}

#[kithara::test(tokio)]
async fn processor_multiple_seek_epochs_only_last_applies() {
    let seek_log = Arc::new(Mutex::new(Vec::new()));
    let resource = create_tracking_player_resource("track1.mp3", seek_log.clone());

    let (mut processor, mut control) = make_processor();
    let item_id = TrackId::allocate();
    control.send(DeckPart::Attach { resource, item_id }).ok();
    block(&mut processor);
    control
        .send(DeckPart::Fade(TrackTransition::FadeIn {
            item_id,
            settings: kithara_play::CrossfadeSettings::default(),
            epoch: 0,
        }))
        .ok();
    block(&mut processor);

    let playback = processor.playback().clone();
    let first = playback.next_seek_epoch();
    playback.seek_epoch.store(first, AtomicOrdering::SeqCst);
    control
        .send(DeckPart::Seek {
            seconds: 10.0,
            seek_epoch: first,
        })
        .ok();
    let second = playback.next_seek_epoch();
    playback.seek_epoch.store(second, AtomicOrdering::SeqCst);
    control
        .send(DeckPart::Seek {
            seconds: 20.0,
            seek_epoch: second,
        })
        .ok();
    let third = playback.next_seek_epoch();
    playback.seek_epoch.store(third, AtomicOrdering::SeqCst);
    control
        .send(DeckPart::Seek {
            seconds: 30.0,
            seek_epoch: third,
        })
        .ok();

    block(&mut processor);

    // Only the current epoch re-bases the track: the two superseded commands are dropped, so the
    // media clock lands on the last target rather than replaying every one of them.
    let position = processor
        .track(item_id)
        .expect("BUG: track must stay loaded")
        .position();
    assert!(
        (position - 30.0).abs() < 0.001,
        "stale seek epochs must not move the media clock, got {position}"
    );
    assert!(
        seek_log.lock().is_empty(),
        "the audio thread must not reach the reader's blocking seek"
    );
    assert_eq!(playback.seek_epoch.load(AtomicOrdering::SeqCst), third);
}

#[kithara::test(tokio)]
#[case(TrackCommandScenario::LoadOnly, 1, true)]
#[case(TrackCommandScenario::DuplicateLoad, 1, true)]
#[case(TrackCommandScenario::LoadThenUnload, 0, false)]
async fn processor_track_command_scenarios(
    constant_half: &'static [u8],
    #[case] scenario: TrackCommandScenario,
    #[case] expected_tracks: usize,
    #[case] should_contain_track: bool,
) {
    let (mut processor, mut control) = make_processor();
    let item_id = TrackId::allocate();

    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource(constant_half, "track1.mp3"),
            item_id,
        })
        .ok();

    match scenario {
        TrackCommandScenario::LoadOnly => {}
        TrackCommandScenario::DuplicateLoad => {
            control
                .send(DeckPart::Attach {
                    resource: create_mock_player_resource(constant_half, "track1.mp3"),
                    item_id,
                })
                .ok();
        }
        TrackCommandScenario::LoadThenUnload => {
            control.send(DeckPart::Detach { item_id }).ok();
        }
    }

    block(&mut processor);

    assert_eq!(processor.track_count(), expected_tracks);
    assert_eq!(processor.track(item_id).is_some(), should_contain_track);

    if matches!(scenario, TrackCommandScenario::DuplicateLoad) {
        let mut loaded = 0usize;
        let mut unloaded = false;
        while let Some(notification) = control.notif_rx.try_pop() {
            match notification {
                PlayerNotification::Loaded { .. } => loaded += 1,
                PlayerNotification::Unloaded { .. } => unloaded = true,
                _ => {}
            }
        }
        assert!(unloaded);
        assert!(loaded >= 2);
    }
}

#[kithara::test(tokio)]
#[case::one(1)]
#[case::two(2)]
async fn a_deck_holds_as_many_tracks_as_its_config_gives_it_slots(
    constant_half: &'static [u8],
    #[case] slots: usize,
) {
    let config = DeckMixerConfig::builder()
        .slots(NonZeroUsize::new(slots).expect("a test deck has a slot"))
        .build();
    let (inputs, mut control) = slot_channels(SharedEq::new(0));
    let mut processor = DeckMixer::new(inputs, stream_shape(SAMPLE_RATE), &pools(), config);
    let ids: Vec<TrackId> = (0..=slots).map(|_| TrackId::allocate()).collect();

    for (idx, &item_id) in ids.iter().enumerate() {
        let resource = create_mock_player_resource(constant_half, &format!("track-{idx}.mp3"));
        control
            .send(DeckPart::Attach { resource, item_id })
            .expect("the deck channel has room for one attach a block");
        block(&mut processor);
    }

    assert_eq!(
        processor.track_count(),
        slots,
        "a deck holds one track per configured slot, never more"
    );
    assert!(
        ids.last()
            .is_some_and(|&newest| processor.track(newest).is_some()),
        "the newest attach takes the slot an older track gave up"
    );
}

#[kithara::test(tokio)]
async fn processor_cleanup_finished_tracks(constant_half: &'static [u8]) {
    let (mut processor, mut control) = make_processor();

    let resource = create_mock_player_resource(constant_half, "track1.mp3");
    let item_id = TrackId::allocate();
    control.send(DeckPart::Attach { resource, item_id }).ok();
    block(&mut processor);

    if let Some(track) = processor.track_mut(item_id) {
        track.finish();
    }

    block(&mut processor);
    assert_eq!(processor.track_count(), 0);
}

#[kithara::test(tokio)]
async fn render_audio_handover_fills_tail_from_next_playing_track(constant_half: &'static [u8]) {
    let (mut processor, mut control) = make_processor();
    let short_id = TrackId::allocate();
    let long_id = TrackId::allocate();
    let frames = 1024usize;

    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource_with_duration(constant_half, "short.mp3", 0.01),
            item_id: short_id,
        })
        .ok();
    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource(constant_half, "long.mp3"),
            item_id: long_id,
        })
        .ok();
    control.send(DeckPart::StartAll).ok();
    block(&mut processor);

    processor
        .track_mut(short_id)
        .expect("BUG: short track must be loaded")
        .play();
    processor
        .track_mut(long_id)
        .expect("BUG: long track must be loaded")
        .play();

    let (rendered, out_l, out_r) = render(&mut processor, frames);

    assert!(rendered);
    assert!(
        out_l
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON)
    );
    assert!(
        out_r
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON)
    );
}

#[kithara::test(tokio)]
async fn render_audio_handover_promotes_preloading_track_without_silence(
    constant_half: &'static [u8],
) {
    let (mut processor, mut control) = make_processor();
    let short_id = TrackId::allocate();
    let preload_id = TrackId::allocate();
    let frames = 1024usize;

    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource_with_duration(constant_half, "short.mp3", 0.01),
            item_id: short_id,
        })
        .ok();
    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource(constant_half, "preload.mp3"),
            item_id: preload_id,
        })
        .ok();
    control
        .send(DeckPart::Chain {
            from: short_id,
            to: preload_id,
            epoch: 0,
        })
        .ok();
    control.send(DeckPart::StartAll).ok();
    block(&mut processor);

    processor
        .track_mut(short_id)
        .expect("BUG: short track must be loaded")
        .play();

    let (rendered, out_l, out_r) = render(&mut processor, frames);

    assert!(rendered);
    assert!(
        out_l
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON)
    );
    assert!(
        out_r
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON)
    );
    assert_eq!(
        processor
            .track(preload_id)
            .expect("BUG: preloading track must remain loaded")
            .state(),
        TrackState::Playing
    );
}

/// A track that ends starts the successor chained to it on the frame after its last, whatever
/// else the deck holds preloaded.
#[kithara::test(tokio)]
async fn an_ending_track_starts_only_the_track_chained_to_it(constant_half: &'static [u8]) {
    let (mut processor, mut control) = make_processor();
    let leading_id = TrackId::allocate();
    let other_id = TrackId::allocate();
    let chained_id = TrackId::allocate();

    for (src, secs, item_id) in [
        ("leading.mp3", 0.01, leading_id),
        ("other.mp3", 60.0, other_id),
        ("chained.mp3", 60.0, chained_id),
    ] {
        control
            .send(DeckPart::Attach {
                resource: create_mock_player_resource_with_duration(constant_half, src, secs),
                item_id,
            })
            .ok();
    }
    control
        .send(DeckPart::Chain {
            from: leading_id,
            to: chained_id,
            epoch: 0,
        })
        .ok();
    control.send(DeckPart::StartAll).ok();
    block(&mut processor);
    processor
        .track_mut(leading_id)
        .expect("BUG: leading track must be loaded")
        .play();

    let (rendered, out_l, out_r) = render(&mut processor, MAX_BLOCK_FRAMES as usize);

    assert!(rendered);
    assert!(
        out_l
            .iter()
            .chain(&out_r)
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "the chained track sounds from the frame after the leading track's last"
    );
    let state = |item_id| {
        processor
            .track(item_id)
            .map(kithara_play::rt::track::PlayerTrack::state)
    };
    assert_eq!(state(chained_id), Some(TrackState::Playing));
    assert_eq!(state(other_id), Some(TrackState::Preloading));
}

/// A stitched-in successor that ends before its stitch block does hands the
/// rest of the block to the next preloaded track, as a longer one would at
/// its own end: once it has ended, no leading track is left to stitch it.
#[kithara::test(tokio)]
async fn render_audio_handover_continues_past_a_preload_that_ends_in_its_stitch_block(
    constant_half: &'static [u8],
) {
    let (mut processor, mut control) = make_processor();
    let leading_id = TrackId::allocate();
    let short_preload_id = TrackId::allocate();
    let preload_id = TrackId::allocate();
    let frames = 1024usize;

    for (src, secs, item_id) in [
        ("leading.mp3", 0.01, leading_id),
        ("short-preload.mp3", 0.005, short_preload_id),
        ("preload.mp3", 60.0, preload_id),
    ] {
        control
            .send(DeckPart::Attach {
                resource: create_mock_player_resource_with_duration(constant_half, src, secs),
                item_id,
            })
            .ok();
    }
    for (from, to) in [
        (leading_id, short_preload_id),
        (short_preload_id, preload_id),
    ] {
        control.send(DeckPart::Chain { from, to, epoch: 0 }).ok();
    }
    control.send(DeckPart::StartAll).ok();
    block(&mut processor);
    processor
        .track_mut(leading_id)
        .expect("BUG: leading track must be loaded")
        .play();

    let (rendered, out_l, out_r) = render(&mut processor, frames);

    assert!(rendered);
    assert!(
        out_l
            .iter()
            .chain(&out_r)
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "the block is filled end to end"
    );
    assert_eq!(
        processor
            .track(preload_id)
            .map(kithara_play::rt::track::PlayerTrack::state),
        Some(TrackState::Playing)
    );
}

/// The control side withdraws an armed successor without knowing whether the
/// leading track has already ended and stitched it in. Only a successor still
/// preloading leaves the arena; one already playing keeps playing.
#[kithara::test(tokio)]
#[case::stitched_in(0.01, Some(TrackState::Playing))]
#[case::still_preloading(60.0, None)]
async fn cancel_preload_unloads_a_successor_only_while_it_preloads(
    constant_half: &'static [u8],
    #[case] leading_secs: f64,
    #[case] after_cancel: Option<TrackState>,
) {
    let (mut processor, mut control) = make_processor();
    let leading_id = TrackId::allocate();
    let successor_id = TrackId::allocate();
    let frames = 1024usize;

    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource_with_duration(
                constant_half,
                "leading.mp3",
                leading_secs,
            ),
            item_id: leading_id,
        })
        .ok();
    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource(constant_half, "successor.mp3"),
            item_id: successor_id,
        })
        .ok();
    control
        .send(DeckPart::Chain {
            from: leading_id,
            to: successor_id,
            epoch: 0,
        })
        .ok();
    control.send(DeckPart::StartAll).ok();
    block(&mut processor);
    processor
        .track_mut(leading_id)
        .expect("BUG: leading track must be loaded")
        .play();

    let (rendered, ..) = render(&mut processor, frames);
    assert!(rendered, "the leading track renders");

    control
        .send(DeckPart::Withdraw {
            item_id: successor_id,
        })
        .ok();
    block(&mut processor);

    assert_eq!(
        processor
            .track(successor_id)
            .map(kithara_play::rt::track::PlayerTrack::state),
        after_cancel
    );
}

#[kithara::test(tokio)]
async fn render_audio_handover_does_not_reuse_fading_out_track_tail(constant_half: &'static [u8]) {
    let (mut processor, mut control) = make_processor();
    let short_id = TrackId::allocate();
    let fading_id = TrackId::allocate();
    let preload_id = TrackId::allocate();
    let frames = 1024usize;

    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource_with_duration(constant_half, "short.mp3", 0.01),
            item_id: short_id,
        })
        .ok();
    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource(constant_half, "fading.mp3"),
            item_id: fading_id,
        })
        .ok();
    control
        .send(DeckPart::Attach {
            resource: create_mock_player_resource(constant_half, "preload.mp3"),
            item_id: preload_id,
        })
        .ok();
    control
        .send(DeckPart::Chain {
            from: short_id,
            to: preload_id,
            epoch: 0,
        })
        .ok();
    control.send(DeckPart::StartAll).ok();
    block(&mut processor);

    processor
        .track_mut(short_id)
        .expect("BUG: short track must be loaded")
        .play();
    processor
        .track_mut(fading_id)
        .expect("BUG: fading track must be loaded")
        .play();
    processor
        .track_mut(fading_id)
        .expect("BUG: fading track must remain loaded")
        .fade_out(kithara_play::CrossfadeSettings::default());

    let (rendered, ..) = render(&mut processor, frames);

    assert!(rendered);
    assert_eq!(
        processor
            .track(preload_id)
            .expect("BUG: preloading track must remain loaded")
            .state(),
        TrackState::Playing
    );
}
