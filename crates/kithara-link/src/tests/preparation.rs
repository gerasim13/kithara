use kithara_command::When;
use kithara_play::{HostedDeck, Player, TrackCommand};
use kithara_test_utils::kithara;
use kithara_warp::SpeedCurve;

use super::fixtures::{Command, Rig, answer, deck, grid, load, position, trajectory};
use crate::{LinkedPlayer, SyncStatus};

#[kithara::test]
#[case::preparation_carries_the_next_source_beat_to_the_next_deck_beat(
    10_000, 5_000, 24_000, 0, None, 24_000, 1.0
)]
#[case::preparation_before_the_first_grid_beat_cues_that_first_beat(
    6_000, 6_000, 30_000, 30_000, None, 30_000, 1.25
)]
#[case::different_bpm_grids_produce_one_coherent_phase_and_rate_decision(
    6_000, 6_000, 30_000, 0, None, 30_000, 1.25
)]
#[case::preparation_aligns_a_known_track_downbeat_to_the_session_origin_phase(100_000, 100_000, 24_000, 0, Some((4, 0)), 192_000, 1.0)]
#[case::preparation_preserves_non_four_four_downbeat_phase(50_000, 50_000, 24_000, 0, Some((3, 1)), 96_000, 1.0)]
#[case::preparation_keeps_the_nearest_host_downbeat_for_a_large_source_jump(252_000, 191_999, 24_000, 0, Some((4, 0)), 288_000, 1.0)]
fn sync_load_snaps_to_the_first_strong_beat_and_waits_before_play(
    #[case] cue: u32,
    #[case] now: i64,
    #[case] beat_frames: u32,
    #[case] first: u32,
    #[case] meter: Option<(u16, i64)>,
    #[case] snapped: u32,
    #[case] speed: f32,
) {
    let (mut deck, control) = deck(trajectory(120.0, meter.map_or(4, |(count, _)| count)));
    let mut rig = Rig::new(now);
    assert_eq!(
        rig.run(|out| LinkedPlayer::sync(&mut deck, true, out))
            .expect("mode admitted"),
        None
    );
    let load = load(&mut deck, &mut rig, cue);
    assert_eq!(control.commands(), [(load, Command::Load(position(cue)))]);
    assert_eq!(
        rig.run(|out| deck.apply(TrackCommand::Play { at: When::Next }, out))
            .expect("Play waits"),
        None
    );
    assert_eq!(
        deck.snapshot().sync,
        SyncStatus::WaitingForGrid {
            required: position(cue)
        }
    );
    rig.apply(&mut deck, load, now);
    control.clear();
    answer(
        &mut deck,
        &mut rig,
        load,
        grid(beat_frames, first, meter, 480_000),
    );
    let commands = control.commands();
    assert_eq!(commands.len(), 2);
    let seq = commands[0].0;
    assert_eq!(
        commands,
        [
            (seq, Command::Speed(SpeedCurve::Constant(speed), When::Next)),
            (seq, Command::Seek(position(snapped))),
        ]
    );
    assert_eq!(
        deck.snapshot().as_ref().position,
        position(cue),
        "the cue is not committed early"
    );
    rig.apply(&mut deck, seq, now);
    control.clear();
    rig.run_pass(|out, pass| HostedDeck::tick(&mut deck, pass, out));
    assert_eq!(deck.snapshot().as_ref().position, position(snapped));
    assert_eq!(deck.snapshot().as_ref().speed, speed);
    let commands = control.commands();
    assert_eq!(commands.len(), 1);
    assert!(
        matches!(commands[0].1, Command::Play(When::At(frame)) if frame >= rig.now + rig.delivery)
    );
}

#[kithara::test]
fn sync_load_snaps_between_beats_but_keeps_an_exact_beat() {
    for cue in [10_000, 24_000] {
        let (mut deck, control) = deck(trajectory(120.0, 4));
        let mut rig = Rig::new(10_000);
        rig.run(|out| LinkedPlayer::sync(&mut deck, true, out))
            .expect("sync");
        let load = load(&mut deck, &mut rig, cue);
        rig.apply(&mut deck, load, 10_000);
        control.clear();
        answer(&mut deck, &mut rig, load, grid(24_000, 0, None, 480_000));
        let commands = control.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0].1,
            Command::Speed(SpeedCurve::Constant(1.0), When::Next)
        );
        assert_eq!(commands[1].1, Command::Seek(position(24_000)));
        assert_eq!(commands[0].0, commands[1].0);
    }
}
