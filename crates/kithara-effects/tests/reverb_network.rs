#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use kithara_effects::reverb::{Damping, Decay, Reverb, ReverbConfig, ReverbError, Width};
use kithara_test_utils::{bufpool::pools, kithara};
use num_traits::ToPrimitive;

const RATE: NonZeroU32 = NonZeroU32::MIN.saturating_add(47_999);

#[kithara::test]
fn reverb_network_is_bit_identical_across_uneven_partitions_and_reset() {
    let mut input = [vec![0.0_f32; 8192], vec![0.0_f32; 8192]];
    input[0][0] = 1.0;
    input[1][7] = -0.5;
    let config = ReverbConfig::builder()
        .width(Width::checked(0.37).unwrap())
        .build();
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

#[kithara::test]
fn exhausted_tail_stays_exactly_silent_and_keeps_its_declared_boundary() {
    let rate = NonZeroU32::new(8000).unwrap();
    let config = ReverbConfig::builder()
        .decay(Decay::MIN)
        .damping(Damping::MAX)
        .width(Width::checked(0.37).unwrap())
        .build();
    let mut reverb = Reverb::new(rate, config, &pools()).unwrap();
    assert!(reverb.tail_exhausted());
    let mut left = [0.0_f32];
    let mut right = left;
    reverb
        .process([&[1.0], &[-0.5]], [&mut left, &mut right])
        .unwrap();
    assert!(!reverb.tail_exhausted());
    let bound = declared_tail_frames(rate, Decay::MIN);
    let silence = vec![0.0_f32; bound - 1];
    let mut left = silence.clone();
    let mut right = silence.clone();
    reverb
        .process([&silence, &silence], [&mut left, &mut right])
        .unwrap();
    assert!(!reverb.tail_exhausted());
    reverb
        .process([&[0.0], &[0.0]], [&mut left[..1], &mut right[..1]])
        .unwrap();
    assert!(
        left[..1]
            .iter()
            .chain(&right[..1])
            .all(|sample| sample.abs() <= 1e-6)
    );
    assert!(reverb.tail_exhausted());
    let silence = vec![0.0_f32; 1024];
    let mut after_left = vec![0.125_f32; 1024];
    let mut after_right = vec![-0.25_f32; 1024];
    reverb
        .process([&silence, &silence], [&mut after_left, &mut after_right])
        .unwrap();
    assert!(
        after_left
            .iter()
            .chain(&after_right)
            .all(|sample| sample.to_bits() == 0)
    );
}

#[kithara::test]
fn parameter_extrema_keep_the_wet_network_finite() {
    let input: Vec<_> = [0.25_f32, -0.125, 0.0, 0.5]
        .into_iter()
        .cycle()
        .take(4096)
        .collect();
    for decay in [Decay::MIN, Decay::MAX] {
        for damping in [Damping::MIN, Damping::MAX] {
            for width in [Width::MIN, Width::MAX] {
                let config = ReverbConfig::builder()
                    .decay(decay)
                    .damping(damping)
                    .width(width)
                    .build();
                let mut reverb = Reverb::new(RATE, config, &pools()).unwrap();
                let mut left = vec![0.0_f32; input.len()];
                let mut right = left.clone();
                reverb
                    .process([&input, &input], [&mut left, &mut right])
                    .unwrap();
                assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
                assert!(!reverb.tail_exhausted());
            }
        }
    }
}

#[kithara::test]
fn shape_refusal_preserves_wet_buffers_and_network_history() {
    let config = ReverbConfig::default();
    let mut reverb = Reverb::new(RATE, config, &pools()).unwrap();
    let mut control = Reverb::new(RATE, config, &pools()).unwrap();
    let input = [1.0_f32; 128];
    let mut warm = [vec![0.0; 128], vec![0.0; 128]];
    let [left, right] = &mut warm;
    reverb.process([&input, &input], [left, right]).unwrap();
    let [left, right] = &mut warm;
    control.process([&input, &input], [left, right]).unwrap();
    let mut left = [0.125_f32; 128];
    let mut right = [-0.25_f32; 127];
    assert!(matches!(
        reverb.process([&input, &input], [&mut left, &mut right]),
        Err(ReverbError::Shape)
    ));
    assert_eq!(left, [0.125; 128]);
    assert_eq!(right, [-0.25; 127]);
    let silence = vec![0.0_f32; 4096];
    let mut actual = [vec![0.0_f32; 4096], vec![0.0_f32; 4096]];
    let mut expected = actual.clone();
    let [left, right] = &mut actual;
    reverb.process([&silence, &silence], [left, right]).unwrap();
    let [left, right] = &mut expected;
    control
        .process([&silence, &silence], [left, right])
        .unwrap();
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

// Independent unbounded f64 histories for the declared 44.1 kHz layout.
fn scalar_network(input: [&[f32]; 2], rate: u32, config: ReverbConfig) -> [Vec<f64>; 2] {
    let mut output: [Vec<f64>; 2] = std::array::from_fn(|channel| {
        let spread = if channel == 0 { 0 } else { 13 };
        let comb_lengths = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617].map(|length| {
            ((f64::from(length + spread) * f64::from(rate)) / 44_100.0)
                .round()
                .to_usize()
                .unwrap()
        });
        let diffuser_lengths = [31, 47, 73, 101].map(|length| {
            ((f64::from(length + spread) * f64::from(rate)) / 44_100.0)
                .round()
                .to_usize()
                .unwrap()
        });
        let gains = comb_lengths.map(|length| {
            10.0_f64.powf(
                -3.0 * length.to_f64().unwrap() / (f64::from(rate) * f64::from(config.decay())),
            )
        });
        let mut combs: [Vec<f64>; 8] = std::array::from_fn(|_| Vec::new());
        let mut diffusers: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
        let mut memory = [0.0_f64; 8];
        let coefficient = 0.4 * f64::from(config.damping());
        input[channel]
            .iter()
            .enumerate()
            .map(|(frame, input)| {
                let mut sum = 0.0;
                for (((history, state), delay), gain) in combs
                    .iter_mut()
                    .zip(&mut memory)
                    .zip(comb_lengths)
                    .zip(gains)
                {
                    let delayed = frame
                        .checked_sub(delay)
                        .and_then(|past| history.get(past))
                        .copied()
                        .unwrap_or(0.0);
                    sum += delayed;
                    *state = (1.0 - coefficient) * delayed + coefficient * *state;
                    history.push(gain * *state + f64::from(*input) * (1.0 - gain) / 8.0);
                }
                for (history, delay) in diffusers.iter_mut().zip(diffuser_lengths) {
                    let delayed = frame
                        .checked_sub(delay)
                        .and_then(|past| history.get(past))
                        .copied()
                        .unwrap_or(0.0);
                    history.push(sum + 0.5 * delayed);
                    sum = delayed - sum;
                }
                sum
            })
            .collect()
    });
    let width = f64::from(config.width());
    let [left, right] = &mut output;
    for (left, right) in left.iter_mut().zip(right) {
        let mid = (*left + *right) * 0.5;
        *left = width * *left + (1.0 - width) * mid;
        *right = width * *right + (1.0 - width) * mid;
    }
    output
}

#[kithara::test]
fn comb_damping_and_diffusion_match_an_independent_scalar_network() {
    let rate = NonZeroU32::new(8000).unwrap();
    let mut input = [vec![0.0_f32; 8192], vec![0.0_f32; 8192]];
    input[0][0] = 1.0;
    input[1][7] = -0.5;
    for damping in [Damping::MIN, Damping::MAX] {
        for width in [Width::MIN, Width::checked(0.37).unwrap(), Width::MAX] {
            let config = ReverbConfig::builder()
                .decay(Decay::MIN)
                .damping(damping)
                .width(width)
                .build();
            let expected = scalar_network([&input[0], &input[1]], rate.get(), config);
            let mut actual = [vec![0.0_f32; 8192], vec![0.0_f32; 8192]];
            let mut reverb = Reverb::new(rate, config, &pools()).unwrap();
            let [left, right] = &mut actual;
            reverb
                .process([&input[0], &input[1]], [left, right])
                .unwrap();
            for (channel, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                    assert!(
                        (f64::from(*actual) - expected).abs() <= 1e-6,
                        "damping {damping:?}, width {width:?}, channel {channel}, frame {frame}: {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[kithara::test]
fn retirement_and_reentry_keep_the_same_bits_across_partitions() {
    let rate = NonZeroU32::new(8000).unwrap();
    let config = ReverbConfig::builder()
        .decay(Decay::MIN)
        .width(Width::checked(0.37).unwrap())
        .build();
    let mut input = [vec![0.0_f32; 6000], vec![0.0_f32; 6000]];
    input[0][0] = 1.0;
    input[1][4000] = -0.5;
    let mut whole = Reverb::new(rate, config, &pools()).unwrap();
    let mut split = Reverb::new(rate, config, &pools()).unwrap();
    let mut expected = [vec![0.0_f32; 6000], vec![0.0_f32; 6000]];
    let mut actual = expected.clone();
    let [left, right] = &mut expected;
    whole
        .process([&input[0], &input[1]], [left, right])
        .unwrap();
    let mut start = 0;
    for count in [0, 1, 37, 257, 3].into_iter().cycle() {
        let end = (start + count).min(6000);
        let [left, right] = &mut actual;
        split
            .process(
                [&input[0][start..end], &input[1][start..end]],
                [&mut left[start..end], &mut right[start..end]],
            )
            .unwrap();
        start = end;
        if start == 6000 {
            break;
        }
    }
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    assert_eq!(whole.tail_exhausted(), split.tail_exhausted());
}

#[kithara::test]
fn unrepresentable_rate_is_refused_without_clamping() {
    assert!(
        matches!(Reverb::new(NonZeroU32::MIN, ReverbConfig::default(), &pools()),
        Err(ReverbError::Geometry(rate)) if rate == NonZeroU32::MIN)
    );
}

#[kithara::test]
fn maximum_decay_undamped_boundary_step_is_below_retirement_threshold() {
    let mut maximum = 0.0_f32;
    for rate in [8000, 48000] {
        let rate = NonZeroU32::new(rate).unwrap();
        let config = ReverbConfig::builder()
            .decay(Decay::MAX)
            .damping(Damping::MIN)
            .width(Width::MAX)
            .build();
        let full_scale = [1.0_f32; 512];
        let silence = [0.0_f32; 512];
        let bound = declared_tail_frames(rate, Decay::MAX);
        let dc_frames = (4.0 * f64::from(Decay::MAX) * f64::from(rate.get()))
            .ceil()
            .to_usize()
            .unwrap();
        for excitation in [1, dc_frames] {
            let mut reverb = Reverb::new(rate, config, &pools()).unwrap();
            let mut left = [0.0_f32; 512];
            let mut right = left;
            let mut remaining = excitation;
            while remaining > 0 {
                let count = remaining.min(512);
                reverb
                    .process(
                        [&full_scale[..count], &full_scale[..count]],
                        [&mut left[..count], &mut right[..count]],
                    )
                    .unwrap();
                remaining -= count;
            }
            let mut remaining = bound - 1;
            while remaining > 0 {
                let count = remaining.min(512);
                reverb
                    .process(
                        [&silence[..count], &silence[..count]],
                        [&mut left[..count], &mut right[..count]],
                    )
                    .unwrap();
                remaining -= count;
            }
            assert!(!reverb.tail_exhausted());
            reverb
                .process(
                    [&silence[..1], &silence[..1]],
                    [&mut left[..1], &mut right[..1]],
                )
                .unwrap();
            assert!(reverb.tail_exhausted());
            let peak = left[0].abs().max(right[0].abs());
            eprintln!(
                "boundary rate={} excitation_frames={excitation} decay={:?} damping={:?} width={:?} peak={peak:.12e}",
                rate.get(),
                Decay::MAX,
                Damping::MIN,
                Width::MAX
            );
            maximum = maximum.max(peak);
        }
    }
    assert!(
        maximum <= 1e-6,
        "maximum pre-retirement step {maximum:.12e}"
    );
}

fn declared_tail_frames(rate: NonZeroU32, decay: Decay) -> usize {
    let rate = f64::from(rate.get());
    let longest = (1630.0 * rate / 44_100.0).round().to_usize().unwrap();
    let diffusion: usize = [31, 47, 73, 101]
        .into_iter()
        .map(|length| {
            (f64::from(length + 13) * rate / 44_100.0)
                .round()
                .to_usize()
                .unwrap()
        })
        .sum();
    let rounds = (1e-6_f64.ln() / 0.5_f64.ln()).ceil().to_usize().unwrap() + 1;
    (2.0 * f64::from(decay) * rate).ceil().to_usize().unwrap() + longest + diffusion * rounds
}
