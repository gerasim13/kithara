use kithara_decode::duration_for_frames;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

use super::{
    Consts, StretchControls, WarpRenderer, chunk, f64_of, flush_serviced, render_serviced,
    renderer, sine, spec,
};
use crate::{GridSegment, RegionPlan};

#[kithara::test]
fn exact_output_frames_do_not_drift_across_partitions() {
    let stretch = 1.0 / 1.3;
    let partitions = [127, 509, 2048, 17, 4096];
    let mut remainder = 0.0;
    let mut actual = 0;
    for frames in partitions {
        let (output, next_remainder) = WarpRenderer::output_frames(frames, stretch, remainder)
            .expect("invariant: finite positive stretch");
        actual += output;
        remainder = next_remainder;
    }
    let source_frames = partitions.into_iter().sum::<usize>();
    let expected = (f64_of(source_frames) * stretch)
        .round()
        .to_usize()
        .expect("invariant: fixture output span fits usize");

    assert_eq!(actual, expected);
    assert_eq!(WarpRenderer::balanced_source_block(8193, 8192), 4097);

    let mut remainder = 0.0;
    let actual = [1, 1, 4096]
        .into_iter()
        .map(|frames| {
            let (output, next_remainder) = WarpRenderer::output_frames(frames, 0.5, remainder)
                .expect("singleton spans retain their quantization debt");
            remainder = next_remainder;
            output
        })
        .sum::<usize>();
    assert_eq!(actual, 2049);

    let mut remainder = 0.0;
    let outputs = [1, 1, 1, 1].map(|frames| {
        let (output, next_remainder) = WarpRenderer::output_frames(frames, 0.25, remainder)
            .expect("four sub-frame spans form one exact output frame");
        remainder = next_remainder;
        output
    });
    assert_eq!(outputs, [0, 0, 0, 1]);
    assert_eq!(remainder, 0.0);
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn one_frame_regions_accumulate_into_one_portable_request(#[case] backend: StretchKind) {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    controls.set_region_plan(Some(Arc::new(
        RegionPlan::new(vec![
            GridSegment::new(0, 1, 0.125),
            GridSegment::new(1, 2, 0.25),
            GridSegment::new(2, 3, 0.125),
            GridSegment::new(3, 4, 0.5),
        ])
        .expect("one-frame regions are ordered and non-empty"),
    )));
    let mut fx = renderer(controls);
    let source = sine(4);

    for frame in 0..3_u64 {
        let start = usize::try_from(frame).unwrap_or_default() * usize::from(Consts::CH);
        let mut input = chunk(&source[start..start + usize::from(Consts::CH)]);
        input.meta.frame_offset = frame;
        assert!(render_serviced(&mut fx, input).is_none());
    }

    let mut input = chunk(&source[3 * usize::from(Consts::CH)..]);
    input.meta.frame_offset = 3;
    let output = render_serviced(&mut fx, input)
        .expect("the fourth source frame completes one output frame");
    assert_eq!(output.frames(), 1);
    assert_eq!(output.meta.frame_offset, 0);
    let mut tail_chunks = 0;
    while let Some(tail) = flush_serviced(&mut fx) {
        assert!(tail.frames() > 0, "a flush chunk contains real frames");
        assert_eq!(tail.spec(), spec());
        tail_chunks += 1;
        assert!(tail_chunks < 32, "terminal drain must converge");
    }
    assert!(
        tail_chunks > 0,
        "an active engine exposes its terminal tail"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn pending_span_uses_earliest_start_and_latest_frontier(#[case] backend: StretchKind) {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    controls.set_region_plan(Some(Arc::new(
        RegionPlan::new(vec![
            GridSegment::new(0, 1, 1.0),
            GridSegment::new(1, 2, 0.75),
            GridSegment::new(2, 3, 0.25),
        ])
        .expect("fixture regions are contiguous"),
    )));
    let mut fx = renderer(controls);
    let source = sine(3);
    let mut first = chunk(&source[..2 * usize::from(Consts::CH)]);
    first.meta.end_timestamp = Duration::from_millis(20);
    first.meta.segment_index = Some(1);
    first.meta.variant_index = Some(1);
    first.meta.epoch = 1;
    first.meta.source_byte_offset = Some(10);
    first.meta.source_bytes = 20;
    let first_output = render_serviced(&mut fx, first).expect("first frame renders");

    let mut second = chunk(&source[2 * usize::from(Consts::CH)..]);
    second.meta.frame_offset = 2;
    second.meta.timestamp = Duration::from_millis(20);
    second.meta.end_timestamp = Duration::from_millis(30);
    second.meta.segment_index = Some(2);
    second.meta.variant_index = Some(2);
    second.meta.epoch = 2;
    second.meta.source_byte_offset = Some(30);
    second.meta.source_bytes = 10;
    let second_output =
        render_serviced(&mut fx, second).expect("pending span completes on the next chunk");

    assert!(first_output.meta.end_timestamp < second_output.meta.end_timestamp);
    assert_eq!(second_output.meta.frame_offset, 1);
    assert_eq!(
        second_output.meta.timestamp,
        duration_for_frames(Consts::SR, 1)
    );
    assert_eq!(second_output.meta.end_timestamp, Duration::from_millis(30));
    assert_eq!(second_output.meta.segment_index, Some(2));
    assert_eq!(second_output.meta.variant_index, Some(2));
    assert_eq!(second_output.meta.epoch, 2);
    assert_eq!(second_output.meta.source_byte_offset, None);
    assert_eq!(second_output.meta.source_bytes, 0);
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn pending_span_is_committed_before_live_unity_passthrough(#[case] backend: StretchKind) {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    controls.set_region_plan(Some(Arc::new(
        RegionPlan::new(vec![GridSegment::new(0, 1, 0.75)]).expect("fixture region is valid"),
    )));
    let mut fx = renderer(Arc::clone(&controls));
    let source = sine(3);
    let mut pending = chunk(&source[..usize::from(Consts::CH)]);
    pending.meta.end_timestamp = Duration::from_millis(10);
    assert!(render_serviced(&mut fx, pending).is_none());

    controls.set_region_plan(None);
    let mut unity = chunk(&source[usize::from(Consts::CH)..2 * usize::from(Consts::CH)]);
    unity.meta.frame_offset = 1;
    unity.meta.timestamp = Duration::from_millis(10);
    unity.meta.end_timestamp = Duration::from_millis(20);
    let transition =
        render_serviced(&mut fx, unity).expect("rounded pending frame precedes the unity frame");
    assert_eq!(transition.frames(), 2);
    assert_eq!(transition.meta.frame_offset, 0);
    assert_eq!(transition.meta.end_timestamp, Duration::from_millis(20));

    let mut next = chunk(&source[2 * usize::from(Consts::CH)..]);
    next.meta.frame_offset = 2;
    let next_samples = next.samples.to_vec();
    let passthrough = render_serviced(&mut fx, next).expect("unity remains zero-copy");
    assert_eq!(&passthrough.samples[..], &next_samples);
    assert!(flush_serviced(&mut fx).is_none());
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn negative_rounding_debt_adds_no_frame_at_unity_transition(#[case] backend: StretchKind) {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    controls.set_region_plan(Some(Arc::new(
        RegionPlan::new(vec![
            GridSegment::new(0, 1, 1.6),
            GridSegment::new(1, 2, 0.25),
        ])
        .expect("fixture regions are contiguous"),
    )));
    let mut fx = renderer(Arc::clone(&controls));
    let source = sine(3);
    let first = render_serviced(&mut fx, chunk(&source[..usize::from(Consts::CH)]))
        .expect("the first span rounds to two frames");
    assert_eq!(first.frames(), 2);

    let mut debt = chunk(&source[usize::from(Consts::CH)..2 * usize::from(Consts::CH)]);
    debt.meta.frame_offset = 1;
    assert!(render_serviced(&mut fx, debt).is_none());

    controls.set_region_plan(None);
    let mut unity = chunk(&source[2 * usize::from(Consts::CH)..]);
    unity.meta.frame_offset = 2;
    let expected = unity.samples.to_vec();
    let output = render_serviced(&mut fx, unity).expect("unity chunk passes through");
    assert_eq!(output.frames(), 1);
    assert_eq!(&output.samples[..], &expected);
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn reset_discards_pending_span_before_new_timeline(#[case] backend: StretchKind) {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(true);
    controls.set_backend(backend);
    controls.set_region_plan(Some(Arc::new(
        RegionPlan::new(vec![GridSegment::new(0, 1, 0.75)]).expect("fixture region is valid"),
    )));
    let mut fx = renderer(Arc::clone(&controls));
    let source = sine(2);
    assert!(render_serviced(&mut fx, chunk(&source[..usize::from(Consts::CH)])).is_none());

    fx.reset();
    controls.set_region_plan(None);
    fx.prepare(spec());
    let mut landed = chunk(&source[usize::from(Consts::CH)..]);
    landed.meta.frame_offset = 100;
    landed.meta.timestamp = Duration::from_secs(1);
    landed.meta.end_timestamp = Duration::from_millis(1_010);
    let expected = landed.samples.to_vec();
    let output = render_serviced(&mut fx, landed).expect("post-seek unity passes through");
    assert_eq!(output.meta.frame_offset, 100);
    assert_eq!(output.meta.timestamp, Duration::from_secs(1));
    assert_eq!(&output.samples[..], &expected);
}
