use std::num::NonZeroUsize;

use kithara_test_fixtures::unit_fixtures::{accelerate_ramp, accelerate_wave};
use kithara_test_utils::kithara;

use super::{
    BiquadError, MultichannelBiquad, deinterleave_pair_f32, interleave_pair_f32,
    linear_interpolate_f32, max_magnitude_f32,
};

const SPECIALS: [f32; 8] = [
    0.0,
    -0.0,
    f32::from_bits(1),
    f32::MIN_POSITIVE,
    f32::MAX,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
];

fn bits<const N: usize>(values: [f32; N]) -> [u32; N] {
    values.map(f32::to_bits)
}

#[kithara::test(native, flash(false))]
fn interpolation_outputs_requested_frames(accelerate_ramp: Vec<f32>) {
    let source = accelerate_ramp;
    let positions = [0.0, 0.5, 1.0];
    let mut target = [0.0; 3];
    assert_eq!(
        linear_interpolate_f32(&source, &positions, &mut target),
        target.len()
    );
}

#[kithara::test(native, flash(false))]
fn linear_interpolation_matches_scalar_positions(accelerate_ramp: Vec<f32>) {
    let source = accelerate_ramp;
    let positions = [1.0, 1.25, 1.5, 1.75];
    let mut target = [0.0; 4];

    linear_interpolate_f32(&source, &positions, &mut target);

    assert_eq!(target, [1.0, 1.25, 1.5, 1.75]);
}

#[kithara::test(native, flash(false))]
fn quadratic_interpolation_matches_scalar_positions(accelerate_wave: Vec<f32>) {
    let source = accelerate_wave;
    let positions = [1.0, 1.25, 1.5, 1.75];
    let mut target = [0.0; 4];

    super::quadratic_interpolate_f32(&source, &positions, &mut target);

    let expected = [1.0, 0.9375, 0.75, 0.4375];
    for (actual, expected) in target.iter().zip(expected) {
        assert!((actual - expected).abs() < 0.000_001);
    }
}

#[kithara::test(native)]
fn interleave_pair_writes_only_the_common_prefix() {
    let mut output = [9.0_f32; 7];
    assert_eq!(
        interleave_pair_f32(&[1.0, 2.0, 3.0, 4.0], &[-1.0, -2.0, -3.0], &mut output),
        3
    );
    assert_eq!(bits(output), bits([1.0, -1.0, 2.0, -2.0, 3.0, -3.0, 9.0]));
}

#[kithara::test(native)]
fn deinterleave_pair_reads_only_whole_pairs() {
    let (mut left, mut right) = ([9.0_f32; 4], [9.0_f32; 4]);
    assert_eq!(
        deinterleave_pair_f32(&[1.0, -1.0, 2.0, -2.0, 3.0], &mut left, &mut right),
        2
    );
    assert_eq!(bits(left), bits([1.0, 2.0, 9.0, 9.0]));
    assert_eq!(bits(right), bits([-1.0, -2.0, 9.0, 9.0]));
}

#[kithara::test(native)]
fn layout_moves_special_values_bit_for_bit() {
    let mut interleaved = [1.0_f32; 16];
    assert_eq!(
        interleave_pair_f32(&SPECIALS, &SPECIALS, &mut interleaved),
        8
    );
    let (mut left, mut right) = ([1.0_f32; 8], [1.0_f32; 8]);
    assert_eq!(
        deinterleave_pair_f32(&interleaved, &mut left, &mut right),
        8
    );
    assert_eq!(bits(left), bits(SPECIALS));
    assert_eq!(bits(right), bits(SPECIALS));
}

#[kithara::test(native)]
fn multichannel_biquad_rejects_a_shape_it_was_not_built_for() {
    let two = NonZeroUsize::MIN.saturating_add(1);
    let mut filter = MultichannelBiquad::new(two, NonZeroUsize::MIN)
        .unwrap_or_else(|err| panic!("vDSP setup: {err}"));
    let mut one = [vec![0.0_f32; 8]];
    let mut short = [vec![0.0_f32; 8], vec![0.0_f32; 4]];
    let other = MultichannelBiquad::new(NonZeroUsize::MIN, NonZeroUsize::MIN)
        .unwrap_or_else(|err| panic!("vDSP setup: {err}"));

    assert_eq!(filter.process(&mut one, 0..8), Err(BiquadError::Shape));
    assert_eq!(filter.process(&mut short, 0..8), Err(BiquadError::Shape));
    assert_eq!(
        filter.set_section(1, [1.0, 0.0, 0.0, 0.0, 0.0]),
        Err(BiquadError::Shape)
    );
    assert_eq!(filter.copy_state_from(&other), Err(BiquadError::Shape));
    assert_eq!(filter.process(&mut short, 2..4), Ok(()));
}

#[kithara::test(native)]
fn max_magnitude_reads_the_largest_absolute_value() {
    assert_eq!(
        max_magnitude_f32(&[0.5, -2.0, 1.0]).to_bits(),
        2.0_f32.to_bits()
    );
    assert_eq!(max_magnitude_f32(&[]).to_bits(), 0.0_f32.to_bits());
}
