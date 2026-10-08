#![cfg(not(target_arch = "wasm32"))]
use super::fixture_moved_tests::NodeFixture;
use crate::worker::{PcmPacket, reader_moved_tests::chunk};
use kithara_signal::SegmentId;
use kithara_test_utils::kithara;
use kithara_worker::{Task, TickResult};
fn pop(fixture: &mut NodeFixture) -> Option<i32> {
    fixture.receiver.pop().map(|packet| {
        let PcmPacket::Chunk(chunk) = packet else {
            panic!("PCM");
        };
        chunk.samples[0] as i32
    })
}
fn stage(fixture: &mut NodeFixture, value: i32) {
    fixture.stage(PcmPacket::Chunk(chunk(SegmentId::FIRST, &[value as f32])));
}
#[kithara::test(native, tokio)]
async fn connect_push_pop() {
    let mut fixture = NodeFixture::new(2).await;
    assert_eq!(pop(&mut fixture), None);
    stage(&mut fixture, 1);
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    stage(&mut fixture, 2);
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    stage(&mut fixture, 3);
    assert_eq!(fixture.node.admit(), TickResult::Backpressured);
    assert_eq!(fixture.node.tick(), TickResult::Backpressured);
    assert!(
        matches!(fixture.node.pending.as_ref().map(|pending| &pending.packet), Some(PcmPacket::Chunk(chunk)) if chunk.samples[0] == 3.0)
    );
    assert_eq!(pop(&mut fixture), Some(1));
    assert_eq!(pop(&mut fixture), Some(2));
    assert_eq!(pop(&mut fixture), None);
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    assert_eq!(pop(&mut fixture), Some(3));
    assert_eq!(pop(&mut fixture), None);
}
#[kithara::test(native, tokio)]
async fn try_push_drains_overflow_first() {
    let mut fixture = NodeFixture::new(1).await;
    stage(&mut fixture, 1);
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    stage(&mut fixture, 2);
    assert_eq!(fixture.node.admit(), TickResult::Backpressured);
    assert_eq!(pop(&mut fixture), Some(1));
    assert_eq!(fixture.node.tick(), TickResult::Progress);
    stage(&mut fixture, 3);
    assert_eq!(fixture.node.admit(), TickResult::Backpressured);
    assert_eq!(pop(&mut fixture), Some(2));
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    assert_eq!(pop(&mut fixture), Some(3));
}
#[kithara::test(native, tokio)]
async fn flush_returns_false_when_ring_full() {
    let mut fixture = NodeFixture::new(1).await;
    stage(&mut fixture, 1);
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    stage(&mut fixture, 2);
    assert_eq!(fixture.node.admit(), TickResult::Backpressured);
    assert_eq!(fixture.node.admit(), TickResult::Backpressured);
    assert_eq!(pop(&mut fixture), Some(1));
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    assert_eq!(pop(&mut fixture), Some(2));
    assert_eq!(pop(&mut fixture), None);
}
#[kithara::test(native, tokio)]
async fn direct_push_never_occupies_overflow() {
    let mut fixture = NodeFixture::new(1).await;
    assert!(fixture.node.pending.is_none());
    stage(&mut fixture, 1);
    assert_eq!(fixture.node.admit(), TickResult::Progress);
    assert!(fixture.node.pending.is_none());
    assert_eq!(fixture.node.tick(), TickResult::Backpressured);
    assert_eq!(pop(&mut fixture), Some(1));
    assert_eq!(pop(&mut fixture), None);
    assert!(fixture.node.pending.is_none());
}
