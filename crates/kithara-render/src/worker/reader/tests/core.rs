use std::num::NonZeroU32;

use kithara_platform::sync::Arc;
use kithara_signal::SegmentId;
use kithara_stream::{PlayheadState, PlayheadWrite};
use kithara_test_utils::kithara;

pub(in crate::worker) use super::super::mock::packet_fixture;
use super::*;

#[kithara::test]
fn cache_coverage_updates_while_the_pcm_ring_is_full_and_paused() {
    let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("sample rate"));
    let mut ring = PacketRing::new(spec, Duration::from_secs(187), 1);
    ring.push(PcmPacket::Chunk(Box::new(chunk(
        spec,
        SegmentId::FIRST,
        0,
        0,
        &[0.5; 8],
    ))));
    assert!(!ring.playing());
    let receiver = ring.receiver.as_mut().expect("receiver");
    let playhead = Arc::new(PlayheadState::new());
    receiver.playhead = playhead.clone();
    receiver.set_position(Duration::from_secs(10));
    assert!(receiver.forward.is_full());
    assert_eq!(receiver.cached_span(), Duration::ZERO);

    playhead.set_cached(1, 4);
    assert_eq!(receiver.cached_span(), Duration::ZERO);

    playhead.set_duration(Some(Duration::from_secs(187)));
    assert_eq!(receiver.cached_span(), Duration::from_millis(46_750));

    playhead.set_cached(4, 4);
    assert_eq!(receiver.cached_span(), Duration::from_secs(187));

    playhead.set_cached(3, 0);
    assert_eq!(receiver.cached_span(), Duration::from_secs(187));
    assert_eq!(receiver.position(), Duration::from_secs(10));
    assert!(receiver.forward.is_full());
}
