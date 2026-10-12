use std::num::NonZeroUsize;

use kithara_platform::sync::Arc;
use kithara_signal::{AudioChunkInfo, FrameCount};
use kithara_test_utils::kithara;

use super::*;
use crate::{GridSegment, RegionPlan, SpeedCurve, test_pools::TestPools};

fn renderer() -> (WarpRenderer<TestPools>, AudioChunkInfo) {
    let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("sample rate"));
    let config = WarpConfig::builder()
        .render_quantum_frames(NonZeroUsize::new(128).expect("quantum"))
        .build();
    (
        crate::Warp::new((), &config).renderer(spec, crate::test_pools::pools()),
        AudioChunkInfo {
            spec,
            ..Default::default()
        },
    )
}

#[kithara::test]
fn a_renderer_without_an_entered_plan_names_no_entry_source() {
    let (mut renderer, meta) = renderer();
    renderer.prepare(meta.spec);
    assert!(renderer.plan.is_none());
    assert_eq!(
        renderer
            .prepare_quantum(meta, 16, 16)
            .expect("an unlinked renderer admits its ordinary source"),
        FrameCount::new(16)
    );
    let span = renderer
        .prepared_quantum
        .expect("ordinary quantum")
        .source_span
        .expect("ordinary source mapping");
    assert_eq!(span.start(), 0);
    assert_eq!(span.end(), 16);
}

#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
#[kithara::test]
fn a_short_decoder_suffix_requests_the_next_mapped_source_frame() {
    let (mut renderer, meta) = renderer();
    renderer
        .set_speed(SpeedCurve::Constant(2.0), 1)
        .expect("mapped speed");
    renderer.prepare(meta.spec);
    let requested = renderer
        .prepare_quantum(meta, 1, 1)
        .expect("one mapped output frame spans decoder chunks");
    assert_eq!(requested.get(), 2);
    assert_eq!(
        renderer
            .prepared_quantum
            .expect("prepared request")
            .source_span
            .expect("projected request")
            .output_frames(),
        1
    );
}

#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
#[kithara::test]
fn finite_projection_clips_a_quantum_before_resolving_past_the_recording() {
    let (mut renderer, mut meta) = renderer();
    renderer.plan = Some(Arc::new(
        RegionPlan::new(vec![GridSegment::new(0, 8_820_000, 1.0)])
            .expect("the finite recording is one region"),
    ));
    renderer.prepare(meta.spec);
    let start = 8_819_968;
    meta.frame_offset = start;
    let requested = renderer
        .prepare_quantum(meta, 32, usize::MAX)
        .expect("a covered final quantum survives an out-of-domain search midpoint");
    assert_eq!(requested.get(), 32);
    let prepared = renderer.prepared_quantum.expect("prepared quantum");
    let projection = prepared.source_span.expect("projected quantum");
    assert_eq!(projection.output_frames(), 32);
    assert_eq!(projection.end(), 8_820_000);
    assert_eq!(prepared.landing_frames, Some(32));
    assert_eq!(prepared.source_start, start);
    assert_eq!(
        projection
            .source_ratio_at(32)
            .map(|(numerator, denominator)| (numerator, denominator.get())),
        Some((8_820_000, 1))
    );
}

#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
#[kithara::test]
#[kithara::hang_watchdog]
fn terminal_projection_recomputes_both_endpoints_and_keeps_the_selected_rate() {
    hang_tick!();
    let (mut renderer, meta) = renderer();
    hang_reset!();
    renderer
        .set_speed(SpeedCurve::Constant(1.5), 1)
        .expect("mapped speed");
    renderer.prepare(meta.spec);
    hang_tick!();
    let requested = renderer
        .prepare_quantum(meta, 4096, usize::MAX)
        .expect("initial quantum resolves");
    hang_reset!();
    let prepared = renderer.prepared_quantum.expect("prepared quantum");
    let rate = prepared.rate;
    assert_eq!(requested.get(), 192);
    assert_eq!(
        renderer.prepare_terminal_quantum(96),
        Some(FrameCount::new(96))
    );
    let terminal = renderer
        .prepared_quantum
        .expect("terminal quantum is retained");
    assert_eq!(terminal.rate.revision(), rate.revision());
    assert_eq!(terminal.rate.speed(), rate.speed());
    assert_eq!(terminal.speed, 1.5);
    assert_eq!(terminal.active_frames, 96);
    assert_eq!(terminal.source_start, 0);
    let projection = terminal
        .source_span
        .expect("terminal projection is retained");
    assert_eq!(projection.start(), 0);
    assert_eq!(projection.output_frames(), 64);
    assert_eq!(projection.end(), 96);
    assert_eq!(terminal.landing_frames, Some(64));
    assert_eq!(
        projection
            .source_ratio_at(64)
            .map(|(numerator, denominator)| (numerator, denominator.get())),
        Some((96, 1))
    );
}
