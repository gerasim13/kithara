use super::*;

#[kithara::test]
#[case::seek(false)]
#[case::cue(true)]
fn public_media_target_is_translated_once_before_the_raw_segment(#[case] cue: bool) {
    let (mut track, mut inbox) = prepared_track();
    track.media_origin = Some(Duration::from_millis(576));
    let target = Position::from_secs(2);
    let mut rig = rig();
    in_pass(&mut rig, frame(0), |out| {
        if cue {
            track.cue(target, SpeedCurve::Constant(1.25), out)
        } else {
            track.apply(TrackCommand::Seek { to: target }, out)
        }
    })
    .expect("known media target");
    let commands = lane_commands(&mut inbox);
    assert!(matches!(
        commands.as_slice(),
        [LaneCommand::Segment { from, .. }] if *from == Duration::from_millis(2_576)
    ));
}

#[kithara::test]
#[case::jump(false)]
#[case::align(true)]
fn timed_media_target_is_translated_once_before_the_raw_lane_batch(#[case] align: bool) {
    let (mut track, mut inbox) = prepared_track();
    track.media_origin = Some(Duration::from_millis(576));
    track.status = TrackStatus::Playing { since: frame(0) };
    track.mark = Some(SlotMark {
        session: frame(0),
        lane: LaneFrame {
            segment: track.segment,
            frame: 0,
        },
        position: Duration::from_millis(576),
    });
    let mut rig = rig();
    let target = Position::from_secs(2);
    let at = frame(8_000);
    let command = if align {
        TrackCommand::Align {
            to: target,
            speed: SpeedCurve::Constant(1.25),
            at,
        }
    } else {
        TrackCommand::Jump { to: target, at }
    };
    in_pass(&mut rig, frame(0), |out| track.apply(command, out)).expect("known timed media target");
    inbox.drain();
    let due = inbox
        .next_due(
            LaneFrame {
                segment: track.segment,
                frame: 8_000,
            },
            1,
        )
        .expect("raw lane batch");
    assert!(due.commands().iter().any(|command| {
        matches!(command, LaneCommand::Jump { to } if *to == Duration::from_millis(2_576))
    }));
    due.apply(kithara_render::LaneApplied {
        engine_latency: FrameCount::new(0),
        ready: None,
    });
}

#[kithara::test]
#[case::seek(0)]
#[case::jump(1)]
#[case::align(2)]
#[case::cue(3)]
fn unknown_media_translation_refuses_commands_without_changing_the_snapshot(#[case] command: u8) {
    let (mut track, mut inbox) = prepared_track();
    track.media_origin = None;
    track.position = Position::from_secs(7);
    let before = track.snapshot();
    let mut rig = rig();
    let target = Position::from_secs(2);
    let result = in_pass(&mut rig, frame(0), |out| match command {
        0 => track.apply(TrackCommand::Seek { to: target }, out),
        1 => track.apply(
            TrackCommand::Jump {
                to: target,
                at: frame(8_000),
            },
            out,
        ),
        2 => track.apply(
            TrackCommand::Align {
                to: target,
                speed: SpeedCurve::Constant(1.25),
                at: frame(8_000),
            },
            out,
        ),
        3 => track.cue(target, SpeedCurve::Constant(1.25), out),
        _ => unreachable!("fixture command"),
    });
    assert!(matches!(result, Err(PlayError::Untimed)));
    assert!(lane_commands(&mut inbox).is_empty());
    assert_eq!(track.snapshot().position, before.position);
    assert_eq!(track.snapshot().status, before.status);
    assert_eq!(track.snapshot().mark, before.mark);
}

#[kithara::test]
#[case::inside_prefix(250, 0)]
#[case::past_media_zero(400, 224)]
fn planned_media_position_integrates_the_raw_prefix_before_clamping(
    #[case] at: i64,
    #[case] expected_ms: u64,
) {
    let mut track = track(A);
    track.media_origin = Some(Duration::from_millis(576));
    track.segment_speed = 2.0;
    track.status = TrackStatus::Playing { since: frame(0) };
    track.mark = Some(SlotMark {
        session: frame(0),
        lane: LaneFrame {
            segment: track.segment,
            frame: 0,
        },
        position: Position::ZERO,
    });
    let (position, speed) = track
        .planned(
            frame(at),
            NonZeroU32::new(1_000).expect("millisecond clock"),
        )
        .expect("known media projection");
    assert_eq!(position, Duration::from_millis(expected_ms));
    assert_eq!(speed, 2.0);
    assert_eq!(track.mark.expect("raw mark").position, Position::ZERO);
}

#[kithara::test]
#[case::trimmed(576, 2_000)]
#[case::untrimmed(0, 1_424)]
fn public_media_extent_uses_the_same_basis_as_public_position(
    #[case] visible_ms: u64,
    #[case] expected_ms: u64,
) {
    let mut track = track(A);
    track.media_origin = Some(Duration::from_millis(576));
    track.visible_origin = Some(Duration::from_millis(visible_ms));
    track.duration = Some(Duration::from_secs(2));
    assert_eq!(
        track.snapshot().duration,
        Some(Duration::from_millis(expected_ms))
    );
    assert_eq!(track.duration, Some(Duration::from_secs(2)));
}

#[kithara::test]
fn planned_media_position_is_untimed_until_its_source_basis_is_known() {
    let mut track = track(A);
    track.media_origin = None;
    track.position = Duration::from_secs(7);
    track.status = TrackStatus::Playing { since: frame(0) };
    track.mark = Some(SlotMark {
        session: frame(0),
        lane: LaneFrame {
            segment: track.segment,
            frame: 0,
        },
        position: Duration::from_secs(14),
    });
    assert!(matches!(
        track.planned(frame(1_000), mock::SAMPLE_RATE),
        Err(PlayError::Untimed)
    ));
    assert_eq!(track.snapshot().position, Duration::from_secs(7));
    assert_eq!(track.snapshot().duration, None);
}

#[kithara::test]
#[case::media_position(2_000, 2_576)]
#[case::untrimmed_prefix(0, 250)]
fn host_rate_change_keeps_raw_resume_custody_without_a_media_seek(
    #[case] media_ms: u64,
    #[case] raw_ms: u64,
) {
    let (mut track, mut inbox) = prepared_track();
    track.media_origin = Some(Duration::from_millis(576));
    track.position = Duration::from_millis(media_ms);
    track.status = TrackStatus::Playing { since: frame(0) };
    track.mark = Some(SlotMark {
        session: frame(0),
        lane: LaneFrame {
            segment: track.segment,
            frame: 0,
        },
        position: Duration::from_millis(raw_ms),
    });
    let mut rig = rig();
    let rate = NonZeroU32::new(48_000).expect("host rate");
    in_pass(&mut rig, frame(0), |out| {
        track.apply(TrackCommand::SetHostRate { rate }, out)
    })
    .expect("host rate change does not require a media seek");
    let commands = lane_commands(&mut inbox);
    assert!(matches!(
        commands.as_slice(),
        [LaneCommand::SetHostRate { rate: requested, .. }] if *requested == rate
    ));
    assert_eq!(track.snapshot().position, Duration::from_millis(media_ms));
    assert_eq!(track.media_origin, Some(Duration::from_millis(576)));
}

#[kithara::test]
#[case::resume_without_pending_lane(false)]
#[case::resume_with_pending_lane(true)]
fn stopped_raw_mark_stays_raw_while_pause_and_resume_publish_media_position(
    #[case] pending_lane: bool,
) {
    let mut rig = rig();
    let (mut track, mut inbox) = loaded(&mut rig, A, "media-stop-resume");
    command_at_zero(
        &mut track,
        &mut rig,
        TrackCommand::Play {
            at: When::At(frame(0)),
        },
    );
    let mut started = rig.block(frame(0), 0.0);
    settle(&mut track, &mut rig, &mut started);
    track.media_origin = Some(Duration::from_millis(576));
    let stopped = command_at_zero(
        &mut track,
        &mut rig,
        TrackCommand::Pause {
            at: When::At(frame(128)),
        },
    )
    .expect("native Stop");
    let mut stopped_receipts = rig.block(frame(128), 2.576);
    let receipt = stopped_receipts
        .iter()
        .find(|receipt| receipt.seq == stopped)
        .expect("Stop owner receipt");
    assert!(matches!(receipt.outcome, Outcome::Applied { at, .. } if at == frame(128)));
    assert!(receipt.batch.commands.iter().any(|part| matches!(part,
        DeckPart::Returned(Returned::Stopped { slot: A, resume: Some(mark) })
            if mark.position == Duration::from_millis(2_576)
    )));
    settle(&mut track, &mut rig, &mut stopped_receipts);
    assert_eq!(
        track.resume.expect("raw pause resume").position,
        Duration::from_millis(2_576)
    );
    assert_eq!(track.snapshot().position, Duration::from_secs(2));
    assert_eq!(
        track.snapshot().status,
        TrackStatus::Paused {
            at: Duration::from_secs(2)
        }
    );

    if pending_lane {
        rig.with_outbox(|out| {
            track.apply(
                TrackCommand::Configure(TrackSettingsChange::Speed(1.25), When::Next),
                out,
            )
        })
        .expect("live scope")
        .expect("pending lane speed");
    }
    rig.with_outbox(|out| {
        track.apply(
            TrackCommand::Play {
                at: When::At(frame(256)),
            },
            out,
        )
    })
    .expect("live scope")
    .expect("resume");
    let commands = lane_commands(&mut inbox);
    if pending_lane {
        assert!(commands.iter().any(|command| matches!(command,
            LaneCommand::Segment { from, .. } if *from == Duration::from_millis(2_576)
        )));
    } else {
        assert!(
            commands.is_empty(),
            "a ready paused segment resumes without a seek"
        );
        let mut resumed = rig.block(frame(256), 2.576);
        settle(&mut track, &mut rig, &mut resumed);
        assert_eq!(
            track.snapshot().status,
            TrackStatus::Playing { since: frame(256) }
        );
    }
    assert_eq!(track.snapshot().position, Duration::from_secs(2));
}

#[kithara::test]
#[kithara::hang_watchdog(ctx = String)]
#[case::known_raw_mark(Some(2_576), 2_000, 2_576)]
#[case::commanded_media_without_a_raw_mark(None, 2_000, 2_576)]
#[case::untrimmed_prefix(Some(250), 0, 250)]
fn cancelled_parked_repeat_restores_the_raw_coordinate_without_reapplying_media_origin(
    #[case] mark_ms: Option<u64>,
    #[case] media_ms: u64,
    #[case] expected_raw_ms: u64,
) {
    let mut rig = rig();
    let (mut track, mut inbox) = loaded(&mut rig, A, "media-parked-repeat");
    rig.with_outbox(|out| track.apply(TrackCommand::Play { at: When::Next }, out))
        .expect("live scope")
        .expect("initial Start");
    let mut started = rig.block(frame(0), 0.0);
    settle(&mut track, &mut rig, &mut started);
    let original = track.segment;
    track.media_origin = Some(Duration::from_millis(576));
    track.position = Duration::from_millis(media_ms);
    let mark = mark_ms.map(|raw_ms| SlotMark {
        session: frame(0),
        lane: LaneFrame {
            segment: original,
            frame: 0,
        },
        position: Duration::from_millis(raw_ms),
    });
    let output = mock::output(None).get();
    let deck = crate::DeckSnapshot {
        slots: vec![SlotSnapshot {
            state: SlotState::Playing,
            mark,
            position: mark.map_or(0.0, |mark| mark.position.as_secs_f64()),
            media_origin: track.media_origin,
            visible_origin: track.visible_origin,
            ..Default::default()
        }],
        ..Default::default()
    };
    let repeated = {
        let fixture = &mut *rig;
        let mut scope = fixture.ring.scope(fixture.scope).expect("live scope");
        let mut out = Outbox::new(&mut scope, &mut fixture.dispatcher).in_pass(crate::DeckPass {
            mix: DeckMixSettings::default(),
            suspended: false,
            now: frame(0),
            delivery: FrameCount::new(0),
            output: &output,
            deck: &deck,
        });
        track
            .apply(TrackCommand::PlayAfter { track: A }, &mut out)
            .expect("repeat preparation")
            .expect("parked repeat")
    };
    assert_eq!(track.snapshot().mark, mark);
    assert_eq!(
        track.adopting.last().expect("repeat adoption").parked,
        Some((original, Duration::from_millis(expected_raw_ms)))
    );
    rig.with_outbox(|out| track.apply(TrackCommand::Supersede, out))
        .expect("live scope")
        .expect("cancel repeat");
    let mut refused = rig.block(frame(0), 0.0);
    assert!(refused.iter().any(|receipt| receipt.seq == repeated
        && matches!(receipt.outcome, Outcome::Rejected(Rejection::Stale))));
    settle(&mut track, &mut rig, &mut refused);
    assert_eq!(track.segment, original);
    inbox.drain();
    let mut restored = false;
    while let Some(due) = inbox.next_due(LaneFrame::default(), 1) {
        hang_tick!(format!("restoring raw repeat segment {original:?}"));
        restored |= due.commands().iter().any(|command| {
            matches!(command,
                LaneCommand::Segment { id, from, .. }
                    if *id == original && *from == Duration::from_millis(expected_raw_ms)
            )
        });
        due.apply(kithara_render::LaneApplied {
            engine_latency: FrameCount::new(0),
            ready: Some(original),
        });
        hang_reset!();
    }
    assert!(
        restored,
        "the cancelled adoption restores its actual raw segment position"
    );
    track.settle_lane();
    assert_eq!(track.snapshot().position, Duration::from_millis(media_ms));
    assert_eq!(rig.mixer.held(A), Some("media-parked-repeat"));
}
