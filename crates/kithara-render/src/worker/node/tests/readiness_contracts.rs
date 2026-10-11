use kithara_command::Outcome;
use kithara_signal::SourceSpan;

use super::*;
use crate::dispatcher::LaneTask;

fn entry(segment: SegmentId, source_frame: u64, revision: u64) -> AudioChunk {
    let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("fixture rate"));
    let mut packet = chunk(spec, segment, 0, source_frame, &[0.5; 2_048]);
    packet.meta.source_span =
        SourceSpan::new(source_frame, source_frame + 1_024, spec.sample_rate, 1_024)
            .map(|span| span.with_render_revision(revision));
    packet
}

fn admit(node: &mut DecoderNode<ScriptedSource, TestPools>, packet: AudioChunk) {
    node.pending = Some(PendingPacket {
        packet: PcmPacket::Chunk(Box::new(packet)),
        source_end: None,
    });
    assert_eq!(node.tick(), TickResult::Progress);
}

#[kithara::test(tokio)]
async fn a_chunk_from_another_decode_epoch_proves_nothing() {
    let (mut node, _receiver, mut lane) = prepared_node(ScriptedSource::new([]), 4, 2).await;
    let stale = SegmentId::FIRST;
    let positioned = stale.next();
    node.pending = Some(PendingPacket {
        packet: PcmPacket::Chunk(Box::new(entry(stale, 0, 0))),
        source_end: None,
    });
    let seq = lane
        .send(
            When::Next,
            Batch {
                basis: Vec::new(),
                commands: vec![LaneCommand::Segment {
                    id: positioned,
                    from: Duration::from_secs(1),
                    speed: kithara_warp::SpeedCurve::Constant(1.0),
                }],
            },
        )
        .expect("positioned segment");
    node.synchronize().expect("synchronous positioning");
    node.pending = Some(PendingPacket {
        packet: PcmPacket::Chunk(Box::new(entry(stale, 0, 0))),
        source_end: None,
    });
    node.synchronize().expect("retire stale decode output");
    assert!(
        lane.receipts().next().is_none(),
        "PCM decoded before the lane was positioned neither proves nor fails it"
    );
    assert!(matches!(node.preload_status(), Ok(false)));

    for source_frame in [44_100, 45_124] {
        admit(&mut node, entry(positioned, source_frame, seq.get()));
    }
    let receipt = lane.receipts().next().expect("ready verdict");
    assert_eq!(receipt.seq(), seq);
    assert!(matches!(
        receipt.outcome(),
        Outcome::Applied { data, .. } if data.ready == Some(positioned)
    ));
    assert!(matches!(node.preload_status(), Ok(true)));
}

#[kithara::test(tokio)]
#[case::before_the_origin(-1, 0)]
#[case::after_the_origin(1, 0)]
#[case::from_another_mapping(0, 1)]
async fn readiness_needs_the_exact_origin_and_revision_of_the_activation(
    #[case] offset: i64,
    #[case] revision_shift: u64,
) {
    let (mut node, _receiver, mut lane) = prepared_node(ScriptedSource::new([]), 4, 2).await;
    let positioned = SegmentId::FIRST.next();
    let origin = 44_100_u64;
    assert_ne!(origin, 0, "the fixture activates mid-recording");
    let seq = lane
        .send(
            When::Next,
            Batch {
                basis: Vec::new(),
                commands: vec![LaneCommand::Segment {
                    id: positioned,
                    from: Duration::from_secs(1),
                    speed: kithara_warp::SpeedCurve::Constant(1.0),
                }],
            },
        )
        .expect("activation command");
    node.synchronize().expect("synchronous positioning");
    admit(
        &mut node,
        entry(
            positioned,
            origin.saturating_add_signed(offset),
            seq.get() + revision_shift,
        ),
    );
    admit(&mut node, entry(positioned, origin + 1_024, seq.get()));
    let receipt = lane.receipts().next().expect("readiness verdict");
    assert_eq!(receipt.seq(), seq);
    assert!(
        matches!(receipt.outcome(), Outcome::Rejected(_)),
        "a lane that does not enter at {origin} under revision {} cannot present the plan",
        seq.get()
    );
}
