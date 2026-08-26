//! Conformance suite for the exact-span elastic contract.
//!
//! Every compiled-in engine runs the same bodies through
//! `elastic_engine_conformance!`; engines that can absorb source history also
//! run `elastic_priming_conformance!`. Backend-specific numbers (the declared
//! prepared frame domain and backend latency are asserted next to each engine,
//! and nothing else in the suite names a backend.

use std::f32::consts::TAU;

#[cfg(feature = "stretch-bungee")]
use kithara_bufpool::ByteBudget;
use kithara_bufpool::PcmPool;
use kithara_stretch::{
    ElasticCapabilities, ElasticConfig, ElasticEngine, ElasticError, ElasticRequest,
    ElasticSpanConfig, StretchKind, build_engine,
};
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

const CHANNELS: usize = 2;
const SAMPLE_RATE: u32 = 48_000;
const TONE_HZ: f64 = 440.0;

fn prepared<E: ElasticEngine>(max_source_frames: usize, max_output_frames: usize) -> E {
    let config = kithara_stretch::ElasticConfig::builder()
        .pool(PcmPool::default())
        .sample_rate(SAMPLE_RATE)
        .channels(CHANNELS)
        .max_source_frames(max_source_frames)
        .max_output_frames(max_output_frames)
        .build()
        .expect("the test configuration is valid");
    E::prepare(config).expect("the engine prepares for a valid shape")
}

fn prepared_backend(
    backend: StretchKind,
    max_source_frames: usize,
    max_output_frames: usize,
) -> Box<dyn ElasticEngine> {
    let config = ElasticConfig::builder()
        .backend(backend)
        .pool(PcmPool::default())
        .sample_rate(SAMPLE_RATE)
        .channels(CHANNELS)
        .max_source_frames(max_source_frames)
        .max_output_frames(max_output_frames)
        .build()
        .expect("the test configuration is valid");
    build_engine(config).expect("the selected engine prepares for a valid shape")
}

fn interleaved_signal(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let sample = if frame % 64 < 32 { 0.25 } else { -0.25 };
            [sample, -sample]
        })
        .collect()
}

fn impulse_markers(frames: usize, offset: usize) -> Vec<f32> {
    marker_signal(frames, offset, |index| {
        if index.is_multiple_of(64) {
            let marker_index = u16::try_from((index / 64) % 7)
                .expect("invariant: marker index is bounded below 7");
            0.5 + f32::from(marker_index) / 14.0
        } else {
            0.0
        }
    })
}

fn marker_signal(
    frames: usize,
    offset: usize,
    mut marker_at: impl FnMut(usize) -> f32,
) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let index = offset.wrapping_add(frame);
            let marker = marker_at(index);
            [marker, marker * -0.5]
        })
        .collect()
}

fn first_audible_frame(samples: &[f32], channels: usize) -> Option<usize> {
    samples
        .chunks_exact(channels)
        .position(|frame| frame.iter().any(|sample| sample.abs() >= 1.0e-4))
}

fn assert_exact_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual, expected, "sample {index} differs");
    }
}

/// The source span an engine accepts at a declared envelope edge: the planner
/// quantizes the same way, so a conformance request is never a rounding step
/// outside the window it is meant to exercise.
#[cfg(feature = "stretch-signalsmith")]
fn source_frames_at(rate: f64, output_frames: usize, round_up: bool) -> usize {
    let frames = output_frames
        .to_f64()
        .map(|frames| frames * rate)
        .expect("invariant: the test block fits in f64");
    let frames = if round_up {
        frames.ceil()
    } else {
        frames.floor()
    };
    frames
        .to_usize()
        .expect("invariant: the edge span fits in usize")
}

fn edge_requests(capabilities: ElasticCapabilities) -> [ElasticRequest; 3] {
    let unity_frames = capabilities
        .max_source_frames()
        .min(capabilities.max_output_frames());
    [
        (1, capabilities.max_output_frames()),
        (unity_frames, unity_frames),
        (capabilities.max_source_frames(), 1),
    ]
    .map(|(source_frames, output_frames)| {
        ElasticRequest::new(source_frames, output_frames)
            .expect("invariant: prepared-domain request is non-empty")
    })
}

macro_rules! elastic_engine_conformance {
    ($module:ident) => {
        mod $module {
            use super::*;

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn renders_the_requested_output_frame_count(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 8192, 8192);
                let request = ElasticRequest::new(4800, 4000).expect("the request is non-empty");
                let source = interleaved_signal(request.source_frames());
                let mut output = vec![f32::NAN; request.output_frames() * CHANNELS];

                engine
                    .process(request, &source, &mut output)
                    .expect("the request is inside the prepared envelope");

                assert_eq!(output.len(), 4000 * CHANNELS);
                assert!(output.iter().all(|sample| sample.is_finite()));
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn renders_exact_spans_at_both_declared_rate_edges(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 8192, 4096);
                let capabilities = engine.capabilities();

                for request in edge_requests(capabilities) {
                    let source = interleaved_signal(request.source_frames());
                    let mut output = vec![f32::NAN; request.output_frames() * CHANNELS];

                    engine
                        .process(request, &source, &mut output)
                        .expect("a declared edge rate is supported");

                    assert_eq!(output.len(), request.output_frames() * CHANNELS);
                    assert!(output.iter().all(|sample| sample.is_finite()));
                }
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn output_is_independent_of_request_partitioning(#[case] backend: StretchKind) {
                const FRAMES: usize = 16_384;
                const PARTITION_FRAMES: usize = 512;

                let mut whole = prepared_backend(backend, FRAMES, FRAMES);
                let mut partitioned = prepared_backend(backend, FRAMES, FRAMES);
                let source = impulse_markers(FRAMES, 0);
                let mut whole_output = vec![0.0; FRAMES * CHANNELS];
                whole
                    .process(
                        ElasticRequest::new(FRAMES, FRAMES).expect("whole unity request"),
                        &source,
                        &mut whole_output,
                    )
                    .expect("the whole block renders");

                let mut partitioned_output = vec![0.0; FRAMES * CHANNELS];
                for (source, output) in source
                    .chunks_exact(PARTITION_FRAMES * CHANNELS)
                    .zip(partitioned_output.chunks_exact_mut(PARTITION_FRAMES * CHANNELS))
                {
                    partitioned
                        .process(
                            ElasticRequest::new(PARTITION_FRAMES, PARTITION_FRAMES)
                                .expect("partition unity request"),
                            source,
                            output,
                        )
                        .expect("every partition renders");
                }

                assert!(
                    first_audible_frame(&whole_output, CHANNELS).is_some(),
                    "the block must outlast the engine latency for this to compare audio"
                );
                assert_exact_samples(&partitioned_output, &whole_output);
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn keeps_capabilities_stable_through_rate_changes(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 8192, 8192);
                let capabilities = engine.capabilities();

                for request in [
                    ElasticRequest::new(4096, 4096).expect("unity request"),
                    ElasticRequest::new(4800, 4000).expect("faster request"),
                    ElasticRequest::new(4096, 4096).expect("unity request"),
                ] {
                    let source = interleaved_signal(request.source_frames());
                    let mut output = vec![f32::NAN; request.output_frames() * CHANNELS];

                    engine
                        .process(request, &source, &mut output)
                        .expect("the request is supported");

                    assert!(output.iter().all(|sample| sample.is_finite()));
                    assert_eq!(engine.capabilities(), capabilities);
                }
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn pitch_control_is_independent_of_exact_frame_advance(#[case] backend: StretchKind) {
                let mut reference = prepared_backend(backend, 8192, 8192);
                let mut pitched = prepared_backend(backend, 8192, 8192);
                let request = ElasticRequest::new(4096, 4096).expect("unity request");
                let mut changed = false;

                pitched.set_pitch(1.25).expect("positive pitch scale");
                for block in 0..4 {
                    let source = marker_signal(request.source_frames(), block * 4096, |index| {
                        f32::from(u16::try_from(index % 997).expect("marker index fits")) / 997.0
                            - 0.5
                    });
                    let mut reference_output = vec![f32::NAN; request.output_frames() * CHANNELS];
                    let mut pitched_output = vec![f32::NAN; request.output_frames() * CHANNELS];
                    reference
                        .process(request, &source, &mut reference_output)
                        .expect("reference engine renders the exact span");
                    pitched
                        .process(request, &source, &mut pitched_output)
                        .expect("pitch does not replace exact frame control");

                    assert!(pitched_output.iter().all(|sample| sample.is_finite()));
                    changed |= pitched_output != reference_output;
                }

                assert!(changed, "pitch control must alter rendered samples");
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn rejects_invalid_pitch_scales(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 8192, 8192);

                for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
                    assert!(matches!(
                        engine.set_pitch(scale),
                        Err(ElasticError::InvalidPitch(_))
                    ));
                }
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn terminal_flush_is_idempotent(#[case] backend: StretchKind) {
                const FRAMES: usize = 8192;

                let mut engine = prepared_backend(backend, FRAMES, FRAMES);
                let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
                let source = interleaved_signal(FRAMES);
                let mut output = vec![0.0; FRAMES * CHANNELS];
                engine
                    .process(request, &source, &mut output)
                    .expect("the source block renders");
                let terminal_samples =
                    engine.capabilities().latency().output_frames() * CHANNELS;
                let mut terminal = vec![0.0; terminal_samples];

                let first = engine.flush(&mut terminal).expect("first terminal flush");
                let repeated = engine
                    .flush(&mut terminal)
                    .expect("repeated terminal flush");

                assert!(first <= engine.capabilities().latency().output_frames());
                assert_eq!(repeated, 0);
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn fresh_engine_has_no_terminal_tail(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 8192, 8192);
                let terminal_samples =
                    engine.capabilities().latency().output_frames() * CHANNELS;
                let mut terminal = vec![0.0; terminal_samples];

                let frames = engine.flush(&mut terminal).expect("fresh terminal drain");

                assert_eq!(frames, 0);
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn reset_engine_has_no_terminal_tail(#[case] backend: StretchKind) {
                const FRAMES: usize = 8192;

                let mut engine = prepared_backend(backend, FRAMES, FRAMES);
                let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
                let source = interleaved_signal(FRAMES);
                let mut output = vec![0.0; FRAMES * CHANNELS];
                engine
                    .process(request, &source, &mut output)
                    .expect("the source block renders");
                engine.reset().expect("the engine clears its history");
                let terminal_samples =
                    engine.capabilities().latency().output_frames() * CHANNELS;
                let mut terminal = vec![0.0; terminal_samples];

                let frames = engine.flush(&mut terminal).expect("reset terminal drain");

                assert_eq!(frames, 0);
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn reset_clears_stream_history_without_changing_capabilities(#[case] backend: StretchKind) {
                const LONG_FRAMES: usize = 16_384;
                const SHORT_FRAMES: usize = 4096;

                let mut engine = prepared_backend(backend, LONG_FRAMES, LONG_FRAMES);
                let capabilities = engine.capabilities();
                let source = interleaved_signal(LONG_FRAMES);
                let mut output = vec![0.0; LONG_FRAMES * CHANNELS];
                engine
                    .process(
                        ElasticRequest::new(LONG_FRAMES, LONG_FRAMES).expect("unity request"),
                        &source,
                        &mut output,
                    )
                    .expect("the warm request is supported");
                assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));

                engine.reset().expect("the engine clears its history");
                output[..SHORT_FRAMES * CHANNELS].fill(f32::NAN);
                engine
                    .process(
                        ElasticRequest::new(SHORT_FRAMES, SHORT_FRAMES).expect("unity request"),
                        &source[..SHORT_FRAMES * CHANNELS],
                        &mut output[..SHORT_FRAMES * CHANNELS],
                    )
                    .expect("the request after reset is supported");

                assert_eq!(engine.capabilities(), capabilities);
                assert!(
                    output[..SHORT_FRAMES * CHANNELS]
                        .iter()
                        .all(|sample| sample.abs() <= f32::EPSILON),
                    "a reset engine must start from its own latency again"
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn preserves_tone_pitch_when_source_advance_changes(#[case] backend: StretchKind) {
                const SOURCE_FRAMES: usize = 19_200;
                const OUTPUT_FRAMES: usize = 16_000;

                let config = ElasticConfig::builder()
                    .backend(backend)
                    .pool(PcmPool::default())
                    .sample_rate(SAMPLE_RATE)
                    .channels(1)
                    .max_source_frames(SOURCE_FRAMES)
                    .max_output_frames(OUTPUT_FRAMES)
                    .build()
                    .expect("the test configuration is valid");
                let mut engine = build_engine(config).expect("the selected engine prepares");
                let request =
                    ElasticRequest::new(SOURCE_FRAMES, OUTPUT_FRAMES).expect("non-empty request");
                let phase_step = TAU * 440.0 / 48_000.0;
                let mut phase: f32 = 0.0;
                let source = (0..SOURCE_FRAMES)
                    .map(|_| {
                        let sample = phase.sin();
                        phase += phase_step;
                        sample
                    })
                    .collect::<Vec<_>>();
                let mut output = vec![0.0; OUTPUT_FRAMES];

                engine
                    .process(request, &source, &mut output)
                    .expect("the request is supported");

                let latency = engine.capabilities().latency().output_frames();
                let audible = output
                    .len()
                    .checked_sub(latency)
                    .expect("the block must outlast the engine latency");
                let expected = TONE_HZ * audible.to_f64().expect("audible span fits in f64")
                    / f64::from(SAMPLE_RATE);
                let positive_crossings = output[latency..]
                    .windows(2)
                    .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                    .count()
                    .to_f64()
                    .expect("crossing count fits in f64");
                assert!(
                    (positive_crossings - expected).abs() <= expected * 0.1,
                    "expected a pitch-locked {TONE_HZ} Hz tone (~{expected} crossings), counted {positive_crossings}"
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn rate_envelope_is_the_complete_prepared_non_empty_domain(#[case] backend: StretchKind) {
                let engine = prepared_backend(backend, 8192, 4096);
                let capabilities = engine.capabilities();
                let envelope = capabilities.rate_envelope();
                let max_output_frames = capabilities
                    .max_output_frames()
                    .to_f64()
                    .expect("prepared output limit fits f64");
                let max_source_frames = capabilities
                    .max_source_frames()
                    .to_f64()
                    .expect("prepared source limit fits f64");

                assert_eq!(
                    envelope.min_source_frames_per_output(),
                    1.0 / max_output_frames
                );
                assert_eq!(
                    envelope.max_source_frames_per_output(),
                    max_source_frames
                );
                assert!(
                    !envelope.contains_rate(envelope.min_source_frames_per_output() / 2.0)
                );
                assert!(
                    !envelope.contains_rate(envelope.max_source_frames_per_output() * 2.0)
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn rejects_buffers_that_do_not_match_the_request(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 8192, 8192);
                let request = ElasticRequest::new(4800, 4000).expect("non-empty request");
                let source = interleaved_signal(request.source_frames());
                let mut output = vec![0.0; request.output_frames() * CHANNELS];

                assert_eq!(
                    engine.process(request, &source[..source.len() - 1], &mut output),
                    Err(ElasticError::SourceSampleCount {
                        actual: source.len() - 1,
                        expected: 4800 * CHANNELS,
                    })
                );

                let output_len = output.len();
                assert_eq!(
                    engine.process(request, &source, &mut output[..output_len - 1]),
                    Err(ElasticError::OutputSampleCount {
                        actual: output_len - 1,
                        expected: 4000 * CHANNELS,
                    })
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn rejects_spans_beyond_the_prepared_block_limits(#[case] backend: StretchKind) {
                const MAX_SOURCE_FRAMES: usize = 2048;
                const MAX_OUTPUT_FRAMES: usize = 2048;

                let mut engine = prepared_backend(backend, MAX_SOURCE_FRAMES, MAX_OUTPUT_FRAMES);
                let mut output = vec![0.0; MAX_OUTPUT_FRAMES * CHANNELS];
                let source = interleaved_signal(MAX_SOURCE_FRAMES + 1);

                let request = ElasticRequest::new(MAX_SOURCE_FRAMES + 1, MAX_OUTPUT_FRAMES)
                    .expect("non-empty request");
                assert_eq!(
                    engine.process(request, &source, &mut output),
                    Err(ElasticError::SourceFrameLimit {
                        frames: MAX_SOURCE_FRAMES + 1,
                        limit: MAX_SOURCE_FRAMES,
                    })
                );

                let mut long_output = vec![0.0; (MAX_OUTPUT_FRAMES + 1) * CHANNELS];
                let request = ElasticRequest::new(MAX_SOURCE_FRAMES, MAX_OUTPUT_FRAMES + 1)
                    .expect("non-empty request");
                assert_eq!(
                    engine.process(
                        request,
                        &source[..MAX_SOURCE_FRAMES * CHANNELS],
                        &mut long_output,
                    ),
                    Err(ElasticError::OutputFrameLimit {
                        frames: MAX_OUTPUT_FRAMES + 1,
                        limit: MAX_OUTPUT_FRAMES,
                    })
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn plans_and_renders_one_block_of_continuous_source_spans(#[case] backend: StretchKind) {
                use kithara_stretch::{ElasticSpan, ElasticSpanPlan};

                const OUTPUT_FRAMES: usize = 512;

                let mut engine = prepared_backend(backend, 4096, 4096);
                let capabilities = engine.capabilities();
                let span_config = ElasticSpanConfig::builder()
                    .build()
                    .expect("finite positive span policy");
                let source_end = OUTPUT_FRAMES
                    .to_f64()
                    .expect("invariant: the block fits in f64");
                let plan = ElasticSpanPlan::new(
                    [
                        ElasticSpan::try_from((0.0..source_end / 2.0, OUTPUT_FRAMES / 2))
                            .expect("first continuous span"),
                        ElasticSpan::try_from((source_end / 2.0..source_end, OUTPUT_FRAMES / 2))
                            .expect("second continuous span"),
                    ],
                    None,
                    capabilities,
                    span_config,
                )
                .expect("a unity path is inside every declared envelope");

                let source = interleaved_signal(OUTPUT_FRAMES);
                let mut consumed = 0;
                for segment in plan.segments() {
                    let request = segment.request();
                    let samples = request.source_frames() * CHANNELS;
                    let mut output = vec![f32::NAN; request.output_frames() * CHANNELS];

                    engine
                        .process(request, &source[consumed..consumed + samples], &mut output)
                        .expect("a planned segment is always renderable");

                    assert!(output.iter().all(|sample| sample.is_finite()));
                    consumed += samples;
                }
                assert_eq!(plan.cursor().integer(), i64::try_from(OUTPUT_FRAMES).expect("cursor fits"));
            }
        }
    };
}

#[cfg(feature = "stretch-signalsmith")]
macro_rules! elastic_priming_conformance {
    ($module:ident, $engine:ty) => {
        mod $module {
            use kithara_stretch::{ElasticCapabilities, ElasticPriming};

            use super::*;

            fn indexed_markers(frames: usize, offset: usize) -> Vec<f32> {
                marker_signal(frames, offset, |index| {
                    let marker_index = u16::try_from(index.wrapping_mul(73) % 997)
                        .expect("invariant: marker index is bounded below 997");
                    (f32::from(marker_index) / 997.0) * 1.5 - 0.75
                })
            }

            fn warmup_request(
                capabilities: ElasticCapabilities,
                source_frames_per_output: f64,
            ) -> ElasticRequest {
                assert!(
                    capabilities
                        .rate_envelope()
                        .contains_rate(source_frames_per_output),
                    "invariant: warmup rate stays inside the envelope"
                );
                let output_frames = capabilities.latency().output_frames();
                let source_frames =
                    source_frames_at(source_frames_per_output, output_frames, false);
                ElasticRequest::new(source_frames, output_frames)
                    .expect("invariant: warmup request is valid")
            }

            #[kithara::test]
            fn history_and_output_warmup_remove_the_initial_gap() {
                const FRAMES: usize = 512;

                let mut engine: $engine = prepared(FRAMES * 2, FRAMES);
                let capabilities = engine.capabilities();
                let history_frames = capabilities.latency().source_frames();
                let history = impulse_markers(history_frames, 0);
                let warmup = warmup_request(capabilities, 1.0);
                let warm_source = impulse_markers(warmup.source_frames(), history_frames);
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                engine
                    .prime(warmup, &history, &warm_source, &mut discarded)
                    .expect("history and output latency warmup");
                let source = impulse_markers(FRAMES, history_frames + warmup.source_frames());
                let mut output = vec![0.0; FRAMES * CHANNELS];

                engine
                    .process(
                        ElasticRequest::new(FRAMES, FRAMES).expect("unity request"),
                        &source,
                        &mut output,
                    )
                    .expect("primed unity request");

                assert_eq!(first_audible_frame(&output, CHANNELS), Some(0));
            }

            #[kithara::test]
            fn non_unity_warmup_aligns_the_first_audible_frame() {
                const SOURCE_FRAMES: usize = 600;
                const OUTPUT_FRAMES: usize = 500;

                let mut engine: $engine = prepared(SOURCE_FRAMES, OUTPUT_FRAMES);
                let capabilities = engine.capabilities();
                let history_frames = capabilities.latency().source_frames();
                let history = impulse_markers(history_frames, 0);
                let warmup = warmup_request(capabilities, 1.2);
                let warm_source = impulse_markers(warmup.source_frames(), history_frames);
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                engine
                    .prime(warmup, &history, &warm_source, &mut discarded)
                    .expect("non-unity history and output latency warmup");
                let source =
                    impulse_markers(SOURCE_FRAMES, history_frames + warmup.source_frames());
                let mut output = vec![f32::NAN; OUTPUT_FRAMES * CHANNELS];

                engine
                    .process(
                        ElasticRequest::new(SOURCE_FRAMES, OUTPUT_FRAMES)
                            .expect("non-unity request"),
                        &source,
                        &mut output,
                    )
                    .expect("primed non-unity request");

                assert!(output.iter().all(|sample| sample.is_finite()));
                assert_eq!(first_audible_frame(&output, CHANNELS), Some(0));
            }

            #[kithara::test]
            fn prime_rejects_every_ambiguous_buffer_count() {
                let mut engine: $engine = prepared(1024, 512);
                let capabilities = engine.capabilities();
                let warmup = warmup_request(capabilities, 1.0);
                let history = vec![0.25; capabilities.latency().source_frames() * CHANNELS];
                let source = vec![0.25; warmup.source_frames() * CHANNELS];
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];

                assert_eq!(
                    engine.prime(
                        warmup,
                        &history[..history.len() - 1],
                        &source,
                        &mut discarded
                    ),
                    Err(ElasticError::HistorySampleCount {
                        actual: history.len() - 1,
                        expected: history.len(),
                    })
                );
                assert_eq!(
                    engine.prime(
                        warmup,
                        &history,
                        &source[..source.len() - 1],
                        &mut discarded
                    ),
                    Err(ElasticError::SourceSampleCount {
                        actual: source.len() - 1,
                        expected: source.len(),
                    })
                );
                let discarded_len = discarded.len();
                assert_eq!(
                    engine.prime(
                        warmup,
                        &history,
                        &source,
                        &mut discarded[..discarded_len - 1]
                    ),
                    Err(ElasticError::OutputSampleCount {
                        actual: discarded_len - 1,
                        expected: discarded_len,
                    })
                );
                let wrong_output =
                    ElasticRequest::new(warmup.source_frames(), warmup.output_frames() - 1)
                        .expect("non-empty mismatched warmup request");
                assert_eq!(
                    engine.prime(wrong_output, &history, &source, &mut discarded),
                    Err(ElasticError::WarmupOutputFrameCount {
                        actual: warmup.output_frames() - 1,
                        expected: warmup.output_frames(),
                    })
                );
            }

            #[kithara::test]
            fn reset_reprime_keeps_the_first_frame_aligned() {
                const SOURCE_FRAMES: usize = 600;
                const OUTPUT_FRAMES: usize = 500;

                let mut engine: $engine = prepared(SOURCE_FRAMES, OUTPUT_FRAMES);
                let capabilities = engine.capabilities();
                let warmup = warmup_request(capabilities, 1.2);
                let history = vec![0.25; capabilities.latency().source_frames() * CHANNELS];
                let warm_source = vec![0.25; warmup.source_frames() * CHANNELS];
                let source = vec![0.25; SOURCE_FRAMES * CHANNELS];
                let request =
                    ElasticRequest::new(SOURCE_FRAMES, OUTPUT_FRAMES).expect("non-unity request");
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                let mut output = vec![0.0; OUTPUT_FRAMES * CHANNELS];

                for cycle in 0..8 {
                    if cycle > 0 {
                        engine.reset().expect("the engine clears its history");
                    }
                    engine
                        .prime(warmup, &history, &warm_source, &mut discarded)
                        .expect("reset engine primes again");
                    engine
                        .process(request, &source, &mut output)
                        .expect("request after reset is supported");

                    assert_eq!(engine.capabilities(), capabilities);
                    assert!(output[..CHANNELS].iter().all(|sample| sample.is_finite()));
                    assert!(
                        output[..CHANNELS]
                            .iter()
                            .any(|sample| sample.abs() > f32::EPSILON),
                        "cycle {cycle} retained stale latency"
                    );
                }
            }

            #[kithara::test]
            fn prime_discards_previous_stream_state() {
                const FRAMES: usize = 4096;

                let mut fresh: $engine = prepared(FRAMES, FRAMES);
                let mut reused: $engine = prepared(FRAMES, FRAMES);
                let capabilities = fresh.capabilities();
                let warmup = warmup_request(capabilities, 1.0);
                let history = indexed_markers(capabilities.latency().source_frames(), 0);
                let warm_source = indexed_markers(warmup.source_frames(), history.len() / CHANNELS);
                let source =
                    indexed_markers(FRAMES, (history.len() + warm_source.len()) / CHANNELS);
                let dirty_source = interleaved_signal(FRAMES);
                let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
                let mut dirty_output = vec![0.0; FRAMES * CHANNELS];
                reused
                    .process(request, &dirty_source, &mut dirty_output)
                    .expect("the dirtying request is supported");

                let mut fresh_discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                let mut reused_discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                fresh
                    .prime(warmup, &history, &warm_source, &mut fresh_discarded)
                    .expect("fresh engine primes");
                reused
                    .prime(warmup, &history, &warm_source, &mut reused_discarded)
                    .expect("reused engine primes");

                let mut fresh_output = vec![0.0; FRAMES * CHANNELS];
                let mut reused_output = vec![0.0; FRAMES * CHANNELS];
                fresh
                    .process(request, &source, &mut fresh_output)
                    .expect("fresh engine renders after priming");
                reused
                    .process(request, &source, &mut reused_output)
                    .expect("reused engine renders after priming");

                assert_exact_samples(&reused_discarded, &fresh_discarded);
                assert_exact_samples(&reused_output, &fresh_output);
            }
        }
    };
}

elastic_engine_conformance!(facade);

#[cfg(feature = "stretch-signalsmith")]
elastic_priming_conformance!(signalsmith_priming, kithara_stretch::SignalsmithElastic);

#[cfg(feature = "stretch-signalsmith")]
#[kithara::test]
fn signalsmith_declares_its_prepared_domain_and_latency() {
    let engine: kithara_stretch::SignalsmithElastic = prepared(8192, 8192);
    let capabilities = engine.capabilities();

    assert_eq!(capabilities.sample_rate(), SAMPLE_RATE);
    assert_eq!(capabilities.channels(), CHANNELS);
    assert_eq!(
        capabilities.rate_envelope().min_source_frames_per_output(),
        1.0 / 8192.0
    );
    assert_eq!(
        capabilities.rate_envelope().max_source_frames_per_output(),
        8192.0
    );
    assert_eq!(capabilities.latency().source_frames(), 2880);
    assert_eq!(capabilities.latency().output_frames(), 2880);
}

#[cfg(feature = "stretch-signalsmith")]
#[kithara::test]
fn signalsmith_unity_render_exposes_the_declared_latency() {
    const FRAMES: usize = 8192;

    let mut engine: kithara_stretch::SignalsmithElastic = prepared(FRAMES, FRAMES);
    let latency = engine.capabilities().latency();
    let source = impulse_markers(FRAMES, 0);
    let mut output = vec![f32::NAN; FRAMES * CHANNELS];

    engine
        .process(
            ElasticRequest::new(FRAMES, FRAMES).expect("unity request"),
            &source,
            &mut output,
        )
        .expect("unity is inside the supported envelope");

    assert_eq!(
        first_audible_frame(&output, CHANNELS),
        Some(latency.source_frames() + latency.output_frames())
    );
}

#[cfg(feature = "stretch-signalsmith")]
#[kithara::test]
fn signalsmith_flush_drains_a_real_tail_once() {
    const FRAMES: usize = 8192;

    let mut engine: kithara_stretch::SignalsmithElastic = prepared(FRAMES, FRAMES);
    let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
    let source = interleaved_signal(FRAMES);
    let mut output = vec![0.0; FRAMES * CHANNELS];
    engine
        .process(request, &source, &mut output)
        .expect("the source block renders");
    let tail_frames = engine.capabilities().latency().output_frames();
    let mut terminal = vec![0.0; tail_frames * CHANNELS];

    let first = engine.flush(&mut terminal).expect("terminal tail drains");
    let repeated = engine
        .flush(&mut terminal)
        .expect("terminal drain is one-shot");

    assert_eq!(first, tail_frames);
    assert_eq!(repeated, 0);
    assert!(terminal.iter().any(|sample| sample.abs() > f32::EPSILON));
}

#[cfg(feature = "stretch-signalsmith")]
#[kithara::test]
fn signalsmith_flush_rejects_wrong_storage_without_disarming_tail() {
    const FRAMES: usize = 8192;

    let mut engine: kithara_stretch::SignalsmithElastic = prepared(FRAMES, FRAMES);
    let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
    let source = interleaved_signal(FRAMES);
    let mut output = vec![0.0; FRAMES * CHANNELS];
    engine
        .process(request, &source, &mut output)
        .expect("the source block renders");
    let tail_frames = engine.capabilities().latency().output_frames();
    let tail_samples = tail_frames * CHANNELS;
    let mut short = vec![0.0; tail_samples - CHANNELS];

    let error = engine
        .flush(&mut short)
        .expect_err("an armed tail requires latency-sized storage");

    assert_eq!(
        error,
        ElasticError::OutputSampleCount {
            actual: short.len(),
            expected: tail_samples,
        }
    );
    let mut terminal = vec![0.0; tail_samples];
    assert_eq!(
        engine
            .flush(&mut terminal)
            .expect("the rejected call keeps the tail armed"),
        tail_frames
    );
}

#[cfg(feature = "stretch-bungee")]
#[kithara::test]
fn bungee_declares_the_prepared_domain_and_latency() {
    let engine: kithara_stretch::BungeeElastic = prepared(8192, 8192);
    let capabilities = engine.capabilities();

    assert_eq!(capabilities.sample_rate(), SAMPLE_RATE);
    assert_eq!(capabilities.channels(), CHANNELS);
    assert_eq!(
        capabilities.rate_envelope().min_source_frames_per_output(),
        1.0 / 8192.0
    );
    assert_eq!(
        capabilities.rate_envelope().max_source_frames_per_output(),
        8192.0
    );
    assert!(
        capabilities.latency().output_frames() > 0,
        "the engine must report the lag its pipeline carries"
    );
}

#[cfg(feature = "stretch-bungee")]
#[kithara::test]
fn bungee_flush_honestly_emits_no_synthetic_tail() {
    let mut engine: kithara_stretch::BungeeElastic = prepared(8192, 8192);
    let mut terminal = vec![0.0; engine.capabilities().latency().output_frames() * CHANNELS];

    let frames = engine.flush(&mut terminal).expect("no-op flush succeeds");

    assert_eq!(frames, 0);
}

#[cfg(feature = "stretch-bungee")]
#[kithara::test]
fn bungee_prepare_uses_the_injected_pcm_pool_budget() {
    let config = kithara_stretch::ElasticConfig::builder()
        .pool(PcmPool::with_byte_budget(8, 8192, ByteBudget(0)))
        .sample_rate(SAMPLE_RATE)
        .channels(CHANNELS)
        .max_source_frames(8192)
        .max_output_frames(8192)
        .build()
        .expect("the numeric preparation shape is valid");

    let error = kithara_stretch::BungeeElastic::prepare(config)
        .expect_err("zero pool budget cannot prepare planar PCM scratch");

    assert_eq!(error, ElasticError::PcmPoolBudgetExhausted);
}
