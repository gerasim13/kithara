use kithara_command::{Rejection, When};
use kithara_play::{HostedDeck, PlayError, Player, Settled, TrackCommand, TrackStatus};
use kithara_signal::{FrameCount, SessionFrame};
use kithara_test_utils::kithara;
use kithara_warp::SpeedCurve;

use super::fixtures::{Command, answer, grid, load, loaded, position, sounding, trajectory};
use crate::{LinkedPlayer, SyncStatus};

#[kithara::test]
fn aligned_seek_preserves_playback_until_its_receipt() {
    let (mut deck, control, mut rig, _) = sounding();
    rig.now = SessionFrame::new(48_000);
    rig.delivery = FrameCount::new(48_000);
    control.edit(|script| {
        script.snapshot.position = position(48_000);
        script.snapshot.status = TrackStatus::Playing {
            since: SessionFrame::new(48_000),
        };
    });
    let before = deck.snapshot();
    let seq = rig
        .run(|out| {
            deck.apply(
                TrackCommand::Seek {
                    to: position(36_000),
                },
                out,
            )
        })
        .expect("aligned seek")
        .expect("jump sequence");
    assert_eq!(
        control.commands(),
        [(seq, Command::Jump(position(0), SessionFrame::new(96_000)))]
    );
    assert_eq!(deck.snapshot().as_ref().position, before.as_ref().position);
    assert_eq!(deck.snapshot().as_ref().status, before.as_ref().status);
    assert_eq!(deck.snapshot().sync, SyncStatus::On);
    assert!(
        matches!(rig.apply(&mut deck, seq, 96_000), Settled::Applied { seq: applied, .. } if applied == seq)
    );
    assert_eq!(deck.snapshot().as_ref().position, position(0));
    assert_eq!(deck.snapshot().as_ref().status, before.as_ref().status);
}

#[kithara::test]
fn aligned_seek_respects_lane_lead_and_refusal_keeps_the_pending_jump() {
    let (mut deck, control, mut rig, _) = sounding();
    rig.now = SessionFrame::new(96_000);
    control.edit(|script| {
        script.snapshot.ring_depth = FrameCount::new(256);
        script.snapshot.engine_latency = FrameCount::new(64);
        script.snapshot.declick = FrameCount::new(32);
    });
    let seq = rig
        .run(|out| deck.apply(TrackCommand::Seek { to: position(0) }, out))
        .expect("seek")
        .expect("pending jump");
    let at = SessionFrame::new(96_480);
    assert_eq!(
        control.commands(),
        [(seq, Command::Jump(position(480), at))]
    );
    control.edit(|script| script.reject = Some(PlayError::Full("lane")));
    assert!(matches!(
        rig.run(|out| deck.apply(
            TrackCommand::Seek {
                to: position(36_000)
            },
            out
        )),
        Err(PlayError::Full("lane"))
    ));
    assert_eq!(control.commands().len(), 1);
    rig.apply(&mut deck, seq, 96_480);
    assert_eq!(deck.snapshot().as_ref().position, position(480));
}

#[kithara::test]
fn a_new_load_owns_position_and_obsolete_jump_receipts_change_nothing() {
    let (mut deck, control, mut rig, old_load) = sounding();
    let jump = rig
        .run(|out| {
            deck.apply(
                TrackCommand::Seek {
                    to: position(36_000),
                },
                out,
            )
        })
        .expect("seek")
        .expect("jump");
    let current = load(&mut deck, &mut rig, 6_000);
    rig.apply(&mut deck, current, 0);
    control.clear();
    let before = deck.snapshot();
    assert!(matches!(rig.apply(&mut deck, jump, 128), Settled::Pending));
    answer(
        &mut deck,
        &mut rig,
        old_load,
        grid(24_000, 0, None, 960_000),
    );
    assert_eq!(deck.snapshot().as_ref().position, position(6_000));
    assert_eq!(deck.snapshot().as_ref().status, before.as_ref().status);
    assert_eq!(deck.snapshot().sync, before.sync);
    assert!(control.commands().is_empty());
}

#[kithara::test]
fn aligned_seek_waits_for_a_grid_covering_its_cue() {
    let (mut deck, control, mut rig, load) = loaded(grid(24_000, 0, None, 192_000));
    let sync = rig
        .run(|out| LinkedPlayer::sync(&mut deck, true, out))
        .expect("sync")
        .expect("cue");
    rig.apply(&mut deck, sync, 0);
    control.edit(|script| {
        script.snapshot.status = TrackStatus::Playing {
            since: SessionFrame::new(0),
        }
    });
    control.clear();
    assert_eq!(
        rig.run(|out| deck.apply(
            TrackCommand::Seek {
                to: position(300_000)
            },
            out
        ))
        .expect("wait"),
        None
    );
    assert_eq!(
        deck.snapshot().sync,
        SyncStatus::WaitingForGrid {
            required: position(300_000)
        }
    );
    assert!(control.commands().is_empty());
    answer(&mut deck, &mut rig, load, grid(24_000, 0, None, 480_000));
    let commands = control.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(
        commands[0].1,
        Command::Jump(position(288_128), SessionFrame::new(128))
    );
    assert_eq!(deck.snapshot().sync, SyncStatus::On);
    assert_eq!(deck.snapshot().as_ref().position, position(0));
}

#[kithara::test]
fn synchronized_transport_intercepts_seek_and_resume_but_admits_pause() {
    let (mut deck, control, mut rig, _) = sounding();
    let jump = rig
        .run(|out| deck.apply(TrackCommand::Seek { to: position(0) }, out))
        .expect("seek")
        .expect("jump");
    assert_eq!(
        control.commands(),
        [(jump, Command::Jump(position(128), SessionFrame::new(128)))]
    );
    let pause = rig
        .run(|out| deck.apply(TrackCommand::Pause { at: When::Next }, out))
        .expect("pause")
        .expect("pause sequence");
    rig.apply(&mut deck, pause, 0);
    control.clear();
    let seek = rig
        .run(|out| deck.apply(TrackCommand::Seek { to: position(0) }, out))
        .expect("paused seek")
        .expect("seek sequence");
    assert_eq!(control.commands(), [(seek, Command::Seek(position(0)))]);
    rig.apply(&mut deck, seek, 0);
    control.clear();
    let start = rig
        .run(|out| deck.apply(TrackCommand::Play { at: When::Next }, out))
        .expect("resume")
        .expect("start sequence");
    assert_eq!(
        control.commands(),
        [(start, Command::Play(When::At(SessionFrame::new(96_000))))]
    );
}

#[kithara::test]
fn unsynchronized_seek_passes_the_position_unchanged() {
    let (mut deck, control, mut rig, _) = loaded(grid(24_000, 0, None, 480_000));
    let seq = rig
        .run(|out| deck.apply(TrackCommand::Seek { to: position(0) }, out))
        .expect("seek")
        .expect("seek sequence");
    assert_eq!(control.commands(), [(seq, Command::Seek(position(0)))]);
    assert_eq!(deck.snapshot().sync, SyncStatus::Off);
}

#[kithara::test]
fn rejected_jump_keeps_active_playback_and_cannot_be_resurrected() {
    let (mut deck, control, mut rig, _) = sounding();
    rig.now = SessionFrame::new(48_000);
    rig.delivery = FrameCount::new(48_000);
    control.edit(|script| {
        script.snapshot.position = position(48_000);
        script.snapshot.status = TrackStatus::Playing {
            since: SessionFrame::new(48_000),
        };
    });
    let before = deck.snapshot();
    let seq = rig
        .run(|out| deck.apply(TrackCommand::Seek { to: position(0) }, out))
        .expect("seek")
        .expect("jump");
    assert_eq!(
        control.commands(),
        [(seq, Command::Jump(position(0), SessionFrame::new(96_000)))]
    );
    assert!(matches!(
        rig.settle(
            &mut deck,
            seq,
            &kithara_command::Outcome::Rejected(Rejection::Refused(
                kithara_render::bridge::DeckRefusal::Occupied {
                    slot: kithara_render::bridge::Slot::new(0)
                }
            ))
        ),
        Settled::Rejected { .. }
    ));
    assert_eq!(deck.snapshot().as_ref().position, before.as_ref().position);
    assert_eq!(deck.snapshot().as_ref().status, before.as_ref().status);
    assert_eq!(deck.snapshot().sync, before.sync);
    assert!(matches!(
        rig.apply(&mut deck, seq, 96_000),
        Settled::Pending
    ));
    assert_eq!(deck.snapshot().as_ref().position, before.as_ref().position);
}

#[kithara::test]
fn retime_owns_the_replacement_start_and_ignores_the_withdrawn_receipt() {
    let (mut deck, control, mut rig, _) = loaded(grid(24_000, 0, Some((4, 0)), 960_000));
    let sync = rig
        .run(|out| LinkedPlayer::sync(&mut deck, true, out))
        .expect("sync")
        .expect("cue");
    rig.apply(&mut deck, sync, 0);
    let old_start = rig
        .run(|out| deck.apply(TrackCommand::Play { at: When::Next }, out))
        .expect("play")
        .expect("start");
    control.clear();
    rig.run(|out| {
        LinkedPlayer::retime(
            &mut deck,
            &trajectory(90.0, 4),
            SessionFrame::new(48_000),
            out,
        );
    });
    let commands = control.commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(
        commands[0].1,
        Command::Speed(SpeedCurve::Constant(0.75), When::Next)
    );
    assert_eq!(commands[1].1, Command::Seek(position(0)));
    assert_eq!(
        deck.snapshot().sync,
        SyncStatus::WaitingForGrid {
            required: position(0)
        }
    );
    assert!(matches!(
        rig.apply(&mut deck, old_start, 96_000),
        Settled::Pending
    ));
    assert_eq!(deck.snapshot().as_ref().status, TrackStatus::Loaded);
    rig.apply(&mut deck, commands[0].0, 0);
    control.clear();
    rig.run_pass(|out, pass| HostedDeck::tick(&mut deck, pass, out));
    let commands = control.commands();
    assert_eq!(commands.len(), 1);
    assert_ne!(commands[0].0, old_start);
    assert_eq!(
        commands[0].1,
        Command::Play(When::At(SessionFrame::new(128_000)))
    );
}
