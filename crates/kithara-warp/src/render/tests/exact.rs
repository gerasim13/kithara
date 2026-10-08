use std::num::NonZeroU64;

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioChunkInfo};
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

use super::{WarpRenderer, chunk, renderer, spec};
use crate::{GridSegment, RegionPlan, SpeedCurve, WarpConfig};

#[kithara::test]
#[case::resample(StretchKind::Glide)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn region_boundaries_publish_exact_phase_under_every_output_budget(#[case] backend: StretchKind) {
    let render = |budgets: &[usize]| {
        let mut renderer = renderer(
            &WarpConfig::builder()
                .backend(backend)
                .keylock(true)
                .region_plan(Arc::new(
                    RegionPlan::new(vec![GridSegment::new(0, 8, 1.25)]).expect("region"),
                ))
                .build(),
        );
        let mut source = 0;
        let mut frame = 0;
        let mut samples = Vec::new();
        while frame < 32 {
            let budget = budgets[frame % budgets.len()].min(32 - frame);
            let output = mapped_render(&mut renderer, &mut source, budget);
            assert!(output.frames() <= budget);
            let span = output.meta.source_span.expect("region mapping");
            for offset in 0..=output.frames() {
                let boundary = frame + offset;
                let expected = if boundary <= 10 {
                    boundary * 4
                } else {
                    (boundary - 2) * 5
                };
                let (numerator, denominator) = span.source_ratio_at(offset as u64).expect("phase");
                assert_eq!(
                    numerator * 5,
                    expected as u128 * u128::from(denominator.get())
                );
            }
            frame += output.frames();
            samples.extend_from_slice(&output.samples);
        }
        samples
    };
    assert_eq!(render(&[32]), render(&[1, 3, 7, 2, 11]));
}

#[kithara::test]
#[case::resample(StretchKind::Glide)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn fractional_region_crossing_is_an_exact_single_frame_run(#[case] backend: StretchKind) {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(backend)
            .keylock(true)
            .region_plan(Arc::new(
                RegionPlan::new(vec![GridSegment::new(0, 3, 1.25)]).expect("region"),
            ))
            .build(),
    );
    let mut source = 0;
    let before = mapped_render(&mut renderer, &mut source, 32);
    assert_eq!(before.frames(), 3);
    let crossing = mapped_render(&mut renderer, &mut source, 32);
    assert_eq!(crossing.frames(), 1);
    let span = crossing.meta.source_span.expect("crossing mapping");
    assert_eq!(
        span.source_ratio_at(0),
        Some((12, NonZeroU64::new(5).expect("denominator")))
    );
    assert_eq!(
        span.source_ratio_at(1),
        Some((13, NonZeroU64::new(4).expect("denominator")))
    );
    let after = mapped_render(&mut renderer, &mut source, 7);
    assert_positions(&after, 0, |frame| 3.25 + frame as f64);
}

#[kithara::test]
#[case::resample(StretchKind::Glide)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn a_ramp_in_a_region_keeps_the_exact_scaled_integral(#[case] backend: StretchKind) {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(backend)
            .keylock(true)
            .speed(0.5)
            .region_plan(Arc::new(
                RegionPlan::new(vec![GridSegment::new(0, 128, 1.25)]).expect("region"),
            ))
            .build(),
    );
    renderer
        .set_speed(
            SpeedCurve::Ramp {
                to: 1.5,
                frames: NonZeroU64::new(32).expect("ramp"),
            },
            1,
        )
        .expect("curve");
    let mut source = 0;
    let mut frame = 0;
    while frame < 32 {
        let output = mapped_render(&mut renderer, &mut source, 7.min(32 - frame));
        let span = output.meta.source_span.expect("region ramp mapping");
        for offset in 0..=output.frames() {
            let boundary = (frame + offset) as u128;
            let (numerator, denominator) = span.source_ratio_at(offset as u64).expect("phase");
            assert_eq!(
                numerator * 80,
                (32 * boundary + boundary * boundary) * u128::from(denominator.get())
            );
        }
        frame += output.frames();
    }
}

#[kithara::test]
fn interrupted_backend_crossfades_keep_the_current_curve_and_phase() {
    let render = |budgets: &[usize]| {
        let mut renderer = renderer(
            &WarpConfig::builder()
                .backend(StretchKind::Signalsmith)
                .keylock(true)
                .speed(0.5)
                .build(),
        );
        let mut source = 0;
        let mut frame = 0;
        let mut samples = Vec::new();
        while frame < 64 {
            if frame == 7 {
                renderer.set_backend(StretchKind::Bungee);
            }
            if frame == 11 {
                renderer.set_backend(StretchKind::Signalsmith);
                renderer
                    .set_speed(
                        SpeedCurve::Ramp {
                            to: 1.0,
                            frames: NonZeroU64::new(8).expect("ramp"),
                        },
                        2,
                    )
                    .expect("replacement curve");
            }
            let next = if frame < 7 {
                7
            } else if frame < 11 {
                11
            } else {
                64
            };
            let output = mapped_render(
                &mut renderer,
                &mut source,
                budgets[frame % budgets.len()].min(next - frame),
            );
            assert_positions(&output, frame, |frame| {
                if frame <= 11 {
                    frame as f64 * 0.5
                } else {
                    let ramp = (frame - 11).min(8) as f64;
                    5.5 + ramp * 0.5 + ramp * ramp / 32.0 + (frame - 11).saturating_sub(8) as f64
                }
            });
            frame += output.frames();
            samples.extend_from_slice(&output.samples);
        }
        samples
    };
    assert_eq!(render(&[64]), render(&[1, 3, 7, 2, 11]));
}

fn duration(source: f64) -> Duration {
    Duration::from_nanos(
        (source * 1_000_000_000.0 / f64::from(spec().sample_rate.get())).floor() as u64,
    )
}

fn mapped_render(renderer: &mut WarpRenderer, source: &mut u64, budget: usize) -> AudioChunk {
    mapped_signal(renderer, source, budget, |frame| frame as f32 / 4096.0)
}

fn mapped_signal(
    renderer: &mut WarpRenderer,
    source: &mut u64,
    budget: usize,
    signal: impl Fn(u64) -> f32,
) -> AudioChunk {
    renderer.prepare(spec());
    renderer
        .prepare_engine_latency(spec())
        .expect("prepared engine");
    let meta = AudioChunkInfo {
        spec: spec(),
        frame_offset: *source,
        timestamp: duration(*source as f64),
        ..AudioChunkInfo::default()
    };
    let frames = renderer
        .prepare_quantum(meta, 65_536, budget)
        .expect("bounded quantum")
        .get();
    let pools = renderer.pools.clone();
    let samples: Vec<_> = (0..frames)
        .flat_map(|offset| [signal(*source + offset as u64); 2])
        .collect();
    let mut input = chunk(&pools, &samples);
    input.meta = AudioChunkInfo {
        frames: u32::try_from(frames).expect("source size"),
        ..meta
    };
    *source += u64::try_from(frames).expect("source size");
    renderer
        .render_quantum(input)
        .continue_value()
        .expect("prepared input")
        .expect("rendered output")
}

#[kithara::test]
fn mapped_varispeed_filters_above_the_output_nyquist() {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(StretchKind::Glide)
            .speed(4.0)
            .build(),
    );
    let mut source = 0;
    let output = mapped_signal(&mut renderer, &mut source, 256, |frame| {
        if frame % 2 == 0 { 1.0 } else { -1.0 }
    });
    assert!(
        output.samples[64..]
            .iter()
            .all(|sample| sample.abs() < 0.05)
    );
    assert_positions(&output, 0, |frame| frame as f64 * 4.0);
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn primed_native_unity_keeps_the_physical_impulse_at_its_source_frame(
    #[case] backend: StretchKind,
) {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(backend)
            .keylock(true)
            .speed(1.0)
            .build(),
    );
    let mut source = 0;
    let output = mapped_signal(&mut renderer, &mut source, 1024, |frame| {
        if frame == 512 { 1.0 } else { 0.0 }
    });
    let peak = output
        .samples
        .chunks_exact(2)
        .enumerate()
        .max_by(|(_, first), (_, second)| first[0].abs().total_cmp(&second[0].abs()))
        .map(|(frame, _)| frame)
        .expect("impulse output");
    assert_eq!(peak, 512);
    assert_positions(&output, 0, |frame| frame as f64);
}

fn assert_positions(output: &AudioChunk, at: usize, expected: impl Fn(usize) -> f64) {
    let span = output
        .meta
        .source_span
        .expect("Warp owns the output mapping");
    for offset in 0..=output.frames() {
        assert_eq!(
            span.position_at(offset as u64),
            Some(duration(expected(at + offset))),
            "output boundary {}",
            at + offset
        );
    }
    assert_eq!(output.meta.timestamp, span.position_at(0).expect("start"));
    assert_eq!(
        output.meta.end_timestamp,
        span.position_at(output.frames() as u64).expect("end")
    );
}

#[kithara::test]
#[case::identity(StretchKind::Identity)]
#[case::resample(StretchKind::Glide)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn every_engine_publishes_its_output_source_mapping(#[case] backend: StretchKind) {
    let config = WarpConfig::builder().backend(backend).speed(1.0).build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    let mut output_frame = 0;
    for budget in [3, 7, 11, 1, 29] {
        let output = mapped_render(&mut renderer, &mut source, budget);
        assert!(output.frames() <= budget);
        assert_positions(&output, output_frame, |frame| frame as f64);
        output_frame += output.frames();
    }
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn an_unbounded_output_budget_stays_within_the_resident_shape(#[case] backend: StretchKind) {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(backend)
            .keylock(true)
            .speed(4.0)
            .build(),
    );
    let mut source = 0;
    for _ in 0..3 {
        let output = mapped_render(&mut renderer, &mut source, usize::MAX);
        assert!(output.frames() <= renderer.source_block_frames.get());
        assert!(output.samples.iter().all(|sample| sample.is_finite()));
    }
}

#[kithara::test]
fn a_tighter_budget_replans_a_cached_quantum_before_rendering() {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(StretchKind::Glide)
            .speed(0.5)
            .build(),
    );
    renderer.prepare_engine_latency(spec()).expect("engine");
    let meta = AudioChunkInfo {
        spec: spec(),
        ..AudioChunkInfo::default()
    };
    renderer
        .prepare_quantum(meta, 65_536, 64)
        .expect("initial quantum");
    let frames = renderer
        .prepare_quantum(meta, 65_536, 3)
        .expect("new budget")
        .get();
    let pools = renderer.pools.clone();
    let mut input = chunk(&pools, &vec![0.5; frames * 2]);
    input.meta = AudioChunkInfo {
        frames: u32::try_from(frames).expect("frames"),
        ..meta
    };
    let output = renderer
        .render_quantum(input)
        .continue_value()
        .expect("prepared")
        .expect("output");
    assert_eq!(output.frames(), 3);
    assert_positions(&output, 0, |frame| frame as f64 * 0.5);
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn switching_to_keylock_retains_the_full_reprime_history(#[case] backend: StretchKind) {
    let mut renderer = renderer(
        &WarpConfig::builder()
            .backend(StretchKind::Glide)
            .speed(4.0)
            .build(),
    );
    let mut source = 0;
    for _ in 0..64 {
        mapped_render(&mut renderer, &mut source, 512);
    }
    renderer.set_backend(backend);
    renderer.set_keylock(true);
    renderer
        .set_speed(SpeedCurve::Constant(0.05), 2)
        .expect("speed");
    renderer.prepare_engine_latency(spec()).expect("engine");
    let latency = renderer
        .engine
        .as_ref()
        .expect("engine")
        .capabilities()
        .latency();
    let needed = latency.source_frames() * 3;
    let resident = renderer.residency.as_ref().expect("history");
    assert!(resident.history_frames >= needed * 4);
    let (numerator, denominator) = resident
        .history_position(needed as u64)
        .expect("mapped history");
    assert!(numerator / u128::from(denominator.get()) >= resident.start as u128);
    let output = mapped_render(&mut renderer, &mut source, 7);
    assert_positions(&output, 0, |frame| {
        131_072.0 + frame as f64 * f64::from(0.05f32)
    });
    assert!(output.samples.iter().all(|sample| sample.is_finite()));
}

#[kithara::test]
fn native_replacement_is_source_aligned_and_quantum_independent() {
    let render = |budgets: &[usize]| {
        let mut renderer = renderer(
            &WarpConfig::builder()
                .backend(StretchKind::Signalsmith)
                .keylock(true)
                .speed(0.5)
                .build(),
        );
        let mut source = 0;
        mapped_render(&mut renderer, &mut source, 31);
        renderer.set_backend(StretchKind::Bungee);
        renderer
            .prepare_engine_latency(spec())
            .expect("replacement");
        assert!(renderer.retiring_target.is_some());
        let mut frame = 0;
        let mut samples = Vec::new();
        while frame < 257 {
            let output = mapped_render(
                &mut renderer,
                &mut source,
                budgets[frame % budgets.len()].min(257 - frame),
            );
            assert_positions(&output, 0, |offset| 15.5 + (frame + offset) as f64 * 0.5);
            frame += output.frames();
            samples.extend_from_slice(&output.samples);
        }
        samples
    };
    assert_eq!(render(&[257]), render(&[1, 7, 3, 29]));
}

#[kithara::test]
#[case::resample(StretchKind::Glide)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn fractional_mapping_continues_through_speed_and_backend_changes(#[case] backend: StretchKind) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    let first = mapped_render(&mut renderer, &mut source, 7);
    assert_eq!(first.frames(), 7);
    assert_positions(&first, 0, |frame| frame as f64 * 0.5);
    renderer
        .set_speed(SpeedCurve::Constant(1.0), 1)
        .expect("speed");
    renderer.set_backend(StretchKind::Glide);
    renderer.set_keylock(false);
    let second = mapped_render(&mut renderer, &mut source, 5);
    assert_eq!(second.frames(), 5);
    assert_positions(&second, 0, |frame| 3.5 + frame as f64);
}

fn steps() -> SpeedCurve {
    SpeedCurve::Steps(Arc::from([(0, 0.5), (7, 1.25), (19, 0.75), (31, 1.0)]))
}

fn steps_integral(frame: usize) -> f64 {
    let first = frame.min(7) as f64 * 0.5;
    let second = frame.saturating_sub(7).min(12) as f64 * 1.25;
    let third = frame.saturating_sub(19).min(12) as f64 * 0.75;
    first + second + third + frame.saturating_sub(31) as f64
}

fn ramp_integral(frame: usize) -> f64 {
    let ramp = frame.min(32) as f64;
    0.5 * ramp + ramp * ramp / 64.0 + frame.saturating_sub(32) as f64 * 1.5
}

fn curve_output_for(
    backend: StretchKind,
    curve: SpeedCurve,
    budgets: &[usize],
    expected: impl Fn(usize) -> f64,
) -> (Vec<Duration>, Vec<f32>) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(backend != StretchKind::Glide)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    renderer.set_speed(curve, 1).expect("curve");
    let mut source = 0;
    let mut frame = 0;
    let mut positions = Vec::new();
    let mut samples = Vec::new();
    while frame < 64 {
        let budget = budgets[frame % budgets.len()].min(64 - frame);
        let output = mapped_render(&mut renderer, &mut source, budget);
        assert!(output.frames() <= budget);
        assert_positions(&output, frame, &expected);
        let mapping = output.meta.source_span.expect("mapping");
        positions.extend(
            (0..output.frames())
                .map(|offset| mapping.position_at(offset as u64).expect("position")),
        );
        samples.extend_from_slice(&output.samples);
        frame += output.frames();
    }
    assert_eq!(positions.len(), 64);
    (positions, samples)
}

fn curve_output(
    curve: SpeedCurve,
    budgets: &[usize],
    expected: impl Fn(usize) -> f64,
) -> (Vec<Duration>, Vec<f32>) {
    curve_output_for(StretchKind::Glide, curve, budgets, expected)
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn native_curves_are_exact_and_partition_independent(#[case] backend: StretchKind) {
    for (curve, integral) in [
        (steps(), steps_integral as fn(usize) -> f64),
        (
            SpeedCurve::Ramp {
                to: 1.5,
                frames: NonZeroU64::new(32).expect("duration"),
            },
            ramp_integral,
        ),
    ] {
        let whole = curve_output_for(backend, curve.clone(), &[64], integral);
        let split = curve_output_for(backend, curve, &[1, 3, 7, 2, 11], integral);
        assert_eq!(split, whole);
    }
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn native_minimum_speed_keeps_exact_positions(#[case] backend: StretchKind) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.05)
        .build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    let mut at = 0;
    for budget in [3, 7, 11, 1, 29] {
        let output = mapped_render(&mut renderer, &mut source, budget);
        assert_positions(&output, at, |frame| frame as f64 * f64::from(0.05_f32));
        assert!(output.samples.iter().all(|sample| sample.is_finite()));
        at += output.frames();
    }
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn native_pitch_cascade_reports_its_full_latency(#[case] backend: StretchKind) {
    let mut unity = renderer(&WarpConfig::builder().backend(backend).keylock(true).build());
    let baseline = unity
        .prepare_engine_latency(spec())
        .expect("unity latency")
        .get();
    let mut slow = renderer(
        &WarpConfig::builder()
            .backend(backend)
            .keylock(true)
            .speed(0.05)
            .build(),
    );
    let latency = slow.prepare_engine_latency(spec()).expect("slow latency");
    assert_eq!(latency.get(), baseline * 3);
    assert_eq!(slow.engine_latency(), latency);
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn replacing_a_curve_during_drain_applies_at_the_next_frame(#[case] backend: StretchKind) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    let first = mapped_render(&mut renderer, &mut source, 7);
    assert_eq!(first.frames(), 7);
    renderer.prepare(spec());
    let tail = renderer.drain(3).expect("drain").expect("retained source");
    assert_positions(&tail, 7, |frame| frame as f64 * 0.5);
    renderer
        .set_speed(SpeedCurve::Constant(1.0), 2)
        .expect("replacement");
    renderer.prepare_engine_latency(spec()).expect("re-prime");
    let next = renderer.drain(2).expect("drain").expect("retained source");
    assert_eq!(next.frames(), 2);
    assert_eq!(next.meta.render_revision, 2);
    assert_eq!(next.meta.source_span.expect("mapping").render_revision(), 2);
    assert_positions(&next, 0, |frame| 5.0 + frame as f64);
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn repriming_reads_the_published_history_not_the_replacement_curve(#[case] backend: StretchKind) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    renderer
        .set_speed(
            SpeedCurve::Ramp {
                to: 1.5,
                frames: NonZeroU64::new(32).expect("duration"),
            },
            1,
        )
        .expect("ramp");
    let mut source = 0;
    let first = mapped_render(&mut renderer, &mut source, 7);
    renderer
        .set_speed(SpeedCurve::Constant(0.75), 2)
        .expect("replacement");
    renderer.prepare_engine_latency(spec()).expect("re-prime");
    let history = renderer.residency.as_ref().expect("source history");
    for before in 0..=7 {
        assert_eq!(
            history.history_position(before),
            first
                .meta
                .source_span
                .expect("mapping")
                .source_ratio_at(7 - before)
        );
    }
}

#[kithara::test]
fn steps_integral_is_exact_and_partition_independent() {
    let whole = curve_output(steps(), &[64], steps_integral);
    let split = curve_output(steps(), &[1, 3, 7, 2, 11], steps_integral);
    assert_eq!(split.0, whole.0);
    assert_eq!(split.1, whole.1);
}

#[kithara::test]
fn ramp_integral_is_exact_and_partition_independent() {
    let curve = SpeedCurve::Ramp {
        to: 1.5,
        frames: NonZeroU64::new(32).expect("duration"),
    };
    let whole = curve_output(curve.clone(), &[64], ramp_integral);
    let split = curve_output(curve, &[1, 3, 7, 2, 11], ramp_integral);
    assert_eq!(split.0, whole.0);
    assert_eq!(split.1, whole.1);
}

#[kithara::test]
fn replacing_a_ramp_preserves_its_exact_current_speed_and_position() {
    let config = WarpConfig::builder()
        .backend(StretchKind::Glide)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    renderer
        .set_speed(
            SpeedCurve::Ramp {
                to: 1.5,
                frames: NonZeroU64::new(32).expect("duration"),
            },
            1,
        )
        .expect("ramp");
    let mut source = 0;
    let first = mapped_render(&mut renderer, &mut source, 7);
    assert_eq!(first.frames(), 7);
    assert_positions(&first, 0, ramp_integral);
    renderer
        .set_speed(
            SpeedCurve::Ramp {
                to: 1.0,
                frames: NonZeroU64::new(8).expect("duration"),
            },
            2,
        )
        .expect("replacement");
    let mut frame = 0;
    while frame < 12 {
        let output = mapped_render(&mut renderer, &mut source, (12 - frame).min(3));
        assert_positions(&output, frame, |offset| {
            let ramp = offset.min(8) as f64;
            4.265_625
                + 0.718_75 * ramp
                + 0.281_25 * ramp * ramp / 16.0
                + offset.saturating_sub(8) as f64
        });
        frame += output.frames();
    }
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn eof_drain_obeys_each_output_budget_and_keeps_mapping(#[case] backend: StretchKind) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    let output = mapped_render(&mut renderer, &mut source, 31);
    assert_eq!(output.frames(), 31);
    assert_positions(&output, 0, |frame| frame as f64 * 0.5);
    renderer.prepare(spec());
    assert!(renderer.drain(0).expect("zero budget").is_none());
    let mut frame = 31;
    for budget in [1, 2, 7, 3].into_iter().cycle().take(32_768) {
        renderer.prepare(spec());
        let Some(output) = renderer.drain(budget).expect("bounded drain") else {
            break;
        };
        assert!(output.frames() <= budget);
        assert_positions(&output, frame, |frame| frame as f64 * 0.5);
        frame += output.frames();
    }
    renderer.prepare(spec());
    assert!(renderer.drain(1).expect("completed drain").is_none());
    assert!(frame > 31);
}

#[kithara::test]
fn resampled_pcm_uses_the_published_fractional_phase() {
    let config = WarpConfig::builder()
        .backend(StretchKind::Glide)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    for budget in [7, 3, 1, 11] {
        let output = mapped_render(&mut renderer, &mut source, budget);
        let span = output.meta.source_span.expect("mapping");
        for (frame, samples) in output.samples.chunks_exact(2).enumerate() {
            let (numerator, denominator) = span.source_ratio_at(frame as u64).expect("position");
            let position = numerator as f64 / denominator.get() as f64;
            let first = position.floor() as f32 / 4096.0;
            let second = (position.floor() as f32 + 1.0) / 4096.0;
            let expected = (second - first).mul_add(position.fract() as f32, first);
            assert_eq!(samples, [expected, expected]);
        }
    }
}

#[kithara::test]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn backend_retirement_does_not_blend_old_control_pcm_into_mapped_output(
    #[case] backend: StretchKind,
) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    let mut source = 0;
    mapped_render(&mut renderer, &mut source, 7);
    renderer
        .set_speed(SpeedCurve::Constant(1.0), 2)
        .expect("speed");
    renderer.set_backend(StretchKind::Glide);
    renderer.set_keylock(false);
    let output = mapped_render(&mut renderer, &mut source, 5);
    assert_positions(&output, 0, |frame| 3.5 + frame as f64);
    for (frame, samples) in output.samples.chunks_exact(2).enumerate() {
        let position = 3.5 + frame as f32;
        let first = position.floor() / 4096.0;
        let second = (position.floor() + 1.0) / 4096.0;
        assert_eq!(samples, [(second - first).mul_add(0.5, first); 2]);
    }
}

#[kithara::test]
#[case::resample(StretchKind::Glide)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[case::bungee(StretchKind::Bungee)]
fn terminal_quantum_and_drain_do_not_map_padding_as_source(#[case] backend: StretchKind) {
    let config = WarpConfig::builder()
        .backend(backend)
        .keylock(true)
        .speed(0.5)
        .build();
    let mut renderer = renderer(&config);
    renderer.prepare(spec());
    renderer.prepare_engine_latency(spec()).expect("engine");
    let meta = AudioChunkInfo {
        spec: spec(),
        ..AudioChunkInfo::default()
    };
    renderer.prepare_quantum(meta, 65_536, 31).expect("quantum");
    renderer
        .prepare_terminal_quantum(3)
        .expect("terminal input");
    let pools = renderer.pools.clone();
    let mut input = chunk(&pools, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    input.meta = AudioChunkInfo { frames: 3, ..meta };
    let first = renderer
        .render_quantum(input)
        .continue_value()
        .expect("input");
    let mut frames = 0;
    if let Some(output) = first {
        assert_positions(&output, frames, |frame| frame as f64 * 0.5);
        frames += output.frames();
    }
    for _ in 0..8 {
        renderer.prepare(spec());
        let Some(output) = renderer.drain(1).expect("tail") else {
            break;
        };
        assert_eq!(output.frames(), 1);
        assert_positions(&output, frames, |frame| frame as f64 * 0.5);
        frames += output.frames();
    }
    assert_eq!(frames, 6);
    assert_eq!(
        renderer.rendered_source_end(),
        Some((3, spec().sample_rate))
    );
    assert!(renderer.drain(1).expect("completed").is_none());
}
