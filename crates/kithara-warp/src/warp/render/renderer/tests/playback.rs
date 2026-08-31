use std::num::{NonZero, NonZeroU32};

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunkInfo, AudioSpec, FrameCount};
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

use super::{
    Consts, StretchControls, WarpRenderer, chunk, chunk_at, dominant_bin, expected_bin, f64_of,
    flush_serviced, render_serviced, renderer, sine, spec,
};

fn keylocked(kind: StretchKind, speed: f32) -> WarpRenderer {
    let controls = StretchControls::new(speed);
    controls.set_keylock(true);
    controls.set_backend(kind);
    renderer(controls)
}

fn vinyl(kind: StretchKind, speed: f32) -> WarpRenderer {
    let controls = StretchControls::new(speed);
    controls.set_keylock(false);
    controls.set_backend(kind);
    renderer(controls)
}

fn render_with_tail(fx: &mut WarpRenderer, input: &[f32]) -> (Vec<f32>, usize) {
    let mut out: Vec<f32> = Vec::new();
    let mut tail_frames = 0;
    let block = 4096 * usize::from(Consts::CH);
    let mut frame_offset = 0_u64;
    for data in input.chunks(block) {
        if let Some(c) = render_serviced(fx, chunk_at(data, frame_offset)) {
            assert_eq!(
                c.spec().sample_rate.get(),
                Consts::SR,
                "stretch preserves sample rate"
            );
            assert_eq!(c.spec().channels, Consts::CH);
            out.extend_from_slice(&c.samples);
        }
        frame_offset = frame_offset
            .checked_add(
                u64::try_from(data.len() / usize::from(Consts::CH)).expect("test frame count fits"),
            )
            .expect("test frame offset fits");
    }
    while let Some(c) = flush_serviced(fx) {
        // A non-empty flush chunk carries real audio, so its spec must stay
        // the source spec - never the `AudioChunkInfo::default()` sentinel (0
        // channels) that a `None` `last_input_meta` would otherwise yield.
        assert_eq!(c.spec().channels, Consts::CH, "flush preserves channels");
        assert_eq!(
            c.spec().sample_rate.get(),
            Consts::SR,
            "flush preserves sample rate"
        );
        tail_frames += c.frames();
        out.extend_from_slice(&c.samples);
    }
    (out, tail_frames)
}

fn render(fx: &mut WarpRenderer, input: &[f32]) -> Vec<f32> {
    render_with_tail(fx, input).0
}

fn render_quantum_frames(fx: &mut WarpRenderer, input: &[f32], frame_offset: u64) -> usize {
    let channels = usize::from(Consts::CH);
    let mut consumed = 0usize;
    let mut rendered = 0usize;
    while consumed < input.len() / channels {
        fx.prepare(spec());
        let remaining = input.len() / channels - consumed;
        let mut meta = chunk(&input[consumed * channels..]).meta;
        meta.frame_offset = frame_offset
            .checked_add(u64::try_from(consumed).expect("test frame count fits u64"))
            .expect("test frame offset fits u64");
        meta.timestamp = spec()
            .duration_for(meta.frame_offset)
            .expect("test timestamp fits");
        let frames = fx
            .prepare_quantum(meta, remaining)
            .expect("test quantum fits")
            .get();
        let end = (consumed + frames) * channels;
        let input_chunk = chunk_at(&input[consumed * channels..end], meta.frame_offset);
        if let Some(output) = fx.render_quantum(input_chunk) {
            rendered += output.frames();
        }
        consumed += frames;
    }
    fx.prepare(spec());
    rendered
}

fn run_keylocked_with_tail(kind: StretchKind, speed: f32, in_frames: usize) -> (Vec<f32>, usize) {
    let input = sine(in_frames);
    render_with_tail(&mut keylocked(kind, speed), &input)
}

fn run_vinyl(kind: StretchKind, speed: f32, in_frames: usize) -> Vec<f32> {
    let input = sine(in_frames);
    render(&mut vinyl(kind, speed), &input)
}

/// Half playback speed -> stretch 2.0 -> ~double duration, pitch held.
/// Shared across every compiled-in backend.
fn assert_half_speed_contract(kind: StretchKind) {
    let channels = usize::from(Consts::CH);
    let in_frames = usize::try_from(Consts::SR).unwrap() * 2; // 2 s
    let (out, tail_frames) = run_keylocked_with_tail(kind, 0.5, in_frames);
    let out_frames = out.len() / channels;
    let timeline_frames = out_frames - tail_frames;
    let expected_timeline = in_frames * 2;

    assert_eq!(
        timeline_frames, expected_timeline,
        "{kind:?}: exact half-speed timeline"
    );
    assert!(tail_frames > 0, "{kind:?}: terminal history is drained");

    // Pitch is still measured over the complete emitted stream, including
    // the latency fill and its matching terminal drain.
    assert!(
        out_frames >= expected_timeline,
        "{kind:?}: terminal drain cannot shorten the exact timeline"
    );

    // Pitch preserved: dominant bin still at F0 (the load-bearing check -
    // a resampler-in-disguise would shift it).
    let mono: Vec<f32> = out.iter().step_by(channels).copied().collect();
    assert!(
        mono.len() >= Consts::N,
        "{kind:?}: not enough output for the FFT window"
    );
    let peak = dominant_bin(&mono);
    let want = expected_bin(Consts::F0);
    assert!(
        peak.abs_diff(want) <= 3,
        "{kind:?}: pitch moved under time-stretch: peak bin {peak}, expected {want}"
    );
}

fn assert_unity_contract(kind: StretchKind) {
    let in_frames = usize::try_from(Consts::SR).unwrap() * 2;
    let input = sine(in_frames);
    let out = render(&mut keylocked(kind, 1.0), &input);
    assert_eq!(out, input, "{kind:?}: unity speed must bypass byte-exact");
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn half_speed_and_unity_contracts(#[case] backend: StretchKind) {
    assert_half_speed_contract(backend);
    assert_unity_contract(backend);
}

#[kithara::test]
fn unity_passthrough_admits_the_whole_source_chunk() {
    let mut renderer = renderer(StretchControls::new(1.0));
    let frames = renderer.render_quantum_frames.get() * 2;

    assert_eq!(
        renderer.prepare_quantum(AudioChunkInfo::default(), frames),
        Some(FrameCount::new(frames)),
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn rendered_source_frontier_excludes_backend_lookahead(#[case] backend: StretchKind) {
    const SOURCE_START: u64 = 10_000;
    const SOURCE_FRAMES: usize = 4096;

    let mut renderer = keylocked(backend, 0.5);
    let source = sine(SOURCE_FRAMES);
    let mut input = chunk(&source);
    input.meta.frame_offset = SOURCE_START;
    input.meta.timestamp = spec()
        .duration_for(SOURCE_START)
        .expect("test source timestamp fits");
    let admitted = SOURCE_START
        .checked_add(u64::try_from(SOURCE_FRAMES).expect("source frame count fits u64"))
        .expect("source frontier fits u64");
    input.meta.end_timestamp = spec()
        .duration_for(admitted)
        .expect("test source end timestamp fits");
    let source_latency = renderer
        .engine
        .as_ref()
        .expect("compiled backend is available")
        .capabilities()
        .latency()
        .source_frames();
    assert!(source_latency > 0, "backend must declare source lookahead");

    let output = render_serviced(&mut renderer, input).expect("half-speed render emits samples");
    let held =
        u64::try_from(source_latency.min(SOURCE_FRAMES)).expect("backend source latency fits u64");
    let expected_frame = admitted.saturating_sub(held);

    assert_eq!(
        renderer.rendered_source_end(),
        Some((
            expected_frame,
            NonZeroU32::new(Consts::SR).expect("test sample rate is non-zero"),
        )),
        "rendered progress excludes source still retained by the backend"
    );
    assert_ne!(
        output
            .meta
            .frame_offset
            .saturating_add(u64::from(output.meta.frames)),
        expected_frame,
        "the oracle must distinguish transformed output frames from the source frontier"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn rendered_source_frontier_reaches_end_only_on_completed_drain(#[case] backend: StretchKind) {
    const SOURCE_START: u64 = 10_000;
    const SOURCE_FRAMES: usize = WarpRenderer::MAX_SOURCE_FRAMES;

    let mut renderer = keylocked(backend, StretchControls::MIN_SPEED);
    let source = sine(SOURCE_FRAMES);
    let mut input = chunk(&source);
    input.meta.frame_offset = SOURCE_START;
    let admitted = SOURCE_START
        .checked_add(u64::try_from(SOURCE_FRAMES).expect("source frame count fits u64"))
        .expect("source frontier fits u64");
    let source_latency = renderer
        .engine
        .as_ref()
        .expect("compiled backend is available")
        .capabilities()
        .latency()
        .source_frames();
    let held =
        u64::try_from(source_latency.min(SOURCE_FRAMES)).expect("backend source latency fits u64");

    render_serviced(&mut renderer, input).expect("minimum-speed render emits samples");
    assert_eq!(
        renderer.rendered_source_end(),
        Some((admitted - held, spec().sample_rate))
    );

    let mut frontiers = Vec::new();
    while let Some(tail) = flush_serviced(&mut renderer) {
        assert!(tail.frames() > 0, "terminal chunk carries real samples");
        frontiers.push(renderer.rendered_source_end());
        assert!(frontiers.len() < 64, "terminal drain must converge");
    }

    assert!(
        frontiers.len() > 1,
        "fixture must exercise a multi-chunk terminal drain"
    );
    assert!(
        frontiers[..frontiers.len() - 1]
            .iter()
            .all(|frontier| *frontier != Some((admitted, spec().sample_rate))),
        "an incomplete tail chunk cannot publish the full source frontier"
    );
    assert_eq!(
        frontiers.last().copied().flatten(),
        Some((admitted, spec().sample_rate)),
        "the completed tail releases the held source frontier"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn output_meta_preserves_decoder_timeline(#[case] backend: StretchKind) {
    let channels = usize::from(Consts::CH);
    let mut fx = keylocked(backend, 0.5);
    let cf = 1024usize;
    let block = sine(cf);
    let mut emitted = Vec::new();
    for i in 0..40u64 {
        let mut c = chunk(&block);
        c.meta.frame_offset = i * u64::try_from(cf).unwrap();
        c.meta.timestamp = spec()
            .duration_for(c.meta.frame_offset)
            .expect("test timestamp fits");
        c.meta.end_timestamp = spec()
            .duration_for(c.meta.frame_offset + u64::try_from(cf).unwrap())
            .expect("test end timestamp fits");
        if let Some(o) = render_serviced(&mut fx, c) {
            emitted.push(o);
        }
    }
    while let Some(o) = flush_serviced(&mut fx) {
        emitted.push(o);
    }
    assert!(!emitted.is_empty(), "stretch produced no output");
    for o in &emitted {
        assert_eq!(
            o.spec(),
            AudioSpec {
                channels: Consts::CH,
                sample_rate: NonZero::new(Consts::SR).unwrap()
            },
            "spec (incl. sample rate) preserved verbatim"
        );
        assert_eq!(
            usize::try_from(o.meta.frames).unwrap(),
            o.samples.len() / channels,
            "frames recomputed to the actual output count"
        );
        assert!(o.meta.timestamp <= o.meta.end_timestamp);
    }
    assert_eq!(
        emitted.first().expect("first output").meta.timestamp,
        Duration::ZERO,
        "first presented sample begins at the source start"
    );
    assert!(
        emitted
            .windows(2)
            .all(|pair| pair[0].meta.end_timestamp == pair[1].meta.timestamp),
        "presented source intervals stay contiguous across backend lookahead and tail"
    );
    assert_eq!(
        emitted.last().expect("last output").meta.end_timestamp,
        spec()
            .duration_for(40 * u64::try_from(cf).unwrap())
            .expect("test final timestamp fits"),
        "only the completed tail reaches the admitted source end"
    );
}

/// Key-lock off is vinyl mode: speed changes duration and pitch in the
/// stretch slot, with no resampler-rate handoff.
#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn vinyl_speed_scales_duration_and_pitch(#[case] backend: StretchKind) {
    let channels = usize::from(Consts::CH);
    let in_frames = usize::try_from(Consts::SR).unwrap() * 2;
    let out = run_vinyl(backend, 2.0, in_frames);
    let out_frames = out.len() / channels;
    assert!(
        out_frames * 10 >= in_frames * 4 && out_frames * 10 <= in_frames * 6,
        "vinyl 2x should roughly halve duration, got {out_frames} from {in_frames}"
    );
    let mono: Vec<f32> = out.iter().step_by(channels).copied().collect();
    assert!(
        mono.len() >= Consts::N,
        "not enough vinyl output for the FFT window"
    );
    let peak = dominant_bin(&mono);
    let want = expected_bin(Consts::F0 * 2.0);
    assert!(
        peak.abs_diff(want) <= 4,
        "vinyl pitch did not follow speed: peak bin {peak}, expected {want}"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn live_speed_change_updates_stretch_duration(#[case] backend: StretchKind) {
    #[cfg(feature = "perf")]
    let _guard = hotpath::HotpathGuardBuilder::new("live_speed_change").build();
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = renderer(Arc::clone(&controls));
    let block = sine(4096);
    let unity = render_serviced(&mut fx, chunk(&block)).expect("unity bypass emits");
    assert_eq!(&unity.samples[..], &block[..], "unity phase bypasses");

    controls.set_speed(0.5);
    let mut stretched: Vec<f32> = Vec::new();
    for index in 1..=24 {
        let offset = u64::try_from(index * 4096).expect("test frame offset fits");
        if let Some(c) = render_serviced(&mut fx, chunk_at(&block, offset)) {
            stretched.extend_from_slice(&c.samples);
        }
    }
    while let Some(c) = flush_serviced(&mut fx) {
        stretched.extend_from_slice(&c.samples);
    }
    assert!(
        stretched.len() > block.len() * 24,
        "half-speed key-lock should lengthen output after a live speed change"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn live_speed_change_ramps_the_first_output_block(#[case] backend: StretchKind) {
    const RESPONSE_QUANTA: usize = 2;

    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = renderer(Arc::clone(&controls));
    let frames = fx.render_quantum_frames.get();
    let channels = usize::from(Consts::CH);
    let source = sine(frames * 8);
    assert!(
        f64::from(Consts::SR) * f64::from(WarpRenderer::RATE_SMOOTH_SECONDS) < f64_of(frames),
        "configured rate time constant fits one output quantum"
    );

    fx.prepare(spec());
    let warm_frames = fx
        .prepare_quantum(chunk(&source).meta, source.len() / channels)
        .expect("warm-up quantum fits");
    let split = warm_frames.get() * channels;
    let warm = fx
        .render_quantum(chunk(&source[..split]))
        .expect("warm-up emits samples");
    assert!(
        warm.frames() > 0 && warm.frames() <= frames,
        "warm-up output is finite and quantum-bounded"
    );
    fx.prepare(spec());

    controls.set_speed(2.0);
    let source_offset = u64::try_from(warm_frames.get()).expect("test frame count fits u64");
    let mut remaining = chunk(&source[split..]);
    remaining.meta.frame_offset = source_offset;
    remaining.meta.timestamp = spec()
        .duration_for(source_offset)
        .expect("test timestamp fits");
    let next_frames = fx
        .prepare_quantum(remaining.meta, remaining.frames())
        .expect("post-change quantum fits");
    let capabilities = fx
        .engine
        .as_ref()
        .expect("compiled backend is available")
        .capabilities();
    let hard_step_frames = WarpRenderer::source_block_limit(
        0.5,
        capabilities.max_source_frames(),
        capabilities
            .max_output_frames()
            .min(fx.render_quantum_frames.get()),
    )
    .expect("hard-step span fits");
    let end = split + next_frames.get() * channels;
    let mut input = chunk(&source[split..end]);
    input.meta.frame_offset = source_offset;
    input.meta.timestamp = remaining.meta.timestamp;
    let output = fx
        .render_quantum(input)
        .expect("post-change block emits samples");
    assert!(
        output.frames() > 0 && output.frames() <= frames,
        "ramped output is finite and quantum-bounded"
    );
    fx.prepare(spec());

    assert!(
        next_frames.get() > warm_frames.get() && next_frames.get() < hard_step_frames,
        "{backend:?}: first post-change source span must ramp between {} and {hard_step_frames}, got {} frames",
        warm_frames.get(),
        next_frames.get()
    );

    let settled_offset = source_offset
        .checked_add(u64::try_from(next_frames.get()).expect("test frame count fits u64"))
        .expect("test source offset fits u64");
    let mut remaining = chunk(&source[end..]);
    remaining.meta.frame_offset = settled_offset;
    remaining.meta.timestamp = spec()
        .duration_for(settled_offset)
        .expect("test timestamp fits");
    let settled_frames = fx
        .prepare_quantum(remaining.meta, remaining.frames())
        .expect("settled quantum fits");
    let settled_end = end + settled_frames.get() * channels;
    let mut input = chunk(&source[end..settled_end]);
    input.meta.frame_offset = settled_offset;
    input.meta.timestamp = remaining.meta.timestamp;
    let output = fx
        .render_quantum(input)
        .expect("settled block emits samples");
    assert!(
        output.frames() > 0 && output.frames() <= frames,
        "settled output is finite and quantum-bounded"
    );
    assert!(
        settled_frames.get() >= next_frames.get(),
        "{backend:?}: source spans must approach the faster target monotonically"
    );
    assert!(
        settled_frames.get().abs_diff(hard_step_frames) <= 1
            && fx.applied_speed.has_settled_at(2.0),
        "{backend:?}: rate must settle within {RESPONSE_QUANTA} quanta; \
         settled span={}, target span={hard_step_frames}",
        settled_frames.get()
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn live_speed_ramp_is_independent_of_source_partitioning(#[case] backend: StretchKind) {
    const BLOCK_FRAMES: usize = 4096;

    let direct_controls = StretchControls::new(0.5);
    direct_controls.set_keylock(true);
    direct_controls.set_backend(backend);
    let mut direct = renderer(Arc::clone(&direct_controls));

    let quantum_controls = StretchControls::new(0.5);
    quantum_controls.set_keylock(true);
    quantum_controls.set_backend(backend);
    let mut quantum = renderer(Arc::clone(&quantum_controls));

    let source = sine(BLOCK_FRAMES * 2);
    let channels = usize::from(Consts::CH);
    let split = BLOCK_FRAMES * channels;
    let direct_warm = render_serviced(&mut direct, chunk(&source[..split]))
        .expect("direct warm-up emits samples");
    let quantum_warm = render_quantum_frames(&mut quantum, &source[..split], 0);
    assert_eq!(direct_warm.frames(), quantum_warm, "warm-up is exact");

    direct_controls.set_speed(2.0);
    quantum_controls.set_speed(2.0);
    let mut direct_input = chunk(&source[split..]);
    direct_input.meta.frame_offset = u64::try_from(BLOCK_FRAMES).expect("fixture fits u64");
    direct_input.meta.timestamp = spec()
        .duration_for(direct_input.meta.frame_offset)
        .expect("test timestamp fits");
    let direct_frames = render_serviced(&mut direct, direct_input)
        .expect("direct post-change render emits samples")
        .frames();
    let quantum_frames = render_quantum_frames(
        &mut quantum,
        &source[split..],
        u64::try_from(BLOCK_FRAMES).expect("fixture fits u64"),
    );

    assert_eq!(
        direct_frames, quantum_frames,
        "{backend:?}: one source span and the scheduler partition must apply the same rate ramp"
    );
}

/// Flipping key-lock mid-stream switches from vinyl pitch shift to
/// pitch-preserving stretch - no reload.
#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn live_keylock_toggle_switches_pitch_mode(#[case] backend: StretchKind) {
    let controls = StretchControls::new(0.5);
    controls.set_keylock(false);
    controls.set_backend(backend);
    let mut fx = renderer(Arc::clone(&controls));
    let block = sine(4096);

    let mut vinyl_out: Vec<f32> = Vec::new();
    for index in 0..24 {
        let offset = u64::try_from(index * 4096).expect("test frame offset fits");
        if let Some(c) = render_serviced(&mut fx, chunk_at(&block, offset)) {
            vinyl_out.extend_from_slice(&c.samples);
        }
    }
    let vinyl_mono: Vec<f32> = vinyl_out
        .iter()
        .step_by(usize::from(Consts::CH))
        .copied()
        .collect();
    assert!(
        dominant_bin(&vinyl_mono).abs_diff(expected_bin(Consts::F0 * 0.5)) <= 4,
        "off: vinyl pitch follows speed"
    );

    controls.set_keylock(true);
    let mut stretched: Vec<f32> = Vec::new();
    for index in 24..48 {
        let offset = u64::try_from(index * 4096).expect("test frame offset fits");
        if let Some(c) = render_serviced(&mut fx, chunk_at(&block, offset)) {
            stretched.extend_from_slice(&c.samples);
        }
    }
    while let Some(c) = flush_serviced(&mut fx) {
        stretched.extend_from_slice(&c.samples);
    }
    let mono: Vec<f32> = stretched
        .iter()
        .step_by(usize::from(Consts::CH))
        .copied()
        .collect();
    assert!(
        mono.len() >= Consts::N,
        "on: not enough output for the FFT window"
    );
    assert!(
        dominant_bin(&mono).abs_diff(expected_bin(Consts::F0)) <= 3,
        "on: pitch preserved after live toggle"
    );
}
