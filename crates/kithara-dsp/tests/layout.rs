#![forbid(unsafe_code)]

use std::num::NonZeroUsize;

use kithara_test_utils::kithara;

const FRAMES: [usize; 9] = [0, 1, 3, 4, 5, 8, 16, 17, 1023];
const STARTS: [usize; 2] = [0, 3];
const MAX_CHANNELS: usize = 8;
const TAIL: usize = 3;
const UNWRITTEN: f32 = -1.0;

fn count(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("channel counts are non-zero")
}

/// Scrambled bit patterns, so `NaN` payloads, infinities, subnormals and −0.0
/// all pass through the kernels.
fn sample(channel: usize, frame: usize) -> f32 {
    let key = u32::try_from(channel << 16 | frame).expect("fixture index fits u32");
    f32::from_bits(key.wrapping_mul(0x9E37_79B9).rotate_left(11))
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

#[kithara::test]
fn interleave_variable_matches_fast_interleave() {
    for inputs in 1..=MAX_CHANNELS {
        for outputs in 1..=MAX_CHANNELS {
            for frames in FRAMES {
                for start in STARTS {
                    let planes: Vec<Vec<f32>> = (0..inputs)
                        .map(|channel| {
                            (0..start + frames + TAIL)
                                .map(|frame| sample(channel, frame))
                                .collect()
                        })
                        .collect();
                    let mut expected = vec![UNWRITTEN; frames * outputs + TAIL];
                    let mut actual = expected.clone();
                    let range = start..start + frames;
                    fast_interleave::interleave_variable(
                        &planes,
                        range.clone(),
                        &mut expected,
                        count(outputs),
                    );
                    kithara_dsp::interleave_variable(&planes, range, &mut actual, count(outputs));
                    assert_eq!(
                        bits(&actual),
                        bits(&expected),
                        "{inputs} into {outputs} channels, {frames} frames from {start}"
                    );
                }
            }
        }
    }
}

#[kithara::test]
fn deinterleave_variable_matches_fast_interleave() {
    for inputs in 1..=MAX_CHANNELS {
        for outputs in 1..=MAX_CHANNELS {
            for frames in FRAMES {
                for start in STARTS {
                    let interleaved: Vec<f32> = (0..frames * inputs + TAIL)
                        .map(|slot| sample(0, slot))
                        .collect();
                    let mut expected = vec![vec![UNWRITTEN; start + frames + TAIL]; outputs];
                    let mut actual = expected.clone();
                    let range = start..start + frames;
                    fast_interleave::deinterleave_variable(
                        &interleaved,
                        count(inputs),
                        &mut expected,
                        range.clone(),
                    );
                    kithara_dsp::deinterleave_variable(
                        &interleaved,
                        count(inputs),
                        &mut actual,
                        range,
                    );
                    for (channel, (got, want)) in actual.iter().zip(&expected).enumerate() {
                        assert_eq!(
                            bits(got),
                            bits(want),
                            "{inputs} into {outputs} channels, {frames} frames from {start}, \
                             channel {channel}"
                        );
                    }
                }
            }
        }
    }
}

#[kithara::test]
fn a_short_slice_bounds_whole_frames_instead_of_panicking() {
    const U: f32 = UNWRITTEN;
    let planes = [vec![1.0_f32, 2.0, 3.0, 4.0], vec![-1.0, -2.0]];

    let mut mono = [U; 3];
    kithara_dsp::interleave_variable(&planes[..1], 0..4, &mut mono, count(1));
    assert_eq!(bits(&mono), bits(&[1.0, 2.0, 3.0]));

    let mut stereo = [U; 8];
    kithara_dsp::interleave_variable(&planes, 0..4, &mut stereo, count(2));
    assert_eq!(bits(&stereo), bits(&[1.0, -1.0, 2.0, -2.0, U, U, U, U]));

    let mut three = [U; 8];
    kithara_dsp::interleave_variable(&planes[..1], 1..4, &mut three, count(3));
    assert_eq!(bits(&three), bits(&[2.0, U, U, 3.0, U, U, U, U]));

    let mut mono = [[U; 3]];
    kithara_dsp::deinterleave_variable(&[1.0, 2.0], count(1), &mut mono, 0..3);
    assert_eq!(bits(&mono[0]), bits(&[1.0, 2.0, U]));

    let mut stereo = [[U; 4]; 2];
    kithara_dsp::deinterleave_variable(&[1.0, -1.0, 2.0, -2.0, 3.0], count(2), &mut stereo, 0..4);
    assert_eq!(bits(&stereo[0]), bits(&[1.0, 2.0, U, U]));
    assert_eq!(bits(&stereo[1]), bits(&[-1.0, -2.0, U, U]));

    let mut three = [[U; 3]; 2];
    kithara_dsp::deinterleave_variable(
        &[1.0, -1.0, 9.0, 2.0, -2.0, 9.0, 3.0, -3.0, 9.0],
        count(3),
        &mut three,
        1..5,
    );
    assert_eq!(bits(&three[0]), bits(&[U, 1.0, 2.0]));
    assert_eq!(bits(&three[1]), bits(&[U, -1.0, -2.0]));
}
