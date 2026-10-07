//! The audio thread reports trouble through lock-free counters, not `tracing`.
//!
//! Each test drives one failure branch of `process()` and asserts the matching counter moved, so a
//! change that reintroduces a log — or drops the signal altogether — fails here.
#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use firewheel::node::ProcBuffers;
use kithara_audio::mock::{Fault, MockReader, TEST_PCM_DEFAULT_VALUE, TestPcmReader};
use kithara_events::TrackId;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_play::Resource;
use kithara_render::{
    bridge::{DeckPart, RtMetricsSnapshot, SlotControl, TrackTransition, slot_channels},
    rt::{DeckMixer, DeckMixerConfig, StreamShape, track::PlayerResource},
};
use kithara_signal::{AudioSpec, SessionFrame};
use kithara_test_fixtures::integration_fixtures::constant_half;
use kithara_test_utils::{bufpool::pools, kithara};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: u32 = 128;
const CROSSFADE_SECONDS: f32 = 0.5;
const CROSSFADE_BLOCKS: usize = 8;
const AUDIBLE_FRACTION: f32 = 0.5;

fn crossfade(duration: f32) -> kithara_play::CrossfadeSettings {
    kithara_play::CrossfadeSettings {
        duration,
        ..kithara_play::CrossfadeSettings::default()
    }
}

fn block_len() -> usize {
    usize::try_from(BLOCK_FRAMES).expect("block frames fit usize")
}

fn spec() -> AudioSpec {
    AudioSpec::new(2, NonZeroU32::new(SAMPLE_RATE).expect("non-zero rate"))
}

fn processor() -> (DeckMixer, SlotControl) {
    let (inputs, control) = slot_channels();
    let shape = StreamShape {
        sample_rate: NonZeroU32::new(SAMPLE_RATE).expect("non-zero rate"),
        max_block_frames: NonZeroU32::new(BLOCK_FRAMES).expect("non-zero block"),
    };
    (
        DeckMixer::new(inputs, shape, &pools(), DeckMixerConfig::default()),
        control,
    )
}

fn faulty_track(src: &str, fault: Fault) -> Box<PlayerResource> {
    boxed(
        Resource::from_reader(MockReader::faulty(spec(), fault), None),
        src,
    )
}

fn healthy_track(constant_half: &'static [u8], src: &str) -> Box<PlayerResource> {
    boxed(
        Resource::from_reader(TestPcmReader::with_pcm(spec(), 60.0, constant_half), None),
        src,
    )
}

fn boxed(resource: Resource, src: &str) -> Box<PlayerResource> {
    Box::new(
        PlayerResource::new(resource.into(), Arc::from(src), &pools())
            .expect("player resource fits the test pool budget"),
    )
}

fn load(control: &mut SlotControl, resource: Box<PlayerResource>) -> TrackId {
    let item_id = TrackId::allocate();
    control.send(DeckPart::Attach { resource, item_id }).ok();
    item_id
}

/// Renders `blocks` blocks on a clock that stands still: every part sent applies on the next
/// block, whatever frame it starts on.
fn pump(processor: &mut DeckMixer, blocks: usize) -> Vec<f32> {
    let mut out_l = vec![0.0f32; block_len()];
    for _ in 0..blocks {
        let mut out_r = vec![0.0f32; block_len()];
        let inputs: [&[f32]; 0] = [];
        let mut outputs = [&mut out_l[..], &mut out_r[..]];
        let mut buffers = ProcBuffers {
            inputs: &inputs,
            outputs: &mut outputs,
        };
        processor.render_block(SessionFrame::default(), &mut buffers, block_len());
    }

    out_l
}

fn peak(rendered: &[f32]) -> f32 {
    rendered.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
}

fn render_loaded_blocks(resource: Box<PlayerResource>, blocks: usize) -> (DeckMixer, Vec<f32>) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, resource);
    control.send(DeckPart::StartAll).ok();
    pump(&mut processor, 1);

    if let Some(track) = processor.track_mut(item_id) {
        track.play();
    }

    let rendered = pump(&mut processor, blocks);

    (processor, rendered)
}

fn render_loaded(resource: Box<PlayerResource>) -> DeckMixer {
    render_loaded_blocks(resource, 1).0
}

fn metrics(processor: &DeckMixer) -> RtMetricsSnapshot {
    processor.playback().metrics().snapshot()
}

#[kithara::test]
fn decode_error_is_counted_not_logged() {
    let processor = render_loaded(faulty_track("broken.mp3", Fault::DecodeError));

    assert!(
        metrics(&processor).decode_errors() > 0,
        "a decode error inside process() must land in the counters"
    );
}

#[kithara::test]
fn source_with_nothing_ready_renders_silence_and_counts_an_underrun() {
    let (processor, rendered) = render_loaded_blocks(faulty_track("stalled.mp3", Fault::Stall), 4);

    assert!(
        metrics(&processor).underruns() > 0,
        "a zero-filled block short of EOF is an underrun"
    );
    let peak = peak(&rendered);
    assert!(
        peak == 0.0,
        "an underrun must render silence, not stale scratch (peak {peak})"
    );
}

/// A crossfade underruns the incoming track instead of waiting for its PCM.
///
/// The outgoing track keeps carrying the mix while the incoming one has nothing
/// ready. A `process()` that waits for the incoming PCM never returns from
/// `render_audio`, and one that mixes the stall over the outgoing track drops a
/// mix that was at full level one block earlier. The crossfade lasts
/// `CROSSFADE_SECONDS` and only `CROSSFADE_BLOCKS` of it are rendered, so the
/// outgoing gain has barely left 1.0 and the surviving level is the outgoing
/// material, not a residue of the fade.
#[kithara::test]
fn a_crossfade_into_a_stalled_track_underruns_instead_of_waiting(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let outgoing = load(&mut control, healthy_track(constant_half, "outgoing.mp3"));
    control.send(DeckPart::StartAll).ok();
    control
        .send(DeckPart::Fade(TrackTransition::FadeIn {
            item_id: outgoing,
            settings: crossfade(0.0),
            epoch: 0,
        }))
        .ok();
    pump(&mut processor, 1);

    let before = peak(&pump(&mut processor, CROSSFADE_BLOCKS));
    assert!(
        (before - TEST_PCM_DEFAULT_VALUE).abs() < f32::EPSILON,
        "the outgoing track plays at full level before the crossfade ({before})"
    );

    let incoming = load(&mut control, faulty_track("incoming.mp3", Fault::Stall));
    control
        .send(DeckPart::Fade(TrackTransition::FadeIn {
            item_id: incoming,
            settings: crossfade(CROSSFADE_SECONDS),
            epoch: 0,
        }))
        .ok();
    pump(&mut processor, 1);

    let during = peak(&pump(&mut processor, CROSSFADE_BLOCKS));

    assert!(
        metrics(&processor).underruns() > 0,
        "a crossfade into a source with nothing ready must count an underrun"
    );
    assert!(
        during >= before * AUDIBLE_FRACTION,
        "the outgoing track must keep carrying the mix while the incoming one \
         underruns (before {before}, during {during})"
    );
}

#[kithara::test]
fn a_healthy_track_reports_no_trouble(constant_half: &'static [u8]) {
    let processor = render_loaded(healthy_track(constant_half, "ok.mp3"));

    assert_eq!(metrics(&processor), RtMetricsSnapshot::default());
}

#[kithara::test]
fn a_seek_on_the_audio_thread_only_syncs_never_blocks() {
    let (reader, counts) = MockReader::seek_split(spec());
    let (mut processor, mut control) = processor();
    let item_id = load(
        &mut control,
        boxed(Resource::from_reader(reader, None), "split.mp3"),
    );
    pump(&mut processor, 1);

    if let Some(track) = processor.track_mut(item_id) {
        track.seek(30.0);
    }

    assert_eq!(
        counts.blocking_seeks(),
        0,
        "the audio thread must not reach the blocking seek"
    );
    assert_eq!(
        counts.syncs(),
        1,
        "it adopts the target that begin published"
    );
    assert_eq!(
        counts.begins(),
        0,
        "beginning belongs to the control thread, not to this call"
    );
    assert!(
        (processor.track(item_id).expect("track loaded").position() - 30.0).abs() < 0.001,
        "the media clock still re-bases on the new position"
    );
}

#[kithara::test]
fn the_slot_begins_seeks_for_the_tracks_it_shipped() {
    let (reader, counts) = MockReader::seek_split(spec());
    let resource = boxed(Resource::from_reader(reader, None), "split.mp3");
    let handle = resource.seek_handle().expect("reader splits its seek");
    let (_, mut control) = processor();
    let item_id = TrackId::allocate();

    control.bind_seek(item_id, Arc::clone(&handle));
    control.begin_seek(Duration::from_secs(30));
    assert_eq!(counts.begins(), 1);
    assert_eq!(counts.blocking_seeks(), 0);

    control.unbind_seek(item_id, &handle);
    control.begin_seek(Duration::from_secs(45));
    assert_eq!(
        counts.begins(),
        1,
        "an unloaded track must not be seeked any more"
    );
}

#[kithara::test]
fn unloading_one_seek_binding_preserves_other_identity() {
    let (first_reader, first_counts) = MockReader::seek_split(spec());
    let first = boxed(Resource::from_reader(first_reader, None), "same.mp3");
    let first_handle = first.seek_handle().expect("reader splits its seek");
    let first_id = TrackId::allocate();

    let (second_reader, second_counts) = MockReader::seek_split(spec());
    let second = boxed(Resource::from_reader(second_reader, None), "same.mp3");
    let second_handle = second.seek_handle().expect("reader splits its seek");
    let second_id = TrackId::allocate();

    let (_, mut control) = processor();
    control.bind_seek(first_id, Arc::clone(&first_handle));
    control.bind_seek(second_id, Arc::clone(&second_handle));
    control.unbind_seek(first_id, &first_handle);
    control.begin_seek(Duration::from_secs(30));

    assert_eq!(first_counts.begins(), 0, "the unloaded item stays detached");
    assert_eq!(
        second_counts.begins(),
        1,
        "the other queue item keeps its seek path despite sharing the URL"
    );

    let (replacement_reader, replacement_counts) = MockReader::seek_split(spec());
    let replacement = boxed(Resource::from_reader(replacement_reader, None), "same.mp3");
    let replacement_handle = replacement.seek_handle().expect("reader splits its seek");
    control.bind_seek(second_id, replacement_handle);
    control.unbind_seek(second_id, &second_handle);
    control.begin_seek(Duration::from_secs(45));

    assert_eq!(
        second_counts.begins(),
        1,
        "the retired resource generation stays detached"
    );
    assert_eq!(
        replacement_counts.begins(),
        1,
        "retiring the old generation must keep its replacement bound"
    );
}

#[kithara::test]
fn evicting_an_audible_track_is_counted(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();

    for idx in 0..DeckMixerConfig::default().slots().get() {
        let src = format!("track-{idx}.mp3");
        let item_id = load(&mut control, healthy_track(constant_half, &src));
        pump(&mut processor, 1);
        if let Some(track) = processor.track_mut(item_id) {
            track.play();
        }
    }

    load(&mut control, healthy_track(constant_half, "newcomer.mp3"));
    pump(&mut processor, 1);

    assert!(
        metrics(&processor).evicted_playing() > 0,
        "dropping an audible track to make room is a real defect and must stay visible"
    );
}

#[kithara::test]
fn a_block_larger_than_declared_is_clamped_not_grown(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, healthy_track(constant_half, "ok.mp3"));
    control.send(DeckPart::StartAll).ok();
    pump(&mut processor, 1);
    if let Some(track) = processor.track_mut(item_id) {
        track.play();
    }

    let oversized = block_len() * 2;
    let mut out_l = vec![f32::NAN; oversized];
    let mut out_r = vec![f32::NAN; oversized];

    let inputs: [&[f32]; 0] = [];

    let mut outputs = [&mut out_l[..], &mut out_r[..]];

    let mut buffers = ProcBuffers {
        inputs: &inputs,
        outputs: &mut outputs,
    };

    let rendered = processor.render_block(SessionFrame::default(), &mut buffers, oversized);

    assert!(rendered, "the declared part of the block still renders");
    assert!(
        out_l[..block_len()].iter().all(|s| s.is_finite()),
        "frames up to max_block_frames are written"
    );
    assert!(
        out_l[block_len()..].iter().all(|s| *s == 0.0),
        "frames beyond the declared block are silence, since the host is told the \
         whole block is valid"
    );
}
