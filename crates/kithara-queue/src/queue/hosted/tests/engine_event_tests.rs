use kithara_audio::{DecodeErrorKind, TrackFailureKind};
use kithara_command::Port;
use kithara_play::{DeckEvent, PlaybackFault, Slot};
use kithara_signal::SegmentId;

use super::*;
use crate::{AdvanceReason, queue::state::tests::wait_for_queue_event};

#[kithara::test(tokio, timeout(Duration::from_secs(10)))]
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn pause_and_none_suppress_natural_eof_progression() {
    for action in [ActionAtItemEnd::Pause, ActionAtItemEnd::None] {
        hang_tick!();
        let (mut queue, first, _second, mut rig, _dir) = crate::mock::selected().await;
        hang_reset!();
        with_outbox(&mut queue, &mut rig, |queue, out| {
            Player::apply(queue, QueueCommand::SetActionAtItemEnd(action), out)
        })
        .expect("end action");
        let slot = queue
            .active
            .iter()
            .find(|active| active.item == first)
            .expect("current slot")
            .slot;
        let mut events = queue.subscribe::<QueueEvent>();
        rig.mixer
            .report(DeckEvent::Ended {
                slot,
                at: SessionFrame::new(1024),
            })
            .expect("natural end");
        tick_fixture(&mut queue, &mut rig);
        assert_eq!(queue.current().map(|entry| entry.id), Some(first));
        assert!(queue.target.is_none());
        if action == ActionAtItemEnd::Pause {
            settle_natural_end_pause(&mut queue, &mut rig, slot);
            assert!(queue.current_track().is_some_and(|track| matches!(
                track.snapshot().status,
                PlayingStatus::Paused { .. }
            )));
        }
        assert!(
            !wait_for_queue_event(
                &mut events,
                |event| matches!(event, QueueEvent::QueueEnded),
                50
            )
            .await
        );
    }
}

#[kithara::test(tokio, timeout(Duration::from_secs(10)))]
async fn a_tick_pauses_at_the_natural_end_it_drains() {
    let (mut queue, first, _, mut rig, _dir) = crate::mock::selected().await;
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(
            queue,
            QueueCommand::SetActionAtItemEnd(ActionAtItemEnd::Pause),
            out,
        )
    })
    .expect("pause at end");
    let slot = queue
        .active
        .iter()
        .find(|active| active.item == first)
        .expect("current slot")
        .slot;
    rig.mixer
        .report(DeckEvent::Ended {
            slot,
            at: SessionFrame::new(1024),
        })
        .expect("natural end");
    tick_fixture(&mut queue, &mut rig);
    settle_natural_end_pause(&mut queue, &mut rig, slot);
    assert!(
        queue
            .current_track()
            .is_some_and(|track| matches!(track.snapshot().status, PlayingStatus::Paused { .. }))
    );
}

fn settle_natural_end_pause(
    queue: &mut Queue<TestPools>,
    rig: &mut mock::DeckRig<TestPools>,
    slot: Slot,
) {
    let current = queue.active_current_index().expect("ended current slot");
    let ended = PlayingStatus::Ended {
        at: SessionFrame::new(1024),
    };
    assert_eq!(
        queue
            .active
            .get(current)
            .expect("ended current track")
            .track
            .snapshot()
            .status,
        ended
    );
    let pause = rig
        .ring
        .scope(rig.scope)
        .expect("deck scope")
        .basis(slot, When::Next)
        .expect("native Pause sequence");
    let stopped_mark = SlotMark {
        session: SessionFrame::new(2048),
        lane: LaneFrame {
            segment: SegmentId::FIRST,
            frame: 0,
        },
        position: Duration::ZERO,
    };
    let mut stopped = rig
        .block(SessionFrame::new(2048), 0.0)
        .expect("native Stop block");
    assert_eq!(stopped.len(), 1);
    let receipt = stopped.pop().expect("native Stop receipt");
    assert_eq!(receipt.seq(), pause);
    let (outcome, mut batch) = receipt.into();
    assert!(matches!(outcome, Outcome::Applied { at, .. } if at == SessionFrame::new(2048)));
    assert!(matches!(
        batch.commands.as_slice(),
        [DeckPart::Returned(Returned::Stopped { slot: stopped_slot, resume: Some(mark) })]
            if *stopped_slot == slot && *mark == stopped_mark
    ));
    assert_eq!(
        queue
            .active
            .get(current)
            .expect("unsettled current track")
            .track
            .snapshot()
            .status,
        ended
    );
    with_outbox(queue, rig, |queue, out| {
        Player::settle(
            queue,
            TrackReceipt::Deck {
                seq: pause,
                outcome: &outcome,
                batch: &mut batch,
            },
            out,
        )
    });
    assert_eq!(
        queue
            .current_track()
            .expect("stopped current track")
            .snapshot()
            .position,
        stopped_mark.position
    );
}

#[kithara::test(tokio)]
async fn the_deck_leading_on_past_a_failed_track_advances_for_the_failure() {
    let (mut queue, failed, successor, mut rig, dir) = crate::mock::selected().await;
    let slot = queue
        .active
        .iter()
        .find(|active| active.item == failed)
        .expect("failed slot")
        .slot;
    let mut events = queue.subscribe::<QueueEvent>();
    rig.mixer
        .report(DeckEvent::Failed {
            slot,
            at: SessionFrame::new(1024),
            fault: PlaybackFault::Source(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData,
            }),
        })
        .expect("source failure");
    tick_fixture(&mut queue, &mut rig);
    answer_load(&mut queue, &mut rig, &dir).await;
    crate::mock::finish(&mut queue, &mut rig);
    let advanced = wait_for_queue_event(
        &mut events,
        |event| {
            matches!(event, QueueEvent::CurrentTrackAdvance {
            reason: AdvanceReason::TrackFailed, id: Some(id),
        } if *id == successor)
        },
        200,
    )
    .await;
    assert!(
        advanced,
        "the cursor leaves the failed track for its failure"
    );
}

#[kithara::test(tokio)]
async fn lag_recovery_keeps_an_ended_queue_ended() {
    let (mut queue, last, _, mut rig, _dir) = crate::mock::selected().await;
    queue.current = Some(last);
    queue.current = None;
    queue.navigation = crate::NavigationState::new(queue.navigation.history_limit());
    queue.publish();
    let mut events = queue.subscribe::<QueueEvent>();
    tick_fixture(&mut queue, &mut rig);
    assert_eq!(queue.current().map(|entry| entry.id), None);
    assert!(
        !wait_for_queue_event(
            &mut events,
            |event| matches!(event, QueueEvent::CurrentTrackAdvance { .. }),
            50
        )
        .await,
        "the ended queue advances nowhere"
    );
}

#[kithara::test(tokio)]
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn lagged_player_events_resynchronize_current_track() {
    hang_tick!();
    let (mut queue, id, _, mut rig, _dir) = crate::mock::selected().await;
    hang_reset!();
    let unrelated = DeckEvent::Ended {
        slot: Slot::new(3),
        at: SessionFrame::new(128),
    };
    while rig.mixer.report(unrelated).is_ok() {
        hang_tick!();
        hang_reset!();
    }
    rig.events.drain().for_each(drop);
    let mut events = queue.subscribe::<QueueEvent>();
    tick_fixture(&mut queue, &mut rig);
    hang_tick!();
    let saw_current_track = wait_for_queue_event(&mut events, |event| {
        matches!(event, QueueEvent::CurrentTrackChanged { id: Some(current_id) } if *current_id == id)
    }, 200).await;
    hang_reset!();
    assert!(
        saw_current_track,
        "lag recovery should re-announce the current track"
    );
}
