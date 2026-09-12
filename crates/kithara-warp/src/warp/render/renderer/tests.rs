use std::{num::NonZero, ops::Range};

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use kithara_test_fixtures::unit_fixtures::warp_pair;
use kithara_test_utils::kithara;
use realfft::RealFftPlanner;

use super::{StretchControls, WarpRenderer as GenericWarpRenderer};
use crate::{
    GridSegment, PresentationFrontier, RateTarget, RegionPlan, RegionPlanSlot, RenderContext,
    RenderPublisher, SessionBeat, SessionEpoch, SessionFrame, SyncMode, TransportRevision, Warp,
    WarpConfig, WarpMap, WarpMapRevision,
    test_pools::{Pools, TestPools, pools, sample_buffer},
};

type WarpRenderer = GenericWarpRenderer<TestPools>;

mod playback;
mod target;
mod timeline;

struct Consts;

impl Consts {
    const CH: u16 = 2;
    const F0: f64 = 440.0;
    /// FFT length for the pitch (dominant-frequency) check.
    const N: usize = 1 << 14;
    const SR: u32 = 44_100;
    const HOST_BPM: f64 = 124.0;
    const HOST_SR: f64 = 48_000.0;
}

fn host_beats(output: Range<SessionFrame>) -> Range<SessionBeat> {
    let beat = |frame| {
        let frame: f64 = num_traits::cast(i64::from(frame)).unwrap_or_default();
        SessionBeat::new(frame * Consts::HOST_BPM / (Consts::HOST_SR * 60.0))
            .expect("fixture host beat")
    };
    beat(output.start)..beat(output.end)
}

fn f64_of(x: usize) -> f64 {
    num_traits::cast(x).unwrap_or_default()
}

fn chunk(pools: &Pools, samples: &[f32]) -> AudioChunk {
    let frames = samples.len() / usize::from(Consts::CH);
    AudioChunk::new(
        AudioChunkInfo {
            spec: AudioSpec {
                channels: Consts::CH,
                sample_rate: NonZero::new(Consts::SR).unwrap(),
            },
            frames: u32::try_from(frames).unwrap_or(0),
            timestamp: Duration::ZERO,
            ..Default::default()
        },
        sample_buffer(pools, samples),
    )
}

/// Index of the strongest spectral bin (skipping DC) of a mono window
/// taken from the middle of `mono`.
fn dominant_bin(mono: &[f32]) -> usize {
    let start = (mono.len().saturating_sub(Consts::N)) / 2;
    let seg = &mono[start..start + Consts::N];
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(Consts::N);
    let mut input = fft.make_input_vec();
    input.copy_from_slice(seg);
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut input, &mut spectrum).unwrap();
    spectrum
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
        .map_or(0, |(i, _)| i)
}

fn expected_bin(freq: f64) -> usize {
    num_traits::cast((freq * f64_of(Consts::N) / f64::from(Consts::SR)).round()).unwrap_or(0)
}

fn spec() -> AudioSpec {
    AudioSpec {
        channels: Consts::CH,
        sample_rate: NonZero::new(Consts::SR).unwrap(),
    }
}

fn renderer(controls: Arc<StretchControls>) -> WarpRenderer {
    let config = WarpConfig::builder().stretch(controls).build();
    Warp::new((), &config).renderer(spec(), pools())
}

fn planned_renderer(controls: Arc<StretchControls>) -> (WarpRenderer, Arc<RegionPlanSlot>) {
    let config = WarpConfig::builder().stretch(Arc::clone(&controls)).build();
    let mut warp = Warp::new((), &config);
    let publisher = warp.take_publisher().expect("fixture owns publisher");
    let context = RenderContext::new(
        SessionFrame::new(0)..SessionFrame::new(i64::from(Consts::SR)),
        spec().sample_rate,
        Some(SessionBeat::default()..SessionBeat::new(1.0).expect("beat")),
        SessionEpoch::new(0),
        Some(TransportRevision::first()),
    )
    .expect("fixture context")
    .with_rate(SyncMode::HostSync, controls.rate_target());
    publisher.publish(
        &context,
        PresentationFrontier::builder()
            .source(0)
            .output(SessionFrame::new(0))
            .build(),
    );
    (
        warp.renderer(spec(), pools()),
        Arc::clone(warp.region_plan()),
    )
}

fn render_serviced(fx: &mut WarpRenderer, input: AudioChunk) -> Option<AudioChunk> {
    fx.prepare(spec());
    let output = fx.render(input);
    fx.prepare(spec());
    output
}

fn flush_serviced(fx: &mut WarpRenderer) -> Option<AudioChunk> {
    fx.prepare(spec());
    let output = fx.flush();
    fx.prepare(spec());
    output
}

#[kithara::test]
fn render_commits_the_context_captured_for_the_operation(warp_pair: Vec<f32>) {
    let pools = pools();
    let controls = StretchControls::new(1.0);
    let config = WarpConfig::builder().stretch(Arc::clone(&controls)).build();
    let mut warp = Warp::new((), &config);
    let publisher = warp.take_publisher().expect("test Warp owns its publisher");
    let mut renderer = warp.renderer(spec(), pools.clone());
    let output = SessionFrame::new(1_000)..SessionFrame::new(1_001);
    let context = RenderContext::new(
        output.clone(),
        spec().sample_rate,
        Some(host_beats(output)),
        SessionEpoch::new(1),
        Some(TransportRevision::first()),
    )
    .expect("fixture context is valid");
    publisher.publish(
        &context,
        PresentationFrontier::builder()
            .source(41)
            .output(SessionFrame::new(1_000))
            .build(),
    );
    let mut input = chunk(&pools, &warp_pair);
    input.meta.frame_offset = 41;

    renderer.prepare(spec());
    renderer
        .prepare_quantum(input.meta, input.frames())
        .expect("quantum is prepared");
    controls.set_speed(2.0);
    publisher.publish(
        &context
            .clone()
            .with_rate(SyncMode::Off, controls.rate_target()),
        PresentationFrontier::builder()
            .source(41)
            .output(SessionFrame::new(1_000))
            .build(),
    );
    let output = renderer
        .render_quantum(input)
        .expect("prepared unity render succeeds");
    assert_eq!(output.meta.render_revision, 0);
    assert_eq!(&output.samples[..], &[0.25, -0.5]);
    let snapshot = renderer
        .committed
        .as_ref()
        .expect("successful render commits a snapshot");

    assert_eq!(output.frames(), 1);
    assert_eq!(snapshot.context(), &context);
    assert_eq!(snapshot.frontier().source(), 42);
    assert_eq!(snapshot.frontier().output(), SessionFrame::new(1_001));
}

#[kithara::test]
fn an_exact_source_output_anchor_marks_the_rendered_pcm() {
    let controls = StretchControls::new(1.0);
    let (mut renderer, slot) = planned_renderer(controls);
    let revision = WarpMapRevision::first();
    let plan = RegionPlan::new(vec![GridSegment::new(0, u64::MAX, 1.0)])
        .expect("fixture plan")
        .with_activation(WarpMap::identity(revision).reanchor(
            0,
            SessionFrame::new(0),
            SessionBeat::default(),
        ));
    slot.install(Some(Arc::new(plan)));
    let pools = renderer.pools.clone();
    let input = chunk(&pools, &[0.0; 256]);

    let frames = renderer
        .prepare_quantum(input.meta, input.frames())
        .expect("exact anchor is plannable");
    assert_eq!(frames.get(), input.frames());
    let output = renderer
        .render_quantum(input)
        .expect("exact anchor renders PCM");

    assert_eq!(
        kithara_signal::render_warp_map_revision(output.meta.render_revision),
        u64::from(revision)
    );
}

#[kithara::test]
fn post_seek_pcm_prepares_at_the_future_activation_without_advancing_presentation() {
    let controls = StretchControls::new(1.0);
    controls.set_speed(31.0 / 24.0);
    let spec = AudioSpec {
        channels: Consts::CH,
        sample_rate: NonZero::new(48_000).expect("fixture sample rate is non-zero"),
    };
    let config = WarpConfig::builder().stretch(Arc::clone(&controls)).build();
    let mut warp = Warp::new((), &config);
    let publisher = warp.take_publisher().expect("fixture owns publisher");
    let reader = publisher.reader();
    let mut renderer = warp.renderer(spec, pools());
    let output = SessionFrame::new(1_000)..SessionFrame::new(1_128);
    let context = RenderContext::new(
        output.clone(),
        spec.sample_rate,
        Some(host_beats(output)),
        SessionEpoch::new(1),
        Some(TransportRevision::first()),
    )
    .expect("fixture context")
    .with_rate(SyncMode::HostSync, controls.rate_target());
    publisher.publish_preparation(&context);
    assert!(reader.load().is_none());
    let cue = 30_000;
    let activation_output = SessionFrame::new(92_903);
    let revision =
        WarpMapRevision::from_raw(NonZero::new(2).expect("fixture revision is non-zero"));
    warp.region_plan().install(Some(Arc::new(
        RegionPlan::new(vec![GridSegment::new(0, u64::MAX, 1.6)])
            .expect("fixture plan")
            .with_activation(WarpMap::identity(revision).reanchor(
                cue,
                activation_output,
                SessionBeat::default(),
            )),
    )));
    renderer.reset();
    renderer.prepare(spec);
    let pools = renderer.pools.clone();
    let mut input = AudioChunk::new(
        AudioChunkInfo {
            spec,
            frames: 128,
            timestamp: Duration::ZERO,
            ..Default::default()
        },
        sample_buffer(&pools, &[0.0; 256]),
    );
    input.meta.frame_offset = cue;

    renderer
        .prepare_quantum(input.meta, input.frames())
        .expect("post-seek activation is plannable");
    assert!((renderer.rate.speed() - 31.0 / 24.0).abs() < f32::EPSILON);
    let output = renderer
        .render_quantum(input)
        .expect("post-seek activation renders PCM");

    assert_eq!(
        kithara_signal::render_warp_map_revision(output.meta.render_revision),
        u64::from(revision)
    );
    assert!(
        renderer.committed.is_none(),
        "preparation does not commit presentation"
    );
    assert!(
        reader.load().is_none(),
        "worker preparation must not acknowledge future PCM as presented"
    );
}

#[kithara::test]
fn servicing_a_new_plan_preserves_an_already_prepared_quantum() {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(false);
    let (mut renderer, slot) = planned_renderer(controls);
    renderer.prepare(spec());
    let pools = renderer.pools.clone();
    let samples = vec![0.25; 128 * usize::from(Consts::CH)];
    let input = chunk(&pools, &samples);

    renderer
        .prepare_quantum(input.meta, input.frames())
        .expect("current plan accepts the source quantum");
    slot.install(Some(Arc::new(
        RegionPlan::new(vec![GridSegment::new(0, u64::MAX, 1.0)]).expect("replacement plan"),
    )));
    renderer.prepare(spec());

    let output = renderer
        .render_quantum(input)
        .expect("accepted source quantum survives scheduler servicing");
    assert_eq!(&*output.samples, samples);
}

fn publish_rate(publisher: &RenderPublisher, rate: RateTarget, source: u64) {
    let frame = SessionFrame::new(i64::try_from(source).expect("fixture frame fits"));
    let context = RenderContext::new(
        frame..frame,
        spec().sample_rate,
        None,
        SessionEpoch::new(1),
        None,
    )
    .expect("fixture context is valid")
    .with_rate(SyncMode::Off, rate);
    publisher.publish(
        &context,
        PresentationFrontier::builder()
            .source(source)
            .output(frame)
            .build(),
    );
}
