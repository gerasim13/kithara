#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use kithara_effects::{
    AudioEffect, apply_effects,
    filter::{FilterConfig, FilterEffect, FilterKind},
};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use kithara_test_fixtures::{
    signal::rms,
    unit_fixtures::{eq_sine_40, eq_sine_1000, eq_sine_10000},
};
use kithara_test_utils::{
    bufpool::{Pools, pools, sample_buffer},
    kithara,
};

fn spec(channels: u16) -> AudioSpec {
    AudioSpec::new(
        channels,
        NonZeroU32::new(44_100).expect("nonzero test rate"),
    )
}

fn config(kind: FilterKind) -> FilterConfig {
    FilterConfig::builder()
        .kind(kind)
        .cutoff_hz(1_000.0)
        .build()
        .expect("valid filter")
}

fn chunk(pools: &Pools, channels: u16, input: &[f32]) -> AudioChunk {
    AudioChunk::new(
        AudioChunkInfo {
            spec: spec(channels),
            ..Default::default()
        },
        sample_buffer(pools, input),
    )
}

#[kithara::test]
fn filter_rejects_invalid_parameters_and_unstable_designs() {
    for cutoff in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            FilterConfig::builder()
                .kind(FilterKind::HighPass)
                .cutoff_hz(cutoff)
                .build()
                .is_err()
        );
    }
    for q in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            FilterConfig::builder()
                .kind(FilterKind::LowPass)
                .cutoff_hz(1_000.0)
                .q(q)
                .build()
                .is_err()
        );
    }
    for gain in [f64::NAN, f64::INFINITY, -25.0, 25.0] {
        assert!(
            FilterConfig::builder()
                .kind(FilterKind::LowShelf)
                .cutoff_hz(1_000.0)
                .gain_db(gain)
                .build()
                .is_err()
        );
    }
    assert!(
        FilterConfig::builder()
            .kind(FilterKind::HighPass)
            .cutoff_hz(1_000.0)
            .gain_db(3.0)
            .build()
            .is_err()
    );
    let at_nyquist = FilterConfig::builder()
        .kind(FilterKind::LowPass)
        .cutoff_hz(22_050.0)
        .build()
        .expect("positive frequency");
    assert!(FilterEffect::new(spec(1), at_nyquist).is_err());
    let below_f32_resolution = FilterConfig::builder()
        .kind(FilterKind::LowPass)
        .cutoff_hz(1e-12)
        .build()
        .expect("positive frequency");
    assert!(FilterEffect::new(spec(1), below_f32_resolution).is_err());
    assert!(FilterEffect::new(spec(0), config(FilterKind::HighPass)).is_err());
}

#[kithara::test]
fn filter_is_a_real_chain_consumer_and_preserves_chunk_metadata() {
    let pools = pools();
    let mut chain: Vec<Box<dyn AudioEffect>> = vec![Box::new(
        FilterEffect::new(spec(2), config(FilterKind::HighPass)).expect("prepared filter"),
    )];
    let input = [0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut source = chunk(&pools, 2, &input);
    source.meta.frame_offset = 123;
    source.meta.frames = 4;
    let meta = source.meta;
    let output = apply_effects(&mut chain, source).expect("causal effect emits immediately");
    assert_eq!(output.meta, meta);
    assert_ne!(&output.samples[..], &input);
    assert!(
        output
            .samples
            .iter()
            .skip(1)
            .step_by(2)
            .all(|sample| sample.to_bits() == 0)
    );
    assert_eq!(chain[0].held_source_frames(), 0);
    assert!(chain[0].flush().is_none());
    assert!(chain[0].flush().is_none());
}

#[kithara::test]
fn filter_keeps_history_across_odd_blocks_and_reset_clears_it() {
    let pools = pools();
    let mut whole =
        FilterEffect::new(spec(1), config(FilterKind::LowPass)).expect("prepared filter");
    let mut split =
        FilterEffect::new(spec(1), config(FilterKind::LowPass)).expect("prepared filter");
    let mut input = vec![0.0; 257];
    input[0] = 0.25;
    input[97] = -0.125;
    let expected = whole
        .process(chunk(&pools, 1, &input))
        .expect("whole chunk");
    let mut actual = Vec::new();
    for block in input.chunks(7) {
        actual.extend_from_slice(
            &split
                .process(chunk(&pools, 1, block))
                .expect("split chunk")
                .samples,
        );
    }
    assert_eq!(actual, &expected.samples[..]);
    split.reset();
    let silence = split
        .process(chunk(&pools, 1, &[0.0; 16]))
        .expect("silence after reset");
    assert!(silence.samples.iter().all(|sample| sample.to_bits() == 0));
    let replay = split
        .process(chunk(&pools, 1, &input))
        .expect("same response after reset");
    assert_eq!(&replay.samples[..], &expected.samples[..]);
}

/// The mono 44.1 kHz fixtures pin Butterworth cutoff and shelf midpoint gain
/// within 0.5 dB after discarding 4096 samples of startup history.
#[kithara::test]
#[case(FilterKind::HighPass, 0.0, -3.010_299_956_64)]
#[case(FilterKind::LowPass, 0.0, -3.010_299_956_64)]
#[case(FilterKind::LowShelf, 12.0, 6.0)]
#[case(FilterKind::HighShelf, -12.0, -6.0)]
fn filter_matches_declared_corner_response(
    eq_sine_1000: Vec<f32>,
    #[case] kind: FilterKind,
    #[case] gain_db: f64,
    #[case] reference_db: f64,
) {
    let pools = pools();
    let config = FilterConfig::builder()
        .kind(kind)
        .cutoff_hz(1_000.0)
        .gain_db(gain_db)
        .build()
        .expect("valid response case");
    let mut filter = FilterEffect::new(spec(1), config).expect("prepared filter");
    let output = filter
        .process(chunk(&pools, 1, &eq_sine_1000))
        .expect("filtered tone");
    let measured_db =
        20.0 * f64::from(rms(&output.samples[4_096..]) / rms(&eq_sine_1000[4_096..])).log10();
    assert!(
        (measured_db - reference_db).abs() <= 0.5,
        "{kind:?}: {measured_db} dB, reference {reference_db} dB"
    );
}

#[kithara::test]
fn rejected_chunk_leaves_filter_history_untouched() {
    let pools = pools();
    let mut filter =
        FilterEffect::new(spec(1), config(FilterKind::HighPass)).expect("prepared filter");
    let mut reference =
        FilterEffect::new(spec(1), config(FilterKind::HighPass)).expect("reference filter");
    for effect in [&mut filter, &mut reference] {
        let output = effect
            .process(chunk(&pools, 1, &[0.25, 0.0, 0.0]))
            .expect("warm filter history");
        assert!(output.samples.iter().any(|sample| *sample != 0.0));
    }
    assert!(filter.process(chunk(&pools, 2, &[0.25, -0.25])).is_none());
    let expected = reference
        .process(chunk(&pools, 1, &[0.0; 3]))
        .expect("reference");
    let actual = filter
        .process(chunk(&pools, 1, &[0.0; 3]))
        .expect("accepted input");
    assert!(expected.samples.iter().any(|sample| *sample != 0.0));
    assert_eq!(&actual.samples[..], &expected.samples[..]);
}

/// Compare settled fixture RMS against the bilinear Butterworth magnitude.
#[kithara::test]
#[case(FilterKind::LowPass, 40.0, eq_sine_40())]
#[case(FilterKind::LowPass, 10_000.0, eq_sine_10000())]
#[case(FilterKind::HighPass, 40.0, eq_sine_40())]
#[case(FilterKind::HighPass, 10_000.0, eq_sine_10000())]
fn filter_matches_pass_and_stop_reference_regions(
    #[case] kind: FilterKind,
    #[case] hz: f64,
    #[case] input: Vec<f32>,
) {
    let pools = pools();
    let mut filter = FilterEffect::new(spec(1), config(kind)).expect("prepared filter");
    let output = filter
        .process(chunk(&pools, 1, &input))
        .expect("filtered tone");
    let measured = 20.0 * f64::from(rms(&output.samples[4_096..]) / rms(&input[4_096..])).log10();
    let omega = std::f64::consts::PI / 44_100.0;
    let ratio = (omega * hz).tan() / (omega * 1_000.0).tan();
    let numerator = if kind == FilterKind::HighPass {
        ratio * ratio
    } else {
        1.0
    };
    let reference = 20.0 * (numerator / (1.0 + ratio.powi(4)).sqrt()).log10();
    assert!(
        (measured - reference).abs() <= 0.5,
        "{kind:?} at {hz} Hz: {measured} dB, reference {reference} dB"
    );
}

#[kithara::test]
fn quiet_tail_reset_is_independent_of_single_frame_chunks() {
    let pools = pools();
    let mut block =
        FilterEffect::new(spec(1), config(FilterKind::LowPass)).expect("prepared filter");
    let mut frames =
        FilterEffect::new(spec(1), config(FilterKind::LowPass)).expect("prepared filter");
    let input = [1e-12, -1e-12, 0.0, 0.0, 0.25, 0.0, 0.0];
    let expected = block.process(chunk(&pools, 1, &input)).expect("block");
    assert!(
        expected.samples[2..4]
            .iter()
            .all(|sample| sample.to_bits() == 0)
    );
    let actual: Vec<_> = input
        .iter()
        .flat_map(|sample| {
            frames
                .process(chunk(&pools, 1, &[*sample]))
                .expect("frame")
                .samples
                .to_vec()
        })
        .collect();
    assert_eq!(actual, &expected.samples[..]);
    block.reset();
    let clean = block
        .process(chunk(&pools, 1, &input[4..]))
        .expect("clean impulse");
    assert_eq!(&expected.samples[4..], &clean.samples[..]);
}
