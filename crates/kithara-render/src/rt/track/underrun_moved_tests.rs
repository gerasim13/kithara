#![cfg(not(target_arch = "wasm32"))]
use super::{PcmConsumer, PlayerResource, PlayerTrack, RtSink, TrackReadOutcome};
use crate::{
    bridge::{DeckEvent, Fade, RtMetrics, Slot},
    test_pools::pools,
    worker::{
        PcmPacket,
        reader_moved_tests::{PcmFixture, chunk},
    },
};
use kithara_platform::sync::Arc;
use kithara_signal::{OutputContext, SegmentId, SessionEpoch, SessionFrame};
use kithara_test_utils::kithara;
use kithara_warp::RenderContext;
use ringbuf::{
    HeapRb,
    traits::{Consumer, Split},
};
use std::num::NonZeroU32;
#[kithara::test(native, tokio)]
async fn underrun_edges_emit_once_per_starvation_window() {
    let mut fixture = PcmFixture::new(8, false).await;
    let resource = PlayerResource::new(
        PcmConsumer::new(fixture.receiver.take().expect("receiver")),
        Arc::from("underrun"),
        &pools(),
    )
    .expect("resource");
    let rate = NonZeroU32::new(48_000).expect("rate");
    let mut track = PlayerTrack::builder()
        .sample_rate(rate)
        .build(Box::new(resource));
    track.start(Fade::Declick);
    let start = SessionFrame::new(15_408);
    let output = OutputContext::new(
        start..SessionFrame::new(15_410),
        rate,
        SessionEpoch::new(1),
        None,
    )
    .expect("output range");
    let context = RenderContext::new(output, None).expect("render context");
    let (mut events, mut receiver) = HeapRb::<DeckEvent>::new(8).split();
    let metrics = RtMetrics::default();
    let mut left = [0.0; 2];
    let mut right = [0.0; 2];
    let mut bus_left = [0.0; 2];
    let mut bus_right = [0.0; 2];
    for _ in 0..2 {
        let mut sink = RtSink::new(&mut events, &metrics, Slot::new(0), start);
        assert!(matches!(
            track.render(
                Some(&context),
                &mut [&mut left, &mut right],
                &mut [&mut bus_left, &mut bus_right],
                0..2,
                &mut 16,
                &mut sink
            ),
            TrackReadOutcome::Full { frames: 0, .. }
        ));
    }
    assert_eq!(metrics.snapshot().underruns(), 1);
    assert!(receiver.try_pop().is_none());
    fixture
        .push(PcmPacket::Chunk(chunk(SegmentId::FIRST, &[0.1, 0.2])))
        .expect("recovery PCM");
    let mut sink = RtSink::new(&mut events, &metrics, Slot::new(0), start);
    assert!(matches!(
        track.render(
            Some(&context),
            &mut [&mut left, &mut right],
            &mut [&mut bus_left, &mut bus_right],
            0..2,
            &mut 16,
            &mut sink
        ),
        TrackReadOutcome::Full { frames: 2, .. }
    ));
    assert!(
        matches!(receiver.try_pop(), Some(DeckEvent::Underrun { slot, at, frames: 4 }) if slot == Slot::new(0) && at == start)
    );
    assert!(receiver.try_pop().is_none());
    assert_eq!(metrics.snapshot().underruns(), 1);
}
