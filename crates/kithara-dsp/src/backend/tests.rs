use std::num::NonZeroUsize;

use fearless_simd::{Level, dispatch};
use kithara_test_fixtures::signal::Wave;
use kithara_test_utils::kithara;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use super::accelerate;
use super::{portable, simd, strided};

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
    let mut pairs = vec![
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
    ];
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pairs.push(Pairs {
        name: "accelerate",
        deinterleave_pair: accelerate::deinterleave_pair,
        interleave_pair: accelerate::interleave_pair,
    });
    pairs
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
