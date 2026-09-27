use std::num::NonZeroUsize;

use fearless_simd::{Level, dispatch};
use kithara_test_fixtures::signal::Wave;
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use super::accelerate;
use super::{cascade, portable, simd, strided};
use crate::{
    filter::{Coefficients, Hertz, Type},
    interp::{InterpError, Interpolation},
};

const SIZES: [usize; 15] = [0, 1, 3, 4, 5, 7, 8, 9, 15, 16, 17, 63, 64, 1023, 4096];
const OFFSETS: [usize; 2] = [0, 1];
const STRIDES: [usize; 5] = [1, 2, 3, 6, 9];
const RATE: u32 = 48_000;
const UNWRITTEN: f32 = -1.0;
/// A signaling `NaN`: a plain copy keeps its quiet bit clear.
const SIGNALING_NAN: u32 = 0x7F80_0001;
const SPECIALS: [f32; 9] = [
    0.0,
    -0.0,
    f32::from_bits(1),
    f32::MIN_POSITIVE,
    f32::MAX,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
    f32::from_bits(SIGNALING_NAN),
];
const SINE: Wave = Wave::Sine {
    hz: 440.0,
    peak: i16::MAX,
};

type DeinterleavePair = fn(&[f32], &mut [f32], &mut [f32]) -> usize;
type InterleavePair = fn(&[f32], &[f32], &mut [f32]) -> usize;

/// One backend's pair kernels, so every check runs the same matrix on each.
struct Pairs {
    name: &'static str,
    deinterleave_pair: DeinterleavePair,
    interleave_pair: InterleavePair,
}

fn pairs() -> Vec<Pairs> {
    Vec::from([
        Pairs {
            name: "portable-native",
            deinterleave_pair: portable::deinterleave_pair,
            interleave_pair: portable::interleave_pair,
        },
        Pairs {
            name: "portable-fallback",
            deinterleave_pair: |input, left, right| dispatch!(Level::fallback(), simd => portable::deinterleave_pair_kernel(simd, input, left, right)),
            interleave_pair: |left, right, output| dispatch!(Level::fallback(), simd => portable::interleave_pair_kernel(simd, left, right, output)),
        },
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        Pairs {
            name: "accelerate",
            deinterleave_pair: accelerate::deinterleave_pair,
            interleave_pair: accelerate::interleave_pair,
        },
    ])
}

fn signal(len: usize, wave: Wave) -> Vec<f32> {
    SPECIALS
        .into_iter()
        .chain((0..).map(|frame| f32::from(wave.sample(frame, RATE)) / 32_768.0))
        .take(len)
        .collect()
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

fn stride(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("strides are non-zero")
}

#[kithara::test]
fn interleave_pair_matches_the_oracle() {
    for layout in pairs() {
        let name = layout.name;
        for size in SIZES {
            for offset in OFFSETS {
                let left = signal(size.saturating_add(offset), SINE);
                let right = signal(size.saturating_add(offset), Wave::Sawtooth);
                let mut expected = vec![UNWRITTEN; size.saturating_mul(2).saturating_add(offset)];
                let mut actual = expected.clone();
                let ((_, left), (_, right)) = (left.split_at(offset), right.split_at(offset));
                let want = oracle::interleave_pair(left, right, expected.split_at_mut(offset).1);
                let got = (layout.interleave_pair)(left, right, actual.split_at_mut(offset).1);
                assert_eq!(got, want, "{name}: frames, size {size}, offset {offset}");
                assert_eq!(
                    bits(&actual),
                    bits(&expected),
                    "{name}: size {size}, offset {offset}"
                );
            }
        }
    }
}

#[kithara::test]
fn deinterleave_pair_matches_the_oracle() {
    for layout in pairs() {
        let name = layout.name;
        for size in SIZES {
            for offset in OFFSETS {
                let input = signal(size.saturating_mul(2).saturating_add(offset), SINE);
                let input = input.split_at(offset).1;
                let plane = vec![UNWRITTEN; size.saturating_add(offset)];
                let (mut want, mut got) = ((plane.clone(), plane.clone()), (plane.clone(), plane));
                let want_frames = oracle::deinterleave_pair(
                    input,
                    want.0.split_at_mut(offset).1,
                    want.1.split_at_mut(offset).1,
                );
                let got_frames = (layout.deinterleave_pair)(
                    input,
                    got.0.split_at_mut(offset).1,
                    got.1.split_at_mut(offset).1,
                );
                assert_eq!(
                    got_frames, want_frames,
                    "{name}: frames, size {size}, offset {offset}"
                );
                assert_eq!(
                    bits(&got.0),
                    bits(&want.0),
                    "{name}: left, size {size}, offset {offset}"
                );
                assert_eq!(
                    bits(&got.1),
                    bits(&want.1),
                    "{name}: right, size {size}, offset {offset}"
                );
            }
        }
    }
}

#[kithara::test]
fn scatter_and_gather_match_the_oracle() {
    for size in SIZES {
        for step in STRIDES {
            for channel in 0..step {
                let plane = signal(size, Wave::Sawtooth);
                let mut want = vec![UNWRITTEN; size.saturating_mul(step)];
                let mut got = want.clone();
                let start = channel.min(want.len());
                let want_frames = oracle::scatter(&plane, want.split_at_mut(start).1, stride(step));
                let got_frames = strided::scatter(&plane, got.split_at_mut(start).1, stride(step));
                assert_eq!(
                    got_frames, want_frames,
                    "scatter frames, size {size}, stride {step}, channel {channel}"
                );
                assert_eq!(
                    bits(&got),
                    bits(&want),
                    "scatter, size {size}, stride {step}, channel {channel}"
                );

                let interleaved = want.split_at(start).1;
                let mut want_plane = vec![UNWRITTEN; size];
                let mut got_plane = want_plane.clone();
                let want_frames = oracle::gather(interleaved, stride(step), &mut want_plane);
                let got_frames = strided::gather(interleaved, stride(step), &mut got_plane);
                assert_eq!(
                    got_frames, want_frames,
                    "gather frames, size {size}, stride {step}, channel {channel}"
                );
                assert_eq!(
                    bits(&got_plane),
                    bits(&want_plane),
                    "gather, size {size}, stride {step}, channel {channel}"
                );
            }
        }
    }
}

#[kithara::test]
fn sanitize_matches_the_oracle_at_every_level() {
    let fallback = |samples: &mut [f32]| {
        dispatch!(Level::fallback(), simd => simd::sanitize_kernel(simd, samples));
    };
    for (name, sanitize) in [
        ("native", simd::sanitize as fn(&mut [f32])),
        ("fallback", fallback),
    ] {
        for size in SIZES {
            for offset in OFFSETS {
                let mut want = signal(size.saturating_add(offset), SINE);
                let mut got = want.clone();
                oracle::sanitize(want.split_at_mut(offset).1);
                sanitize(got.split_at_mut(offset).1);
                assert_eq!(
                    bits(&got),
                    bits(&want),
                    "{name}: size {size}, offset {offset}"
                );
            }
        }
    }
}

#[kithara::test]
fn a_short_side_bounds_every_pair_kernel() {
    for layout in pairs() {
        let name = layout.name;
        let mut output = [UNWRITTEN; 7];
        assert_eq!(
            (layout.interleave_pair)(&[1.0, 2.0, 3.0, 4.0], &[-1.0, -2.0, -3.0], &mut output),
            3,
            "{name}"
        );
        assert_eq!(
            bits(&output),
            bits(&[1.0, -1.0, 2.0, -2.0, 3.0, -3.0, UNWRITTEN]),
            "{name}"
        );

        let (mut left, mut right) = ([UNWRITTEN; 4], [UNWRITTEN; 4]);
        assert_eq!(
            (layout.deinterleave_pair)(&[1.0, -1.0, 2.0, -2.0, 3.0], &mut left, &mut right),
            2,
            "{name}"
        );
        assert_eq!(
            bits(&left),
            bits(&[1.0, 2.0, UNWRITTEN, UNWRITTEN]),
            "{name}"
        );
        assert_eq!(
            bits(&right),
            bits(&[-1.0, -2.0, UNWRITTEN, UNWRITTEN]),
            "{name}"
        );
    }
}

#[kithara::test]
fn a_short_side_bounds_the_strided_copies() {
    let mut output = [UNWRITTEN; 7];
    assert_eq!(
        strided::scatter(&[1.0, 2.0, 3.0, 4.0], &mut output, stride(3)),
        3
    );
    assert_eq!(
        bits(&output),
        bits(&[1.0, UNWRITTEN, UNWRITTEN, 2.0, UNWRITTEN, UNWRITTEN, 3.0])
    );

    let mut plane = [UNWRITTEN; 4];
    assert_eq!(
        strided::gather(&[1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 3.0], stride(3), &mut plane),
        3
    );
    assert_eq!(bits(&plane), bits(&[1.0, 2.0, 3.0, UNWRITTEN]));
}

/// Scalar reference for the kernels: index arithmetic instead of lanes.
mod oracle {
    use std::num::NonZeroUsize;

    pub(super) fn interleave_pair(left: &[f32], right: &[f32], output: &mut [f32]) -> usize {
        let frames = left.len().min(right.len()).min(output.len() / 2);
        for ((pair, l), r) in output.chunks_exact_mut(2).zip(left).zip(right).take(frames) {
            pair.copy_from_slice(&[*l, *r]);
        }
        frames
    }

    pub(super) fn deinterleave_pair(input: &[f32], left: &mut [f32], right: &mut [f32]) -> usize {
        let frames = (input.len() / 2).min(left.len()).min(right.len());
        for ((pair, l), r) in input
            .chunks_exact(2)
            .zip(left.iter_mut())
            .zip(right.iter_mut())
            .take(frames)
        {
            if let [a, b] = pair {
                *l = *a;
                *r = *b;
            }
        }
        frames
    }

    pub(super) fn scatter(plane: &[f32], output: &mut [f32], stride: NonZeroUsize) -> usize {
        let frames = plane.len().min(output.len().div_ceil(stride.get()));
        for (frame, sample) in plane.iter().take(frames).enumerate() {
            if let Some(slot) = frame
                .checked_mul(stride.get())
                .and_then(|slot| output.get_mut(slot))
            {
                *slot = *sample;
            }
        }
        frames
    }

    pub(super) fn gather(input: &[f32], stride: NonZeroUsize, plane: &mut [f32]) -> usize {
        let frames = plane.len().min(input.len().div_ceil(stride.get()));
        for (frame, slot) in plane.iter_mut().take(frames).enumerate() {
            if let Some(sample) = frame
                .checked_mul(stride.get())
                .and_then(|slot| input.get(slot))
            {
                *slot = *sample;
            }
        }
        frames
    }

    pub(super) fn sanitize(samples: &mut [f32]) {
        for sample in samples {
            if !sample.is_finite() || sample.abs() < f32::MIN_POSITIVE {
                *sample = 0.0;
            }
        }
    }
}

/// Largest error of a cascade against the `f64` oracle, relative to the
/// oracle's peak: −100 dB.
const PARITY: f64 = 1.0e-5;
const CUTOFFS: [[f64; 4]; 2] = [
    [1_000.0, 2_000.0, 4_000.0, 8_000.0],
    [3_000.0, 5_000.0, 7_000.0, 9_000.0],
];
/// Peak inputs: specials without `NaN` and subnormals, which `max` orders differently.
const PEAK_SPECIALS: [f32; 6] = [
    0.0,
    -0.0,
    f32::MIN_POSITIVE,
    f32::MAX,
    f32::INFINITY,
    f32::NEG_INFINITY,
];

/// Section coefficients per half of the stream: the second half retunes.
fn halves(sections: usize) -> [Vec<[f32; 5]>; 2] {
    CUTOFFS.map(|cutoffs| {
        cutoffs
            .iter()
            .take(sections)
            .map(|cutoff| {
                let Coefficients { a1, a2, b0, b1, b2 } = Coefficients::from_params(
                    Type::LowPass,
                    Hertz::from_hz(f64::from(RATE)).expect("positive rate"),
                    Hertz::from_hz(*cutoff).expect("positive cutoff"),
                    std::f64::consts::FRAC_1_SQRT_2,
                )
                .expect("valid low-pass");
                [b0, b1, b2, a1, a2].map(|value| value.to_f32().expect("coefficient fits f32"))
            })
            .collect()
    })
}

fn tones(channels: usize) -> Vec<Vec<f32>> {
    (1..=channels)
        .map(|harmonic| {
            let wave = Wave::Sine {
                hz: 440.0 * f64::from(u32::try_from(harmonic).expect("few channels")),
                peak: i16::MAX,
            };
            (0..4_096)
                .map(|frame| f32::from(wave.sample(frame, RATE)) / 32_768.0)
                .collect()
        })
        .collect()
}

/// Direct form I in `f64` over the same `f32` coefficients.
fn oracle(input: &[Vec<f32>], halves: &[Vec<[f32; 5]>; 2]) -> Vec<Vec<f64>> {
    input
        .iter()
        .map(|plane| {
            let [first, second] = halves;
            let mut state = vec![[0.0_f64; 4]; first.len()];
            let half = plane.len().wrapping_div(2);
            plane
                .iter()
                .enumerate()
                .map(|(frame, sample)| {
                    let set = if frame < half { first } else { second };
                    set.iter().zip(state.iter_mut()).fold(
                        f64::from(*sample),
                        |x, (section, delay)| {
                            let [b0, b1, b2, a1, a2] = section.map(f64::from);
                            let [x1, x2, y1, y2] = *delay;
                            let y = b0.mul_add(
                                x,
                                b1.mul_add(x1, b2.mul_add(x2, (-a1).mul_add(y1, -a2 * y2))),
                            );
                            *delay = [x, x1, y, y1];
                            y
                        },
                    )
                })
                .collect()
        })
        .collect()
}

type RunCascade = fn(&mut [Vec<f32>], &[Vec<[f32; 5]>; 2]);

/// Drives a cascade through its whole interface the way glide does: a stale
/// state cleared by `reset`, then the second half on a twin that took over
/// the state and retuned.
macro_rules! handover_run {
    ($cascade:ty) => {
        |planes: &mut [Vec<f32>], halves: &[Vec<[f32; 5]>; 2]| {
            let channels = NonZeroUsize::new(planes.len()).expect("channels");
            let sections = NonZeroUsize::new(halves[0].len()).expect("sections");
            let mut cascade = <$cascade>::new(channels, sections).expect("cascade");
            let mut twin = <$cascade>::new(channels, sections).expect("cascade");
            let frames = planes.first().map_or(0, Vec::len);
            let half = frames.wrapping_div(2);
            for (section, coefficients) in halves[0].iter().enumerate() {
                cascade
                    .set_section(section, *coefficients)
                    .expect("section");
            }
            cascade
                .process(&mut planes.to_vec(), 0..frames)
                .expect("shape matches");
            cascade.reset();
            cascade.process(planes, 0..half).expect("shape matches");
            twin.copy_state_from(&cascade).expect("same shape");
            for (section, coefficients) in halves[1].iter().enumerate() {
                twin.set_section(section, *coefficients).expect("section");
            }
            twin.process(planes, half..frames).expect("shape matches");
        }
    };
}

/// Retunes the live state in place through the kernel at `Level::fallback()`.
fn portable_fallback_run(planes: &mut [Vec<f32>], halves: &[Vec<[f32; 5]>; 2]) {
    let channels = NonZeroUsize::new(planes.len()).expect("channels");
    let sections = NonZeroUsize::new(halves[0].len()).expect("sections");
    let mut cascade = cascade::Cascade::new(channels, sections).expect("cascade");
    let frames = planes.first().map_or(0, Vec::len);
    let half = frames.wrapping_div(2);
    for (range, set) in [(0..half, &halves[0]), (half..frames, &halves[1])] {
        for (section, coefficients) in set.iter().enumerate() {
            cascade
                .set_section(section, *coefficients)
                .expect("section");
        }
        dispatch!(Level::fallback(), simd => cascade::cascade_kernel(simd, &mut cascade, planes, range));
    }
}

#[kithara::test]
fn every_cascade_backend_tracks_the_f64_oracle_across_a_retune() {
    let runs: Vec<(&str, RunCascade)> = Vec::from([
        (
            "portable-native",
            handover_run!(cascade::Cascade) as RunCascade,
        ),
        ("portable-fallback", portable_fallback_run),
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        ("accelerate", handover_run!(accelerate::Cascade)),
    ]);
    for channels in [1_usize, 2, 8] {
        for sections in [1_usize, 4] {
            let halves = halves(sections);
            let input = tones(channels);
            let expected = oracle(&input, &halves);
            for (name, run) in &runs {
                let mut planes = input.clone();
                run(&mut planes, &halves);
                for (plane, reference) in planes.iter().zip(&expected) {
                    let peak = reference.iter().fold(0.0_f64, |peak, y| peak.max(y.abs()));
                    let worst = plane.iter().zip(reference).fold(0.0_f64, |worst, (y, r)| {
                        worst.max((f64::from(*y) - r).abs())
                    });
                    assert!(
                        worst <= PARITY * peak,
                        "{name}: {channels} ch × {sections} sections off by {worst} of {peak}"
                    );
                }
            }
        }
    }
}

fn fallback_peak(samples: &[f32]) -> f32 {
    dispatch!(Level::fallback(), simd => portable::peak_kernel(simd, samples))
}

#[kithara::test]
fn peak_matches_the_scalar_maximum_on_every_backend() {
    type Peak = fn(&[f32]) -> f32;
    let backends: Vec<(&str, Peak)> = Vec::from([
        ("portable-native", portable::peak as Peak),
        ("portable-fallback", fallback_peak),
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        ("accelerate", accelerate::peak),
    ]);
    for len in SIZES {
        let sine: Vec<f32> = signal(len.saturating_add(SPECIALS.len()), SINE)
            .into_iter()
            .skip(SPECIALS.len())
            .collect();
        let with_specials: Vec<f32> = PEAK_SPECIALS
            .iter()
            .copied()
            .chain(sine.iter().copied())
            .take(len)
            .collect();
        for samples in [&sine, &with_specials] {
            let expected = samples
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            for (name, peak) in &backends {
                assert_eq!(
                    peak(samples).to_bits(),
                    expected.to_bits(),
                    "{name} at {len}"
                );
            }
        }
    }
}

/// Largest interpolation error against the `f64` oracle, relative to the
/// window peak: four `f32` epsilons.
const INTERP_PARITY: f64 = 4.768_371_582_031_25e-7;
const INTERP_WINDOW: u16 = 257;
const INTERP_POSITIONS: u16 = 1_000;
const INTERP_METHODS: [Interpolation; 4] = [
    Interpolation::Linear,
    Interpolation::Quadratic,
    Interpolation::Hermite,
    Interpolation::Watte,
];

type Interp = fn(Interpolation, &[f32], &[f32], &mut [f32]) -> Result<usize, InterpError>;

/// The textbook formulas in `f64` over the same `f32` samples and positions.
fn interp_oracle(method: Interpolation, window: &[f32], position: f32) -> f64 {
    let p = f64::from(position);
    let base = p.floor();
    let x = p - base;
    let [ym1, y0, y1, y2] = [-1.0, 0.0, 1.0, 2.0].map(|offset: f64| {
        (base + offset)
            .to_usize()
            .and_then(|index| window.get(index))
            .map_or(0.0, |sample| f64::from(*sample))
    });
    match method {
        Interpolation::Linear => (y1 - y0).mul_add(x, y0),
        Interpolation::Quadratic => {
            let slope = 0.5 * (y1 - ym1);
            let curve = 0.5 * (y1 - 2.0 * y0 + ym1);
            curve.mul_add(x, slope).mul_add(x, y0)
        }
        Interpolation::Hermite => {
            let c1 = 0.5 * (y1 - ym1);
            let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
            let c3 = 1.5 * (y0 - y1) + 0.5 * (y2 - ym1);
            c3.mul_add(x, c2).mul_add(x, c1).mul_add(x, y0)
        }
        Interpolation::Watte => {
            let outer = ym1 + y2;
            let c1 = 1.5 * y1 - 0.5 * (y0 + outer);
            let c2 = 0.5 * (outer - y0 - y1);
            c2.mul_add(x, c1).mul_add(x, y0)
        }
    }
}

#[kithara::test]
fn every_interpolation_backend_tracks_the_f64_oracle() {
    let backends: Vec<(&str, Interp)> = Vec::from([
        ("portable", portable::interpolate as Interp),
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        ("accelerate", accelerate::interpolate),
    ]);
    let window: Vec<f32> = signal(
        usize::from(INTERP_WINDOW).saturating_add(SPECIALS.len()),
        SINE,
    )
    .into_iter()
    .skip(SPECIALS.len())
    .collect();
    let peak = window
        .iter()
        .fold(0.0_f64, |peak, sample| peak.max(f64::from(sample.abs())));
    for method in INTERP_METHODS {
        let (before, after) = method.padding();
        let low = f64::from(before);
        let span = f64::from(INTERP_WINDOW) - f64::from(after) - low;
        let positions: Vec<f32> = (0..INTERP_POSITIONS)
            .filter_map(|step| {
                (f64::from(step) / f64::from(INTERP_POSITIONS))
                    .mul_add(span, low)
                    .to_f32()
            })
            .collect();
        for (name, run) in &backends {
            let mut output = vec![UNWRITTEN; positions.len()];
            assert_eq!(
                run(method, &window, &positions, &mut output),
                Ok(positions.len()),
                "{name}: {method:?}"
            );
            let worst = positions
                .iter()
                .zip(&output)
                .fold(0.0_f64, |worst, (position, sample)| {
                    worst
                        .max((f64::from(*sample) - interp_oracle(method, &window, *position)).abs())
                });
            assert!(
                worst <= INTERP_PARITY * peak,
                "{name}: {method:?} off by {worst} of {peak}"
            );
        }
    }
}

/// Neighbours of opposite sign cancel in the Catmull-Rom coefficients; `f32`
/// arithmetic there loses more than the four epsilons the contract allows.
const CANCELLING_WINDOW: [f32; 4] = [-0.573_767_2, 0.718_489_6, -0.533_961_2, 0.338_097_85];
const CANCELLING_POSITION: f32 = 1.947_738_2;

#[kithara::test]
fn every_interpolation_backend_rounds_once_on_a_cancelling_window() {
    let backends: Vec<(&str, Interp)> = Vec::from([
        ("portable", portable::interpolate as Interp),
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        ("accelerate", accelerate::interpolate),
    ]);
    let peak = CANCELLING_WINDOW
        .iter()
        .fold(0.0_f64, |peak, sample| peak.max(f64::from(sample.abs())));
    for method in INTERP_METHODS {
        let (before, _) = method.padding();
        let positions = [f32::from(before), CANCELLING_POSITION];
        for (name, run) in &backends {
            let mut output = [UNWRITTEN; 2];
            assert_eq!(
                run(method, &CANCELLING_WINDOW, &positions, &mut output),
                Ok(positions.len()),
                "{name}: {method:?}"
            );
            for (position, sample) in positions.iter().zip(output) {
                let error = (f64::from(sample)
                    - interp_oracle(method, &CANCELLING_WINDOW, *position))
                .abs();
                assert!(
                    error <= INTERP_PARITY * peak,
                    "{name}: {method:?} at {position} off by {error} of {peak}"
                );
            }
        }
    }
}
