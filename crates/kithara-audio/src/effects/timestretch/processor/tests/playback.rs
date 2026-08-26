use std::{collections::HashSet, num::NonZero};

use kithara_decode::PcmSpec;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

use super::{
    Consts, TimeStretchProcessor, chunk, dominant_bin, expected_bin, flush_serviced,
    process_serviced, processor, sine,
};
use crate::effects::timestretch::StretchControls;

fn keylocked(kind: StretchKind, speed: f32) -> TimeStretchProcessor {
    let controls = StretchControls::new(speed);
    controls.set_keylock(true);
    controls.set_backend(kind);
    processor(controls)
}

fn vinyl(kind: StretchKind, speed: f32) -> TimeStretchProcessor {
    let controls = StretchControls::new(speed);
    controls.set_keylock(false);
    controls.set_backend(kind);
    processor(controls)
}

fn render_with_tail(fx: &mut TimeStretchProcessor, input: &[f32]) -> (Vec<f32>, usize) {
    let mut out: Vec<f32> = Vec::new();
    let mut tail_frames = 0;
    let block = 4096 * usize::from(Consts::CH);
    for data in input.chunks(block) {
        if let Some(c) = process_serviced(fx, chunk(data)) {
            assert_eq!(
                c.spec().sample_rate.get(),
                Consts::SR,
                "stretch preserves sample rate"
            );
            assert_eq!(c.spec().channels, Consts::CH);
            out.extend_from_slice(&c.samples);
        }
    }
    while let Some(c) = flush_serviced(fx) {
        // A non-empty flush chunk carries real audio, so its spec must stay
        // the source spec - never the `PcmMeta::default()` sentinel (0
        // channels) that a `None` `last_input_meta` would otherwise yield.
        assert_eq!(c.spec().channels, Consts::CH, "flush preserves channels");
        assert_eq!(
            c.spec().sample_rate.get(),
            Consts::SR,
            "flush preserves sample rate"
        );
        tail_frames += c.frames();
        out.extend_from_slice(&c.samples);
    }
    (out, tail_frames)
}

fn render(fx: &mut TimeStretchProcessor, input: &[f32]) -> Vec<f32> {
    render_with_tail(fx, input).0
}

fn run_keylocked_with_tail(kind: StretchKind, speed: f32, in_frames: usize) -> (Vec<f32>, usize) {
    let input = sine(in_frames);
    render_with_tail(&mut keylocked(kind, speed), &input)
}

fn run_vinyl(kind: StretchKind, speed: f32, in_frames: usize) -> Vec<f32> {
    let input = sine(in_frames);
    render(&mut vinyl(kind, speed), &input)
}

/// Half playback speed -> stretch 2.0 -> ~double duration, pitch held.
/// Shared across every compiled-in backend.
fn assert_half_speed_contract(kind: StretchKind) {
    let channels = usize::from(Consts::CH);
    let in_frames = usize::try_from(Consts::SR).unwrap() * 2; // 2 s
    let (out, tail_frames) = run_keylocked_with_tail(kind, 0.5, in_frames);
    let out_frames = out.len() / channels;
    let timeline_frames = out_frames - tail_frames;
    let expected_timeline = in_frames * 2;

    assert_eq!(
        timeline_frames, expected_timeline,
        "{kind:?}: exact half-speed timeline"
    );
    assert!(tail_frames > 0, "{kind:?}: terminal history is drained");

    // Pitch is still measured over the complete emitted stream, including
    // the latency fill and its matching terminal drain.
    assert!(
        out_frames >= expected_timeline,
        "{kind:?}: terminal drain cannot shorten the exact timeline"
    );

    // Pitch preserved: dominant bin still at F0 (the load-bearing check -
    // a resampler-in-disguise would shift it).
    let mono: Vec<f32> = out.iter().step_by(channels).copied().collect();
    assert!(
        mono.len() >= Consts::N,
        "{kind:?}: not enough output for the FFT window"
    );
    let peak = dominant_bin(&mono);
    let want = expected_bin(Consts::F0);
    assert!(
        peak.abs_diff(want) <= 3,
        "{kind:?}: pitch moved under time-stretch: peak bin {peak}, expected {want}"
    );
}

fn assert_unity_contract(kind: StretchKind) {
    let in_frames = usize::try_from(Consts::SR).unwrap() * 2;
    let input = sine(in_frames);
    let out = render(&mut keylocked(kind, 1.0), &input);
    assert_eq!(out, input, "{kind:?}: unity speed must bypass byte-exact");
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn half_speed_and_unity_contracts(#[case] backend: StretchKind) {
    assert_half_speed_contract(backend);
    assert_unity_contract(backend);
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn output_meta_preserves_decoder_timeline(#[case] backend: StretchKind) {
    let channels = usize::from(Consts::CH);
    let mut fx = keylocked(backend, 0.5);
    let cf = 1024usize;
    let block = sine(cf);
    let mut fed_ends = HashSet::new();
    let mut emitted = Vec::new();
    for i in 0..40u64 {
        let mut c = chunk(&block);
        let end = Duration::from_millis(i * 100 + 100);
        c.meta.timestamp = Duration::from_millis(i * 100);
        c.meta.end_timestamp = end;
        c.meta.frame_offset = i * u64::try_from(cf).unwrap();
        fed_ends.insert(end);
        if let Some(o) = process_serviced(&mut fx, c) {
            emitted.push(o);
        }
    }
    while let Some(o) = flush_serviced(&mut fx) {
        emitted.push(o);
    }
    assert!(!emitted.is_empty(), "stretch produced no output");
    for o in &emitted {
        assert_eq!(
            o.spec(),
            PcmSpec {
                channels: Consts::CH,
                sample_rate: NonZero::new(Consts::SR).unwrap()
            },
            "spec (incl. sample rate) preserved verbatim"
        );
        assert_eq!(
            usize::try_from(o.meta.frames).unwrap(),
            o.samples.len() / channels,
            "frames recomputed to the actual output count"
        );
        assert!(
            fed_ends.contains(&o.meta.end_timestamp),
            "end_timestamp carried verbatim from an input chunk (source-track time)"
        );
    }
}

/// Key-lock off is vinyl mode: speed changes duration and pitch in the
/// stretch slot, with no resampler-rate handoff.
#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn vinyl_speed_scales_duration_and_pitch(#[case] backend: StretchKind) {
    let channels = usize::from(Consts::CH);
    let in_frames = usize::try_from(Consts::SR).unwrap() * 2;
    let out = run_vinyl(backend, 2.0, in_frames);
    let out_frames = out.len() / channels;
    assert!(
        out_frames * 10 >= in_frames * 4 && out_frames * 10 <= in_frames * 6,
        "vinyl 2x should roughly halve duration, got {out_frames} from {in_frames}"
    );
    let mono: Vec<f32> = out.iter().step_by(channels).copied().collect();
    assert!(
        mono.len() >= Consts::N,
        "not enough vinyl output for the FFT window"
    );
    let peak = dominant_bin(&mono);
    let want = expected_bin(Consts::F0 * 2.0);
    assert!(
        peak.abs_diff(want) <= 4,
        "vinyl pitch did not follow speed: peak bin {peak}, expected {want}"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn live_speed_change_updates_stretch_duration(#[case] backend: StretchKind) {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = processor(Arc::clone(&controls));
    let block = sine(4096);
    let unity = process_serviced(&mut fx, chunk(&block)).expect("unity bypass emits");
    assert_eq!(&unity.samples[..], &block[..], "unity phase bypasses");

    controls.set_speed(0.5);
    let mut stretched: Vec<f32> = Vec::new();
    for _ in 0..24 {
        if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
            stretched.extend_from_slice(&c.samples);
        }
    }
    while let Some(c) = flush_serviced(&mut fx) {
        stretched.extend_from_slice(&c.samples);
    }
    assert!(
        stretched.len() > block.len() * 24,
        "half-speed key-lock should lengthen output after a live speed change"
    );
}

/// Flipping key-lock mid-stream switches from vinyl pitch shift to
/// pitch-preserving stretch - no reload.
#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn live_keylock_toggle_switches_pitch_mode(#[case] backend: StretchKind) {
    let controls = StretchControls::new(0.5);
    controls.set_keylock(false);
    controls.set_backend(backend);
    let mut fx = processor(Arc::clone(&controls));
    let block = sine(4096);

    let mut vinyl_out: Vec<f32> = Vec::new();
    for _ in 0..24 {
        if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
            vinyl_out.extend_from_slice(&c.samples);
        }
    }
    let vinyl_mono: Vec<f32> = vinyl_out
        .iter()
        .step_by(usize::from(Consts::CH))
        .copied()
        .collect();
    assert!(
        dominant_bin(&vinyl_mono).abs_diff(expected_bin(Consts::F0 * 0.5)) <= 4,
        "off: vinyl pitch follows speed"
    );

    controls.set_keylock(true);
    let mut stretched: Vec<f32> = Vec::new();
    for _ in 0..24 {
        if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
            stretched.extend_from_slice(&c.samples);
        }
    }
    while let Some(c) = flush_serviced(&mut fx) {
        stretched.extend_from_slice(&c.samples);
    }
    let mono: Vec<f32> = stretched
        .iter()
        .step_by(usize::from(Consts::CH))
        .copied()
        .collect();
    assert!(
        mono.len() >= Consts::N,
        "on: not enough output for the FFT window"
    );
    assert!(
        dominant_bin(&mono).abs_diff(expected_bin(Consts::F0)) <= 3,
        "on: pitch preserved after live toggle"
    );
}
