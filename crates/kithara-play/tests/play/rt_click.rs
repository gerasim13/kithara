//! A click is a step in the waveform, so every test here renders PCM through the same calls
//! `process()` makes and measures the largest jump between neighbouring frames. The mock source is
//! constant DC: whatever step the render adds is the transport or the fade, never the material.
#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use firewheel::node::ProcBuffers;
use kithara_audio::mock::{TEST_PCM_DEFAULT_VALUE, TestPcmReader};
use kithara_command::{Batch, Outcome, When};
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_play::{
    Resource, SharedEq,
    bridge::{
        DeckApplied, DeckMixSettingsChange, DeckPart, SlotControl, TrackTransition, slot_channels,
    },
    rt::{
        DeckMixer, DeckMixerConfig, StreamShape,
        track::{PlayerResource, PlayerTrack},
    },
};
use kithara_signal::{AudioSpec, FaderValue, SessionFrame};
use kithara_test_fixtures::integration_fixtures::{constant_half, constant_quarter};
use kithara_test_utils::{bufpool::pools, kithara};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: usize = 128;
const TRACK_SECS: f64 = 60.0;
const SECOND_LEVEL: f32 = 0.25;
const FADE_SECONDS: f32 = 0.25;
const WARMUP_BLOCKS: usize = 24;
const SETTLE_BLOCKS: usize = 40;
const MAX_STEP: f32 = 0.01;
const EXACT: f32 = 1.0e-6;

fn crossfade(duration: f32) -> kithara_play::CrossfadeSettings {
    kithara_play::CrossfadeSettings {
        duration,
        ..kithara_play::CrossfadeSettings::default()
    }
}

fn spec() -> AudioSpec {
    AudioSpec::new(2, NonZeroU32::new(SAMPLE_RATE).expect("non-zero rate"))
}

fn processor() -> (DeckMixer, SlotControl) {
    let (inputs, control) = slot_channels(SharedEq::new(0));
    let shape = StreamShape {
        sample_rate: NonZeroU32::new(SAMPLE_RATE).expect("non-zero rate"),
        max_block_frames: NonZeroU32::new(128).expect("non-zero block"),
    };
    (
        DeckMixer::new(inputs, shape, &pools(), DeckMixerConfig::default()),
        control,
    )
}

fn track(src: &str, input: &'static [u8]) -> Box<PlayerResource> {
    Box::new(
        PlayerResource::new(
            Resource::from_reader(TestPcmReader::with_pcm(spec(), TRACK_SECS, input), None),
            Arc::from(src),
            &pools(),
        )
        .expect("player resource fits the test pool budget"),
    )
}

fn load(control: &mut SlotControl, src: &str, input: &'static [u8]) -> TrackId {
    let item_id = TrackId::allocate();
    push(
        control,
        DeckPart::Attach {
            resource: track(src, input),
            item_id,
        },
    );
    item_id
}

fn push(control: &mut SlotControl, part: DeckPart) {
    control.send(part).expect("the deck channel has room");
}

fn start(processor: &mut DeckMixer, item_id: TrackId) {
    if let Some(track) = processor.track_mut(item_id) {
        track.play();
    }
}

fn block_from(processor: &mut DeckMixer, start: SessionFrame) -> (Vec<f32>, bool) {
    let mut out_l = vec![0.0f32; BLOCK_FRAMES];
    let mut out_r = vec![0.0f32; BLOCK_FRAMES];
    let inputs: [&[f32]; 0] = [];
    let mut outputs = [&mut out_l[..], &mut out_r[..]];
    let mut buffers = ProcBuffers {
        inputs: &inputs,
        outputs: &mut outputs,
    };
    let read = processor.render_block(start, &mut buffers, BLOCK_FRAMES);
    (out_l, read)
}

/// Renders a block on a clock that stands still: every part `push` sends applies on the next
/// block, whatever frame it starts on.
fn block(processor: &mut DeckMixer) -> (Vec<f32>, bool) {
    block_from(processor, SessionFrame::default())
}

fn pump(processor: &mut DeckMixer, blocks: usize) -> Vec<f32> {
    let mut rendered = Vec::with_capacity(blocks * BLOCK_FRAMES);
    for _ in 0..blocks {
        let (out_l, _) = block(processor);
        rendered.extend_from_slice(&out_l);
    }
    rendered
}

fn last(rendered: &[f32]) -> f32 {
    rendered.last().copied().expect("blocks were rendered")
}

fn max_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .fold(0.0f32, |worst, pair| worst.max((pair[1] - pair[0]).abs()))
}

fn across(before: &[f32], after: &[f32]) -> Vec<f32> {
    let mut stream = vec![last(before)];
    stream.extend_from_slice(after);
    stream
}

#[kithara::test]
fn pausing_fades_the_output_out(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, "a.mp3", constant_half);
    push(&mut control, DeckPart::StartAll);
    block(&mut processor);
    start(&mut processor, item_id);

    let playing = pump(&mut processor, WARMUP_BLOCKS);
    assert!(
        (last(&playing) - TEST_PCM_DEFAULT_VALUE).abs() < EXACT,
        "the track plays at full level before the pause ({})",
        last(&playing)
    );

    push(&mut control, DeckPart::StopAll);
    let paused = pump(&mut processor, SETTLE_BLOCKS);

    let step = max_step(&across(&playing, &paused));
    assert!(
        step <= MAX_STEP,
        "pause must fade the output out, not cut the block (step {step})"
    );
    assert!(
        paused[paused.len() - BLOCK_FRAMES..]
            .iter()
            .all(|sample| *sample == 0.0),
        "a paused player still reaches silence"
    );
    let (_, still_reading) = block(&mut processor);
    assert!(
        !still_reading,
        "once the fade has run out the pause stops reading the tracks"
    );
}

#[kithara::test]
fn resuming_fades_the_output_in(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, "a.mp3", constant_half);
    push(&mut control, DeckPart::StartAll);
    block(&mut processor);
    start(&mut processor, item_id);
    pump(&mut processor, WARMUP_BLOCKS);

    push(&mut control, DeckPart::StopAll);
    let paused = pump(&mut processor, SETTLE_BLOCKS);
    assert!(last(&paused) == 0.0, "the pause settled at silence");

    push(&mut control, DeckPart::StartAll);
    let resumed = pump(&mut processor, SETTLE_BLOCKS);

    let step = max_step(&across(&paused, &resumed));
    assert!(
        step <= MAX_STEP,
        "resume must fade the output in, not step into it (step {step})"
    );
    assert!(
        (last(&resumed) - TEST_PCM_DEFAULT_VALUE).abs() < EXACT,
        "playback is back at full level ({})",
        last(&resumed)
    );
}

#[kithara::test]
fn a_start_inside_a_block_sounds_from_its_frame(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, "a.mp3", constant_half);
    block(&mut processor);
    start(&mut processor, item_id);
    assert!(
        pump(&mut processor, 2).iter().all(|sample| *sample == 0.0),
        "a stopped deck stays silent"
    );

    let origin = i64::try_from(BLOCK_FRAMES).expect("a block fits the clock");
    let offset = BLOCK_FRAMES / 2;
    let at = SessionFrame::new(origin + i64::try_from(offset).expect("an offset fits the clock"));
    let seq = control
        .deck
        .send(
            When::At(at),
            Batch {
                basis: Vec::new(),
                commands: vec![DeckPart::StartAll],
            },
        )
        .expect("the deck channel has room");
    let (started, _) = block_from(&mut processor, SessionFrame::new(origin));

    assert!(
        started[..offset].iter().all(|sample| *sample == 0.0),
        "the deck is silent before the frame it starts on"
    );
    assert!(
        started[offset..].iter().any(|sample| *sample > 0.0),
        "the deck sounds from the frame it starts on"
    );
    let answer = control
        .deck
        .receipts()
        .find(|receipt| receipt.seq() == seq)
        .expect("the start is answered in the block it applies in");
    assert!(
        matches!(answer.outcome(), Outcome::Applied { at: applied, .. } if *applied == at),
        "the start is applied at its frame"
    );
}

/// Two media positions read off one clock agree to well under a frame.
const SAME_POSITION: f64 = 1.0e-9;

fn seconds(frames: usize) -> f64 {
    f64::from(u32::try_from(frames).expect("a frame count fits the clock")) / f64::from(SAMPLE_RATE)
}

fn position(processor: &DeckMixer, item_id: TrackId) -> f64 {
    processor
        .track(item_id)
        .map(PlayerTrack::position)
        .expect("the deck holds the track")
}

#[kithara::test]
fn stopping_one_track_inside_a_block_holds_it_from_its_frame(
    constant_half: &'static [u8],
    constant_quarter: &'static [u8],
) {
    let (mut processor, mut control) = processor();
    let first = load(&mut control, "a.mp3", constant_half);
    let second = load(&mut control, "b.mp3", constant_quarter);
    push(&mut control, DeckPart::Start { item_id: first });
    push(&mut control, DeckPart::Start { item_id: second });
    let both = pump(&mut processor, WARMUP_BLOCKS);
    let mixed = TEST_PCM_DEFAULT_VALUE + SECOND_LEVEL;
    assert!(
        (last(&both) - mixed).abs() < EXACT,
        "both started tracks sound ({})",
        last(&both)
    );
    let before = position(&processor, first);

    let origin = i64::try_from(BLOCK_FRAMES).expect("a block fits the clock");
    let offset = BLOCK_FRAMES / 2;
    let at = SessionFrame::new(origin + i64::try_from(offset).expect("an offset fits the clock"));
    let seq = control
        .deck
        .send(
            When::At(at),
            Batch {
                basis: Vec::new(),
                commands: vec![DeckPart::Stop { item_id: first }],
            },
        )
        .expect("the deck channel has room");
    let (stopping, _) = block_from(&mut processor, SessionFrame::new(origin));

    assert!(
        stopping[..offset]
            .iter()
            .all(|sample| (*sample - mixed).abs() < EXACT),
        "both tracks sound up to the frame the stop applies on"
    );
    assert!(
        stopping[offset] < mixed,
        "the stopped track's share falls from the frame the stop applies on"
    );
    let answer = control
        .deck
        .receipts()
        .find(|receipt| receipt.seq() == seq)
        .expect("the stop is answered in the block it applies in");
    let expected = before + seconds(offset);
    assert!(
        matches!(
            answer.outcome(),
            Outcome::Applied {
                at: applied,
                data: DeckApplied { stopped_at: Some(stopped_at) },
            } if *applied == at && (*stopped_at - expected).abs() < SAME_POSITION
        ),
        "the stop is applied at its frame and reports where the track stood on it \
         ({expected}): {:?}",
        answer.outcome()
    );

    let stopped = pump(&mut processor, SETTLE_BLOCKS);
    let ramp = [stopping, stopped.clone()].concat();
    let step = max_step(&across(&both, &ramp));
    assert!(
        step <= MAX_STEP,
        "a stop ramps its track out, it does not cut it (step {step})"
    );
    assert!(
        ramp.iter().all(|sample| *sample >= SECOND_LEVEL - EXACT),
        "the other track keeps sounding through the stop"
    );
    assert!(
        (last(&stopped) - SECOND_LEVEL).abs() < EXACT,
        "the other track sounds alone once the ramp has run out ({})",
        last(&stopped)
    );
    let held = position(&processor, first);
    pump(&mut processor, SETTLE_BLOCKS);
    assert!(
        (position(&processor, first) - held).abs() < SAME_POSITION,
        "a stopped track is not read once its ramp has run out"
    );

    push(&mut control, DeckPart::Start { item_id: first });
    let resumed = pump(&mut processor, SETTLE_BLOCKS);
    let step = max_step(&across(&stopped, &resumed));
    assert!(
        step <= MAX_STEP,
        "a start ramps its track in, it does not step it (step {step})"
    );
    assert!(
        (last(&resumed) - mixed).abs() < EXACT,
        "the restarted track sounds with the other again ({})",
        last(&resumed)
    );
    assert!(
        (position(&processor, first) - (held + seconds(SETTLE_BLOCKS * BLOCK_FRAMES))).abs()
            < SAME_POSITION,
        "the restarted track plays on from where it held"
    );
}

/// Half the fader, a quarter of the amplitude: the deck sounds at the square of its volume.
const HALF_FADER_GAIN: f32 = 0.25;

fn mix(change: DeckMixSettingsChange) -> DeckPart {
    DeckPart::Mix(change)
}

fn half_volume() -> DeckPart {
    mix(DeckMixSettingsChange::Volume(FaderValue::from(0.5)))
}

#[kithara::test]
fn a_volume_change_inside_a_block_moves_the_gain_from_its_frame(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, "a.mp3", constant_half);
    push(&mut control, DeckPart::StartAll);
    block(&mut processor);
    start(&mut processor, item_id);
    let unity = pump(&mut processor, WARMUP_BLOCKS);
    assert!(
        (last(&unity) - TEST_PCM_DEFAULT_VALUE).abs() < EXACT,
        "the deck plays at unity before its volume changes ({})",
        last(&unity)
    );

    let origin = i64::try_from(BLOCK_FRAMES).expect("a block fits the clock");
    let offset = BLOCK_FRAMES / 2;
    let at = SessionFrame::new(origin + i64::try_from(offset).expect("an offset fits the clock"));
    let seq = control
        .deck
        .send(
            When::At(at),
            Batch {
                basis: Vec::new(),
                commands: vec![half_volume()],
            },
        )
        .expect("the deck channel has room");
    let (changed, _) = block_from(&mut processor, SessionFrame::new(origin));

    assert!(
        changed[..offset]
            .iter()
            .all(|sample| (*sample - TEST_PCM_DEFAULT_VALUE).abs() < EXACT),
        "the deck keeps its gain before the frame the change applies on"
    );
    assert!(
        changed[offset] < TEST_PCM_DEFAULT_VALUE,
        "the gain moves from the frame the change applies on"
    );
    let answer = control
        .deck
        .receipts()
        .find(|receipt| receipt.seq() == seq)
        .expect("the change is answered in the block it applies in");
    assert!(
        matches!(answer.outcome(), Outcome::Applied { at: applied, .. } if *applied == at),
        "the change is applied at its frame"
    );

    let settled = pump(&mut processor, SETTLE_BLOCKS);
    let step = max_step(&across(&unity, &[changed, settled.clone()].concat()));
    assert!(
        step <= MAX_STEP,
        "a volume change ramps the gain, it does not step it (step {step})"
    );
    assert!(
        (last(&settled) - TEST_PCM_DEFAULT_VALUE * HALF_FADER_GAIN).abs() < EXACT,
        "the deck settles at the square of its volume ({})",
        last(&settled)
    );
}

#[kithara::test]
fn a_muted_deck_is_silent_at_any_volume_and_unmutes_to_it(constant_half: &'static [u8]) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, "a.mp3", constant_half);
    push(&mut control, DeckPart::StartAll);
    block(&mut processor);
    start(&mut processor, item_id);
    let unity = pump(&mut processor, WARMUP_BLOCKS);

    push(&mut control, mix(DeckMixSettingsChange::Muted(true)));
    let muted = pump(&mut processor, SETTLE_BLOCKS);
    let step = max_step(&across(&unity, &muted));
    assert!(
        step <= MAX_STEP,
        "muting ramps the deck down, it does not cut it (step {step})"
    );
    assert!(last(&muted) == 0.0, "a muted deck reaches silence");

    push(&mut control, half_volume());
    assert!(
        pump(&mut processor, SETTLE_BLOCKS)
            .iter()
            .all(|sample| *sample == 0.0),
        "a volume change leaves a muted deck silent"
    );

    push(&mut control, mix(DeckMixSettingsChange::Muted(false)));
    let unmuted = pump(&mut processor, SETTLE_BLOCKS);
    assert!(
        (last(&unmuted) - TEST_PCM_DEFAULT_VALUE * HALF_FADER_GAIN).abs() < EXACT,
        "unmuting brings the deck back at the volume it was given while muted ({})",
        last(&unmuted)
    );
}

fn fading_in(constant_half: &'static [u8]) -> (DeckMixer, SlotControl, Vec<f32>, TrackId) {
    let (mut processor, mut control) = processor();
    let item_id = load(&mut control, "a.mp3", constant_half);
    push(&mut control, DeckPart::SetFadeDuration(FADE_SECONDS));
    push(&mut control, DeckPart::StartAll);
    push(
        &mut control,
        DeckPart::Fade(TrackTransition::FadeIn {
            item_id,
            settings: crossfade(FADE_SECONDS),
            epoch: 0,
        }),
    );

    let fading = pump(&mut processor, WARMUP_BLOCKS);
    let level = last(&fading);
    assert!(
        level > 0.0 && level < TEST_PCM_DEFAULT_VALUE * 0.9,
        "the fade-in is still climbing ({level})"
    );

    (processor, control, fading, item_id)
}

#[kithara::test]
fn reversing_a_fade_in_continues_from_the_gain_it_reached(constant_half: &'static [u8]) {
    let (mut processor, mut control, fading, item_id) = fading_in(constant_half);

    push(
        &mut control,
        DeckPart::Fade(TrackTransition::FadeOut {
            item_id,
            settings: crossfade(FADE_SECONDS),
        }),
    );
    let reversed = pump(&mut processor, SETTLE_BLOCKS * 3);

    let step = max_step(&across(&fading, &reversed));
    assert!(
        step <= MAX_STEP,
        "a cancelled fade-in fades out from the gain it reached, not from full level (step {step})"
    );
    assert!(
        last(&reversed) == 0.0,
        "the reversed fade still reaches silence ({})",
        last(&reversed)
    );
}

#[kithara::test]
fn reversing_a_fade_out_continues_from_the_gain_it_reached(constant_half: &'static [u8]) {
    let (mut processor, mut control, _, item_id) = fading_in(constant_half);
    pump(&mut processor, SETTLE_BLOCKS * 20);

    push(
        &mut control,
        DeckPart::Fade(TrackTransition::FadeOut {
            item_id,
            settings: crossfade(FADE_SECONDS),
        }),
    );
    let fading_out = pump(&mut processor, WARMUP_BLOCKS);
    let level = last(&fading_out);
    assert!(
        level > TEST_PCM_DEFAULT_VALUE * 0.1 && level < TEST_PCM_DEFAULT_VALUE,
        "the fade-out is still falling ({level})"
    );

    let reached = position(&processor, item_id);
    push(
        &mut control,
        DeckPart::Fade(TrackTransition::FadeIn {
            item_id,
            settings: crossfade(FADE_SECONDS),
            epoch: 0,
        }),
    );
    let reversed = pump(&mut processor, SETTLE_BLOCKS * 3);

    let step = max_step(&across(&fading_out, &reversed));
    assert!(
        step <= MAX_STEP,
        "a cancelled fade-out fades back in from the gain it reached, not from silence \
         (step {step})"
    );
    assert!(
        (position(&processor, item_id) - (reached + seconds(reversed.len()))).abs() < SAME_POSITION,
        "a fade-in does not seek: the track plays on from {reached} s, now at {} s",
        position(&processor, item_id)
    );
    assert!(
        (last(&reversed) - TEST_PCM_DEFAULT_VALUE).abs() < EXACT,
        "the reversed fade reaches full level ({})",
        last(&reversed)
    );
}

#[kithara::test]
fn seeking_a_fading_track_does_not_snap_the_mix(constant_half: &'static [u8]) {
    let (mut processor, mut control, fading, _) = fading_in(constant_half);

    let seek_epoch = processor.playback().next_seek_epoch();
    push(
        &mut control,
        DeckPart::Seek {
            seconds: 5.0,
            seek_epoch,
        },
    );
    let sought = pump(&mut processor, WARMUP_BLOCKS);

    let step = max_step(&across(&fading, &sought));
    assert!(
        step <= MAX_STEP,
        "a seek must not jump the mix of a fading track (step {step})"
    );
}

#[kithara::test]
fn resending_the_crossfade_duration_does_not_snap_the_mix(constant_half: &'static [u8]) {
    let (mut processor, mut control, fading, _) = fading_in(constant_half);

    push(&mut control, DeckPart::SetFadeDuration(FADE_SECONDS));
    let resent = pump(&mut processor, WARMUP_BLOCKS);

    let step = max_step(&across(&fading, &resent));
    assert!(
        step <= MAX_STEP,
        "an unchanged crossfade duration must leave the fade alone (step {step})"
    );
}

#[kithara::test]
fn changing_the_crossfade_duration_mid_fade_keeps_the_running_fade(constant_half: &'static [u8]) {
    let (mut processor, mut control, fading, _) = fading_in(constant_half);

    push(&mut control, DeckPart::SetFadeDuration(FADE_SECONDS / 10.0));
    let changed = pump(&mut processor, WARMUP_BLOCKS);

    let step = max_step(&across(&fading, &changed));
    assert!(
        step <= MAX_STEP,
        "a changed crossfade duration must leave the running fade alone (step {step})"
    );
    let level = last(&changed);
    assert!(
        level < TEST_PCM_DEFAULT_VALUE * 0.9,
        "the running fade keeps its original duration: it is still climbing after the change \
         ({level})"
    );
}

#[kithara::test]
fn a_changed_crossfade_duration_applies_to_the_next_fade(
    constant_half: &'static [u8],
    constant_quarter: &'static [u8],
) {
    let (mut processor, mut control, _, _) = fading_in(constant_half);
    let settled = pump(&mut processor, SETTLE_BLOCKS * 20);
    assert!(
        (last(&settled) - TEST_PCM_DEFAULT_VALUE).abs() < EXACT,
        "the first fade has settled under its original duration before the change ({})",
        last(&settled)
    );

    push(&mut control, DeckPart::SetFadeDuration(FADE_SECONDS / 10.0));
    let second_id = load(&mut control, "b.mp3", constant_quarter);
    push(
        &mut control,
        DeckPart::Fade(TrackTransition::FadeIn {
            item_id: second_id,
            settings: crossfade(FADE_SECONDS / 10.0),
            epoch: 0,
        }),
    );
    let handed_over = pump(&mut processor, SETTLE_BLOCKS * 3);
    assert!(
        (last(&handed_over) - SECOND_LEVEL).abs() < EXACT,
        "the next fade runs under the new duration: a tenth of the original settles within three \
         settle windows, the original would not ({})",
        last(&handed_over)
    );
}

#[kithara::test]
fn a_track_started_without_a_crossfade_is_instant(
    constant_quarter: &'static [u8],
    constant_half: &'static [u8],
) {
    let (mut processor, mut control) = processor();
    let first_id = load(&mut control, "a.mp3", constant_half);
    push(&mut control, DeckPart::SetFadeDuration(0.0));
    push(&mut control, DeckPart::StartAll);
    block(&mut processor);
    start(&mut processor, first_id);

    let playing = pump(&mut processor, WARMUP_BLOCKS);
    assert!(
        (last(&playing) - TEST_PCM_DEFAULT_VALUE).abs() < EXACT,
        "the first track plays at full level ({})",
        last(&playing)
    );

    let second_id = load(&mut control, "b.mp3", constant_quarter);
    block(&mut processor);
    start(&mut processor, second_id);
    let handover = pump(&mut processor, 1);

    assert!(
        (handover[0] - (TEST_PCM_DEFAULT_VALUE + SECOND_LEVEL)).abs() < EXACT,
        "the second track is at full level on its first frame ({})",
        handover[0]
    );
}
