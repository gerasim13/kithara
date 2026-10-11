use std::num::NonZeroUsize;

use super::*;

#[kithara::test]
fn a_ready_paused_segment_keeps_its_cue_over_an_old_slot_snapshot() {
    let mut rig = rig();
    let (mut track, _) = prepared_track();
    let (sender, mut inbox) = channel(ChannelConfig::builder().build());
    track.lane = Some(sender);
    track.status = TrackStatus::Paused { at: Position::ZERO };
    let to = Duration::from_secs(2);
    let _ = command_at_zero(&mut track, &mut rig, TrackCommand::Seek { to });
    let segment = track.segment;
    inbox.drain();
    inbox
        .next_due(LaneFrame { segment, frame: 0 }, 1)
        .expect("cue segment")
        .apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(segment),
        });
    track.settle_lane();
    let output = mock::output(None).get();
    let snapshot = crate::DeckSnapshot::new(DeckMixerConfig::default());
    let pass = crate::DeckPass {
        mix: DeckMixSettings::default(),
        suspended: false,
        now: frame(0),
        delivery: FrameCount::new(0),
        output: &output,
        deck: &snapshot,
    };
    let deck = std::ops::DerefMut::deref_mut(&mut rig);
    let mut scope = deck.ring.scope(deck.scope).expect("live deck scope");
    let out = Outbox::new(&mut scope, &mut deck.dispatcher).in_pass(pass);
    track.observe(&out);
    assert_eq!(track.snapshot().position, to);
    assert_eq!(track.snapshot().status, TrackStatus::Paused { at: to });
}

#[kithara::test]
#[case::applied(true)]
#[case::rejected(false)]
fn an_alignment_uses_one_credit_and_settles_every_part(#[case] applied: bool) {
    let mut rig = rig();
    let (mut track, _) = prepared_track();
    let (sender, mut inbox) = channel(
        ChannelConfig::builder()
            .capacity(NonZeroUsize::new(1).expect("one batch"))
            .build(),
    );
    track.lane = Some(sender);
    track.declick = FrameCount::new(32);
    track.status = TrackStatus::Playing { since: frame(0) };
    track.mark = Some(SlotMark {
        session: frame(0),
        lane: LaneFrame {
            segment: SegmentId::FIRST,
            frame: 0,
        },
        position: Duration::ZERO,
    });
    let to = Duration::from_secs(2);
    let landing = frame(i64::try_from(track.declick.get()).expect("declick fits"));
    let seq = command_at_zero(
        &mut track,
        &mut rig,
        TrackCommand::Align {
            to,
            speed: 1.5,
            at: landing,
        },
    )
    .expect("alignment batch");
    assert_eq!(track.lane.as_ref().expect("lane").available(), 0);
    assert_eq!(track.snapshot().speed, 1.0);
    inbox.drain();
    let at = LaneFrame {
        segment: SegmentId::FIRST,
        frame: 0,
    };
    let due = inbox.next_due(at, 1).expect("one alignment");
    assert_eq!(due.seq(), seq);
    assert!(
        matches!(due.commands(), [LaneCommand::SetSpeed(SpeedCurve::Constant(1.5)), LaneCommand::Jump { to: target }] if *target == to)
    );
    if applied {
        due.apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: None,
        });
    } else {
        drop(due);
    }
    assert!(inbox.next_due(at, 1).is_none());
    track.settle_lane();
    assert_eq!(track.lane_commands.len(), 2);
    assert!(
        track
            .lane_commands
            .iter()
            .all(|operation| operation.seq == seq && operation.applied == Some(applied))
    );
    let receipt = track.speed_receipt().expect("batch verdict");
    assert!(match receipt {
        Settled::Applied { seq: named, .. } => applied && named == seq,
        Settled::Rejected {
            seq: named,
            reason: Rejection::Unanswered,
        } => !applied && named == seq,
        _ => false,
    });
    assert!(track.speed_receipt().is_none());
    let (planned, speed) = track
        .planned(landing, mock::SAMPLE_RATE)
        .expect("planned landing");
    assert_eq!(
        planned,
        if applied {
            to
        } else {
            Duration::from_secs_f64(32.0 / f64::from(mock::SAMPLE_RATE.get()))
        }
    );
    assert_eq!(speed, if applied { 1.5 } else { 1.0 });
    assert_eq!(track.snapshot().speed, if applied { 1.5 } else { 1.0 });
    assert_eq!(track.lane.as_ref().expect("lane").available(), 1);
}
