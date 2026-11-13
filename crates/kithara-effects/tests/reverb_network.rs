#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use kithara_effects::reverb::{Reverb, ReverbConfig, Width};
use kithara_test_utils::{bufpool::pools, kithara};

const RATE: NonZeroU32 = NonZeroU32::MIN.saturating_add(47_999);

#[kithara::test]
fn reverb_network_is_bit_identical_across_uneven_partitions_and_reset() {
    let mut input = [vec![0.0_f32; 8192], vec![0.0_f32; 8192]];
    input[0][0] = 1.0;
    input[1][7] = -0.5;
    let config = ReverbConfig::default();
    let mut whole = Reverb::new(RATE, config, &pools()).unwrap();
    let mut split = Reverb::new(RATE, config, &pools()).unwrap();
    let mut expected = [vec![0.0_f32; 8192], vec![0.0_f32; 8192]];
    let [left, right] = &mut expected;
    whole
        .process([&input[0], &input[1]], [left, right])
        .unwrap();
    assert!(expected.iter().flatten().all(|sample| sample.is_finite()));
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-6));
    let mut actual = [vec![0.0_f32; 8192], vec![0.0_f32; 8192]];
    let mut start = 0;
    for frames in [0, 1, 17, 513, 3, 257].into_iter().cycle() {
        let end = (start + frames).min(8192);
        let [left, right] = &mut actual;
        split
            .process(
                [&input[0][start..end], &input[1][start..end]],
                [&mut left[start..end], &mut right[start..end]],
            )
            .unwrap();
        start = end;
        if start == 8192 {
            break;
        }
    }
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    split.reset();
    let [left, right] = &mut actual;
    split
        .process([&input[0], &input[1]], [left, right])
        .unwrap();
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

#[kithara::test]
fn zero_width_writes_the_same_wet_signal_to_both_channels() {
    let config = ReverbConfig::builder().width(Width::MIN).build();
    let mut reverb = Reverb::new(RATE, config, &pools()).unwrap();
    let mut input = vec![0.0_f32; 8192];
    input[0] = 1.0;
    let silence = vec![0.0_f32; 8192];
    let mut left = vec![0.0_f32; 8192];
    let mut right = left.clone();
    reverb
        .process([&input, &silence], [&mut left, &mut right])
        .unwrap();
    assert!(left.iter().any(|sample| sample.abs() > 1e-6));
    for (left, right) in left.iter().zip(right) {
        assert_eq!(left.to_bits(), right.to_bits());
    }
}
