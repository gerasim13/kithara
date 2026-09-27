use std::num::{NonZeroU32, NonZeroUsize};

use kithara_dsp::interp::Interpolation;
use kithara_test_fixtures::{
    signal::Wave,
    unit_fixtures::{glide_alias, glide_quadratic, glide_transition, glide_unity},
};
use kithara_test_utils::kithara;

use super::{GlideBackend, GlideConfig, resampler::GlideResampler};
use crate::{
    RatioGlide, Resampler, ResamplerBackend, ResamplerCapabilities, ResamplerConfig,
    ResamplerControl, ResamplerMode, ResamplerOptions, ResamplerSettings, create_resampler,
    test_pools::{TestPools, pools},
};

fn channels(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or_else(|| panic!("channel count must be non-zero"))
}

fn rate(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or_else(|| panic!("sample rate must be non-zero"))
}

fn settings(mode: ResamplerMode) -> ResamplerSettings<TestPools> {
    ResamplerSettings::builder()
        .channels(channels(1))
        .mode(mode)
        .options(ResamplerOptions::builder().chunk_size(16).build())
        .pools(pools())
        .build()
}

fn fixed_mode(source: u32, target: u32) -> ResamplerMode {
    ResamplerMode::FixedRatio {
        source_sample_rate: rate(source),
        target_sample_rate: rate(target),
    }
}

fn build_glide(source: u32, target: u32) -> GlideResampler {
    let config = ResamplerConfig::builder()
        .backend(GlideBackend::new())
        .settings(settings(fixed_mode(source, target)))
        .build();
    create_resampler(&config).unwrap_or_else(|err| panic!("glide resampler should build: {err}"))
}

/// Largest sample difference between two chunkings of one stream.
const CHUNKING_TOLERANCE: f32 = 1.0e-4;
/// A constant passes the anti-alias filter as the same constant.
const CONSTANT_TOLERANCE: f32 = 1.0e-6;

fn two_tones(frames: usize) -> Vec<f32> {
    let low = Wave::Sine {
        hz: 1_000.0,
        peak: i16::MAX / 2,
    };
    let high = Wave::Sine {
        hz: 30_000.0,
        peak: i16::MAX / 2,
    };
    (0..frames)
        .map(|frame| {
            (f32::from(low.sample(frame, 96_000)) + f32::from(high.sample(frame, 96_000)))
                / 32_768.0
        })
        .collect()
}

fn stream_through(chunk: usize, input: &[f32]) -> Vec<f32> {
    let settings = ResamplerSettings::builder()
        .channels(channels(1))
        .mode(fixed_mode(96_000, 48_000))
        .options(ResamplerOptions::builder().chunk_size(1_024).build())
        .pools(pools())
        .build();
    let mut resampler = GlideResampler::new("glide", GlideConfig::default(), &settings)
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    let mut block = vec![0.0; resampler.output_frames_next()];
    let mut output = Vec::new();
    let mut offset = 0;
    while input.len() - offset >= 2 {
        let end = (offset + chunk).min(input.len());
        let process = resampler
            .process_into_buffer(&[&input[offset..end]], &mut [&mut block])
            .unwrap_or_else(|err| panic!("stream process should succeed: {err}"));
        output.extend_from_slice(&block[..process.output_frames]);
        if process.input_frames == 0 {
            break;
        }
        offset += process.input_frames;
    }
    output
}

/// Largest error of a method reproducing a straight line.
const RAMP_TOLERANCE: f32 = 1.0e-5;

/// `input[i] = i`: an interpolated sample reads back as its source position.
fn ramp_input(frames: u16) -> Vec<f32> {
    (0..frames).map(f32::from).collect()
}

#[kithara::test(native, flash(false))]
fn backend_reports_glide_capabilities() {
    let capabilities = GlideBackend::new().capabilities();

    assert!(capabilities.contains(ResamplerCapabilities::FIXED_RATIO));
    assert!(capabilities.contains(ResamplerCapabilities::VARIABLE_RATIO));
    assert!(capabilities.contains(ResamplerCapabilities::RATIO_GLIDE));
    assert!(capabilities.contains(ResamplerCapabilities::REALTIME_SAFE));
    assert!(capabilities.contains(ResamplerCapabilities::STANDALONE));
}

#[kithara::test(native, flash(false))]
fn fixed_ratio_output_contract_uses_glide_ratio() {
    let resampler = build_glide(44_100, 48_000);

    assert_eq!(resampler.output_frames_for_input(4_410), 4_800);
}

#[kithara::test(native, flash(false))]
fn unity_fast_path_copies_input(glide_unity: Vec<f32>) {
    let mut resampler = build_glide(44_100, 44_100);
    let input = glide_unity;
    let mut output = [0.0; 4];
    let process = resampler
        .process_into_buffer(&[&input], &mut [&mut output])
        .unwrap_or_else(|err| panic!("unity process should succeed: {err}"));

    assert_eq!(process.input_frames, input.len());
    assert_eq!(process.output_frames, output.len());
    assert_eq!(output.as_slice(), input);
}

#[kithara::test(native, flash(false))]
fn quadratic_interpolates_between_input_frames(glide_quadratic: Vec<f32>) {
    let mut resampler = build_glide(44_100, 88_200);
    let input = glide_quadratic;
    let mut output = [0.0; 12];
    let process = resampler
        .process_into_buffer(&[&input], &mut [&mut output])
        .unwrap_or_else(|err| panic!("glide process should succeed: {err}"));

    assert!(process.output_frames > input.len());
    assert!(output[..process.output_frames].iter().any(|sample| {
        let magnitude = sample.abs();
        magnitude > 0.0 && magnitude < 1.0
    }));
}

#[kithara::test(native, flash(false))]
fn glide_ratio_reaches_target_without_discontinuity(glide_transition: Vec<f32>) {
    let mode = ResamplerMode::VariableRatio {
        sample_rate: rate(48_000),
        initial_ratio: 1.0,
        glide: Some(RatioGlide {
            frames: rate(8),
            target_ratio: 0.5,
        }),
    };
    let settings = settings(mode);
    let mut resampler = GlideResampler::new("glide", GlideConfig::default(), &settings)
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    ResamplerControl::glide_ratio(
        &mut resampler,
        RatioGlide {
            frames: rate(8),
            target_ratio: 0.5,
        },
    )
    .unwrap_or_else(|err| panic!("glide should be accepted: {err}"));
    let input = glide_transition;
    let mut output = [0.0; 24];
    let process = resampler
        .process_into_buffer(&[&input], &mut [&mut output])
        .unwrap_or_else(|err| panic!("glide process should succeed: {err}"));

    assert!(process.output_frames > 0);
    for pair in output[..process.output_frames].windows(2) {
        assert!((pair[1] - pair[0]).abs() < 0.5);
    }
}

#[kithara::test(native, flash(false))]
fn factory_output_exposes_glide_control_surface() {
    let mode = ResamplerMode::VariableRatio {
        sample_rate: rate(48_000),
        initial_ratio: 1.0,
        glide: None,
    };
    let config = ResamplerConfig::builder()
        .backend(GlideBackend::new())
        .settings(settings(mode))
        .build();
    let mut resampler = create_resampler(&config)
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));

    let control = resampler
        .control_mut()
        .unwrap_or_else(|| panic!("glide should expose ratio controls"));
    control
        .glide_ratio(RatioGlide {
            frames: rate(4),
            target_ratio: 0.75,
        })
        .unwrap_or_else(|err| panic!("glide glide should be accepted: {err}"));
}

#[kithara::test(native, flash(false))]
fn linear_mode_can_be_selected_by_config() {
    let backend = GlideBackend::with_config(
        GlideConfig::builder()
            .interpolation(Interpolation::Linear)
            .build(),
    );
    let config = ResamplerConfig::builder()
        .backend(backend)
        .settings(settings(fixed_mode(44_100, 48_000)))
        .build();

    create_resampler(&config)
        .unwrap_or_else(|err| panic!("linear glide resampler should build: {err}"));
}

#[kithara::test(native, flash(false))]
fn anti_alias_smooths_fast_glide(glide_alias: Vec<f32>) {
    let input = glide_alias;
    let mut plain = GlideResampler::new(
        "glide",
        GlideConfig::builder().anti_alias(false).build(),
        &settings(fixed_mode(96_000, 48_000)),
    )
    .unwrap_or_else(|err| panic!("plain glide should build: {err}"));
    let mut filtered = GlideResampler::new(
        "glide",
        GlideConfig::builder().anti_alias(true).build(),
        &settings(fixed_mode(96_000, 48_000)),
    )
    .unwrap_or_else(|err| panic!("filtered glide should build: {err}"));
    let mut plain_output = [0.0; 8];
    let mut filtered_output = [0.0; 8];
    let plain_frames = plain
        .process_into_buffer(&[&input], &mut [&mut plain_output])
        .unwrap_or_else(|err| panic!("plain process should succeed: {err}"))
        .output_frames;
    let filtered_frames = filtered
        .process_into_buffer(&[&input], &mut [&mut filtered_output])
        .unwrap_or_else(|err| panic!("filtered process should succeed: {err}"))
        .output_frames;
    let plain_energy: f32 = plain_output[..plain_frames]
        .iter()
        .map(|sample| sample.abs())
        .sum();
    let filtered_energy: f32 = filtered_output[..filtered_frames]
        .iter()
        .map(|sample| sample.abs())
        .sum();

    assert!(filtered_energy < plain_energy);
}

#[kithara::test(native, flash(false))]
fn exact_span_keeps_a_constant_signal_at_the_right_boundary() {
    let mode = ResamplerMode::VariableRatio {
        sample_rate: rate(48_000),
        initial_ratio: 1.0,
        glide: None,
    };
    let mut resampler = GlideResampler::new("glide", GlideConfig::default(), &settings(mode))
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    let input = [0.75; 15];
    let mut output = [0.0; 16];

    resampler
        .process_exact_span(&[input.as_slice()], &mut [output.as_mut_slice()])
        .unwrap_or_else(|err| panic!("exact span should render: {err}"));

    assert!(
        output.iter().all(|sample| (*sample - 0.75).abs() < 1.0e-6),
        "constant input changed at an exact-span boundary: {output:?}"
    );
}

#[kithara::test(native, flash(false))]
fn near_unity_exact_span_preserves_the_first_mapped_attack() {
    const SOURCE_FRAMES: usize = 10_001;
    const OUTPUT_FRAMES: usize = 10_000;
    let mode = ResamplerMode::VariableRatio {
        sample_rate: rate(48_000),
        initial_ratio: 1.0,
        glide: None,
    };
    let settings = ResamplerSettings::builder()
        .channels(channels(1))
        .mode(mode)
        .options(
            ResamplerOptions::builder()
                .chunk_size(SOURCE_FRAMES)
                .build(),
        )
        .pools(pools())
        .build();
    let mut resampler = GlideResampler::new("glide", GlideConfig::default(), &settings)
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    let mut input = vec![0.0; SOURCE_FRAMES];
    input[0] = 1.0;
    let mut output = vec![0.0; OUTPUT_FRAMES];

    resampler
        .process_exact_span(&[input.as_slice()], &mut [output.as_mut_slice()])
        .unwrap_or_else(|err| panic!("near-unity exact span should render: {err}"));

    assert!(
        (output[0] - 1.0).abs() < 1.0e-6,
        "the source attack mapped to output frame zero moved: {:?}",
        &output[..8]
    );
}

#[kithara::test(native, flash(false))]
async fn exact_span_retunes_anti_alias_filter_without_allocating() {
    let mut resampler = {
        let _permit = kithara_test_utils::no_block::permit();
        let mode = ResamplerMode::VariableRatio {
            sample_rate: rate(48_000),
            initial_ratio: 1.0,
            glide: None,
        };
        GlideResampler::new("glide", GlideConfig::default(), &settings(mode))
            .expect("prepared Glide buffers")
    };
    let source = [0.25; 16];
    let mut target = [0.0; 16];
    for frames in [8, 4, 16, 8] {
        kithara_test_utils::no_block::watch("Glide exact-span filter retuning", 1_000, async {
            resampler.process_exact_span(&[source.as_slice()], &mut [&mut target[..frames]])
        })
        .await
        .expect("retuning reuses filter storage");
        assert!(target[..frames].iter().all(|sample| sample.is_finite()));
    }
    let _permit = kithara_test_utils::no_block::permit();
    drop(resampler);
}

#[kithara::test(native, flash(false))]
#[case::stream_unity(false, 44_100)]
#[case::stream_downsample(false, 22_050)]
#[case::exact_unity(true, 44_100)]
#[case::exact_downsample(true, 22_050)]
fn extra_channel_storage_preserves_the_prepared_channel_shape(
    #[case] exact: bool,
    #[case] target_rate: u32,
    glide_unity: Vec<f32>,
) {
    let mut expected = build_glide(44_100, target_rate);
    let mut actual = build_glide(44_100, target_rate);
    let frames = if target_rate == 44_100 { 4 } else { 2 };
    let mut reference = [0.0; 4];
    let mut rendered = [0.0; 4];
    let mut extra = [0.75; 4];
    let input = glide_unity.as_slice();
    if exact {
        expected
            .process_exact_span(&[input], &mut [&mut reference[..frames]])
            .expect("prepared channel renders");
        actual
            .process_exact_span(
                &[input, &[]],
                &mut [&mut rendered[..frames], &mut extra[..]],
            )
            .expect("additional storage does not change the prepared channel shape");
    } else {
        let reference_span = expected
            .process_into_buffer(&[input], &mut [&mut reference[..frames]])
            .expect("prepared channel renders");
        let actual_span = actual
            .process_into_buffer(
                &[input, &[]],
                &mut [&mut rendered[..frames], &mut extra[..]],
            )
            .expect("additional storage does not change the prepared channel shape");
        assert_eq!(actual_span, reference_span);
    }
    assert_eq!(rendered, reference);
    assert_eq!(extra, [0.75; 4]);
}

#[kithara::test(native)]
fn anti_alias_output_does_not_depend_on_chunking() {
    let input = two_tones(4_096);
    let small = stream_through(64, &input);
    let large = stream_through(1_024, &input);
    let frames = small.len().min(large.len());
    assert!(
        frames > 1_900,
        "both chunkings render the stream: {} / {}",
        small.len(),
        large.len()
    );
    let worst = small[..frames]
        .iter()
        .zip(&large[..frames])
        .map(|(small, large)| (small - large).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        worst <= CHUNKING_TOLERANCE,
        "chunking changed the output by {worst}"
    );
}

#[kithara::test(native)]
fn entering_the_filter_keeps_a_constant_signal_constant() {
    let mode = ResamplerMode::VariableRatio {
        sample_rate: rate(48_000),
        initial_ratio: 1.0,
        glide: None,
    };
    let mut resampler = GlideResampler::new("glide", GlideConfig::default(), &settings(mode))
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    let input = [0.5_f32; 16];
    let mut output = [0.0_f32; 16];
    resampler
        .process_into_buffer(&[&input], &mut [&mut output])
        .unwrap_or_else(|err| panic!("passthrough block should render: {err}"));
    resampler
        .set_ratio(2.0)
        .unwrap_or_else(|err| panic!("ratio 2 is in range: {err}"));
    let process = resampler
        .process_into_buffer(&[&input], &mut [&mut output])
        .unwrap_or_else(|err| panic!("filtered block should render: {err}"));

    assert!(process.output_frames > 0);
    assert!(
        output[..process.output_frames]
            .iter()
            .all(|sample| (sample - 0.5).abs() < CONSTANT_TOLERANCE),
        "the filter did not start in the steady state: {output:?}"
    );
}

/// Positions in the last interval `[n, n + 1)` read the tail, which holds
/// `after + 1` copies of the last input frame.
#[kithara::test(native)]
#[case::linear(Interpolation::Linear)]
#[case::quadratic(Interpolation::Quadratic)]
#[case::hermite(Interpolation::Hermite)]
#[case::watte(Interpolation::Watte)]
fn exact_span_keeps_a_constant_at_the_boundary_for_every_method(
    #[case] interpolation: Interpolation,
) {
    let mode = ResamplerMode::VariableRatio {
        sample_rate: rate(48_000),
        initial_ratio: 1.0,
        glide: None,
    };
    let config = GlideConfig::builder().interpolation(interpolation).build();
    let mut resampler = GlideResampler::new("glide", config, &settings(mode))
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    let input = [0.75; 15];
    let mut output = [0.0; 20];

    resampler
        .process_exact_span(&[input.as_slice()], &mut [output.as_mut_slice()])
        .unwrap_or_else(|err| panic!("exact span should render: {err}"));

    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.75).abs() < CONSTANT_TOLERANCE),
        "{interpolation:?} changed a constant at the exact-span boundary: {output:?}"
    );
}

/// Frames 0 and 1 read the seeded history, which a ramp does not continue.
#[kithara::test(native)]
#[case::linear(Interpolation::Linear)]
#[case::quadratic(Interpolation::Quadratic)]
#[case::hermite(Interpolation::Hermite)]
#[case::watte(Interpolation::Watte)]
fn every_interpolation_reproduces_a_ramp_through_the_factory(#[case] interpolation: Interpolation) {
    let backend = GlideBackend::with_config(
        GlideConfig::builder()
            .interpolation(interpolation)
            .anti_alias(false)
            .build(),
    );
    let config = ResamplerConfig::builder()
        .backend(backend)
        .settings(settings(fixed_mode(44_100, 88_200)))
        .build();
    let mut resampler = create_resampler(&config)
        .unwrap_or_else(|err| panic!("glide resampler should build: {err}"));
    let input = ramp_input(16);
    let mut output = [0.0; 34];
    let process = resampler
        .process_into_buffer(&[&input], &mut [&mut output])
        .unwrap_or_else(|err| panic!("ramp process should succeed: {err}"));

    assert!(
        process.output_frames > 2,
        "{interpolation:?} rendered {} frames",
        process.output_frames
    );
    for (sample, frame) in output[..process.output_frames].iter().zip(0_u16..).skip(2) {
        let expected = f32::from(frame) * 0.5;
        assert!(
            (sample - expected).abs() < RAMP_TOLERANCE,
            "{interpolation:?} frame {frame}: {sample} against {expected}"
        );
    }
}
