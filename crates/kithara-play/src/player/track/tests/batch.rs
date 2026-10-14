use std::num::NonZeroUsize;

use super::*;
use crate::Bound;

#[kithara::test]
#[case::ready(true)]
#[case::rejected(false)]
fn a_segment_entry_keeps_its_native_curve_and_waits_for_native_readiness(#[case] applied: bool) {
    let mut rig = rig();
    let (mut track, mut inbox) = prepared_track();
    let curve = SpeedCurve::Approach(
        kithara_signal::ExponentialApproach::new(1.0, 0.9, 220.5).expect("finite curve"),
    );
    let position = Duration::from_secs(2);
    in_pass(&mut rig, frame(0), |out| {
        track.cue(position, curve.clone(), out)
    })
    .expect("cue");
    let segment = track.segment;
    let bound = Bound::AtOrAfter(frame(128));
    assert_eq!(track.ready, None);
    assert_eq!(track.entry(bound), None);
    track.status = TrackStatus::Loading;
    assert_eq!(track.entry(bound), None);
    assert_ne!(
        track.lane_commands.last().expect("segment command").segment,
        segment
    );
    assert_eq!(track.prepared_speed(), Some(&curve));
    assert!(
        !in_pass(&mut rig, frame(0), |out| track
            .prepare_entry(frame(128), out))
        .expect("preparation")
    );
    inbox.drain();
    let due = inbox
        .next_due(LaneFrame { segment, frame: 0 }, 1)
        .expect("native segment");
    assert!(
        matches!(due.commands(), [LaneCommand::Segment { id, speed, from }] if *id == segment && *speed == curve && *from == position)
    );
    if applied {
        due.apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(segment),
        });
    } else {
        drop(due);
    }
    assert_eq!(
        in_pass(&mut rig, frame(0), |out| track
            .prepare_entry(frame(128), out))
        .expect("native receipt"),
        applied
    );
    assert_eq!(track.prepared_speed(), applied.then_some(&curve));
    assert_eq!(track.ready, applied.then_some(segment));
    assert_eq!(track.entry(bound), applied.then_some(frame(128)));
    track.status = TrackStatus::Loaded;
    assert_eq!(track.entry(bound), applied.then_some(frame(128)));
}

#[kithara::test]
fn a_later_speed_does_not_replace_the_prepared_origin_law() {
    let mut rig = rig();
    let (mut track, mut inbox) = prepared_track();
    let curve = SpeedCurve::Approach(
        kithara_signal::ExponentialApproach::new(1.0, 0.9, 220.5).expect("finite curve"),
    );
    in_pass(&mut rig, frame(0), |out| {
        track.cue(Duration::ZERO, curve.clone(), out)
    })
    .expect("cue");
    let segment = track.segment;
    inbox.drain();
    inbox
        .next_due(LaneFrame { segment, frame: 0 }, 1)
        .expect("native segment")
        .apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(segment),
        });
    track.settle_lane();
    track.resume = Some(SlotMark {
        session: frame(64),
        lane: LaneFrame { segment, frame: 64 },
        position: Duration::from_secs_f64(64.0 / f64::from(mock::SAMPLE_RATE.get())),
    });
    assert_eq!(
        track.prepared_speed(),
        None,
        "a consumed transient is not a fresh entry law"
    );
    track.resume = None;
    track
        .send_lane(
            LaneCommand::SetSpeed(SpeedCurve::Constant(1.5)),
            When::At(LaneFrame { segment, frame: 64 }),
        )
        .expect("later speed");
    assert_eq!(track.prepared_speed(), None);
    inbox.drain();
    drop(
        inbox
            .next_due(LaneFrame { segment, frame: 64 }, 1)
            .expect("later speed"),
    );
    track.settle_lane();
    assert_eq!(
        track.prepared_speed(),
        Some(&curve),
        "a rejected speed does not change the origin law"
    );
    let replacement = SpeedCurve::Constant(1.25);
    track
        .send_lane(
            LaneCommand::SetSpeed(replacement.clone()),
            When::At(LaneFrame { segment, frame: 0 }),
        )
        .expect("origin speed");
    assert_eq!(track.prepared_speed(), Some(&replacement));
}

#[kithara::test]
fn a_replay_entry_waits_for_native_adoption_after_lane_readiness() {
    let mut rig = rig();
    let (mut track, mut inbox) = prepared_track();
    track.status = TrackStatus::Ended { at: frame(480_000) };
    let curve = SpeedCurve::Approach(
        kithara_signal::ExponentialApproach::new(1.025, 1.025, 0.0).expect("exact replay curve"),
    );
    in_pass(&mut rig, frame(0), |out| {
        track.cue(Duration::ZERO, curve.clone(), out)
    })
    .expect("replay cue");
    let segment = track.segment;
    inbox.drain();
    inbox
        .next_due(LaneFrame { segment, frame: 0 }, 1)
        .expect("replay segment")
        .apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(segment),
        });
    assert!(
        !in_pass(&mut rig, frame(0), |out| track
            .prepare_entry(frame(128), out))
        .expect("adoption pending")
    );
    assert_eq!(
        track.lane_commands.len(),
        1,
        "lane readiness cannot reopen the replay again"
    );
    let receipt = answer_next(&mut rig, Ok(()));
    answer_track(&mut track, &mut rig, receipt);
    assert_eq!(track.status, TrackStatus::Loaded);
    assert!(
        in_pass(&mut rig, frame(0), |out| track
            .prepare_entry(frame(128), out))
        .expect("adopted replay")
    );
    assert_eq!(track.prepared_speed(), Some(&curve));
    assert_eq!(track.lane_commands.len(), 1);
}

#[kithara::test]
fn an_older_native_ready_receipt_cannot_admit_the_replacement_segment() {
    let mut rig = rig();
    let (mut track, mut inbox) = prepared_track();
    in_pass(&mut rig, frame(0), |out| {
        track.cue(Duration::ZERO, SpeedCurve::Constant(1.0), out)
    })
    .expect("first cue");
    let old = track.segment;
    let curve = SpeedCurve::Approach(
        kithara_signal::ExponentialApproach::new(1.0, 0.9, 220.5).expect("replacement law"),
    );
    in_pass(&mut rig, frame(0), |out| {
        track.cue(Duration::ZERO, curve.clone(), out)
    })
    .expect("replacement cue");
    let current = track.segment;
    let bound = Bound::AtOrAfter(frame(128));
    assert_eq!(track.ready, None);
    assert_eq!(track.entry(bound), None);
    track.status = TrackStatus::Loading;
    assert_eq!(track.entry(bound), None);
    assert_ne!(old, current);
    inbox.drain();
    inbox
        .next_due(
            LaneFrame {
                segment: old,
                frame: 0,
            },
            1,
        )
        .expect("older native segment")
        .apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(old),
        });
    assert!(
        !in_pass(&mut rig, frame(0), |out| track
            .prepare_entry(frame(128), out))
        .expect("old receipt intake")
    );
    assert_eq!(track.ready, Some(old));
    assert_eq!(track.entry(bound), None);
    track.status = TrackStatus::Loaded;
    assert_eq!(track.entry(bound), None);
    track.status = TrackStatus::Loading;
    assert!(track.snapshot().pending_lane);
    assert_eq!(track.prepared_speed(), Some(&curve));
    inbox
        .next_due(
            LaneFrame {
                segment: current,
                frame: 0,
            },
            1,
        )
        .expect("current native segment")
        .apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(current),
        });
    assert!(
        in_pass(&mut rig, frame(0), |out| track
            .prepare_entry(frame(128), out))
        .expect("current receipt intake")
    );
    assert_eq!(track.ready, Some(current));
    assert_eq!(track.entry(bound), Some(frame(128)));
    track.status = TrackStatus::Loaded;
    assert_eq!(track.entry(bound), Some(frame(128)));
    assert!(!track.snapshot().pending_lane);
}

#[cfg(feature = "usdt")]
#[kithara::test]
#[case::constant(SpeedCurve::Constant(1.5))]
#[case::ramp(SpeedCurve::Ramp {
    to: 1.5,
    frames: std::num::NonZeroU64::new(441).expect("nonempty ramp"),
})]
#[case::steps(SpeedCurve::Steps(Arc::from([(0, 1.25), (441, 1.5)])))]
fn a_queued_speed_publishes_its_target_and_sequence_once(#[case] curve: SpeedCurve) {
    let trace = kithara_test_utils::test::usdt::scope();
    let mut track = track(A);
    let (sender, _inbox) = channel(
        ChannelConfig::builder()
            .capacity(NonZeroUsize::new(1).expect("one batch"))
            .build(),
    );
    track.lane = Some(sender);
    track.mark = Some(SlotMark {
        session: frame(128),
        lane: LaneFrame {
            segment: SegmentId::FIRST,
            frame: 64,
        },
        position: Duration::ZERO,
    });
    let seq = track
        .send_lane(LaneCommand::SetSpeed(curve.clone()), When::Next)
        .expect("accepted speed");
    let requests = trace.events_of("rate_requested");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].field("request_revision"), Some(seq.get()));
    assert_eq!(
        requests[0].field("target_rate_bits"),
        Some(u64::from(1.5_f32.to_bits())),
    );
    assert_eq!(requests[0].field("session_frame"), Some(128));
    assert!(matches!(
        track.send_lane(LaneCommand::SetSpeed(curve), When::Next),
        Err(PlayError::Full(_)),
    ));
    assert_eq!(trace.events_of("rate_requested").len(), 1);
    drop(trace);
}

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
            speed: SpeedCurve::Constant(1.5),
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
