//! Conformance suite for the exact-span elastic contract.
//!
//! Every compiled-in engine runs the same bodies through
//! `elastic_engine_conformance!`, including the mandatory priming lifecycle.
//! Backend-specific prepared limits and latency are asserted next to each
//! engine; nothing else in the suite names a backend.

use std::f32::consts::TAU;

use kithara_bufpool::{ByteBudget, PcmPool};
use kithara_stretch::{
    ElasticCapabilities, ElasticConfig, ElasticEngine, ElasticError, ElasticRequest,
    ElasticSpanConfig, StretchKind, build_engine,
};
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

const CHANNELS: usize = 2;
const CONTROL_QUANTUM: usize = 64;
const SAMPLE_RATE: u32 = 48_000;
const TONE_HZ: f64 = 440.0;

fn prepared<E: ElasticEngine>(max_source_frames: usize, max_output_frames: usize) -> E {
    let config = ElasticConfig::builder()
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

fn drain_terminal(engine: &mut dyn ElasticEngine) -> Vec<f32> {
    const MAX_CHUNKS: usize = 32;

    let chunk_frames = engine.capabilities().terminal_chunk_frames();
    let mut chunk = vec![0.0; chunk_frames * CHANNELS];
    let mut drained = Vec::new();
    for _ in 0..MAX_CHUNKS {
        chunk.fill(0.0);
        let frames = engine.flush(&mut chunk).expect("terminal flush");
        if frames == 0 {
            assert_eq!(engine.flush(&mut chunk).expect("completed drain"), 0);
            return drained;
        }
        drained.extend_from_slice(&chunk[..frames * CHANNELS]);
    }
    panic!("terminal drain must converge to an empty flush");
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
    let slow_output_frames = unity_frames - unity_frames % 20;
    [
        (slow_output_frames / 20, slow_output_frames),
        (unity_frames, unity_frames),
        (unity_frames, unity_frames / 4),
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
                const MIN_NATIVE_RANGE: f64 = 0.25;
                const MAX_NATIVE_RANGE: f64 = 4.0;
                const BELOW_NATIVE_RANGE: f64 = 0.249;
                const ABOVE_NATIVE_RANGE: f64 = 4.001;

                let mut engine = prepared_backend(backend, 8192, 8192);

                for scale in [
                    0.0,
                    -1.0,
                    f64::NAN,
                    f64::INFINITY,
                    BELOW_NATIVE_RANGE,
                    ABOVE_NATIVE_RANGE,
                ] {
                    assert!(matches!(
                        engine.set_pitch(scale),
                        Err(ElasticError::InvalidPitch(_))
                    ));
                }

                for scale in [MIN_NATIVE_RANGE, MAX_NATIVE_RANGE] {
                    engine
                        .set_pitch(scale)
                        .expect("the common native pitch boundary is supported");
                }
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn terminal_flush_drains_real_audio_to_completion(#[case] backend: StretchKind) {
                const FRAMES: usize = 8192;

                for request in [
                    ElasticRequest::new(FRAMES / 2, FRAMES).expect("half-speed request"),
                    ElasticRequest::new(FRAMES, FRAMES).expect("unity request"),
                    ElasticRequest::new(FRAMES, FRAMES / 2).expect("double-speed request"),
                ] {
                    let mut engine = prepared_backend(backend, FRAMES, FRAMES);
                    let source = interleaved_signal(request.source_frames());
                    let mut output = vec![0.0; request.output_frames() * CHANNELS];
                    engine
                        .process(request, &source, &mut output)
                        .expect("the source block renders");
                    let terminal = drain_terminal(engine.as_mut());
                    let drained = terminal.len() / CHANNELS;
                    let terminal_peak = terminal
                        .iter()
                        .map(|sample| sample.abs())
                        .fold(0.0_f32, f32::max);

                    assert!(
                        drained > 0,
                        "an active engine must drain its buffered terminal audio"
                    );
                    assert!(
                        terminal_peak > f32::EPSILON,
                        "the complete drain must contain real terminal audio: backend={backend:?}, request={request:?}, drained={drained}, peak={terminal_peak}"
                    );
                }
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn flush_rejects_wrong_storage_without_disarming_tail(
                #[case] backend: StretchKind,
            ) {
                const FRAMES: usize = 8192;

                let mut engine = prepared_backend(backend, FRAMES, FRAMES);
                let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
                let source = interleaved_signal(FRAMES);
                let mut output = vec![0.0; FRAMES * CHANNELS];
                engine
                    .process(request, &source, &mut output)
                    .expect("the source block renders");
                let tail_frames = engine.capabilities().terminal_chunk_frames();
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
                let drained = engine
                    .flush(&mut terminal)
                    .expect("the rejected call keeps the tail armed");
                assert!(drained > 0);
                assert!(drained <= tail_frames);
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
                    engine.capabilities().terminal_chunk_frames() * CHANNELS;
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
                    engine.capabilities().terminal_chunk_frames() * CHANNELS;
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
            fn rate_envelope_is_the_configured_practical_domain(#[case] backend: StretchKind) {
                let engine = prepared_backend(backend, 8192, 4096);
                let envelope = engine.capabilities().rate_envelope();

                assert_eq!(envelope.min_source_frames_per_output(), 0.05);
                assert_eq!(envelope.max_source_frames_per_output(), 4.0);
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

                let request = ElasticRequest::new(32, 1).expect("non-empty extreme-rate request");
                assert_eq!(
                    engine.process(
                        request,
                        &source[..request.source_frames() * CHANNELS],
                        &mut output[..request.output_frames() * CHANNELS],
                    ),
                    Err(ElasticError::RateOutsideEnvelope {
                        source_frames: 32,
                        output_frames: 1,
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

macro_rules! elastic_priming_conformance {
    ($module:ident) => {
        mod $module {
            use super::*;

            fn indexed_markers(frames: usize, offset: usize) -> Vec<f32> {
                marker_signal(frames, offset, |index| {
                    let marker_index = u16::try_from(index.wrapping_mul(73) % 997)
                        .expect("invariant: marker index is bounded below 997");
                    (f32::from(marker_index) / 997.0) * 1.5 - 0.75
                })
            }

            fn continuous_tone(frames: usize, offset: usize) -> Vec<f32> {
                let phase_step = TAU
                    * TONE_HZ.to_f32().expect("the fixture frequency fits in f32")
                    / SAMPLE_RATE
                        .to_f32()
                        .expect("the fixture sample rate fits in f32");
                marker_signal(frames, offset, |index| {
                    (index.to_f32().expect("the fixture timeline fits in f32") * phase_step).sin()
                        * 0.5
                })
            }

            fn mean(samples: &[f32]) -> f32 {
                samples.iter().sum::<f32>()
                    / samples
                        .len()
                        .to_f32()
                        .expect("the sample window fits in f32")
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

            fn primed_playing_pair(
                backend: StretchKind,
            ) -> (
                Box<dyn ElasticEngine>,
                Box<dyn ElasticEngine>,
                ElasticCapabilities,
                usize,
            ) {
                const MAX_FRAMES: usize = 65_536;

                let mut reference = prepared_backend(backend, MAX_FRAMES, MAX_FRAMES);
                let mut changed = prepared_backend(backend, MAX_FRAMES, MAX_FRAMES);
                let capabilities = reference.capabilities();
                assert_eq!(changed.capabilities(), capabilities);
                let latency = capabilities.latency();
                let warmup = warmup_request(capabilities, 1.0);
                let history = indexed_markers(latency.source_frames(), 0);
                let lookahead = indexed_markers(latency.source_frames(), latency.source_frames());
                let warm_source = indexed_markers(
                    warmup.source_frames(),
                    latency.source_frames().saturating_mul(2),
                );
                let mut reference_discard = vec![0.0; warmup.output_frames() * CHANNELS];
                let mut changed_discard = vec![0.0; warmup.output_frames() * CHANNELS];
                reference
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut reference_discard,
                    )
                    .expect("reference engine primes");
                changed
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut changed_discard,
                    )
                    .expect("changed engine primes");

                let continuation = latency
                    .source_frames()
                    .saturating_mul(2)
                    .saturating_add(warmup.source_frames());
                let source = indexed_markers(CONTROL_QUANTUM, continuation);
                let request = ElasticRequest::new(CONTROL_QUANTUM, CONTROL_QUANTUM)
                    .expect("lead quantum is non-empty");
                let mut reference_output = vec![f32::NAN; CONTROL_QUANTUM * CHANNELS];
                let mut changed_output = vec![f32::NAN; CONTROL_QUANTUM * CHANNELS];
                reference
                    .process(request, &source, &mut reference_output)
                    .expect("reference lead quantum renders");
                changed
                    .process(request, &source, &mut changed_output)
                    .expect("changed lead quantum renders");
                assert_exact_samples(&changed_output, &reference_output);

                (
                    reference,
                    changed,
                    capabilities,
                    continuation + CONTROL_QUANTUM,
                )
            }

            fn assert_control_response(
                reference: &mut dyn ElasticEngine,
                changed: &mut dyn ElasticEngine,
                capabilities: ElasticCapabilities,
                continuation: usize,
                changed_rate: f64,
                changed_pitch: f64,
            ) {
                changed
                    .set_pitch(changed_pitch)
                    .expect("the changed pitch is supported");
                let mut reference_position = continuation;
                let mut changed_position = continuation;
                let mut remaining = capabilities.latency().output_frames();
                while remaining > 0 {
                    let output_frames = remaining.min(CONTROL_QUANTUM);
                    let reference_request = ElasticRequest::new(output_frames, output_frames)
                        .expect("reference quantum is non-empty");
                    let changed_source_frames =
                        source_frames_at(changed_rate, output_frames, false);
                    let changed_request =
                        ElasticRequest::new(changed_source_frames, output_frames)
                            .expect("changed quantum is non-empty");
                    let reference_source = indexed_markers(output_frames, reference_position);
                    let changed_source =
                        indexed_markers(changed_source_frames, changed_position);
                    let mut reference_output = vec![f32::NAN; output_frames * CHANNELS];
                    let mut changed_output = vec![f32::NAN; output_frames * CHANNELS];
                    reference
                        .process(
                            reference_request,
                            &reference_source,
                            &mut reference_output,
                        )
                        .expect("reference control quantum renders");
                    changed
                        .process(changed_request, &changed_source, &mut changed_output)
                        .expect("changed control quantum renders");
                    assert!(reference_output.iter().all(|sample| sample.is_finite()));
                    assert!(changed_output.iter().all(|sample| sample.is_finite()));
                    if changed_output != reference_output {
                        return;
                    }
                    reference_position += output_frames;
                    changed_position += changed_source_frames;
                    remaining -= output_frames;
                }

                panic!("a control change must affect output within the declared native latency");
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn history_and_output_warmup_remove_the_initial_gap(#[case] backend: StretchKind) {
                const FRAMES: usize = 512;

                let mut engine = prepared_backend(backend, FRAMES * 2, FRAMES);
                let capabilities = engine.capabilities();
                let history_frames = capabilities.latency().source_frames();
                let history = vec![0.25; history_frames * CHANNELS];
                let lookahead = vec![0.25; history.len()];
                let warmup = warmup_request(capabilities, 1.0);
                let warm_source = vec![0.25; warmup.source_frames() * CHANNELS];
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                engine
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut discarded,
                    )
                    .expect("history and output latency warmup");
                let source = vec![0.25; FRAMES * CHANNELS];
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
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn post_prime_pitch_change_responds_within_declared_latency(
                #[case] backend: StretchKind,
            ) {
                let (mut reference, mut changed, capabilities, continuation) =
                    primed_playing_pair(backend);

                assert_control_response(
                    reference.as_mut(),
                    changed.as_mut(),
                    capabilities,
                    continuation,
                    1.0,
                    1.5,
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn post_prime_rate_change_responds_within_declared_latency(
                #[case] backend: StretchKind,
            ) {
                let (mut reference, mut changed, capabilities, continuation) =
                    primed_playing_pair(backend);

                assert_control_response(
                    reference.as_mut(),
                    changed.as_mut(),
                    capabilities,
                    continuation,
                    2.0,
                    1.0,
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn source_history_conditions_the_cue_boundary(#[case] backend: StretchKind) {
                const MAX_FRAMES: usize = 65_536;

                let mut conditioned = prepared_backend(backend, MAX_FRAMES, MAX_FRAMES);
                let mut zero_padded = prepared_backend(backend, MAX_FRAMES, MAX_FRAMES);
                let capabilities = conditioned.capabilities();
                assert_eq!(zero_padded.capabilities(), capabilities);
                let latency = capabilities.latency();
                let warmup = warmup_request(capabilities, 1.0);
                let history = continuous_tone(latency.source_frames(), 0);
                let empty_history = vec![0.0; history.len()];
                let lookahead = continuous_tone(latency.source_frames(), latency.source_frames());
                let warm_source = continuous_tone(
                    warmup.source_frames(),
                    latency.source_frames().saturating_mul(2),
                );
                let mut conditioned_discard = vec![0.0; warmup.output_frames() * CHANNELS];
                let mut zero_padded_discard = vec![0.0; warmup.output_frames() * CHANNELS];
                conditioned
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut conditioned_discard,
                    )
                    .expect("conditioned engine primes");
                zero_padded
                    .prime(
                        warmup,
                        &empty_history,
                        &lookahead,
                        &warm_source,
                        &mut zero_padded_discard,
                    )
                    .expect("zero-padded engine primes");

                let quantum = latency.output_frames();
                let source = continuous_tone(
                    quantum,
                    latency
                        .source_frames()
                        .saturating_mul(2)
                        .saturating_add(warmup.source_frames()),
                );
                let request =
                    ElasticRequest::new(quantum, quantum).expect("next quantum is non-empty");
                let mut conditioned_output = vec![f32::NAN; quantum * CHANNELS];
                let mut zero_padded_output = vec![f32::NAN; quantum * CHANNELS];
                conditioned
                    .process(request, &source, &mut conditioned_output)
                    .expect("conditioned next quantum renders");
                zero_padded
                    .process(request, &source, &mut zero_padded_output)
                    .expect("zero-padded next quantum renders");

                assert!(conditioned_output.iter().all(|sample| sample.is_finite()));
                assert!(zero_padded_output.iter().all(|sample| sample.is_finite()));
                assert!(
                    conditioned_output != zero_padded_output,
                    "pre-cue history must condition the cue boundary without becoming audible source"
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith_slow(StretchKind::Signalsmith, 0.05)
            )]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith_fast(StretchKind::Signalsmith, 4.0)
            )]
            #[cfg_attr(
                feature = "stretch-bungee",
                case::bungee_slow(StretchKind::Bungee, 0.05)
            )]
            #[cfg_attr(
                feature = "stretch-bungee",
                case::bungee_fast(StretchKind::Bungee, 4.0)
            )]
            fn prime_accepts_declared_rate_edges(
                #[case] backend: StretchKind,
                #[case] rate: f64,
            ) {
                const FRAMES: usize = 512;

                let mut engine = prepared_backend(backend, FRAMES * 2, FRAMES);
                let capabilities = engine.capabilities();
                let history_frames = capabilities.latency().source_frames();
                let output_frames = capabilities.latency().output_frames();
                let source_frames = source_frames_at(rate, output_frames, rate < 1.0);
                let request = ElasticRequest::new(source_frames, output_frames)
                    .expect("the declared edge request is non-empty");
                let history = vec![0.25; history_frames * CHANNELS];
                let lookahead = vec![0.25; history.len()];
                let source = vec![0.25; source_frames * CHANNELS];
                let mut discarded = vec![f32::NAN; output_frames * CHANNELS];

                engine
                    .prime(request, &history, &lookahead, &source, &mut discarded)
                    .expect("the declared prime rate edge is supported");

                assert!(discarded.iter().all(|sample| sample.is_finite()));
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith_unity(StretchKind::Signalsmith, 1.0)
            )]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith_non_unity(StretchKind::Signalsmith, 1.2)
            )]
            #[cfg_attr(
                feature = "stretch-bungee",
                case::bungee_unity(StretchKind::Bungee, 1.0)
            )]
            #[cfg_attr(
                feature = "stretch-bungee",
                case::bungee_non_unity(StretchKind::Bungee, 1.2)
            )]
            fn priming_hides_history_and_preserves_source_order(
                #[case] backend: StretchKind,
                #[case] source_frames_per_output: f64,
            ) {
                const MAX_FRAMES: usize = 65_536;
                const FOLLOWING_FRAMES: usize = 4096;

                let mut engine = prepared_backend(backend, MAX_FRAMES, MAX_FRAMES);
                let capabilities = engine.capabilities();
                let history_frames = capabilities.latency().source_frames();
                let warmup = warmup_request(capabilities, source_frames_per_output);
                let history = vec![0.9; history_frames * CHANNELS];
                let lookahead = vec![0.2; history.len()];
                let warm_source = vec![0.5; warmup.source_frames() * CHANNELS];
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                engine
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut discarded,
                    )
                    .expect("the engine absorbs the complete preroll");

                let render_output_frames = warmup
                    .output_frames()
                    .checked_mul(3)
                    .and_then(|frames| frames.checked_add(FOLLOWING_FRAMES))
                    .expect("the continuation span fits in usize");
                let render_source_frames = source_frames_at(
                    source_frames_per_output,
                    render_output_frames,
                    false,
                );
                assert!(render_source_frames <= MAX_FRAMES);
                assert!(render_output_frames <= MAX_FRAMES);
                let source = vec![0.8; render_source_frames * CHANNELS];
                let mut output = vec![f32::NAN; render_output_frames * CHANNELS];
                engine
                    .process(
                        ElasticRequest::new(render_source_frames, render_output_frames)
                            .expect("continuation request"),
                        &source,
                        &mut output,
                    )
                    .expect("the primed stream continues");

                let lookahead_output_frames = history_frames
                    .to_f64()
                    .map(|frames| (frames / source_frames_per_output).round())
                    .and_then(|frames| frames.to_usize())
                    .expect("the lookahead output span fits in usize");
                let lookahead_begin = lookahead_output_frames / 4;
                let lookahead_end = lookahead_output_frames * 3 / 4;
                let lookahead_mean = mean(
                    &output[lookahead_begin * CHANNELS..lookahead_end * CHANNELS],
                );
                let warm_begin = lookahead_output_frames + warmup.output_frames() / 4;
                let warm_end = lookahead_output_frames + warmup.output_frames() * 3 / 4;
                let warm_mean = mean(&output[warm_begin * CHANNELS..warm_end * CHANNELS]);
                let following_begin = render_output_frames - FOLLOWING_FRAMES / 2;
                let following_mean = mean(&output[following_begin * CHANNELS..]);

                assert!(
                    lookahead_mean.is_finite() && lookahead_mean > 0.01,
                    "the post-cue lookahead must be audible, mean={lookahead_mean}"
                );
                assert!(
                    lookahead_mean + 0.02 < warm_mean,
                    "pre-cue history leaked or the warmer region was skipped: lookahead={lookahead_mean}, warm={warm_mean}"
                );
                assert!(
                    warm_mean + 0.1 < following_mean,
                    "the warmup was duplicated or following source was skipped: warm={warm_mean}, following={following_mean}"
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn prime_rejects_every_ambiguous_buffer_count(#[case] backend: StretchKind) {
                let mut engine = prepared_backend(backend, 1024, 512);
                let capabilities = engine.capabilities();
                let warmup = warmup_request(capabilities, 1.0);
                let history = vec![0.25; capabilities.latency().source_frames() * CHANNELS];
                let lookahead = vec![0.25; history.len()];
                let source = vec![0.25; warmup.source_frames() * CHANNELS];
                let mut discarded = vec![0.0; warmup.output_frames() * CHANNELS];

                assert_eq!(
                    engine.prime(
                        warmup,
                        &history[..history.len() - 1],
                        &lookahead,
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
                        &lookahead[..lookahead.len() - 1],
                        &source,
                        &mut discarded,
                    ),
                    Err(ElasticError::LookaheadSampleCount {
                        actual: lookahead.len() - 1,
                        expected: lookahead.len(),
                    })
                );
                assert_eq!(
                    engine.prime(
                        warmup,
                        &history,
                        &lookahead,
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
                        &lookahead,
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
                    engine.prime(
                        wrong_output,
                        &history,
                        &lookahead,
                        &source,
                        &mut discarded,
                    ),
                    Err(ElasticError::WarmupOutputFrameCount {
                        actual: warmup.output_frames() - 1,
                        expected: warmup.output_frames(),
                    })
                );
            }

            #[kithara::test]
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn reset_reprime_keeps_the_first_frame_aligned(#[case] backend: StretchKind) {
                const SOURCE_FRAMES: usize = 600;
                const OUTPUT_FRAMES: usize = 500;

                let mut engine = prepared_backend(backend, SOURCE_FRAMES, OUTPUT_FRAMES);
                let capabilities = engine.capabilities();
                let warmup = warmup_request(capabilities, 1.2);
                let history = vec![0.25; capabilities.latency().source_frames() * CHANNELS];
                let lookahead = vec![0.25; history.len()];
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
                        .prime(
                            warmup,
                            &history,
                            &lookahead,
                            &warm_source,
                            &mut discarded,
                        )
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
            #[cfg_attr(
                feature = "stretch-signalsmith",
                case::signalsmith(StretchKind::Signalsmith)
            )]
            #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
            fn prime_discards_previous_stream_state(#[case] backend: StretchKind) {
                const FRAMES: usize = 4096;

                let mut fresh = prepared_backend(backend, FRAMES, FRAMES);
                let mut reused = prepared_backend(backend, FRAMES, FRAMES);
                let capabilities = fresh.capabilities();
                let warmup = warmup_request(capabilities, 1.0);
                let history_frames = capabilities.latency().source_frames();
                let history = indexed_markers(history_frames, 0);
                let lookahead = indexed_markers(history_frames, history_frames);
                let warm_source = indexed_markers(warmup.source_frames(), history_frames * 2);
                let source = indexed_markers(
                    FRAMES,
                    history_frames * 2 + warmup.source_frames(),
                );
                let dirty_source = interleaved_signal(FRAMES);
                let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
                let mut dirty_output = vec![0.0; FRAMES * CHANNELS];
                reused
                    .process(request, &dirty_source, &mut dirty_output)
                    .expect("the dirtying request is supported");

                let mut fresh_discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                let mut reused_discarded = vec![0.0; warmup.output_frames() * CHANNELS];
                fresh
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut fresh_discarded,
                    )
                    .expect("fresh engine primes");
                reused
                    .prime(
                        warmup,
                        &history,
                        &lookahead,
                        &warm_source,
                        &mut reused_discarded,
                    )
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

#[cfg(feature = "stretch-bungee")]
#[kithara::test]
fn bungee_half_speed_flush_reaches_the_last_source_marker() {
    const FRAMES: usize = 8192;
    const MARKER_FRAMES: usize = 1024;

    let request = ElasticRequest::new(FRAMES / 2, FRAMES).expect("half-speed request");
    let mut engine = prepared_backend(StretchKind::Bungee, FRAMES, FRAMES);
    let mut source = vec![0.0; request.source_frames() * CHANNELS];
    let marker_start = request.source_frames() - MARKER_FRAMES;
    for frame in marker_start..request.source_frames() {
        for channel in 0..CHANNELS {
            source[frame * CHANNELS + channel] = 0.5;
        }
    }
    let mut output = vec![0.0; request.output_frames() * CHANNELS];
    engine
        .process(request, &source, &mut output)
        .expect("the source block renders");
    let terminal_peak = drain_terminal(engine.as_mut())
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);

    assert!(
        terminal_peak > f32::EPSILON,
        "the complete drain must reach audio at the end of the source"
    );
}

elastic_priming_conformance!(priming);

#[cfg(feature = "stretch-signalsmith")]
#[kithara::test]
fn signalsmith_declares_its_prepared_domain_and_latency() {
    let engine: kithara_stretch::SignalsmithElastic = prepared(8192, 8192);
    let capabilities = engine.capabilities();

    assert_eq!(capabilities.sample_rate(), SAMPLE_RATE);
    assert_eq!(capabilities.channels(), CHANNELS);
    assert_eq!(
        capabilities.rate_envelope().min_source_frames_per_output(),
        0.05
    );
    assert_eq!(
        capabilities.rate_envelope().max_source_frames_per_output(),
        4.0
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
    let tail_frames = engine.capabilities().terminal_chunk_frames();
    let mut terminal = vec![0.0; tail_frames * CHANNELS];

    let first = engine.flush(&mut terminal).expect("terminal tail drains");
    let repeated = engine
        .flush(&mut terminal)
        .expect("terminal drain is one-shot");

    assert_eq!(first, tail_frames);
    assert_eq!(repeated, 0);
    assert!(terminal.iter().any(|sample| sample.abs() > f32::EPSILON));
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
        0.05
    );
    assert_eq!(
        capabilities.rate_envelope().max_source_frames_per_output(),
        4.0
    );
    assert!(
        capabilities.latency().output_frames() > 0,
        "the engine must report the lag its pipeline carries"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn prepare_uses_the_injected_pcm_pool_budget(#[case] backend: StretchKind) {
    let config = ElasticConfig::builder()
        .backend(backend)
        .pool(PcmPool::with_byte_budget(8, 8192, ByteBudget(0)))
        .sample_rate(SAMPLE_RATE)
        .channels(CHANNELS)
        .max_source_frames(8192)
        .max_output_frames(8192)
        .build()
        .expect("the numeric preparation shape is valid");

    let error = match build_engine(config) {
        Ok(_) => panic!("zero pool budget cannot prepare resident PCM scratch"),
        Err(error) => error,
    };

    assert_eq!(error, ElasticError::PcmPoolBudgetExhausted);
}

#[cfg(feature = "stretch-bungee")]
#[kithara::test]
fn bungee_pool_usage_scales_with_the_prepared_source_limit() {
    fn allocated_bytes(max_source_frames: usize) -> usize {
        let pool = PcmPool::with_byte_budget(8, 8192, ByteBudget(usize::MAX));
        let config = ElasticConfig::builder()
            .backend(StretchKind::Bungee)
            .pool(pool.clone())
            .sample_rate(SAMPLE_RATE)
            .channels(CHANNELS)
            .max_source_frames(max_source_frames)
            .max_output_frames(8192)
            .build()
            .expect("the numeric preparation shape is valid");
        let engine = kithara_stretch::BungeeElastic::prepare(config)
            .expect("the prepared shape fits an unlimited pool");
        let allocated = pool.allocated_bytes();
        drop(engine);
        allocated
    }

    let one_frame = allocated_bytes(1);
    let full_block = allocated_bytes(8192);

    assert!(
        one_frame < full_block,
        "latency probing must not inflate every shape to an 8192-frame allocation"
    );
}
