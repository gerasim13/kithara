#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, FrameCount, SegmentId, SourceSpan};
use kithara_test_utils::kithara;

use super::super::{PcmConsumer, PlayerResource, ReadOutcome};
use crate::{
    mock::pcm_fixture::{PcmFixture, chunk},
    test_pools::{pools, sample_buffer},
    worker::PcmPacket,
};

#[kithara::test(native, tokio)]
#[case(44_100, 8_820)]
#[case(48_000, 9_600)]
#[case(96_000, 19_200)]
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn scratch_holds_200ms_of_frames(#[case] sample_rate: u32, #[case] expected: usize) {
    hang_tick!();
    let mut fixture = PcmFixture::new(4, false).await;
    hang_reset!();
    let spec = AudioSpec::new(2, NonZeroU32::new(sample_rate).expect("fixture rate"));
    let duration = Duration::from_millis(200);
    let frames = spec.frames_for(duration).expect("200 ms input geometry");
    let samples = [0.25, -0.5].repeat(frames.get());
    let packet = AudioChunk::new(
        AudioChunkInfo {
            spec,
            frames: u32::try_from(frames.get()).expect("fixture frame count"),
            segment: SegmentId::FIRST,
            end_timestamp: duration,
            source_span: SourceSpan::new(
                0,
                frames.get() as u64,
                spec.sample_rate,
                frames.get() as u64,
            ),
            ..AudioChunkInfo::default()
        },
        sample_buffer(&pools(), &samples),
    );
    fixture
        .push(PcmPacket::Chunk(Box::new(packet)))
        .expect("200 ms packet fits the ring");
    let receiver = fixture.receiver.take().expect("packet receiver");
    let mut resource =
        PlayerResource::new(PcmConsumer::new(receiver), Arc::from("200ms"), &pools())
            .expect("packet-backed resource");
    let mut left = vec![0.0; expected];
    let mut right = vec![0.0; expected];
    let split = expected / 2;
    let mut budget = 4;
    let ReadOutcome::Full { frames: first } =
        resource.read(&mut [&mut left, &mut right], 0..split, &mut budget)
    else {
        panic!("the first read must consume the packet prefix")
    };
    assert_eq!(first, split);
    let ReadOutcome::Full { frames: second } =
        resource.read(&mut [&mut left, &mut right], split..expected, &mut budget)
    else {
        panic!("the second read must consume the held packet remainder")
    };
    assert_eq!(FrameCount::new(first + second), FrameCount::new(expected));
    assert_eq!(resource.lane.frame, expected as u64);
    assert_eq!(left, vec![0.25; expected]);
    assert_eq!(right, vec![-0.5; expected]);
}

#[kithara::test(native, tokio)]
async fn epoch_validator_keeps_matching_chunks() {
    let mut fixture = PcmFixture::new(4, false).await;
    let receiver = fixture.receiver.take().expect("packet receiver");
    let mut resource = PlayerResource::new(PcmConsumer::new(receiver), Arc::from("test"), &pools())
        .expect("player resource");
    let segment = SegmentId::FIRST.next();
    resource.select_segment(segment);
    fixture
        .push(PcmPacket::Chunk(Box::new(chunk(segment, &[1.0, 2.0, 3.0]))))
        .expect("matching packet");
    let mut left = [0.0; 3];
    let mut right = [0.0; 3];
    let mut budget = 4;
    let result = resource.read(&mut [&mut left, &mut right], 0..3, &mut budget);
    assert!(matches!(result, ReadOutcome::Full { frames: 3 }));
    assert_eq!(left, [1.0, 2.0, 3.0]);
    assert_eq!(right, left);
}

#[kithara::test(native, tokio)]
async fn epoch_validator_rejects_stale_chunks_after_seek() {
    let mut fixture = PcmFixture::new(4, false).await;
    let receiver = fixture.receiver.take().expect("packet receiver");
    let mut resource = PlayerResource::new(PcmConsumer::new(receiver), Arc::from("test"), &pools())
        .expect("player resource");
    let old = SegmentId::FIRST;
    let current = old.next();
    fixture
        .push(PcmPacket::Chunk(Box::new(chunk(old, &[3.0]))))
        .expect("stale packet");
    fixture
        .push(PcmPacket::Chunk(Box::new(chunk(old, &[1.0]))))
        .expect("first packet");
    fixture
        .push(PcmPacket::Chunk(Box::new(chunk(current, &[2.0]))))
        .expect("current packet");
    resource.select_segment(current);
    let mut left = [-1.0; 1];
    let mut right = [-1.0; 1];
    let mut budget = 4;
    resource.read(&mut [&mut left, &mut right], 0..1, &mut budget);
    assert!(!left.contains(&1.0));
    assert!(!left.contains(&3.0));
    assert!(left.contains(&2.0));
    assert_eq!(right, left);
    assert!(fixture.returned().is_some());
    assert!(fixture.returned().is_some());
    assert!(fixture.returned().is_none());
}
