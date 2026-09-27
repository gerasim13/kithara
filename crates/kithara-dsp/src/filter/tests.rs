use std::num::NonZeroUsize;

use kithara_test_fixtures::signal::Wave;
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

use super::{Biquad, FilterError, rbj};

const RATE: f64 = 48_000.0;
const Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
const TWO: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);
/// RBJ cookbook low-pass at 1 kHz, 48 kHz, `Q = 1/√2`, computed in `f64`.
const REFERENCE: [f64; 5] = [
    0.003_916_126_660_547_383,
    0.007_832_253_321_094_766,
    0.003_916_126_660_547_383,
    -1.815_341_082_704_568,
    0.831_005_589_346_757_6,
];
/// A settled filter matches the long-run steady state this closely.
const SETTLED: f32 = 1.0e-6;
const SINE: Wave = Wave::Sine {
    hz: 440.0,
    peak: i16::MAX,
};

fn low_pass(cutoff: f64) -> super::Coefficients {
    rbj::low_pass(RATE, cutoff, Q).expect("valid low-pass")
}

fn stereo(frames: usize, wave: Wave) -> Vec<Vec<f32>> {
    let plane: Vec<f32> = (0..frames)
        .map(|frame| f32::from(wave.sample(frame, 48_000)) / 32_768.0)
        .collect();
    vec![plane.clone(), plane]
}

fn built(cutoff: f64) -> Biquad {
    let mut filter = Biquad::new(TWO, NonZeroUsize::MIN).expect("filter builds");
    filter
        .retune(0, low_pass(cutoff))
        .expect("section 0 exists");
    filter
}

fn bits(planes: &[Vec<f32>]) -> Vec<Vec<u32>> {
    planes
        .iter()
        .map(|plane| plane.iter().map(|sample| sample.to_bits()).collect())
        .collect()
}

#[kithara::test]
fn low_pass_matches_the_cookbook_reference() {
    for (actual, expected) in low_pass(1_000.0).section().into_iter().zip(REFERENCE) {
        let expected = expected.to_f32().expect("reference fits f32");
        assert!(
            (actual - expected).abs() <= f32::EPSILON * expected.abs(),
            "{actual} differs from {expected}"
        );
    }
}

#[kithara::test]
fn low_pass_rejects_parameters_outside_its_domain() {
    for (sample_rate, cutoff, q) in [
        (0.0, 1_000.0, Q),
        (RATE, 0.0, Q),
        (RATE, 24_000.0, Q),
        (RATE, 1_000.0, 0.0),
        (f64::NAN, 1_000.0, Q),
        (RATE, f64::INFINITY, Q),
    ] {
        assert_eq!(
            rbj::low_pass(sample_rate, cutoff, q),
            Err(FilterError::Parameters),
            "{sample_rate} / {cutoff} / {q}"
        );
    }
}

#[kithara::test]
fn settle_reaches_the_long_run_steady_state() {
    let levels = [0.5_f32, -0.25];
    let mut steady = built(1_000.0);
    let mut long_run: Vec<Vec<f32>> = levels.iter().map(|level| vec![*level; 16_384]).collect();
    steady
        .process(&mut long_run, 0..16_384)
        .expect("shape matches");
    let mut settled = built(1_000.0);
    settled.settle(&levels).expect("one level per channel");
    let mut planes: Vec<Vec<f32>> = levels.iter().map(|level| vec![*level; 64]).collect();
    settled.process(&mut planes, 0..64).expect("shape matches");

    for (plane, reference) in planes.iter().zip(&long_run) {
        let target = reference.last().copied().expect("long run is non-empty");
        assert!(
            plane
                .iter()
                .all(|sample| (sample - target).abs() <= SETTLED),
            "settled output left {target}: {plane:?}"
        );
    }
}

#[kithara::test]
fn silence_resets_to_the_state_of_a_fresh_filter() {
    let mut used = built(1_000.0);
    let mut burst = stereo(256, SINE);
    used.process(&mut burst, 0..256).expect("shape matches");
    let mut quiet = vec![vec![0.0_f32; 1_024]; 2];
    used.process(&mut quiet, 0..1_024).expect("shape matches");
    let mut silence = vec![vec![0.0_f32; 64]; 2];
    used.process(&mut silence, 0..64).expect("shape matches");

    let mut after = stereo(256, SINE);
    used.process(&mut after, 0..256).expect("shape matches");
    let mut fresh = stereo(256, SINE);
    built(1_000.0)
        .process(&mut fresh, 0..256)
        .expect("shape matches");
    assert_eq!(bits(&after), bits(&fresh));
}

/// At a quarter of the rate with `Q = 1/2` the section is the FIR
/// `(1 + 2z⁻¹ + z⁻²) / 4`: after `[1, −½]` the next output is `0` while the
/// section still remembers `−½`, so a one-frame call looks silent.
#[kithara::test]
fn a_quiet_call_shorter_than_the_section_memory_keeps_the_state() {
    let fir = rbj::low_pass(RATE, RATE / 4.0, 0.5).expect("valid low-pass");
    let mut split = Biquad::new(TWO, NonZeroUsize::MIN).expect("filter builds");
    let mut whole = Biquad::new(TWO, NonZeroUsize::MIN).expect("filter builds");
    split.retune(0, fir).expect("section 0 exists");
    whole.retune(0, fir).expect("section 0 exists");
    let mut left = vec![vec![1.0_f32, -0.5, 0.0, 0.0]; 2];
    let mut right = left.clone();
    for range in [0..2, 2..3, 3..4] {
        split.process(&mut left, range).expect("shape matches");
    }
    whole.process(&mut right, 0..4).expect("shape matches");
    assert_eq!(bits(&left), bits(&right));
}

#[kithara::test]
fn an_empty_range_keeps_the_state() {
    let mut probed = built(1_000.0);
    let mut plain = built(1_000.0);
    let mut left = stereo(256, SINE);
    let mut right = stereo(256, SINE);
    probed.process(&mut left, 0..128).expect("shape matches");
    probed
        .process(&mut left, 128..128)
        .expect("an empty range is a no-op");
    probed.process(&mut left, 128..256).expect("shape matches");
    plain.process(&mut right, 0..256).expect("shape matches");
    assert_eq!(bits(&left), bits(&right));
}

#[kithara::test]
fn samples_outside_the_range_keep_their_bits() {
    let mut filter = built(1_000.0);
    let original = stereo(64, SINE);
    let mut planes = original.clone();
    filter.process(&mut planes, 16..48).expect("shape matches");
    for (plane, source) in planes.iter().zip(&original) {
        let outside = |values: &[f32]| -> Vec<u32> {
            values
                .iter()
                .take(16)
                .chain(values.iter().skip(48))
                .map(|sample| sample.to_bits())
                .collect()
        };
        assert_eq!(outside(plane), outside(source));
    }
}

#[kithara::test]
fn a_mismatched_shape_is_an_error() {
    let mut filter = built(1_000.0);
    let mut one = vec![vec![0.0_f32; 8]];
    let mut short = vec![vec![0.0_f32; 8], vec![0.0_f32; 4]];
    let other = Biquad::new(NonZeroUsize::MIN, NonZeroUsize::MIN).expect("filter builds");

    assert_eq!(filter.process(&mut one, 0..8), Err(FilterError::Shape));
    assert_eq!(filter.process(&mut short, 0..8), Err(FilterError::Shape));
    assert_eq!(filter.retune(1, low_pass(1_000.0)), Err(FilterError::Shape));
    assert_eq!(filter.settle(&[0.0]), Err(FilterError::Shape));
    assert_eq!(filter.copy_state(&other), Err(FilterError::Shape));
}
