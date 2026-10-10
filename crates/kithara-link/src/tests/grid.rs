use kithara_beat::{BeatGridModel, BeatGridState};
use kithara_events::TrackId;
use kithara_play::{Bound, Player, TrackCommand};
use kithara_signal::SessionFrame;
use kithara_test_utils::kithara;

use super::fixtures::{answer, grid, load, loaded, position, sounding};
use crate::{GridAnswer, LinkedPlayer, SyncStatus};

#[kithara::test]
fn foreign_grid_answers_preserve_the_current_load() {
    let (mut deck, control, mut rig, load) = sounding();
    let before = deck.snapshot();
    let entry = deck.entry(Bound::AtOrAfter(SessionFrame::new(1)));
    rig.run(|out| {
        LinkedPlayer::grid(
            &mut deck,
            GridAnswer {
                item: TrackId::allocate(),
                load,
                model: Ok(grid(32_000, 0, None, 960_000)),
            },
            out,
        );
    });
    assert!(control.commands().is_empty());
    assert_eq!(deck.snapshot().sync, before.sync);
    assert_eq!(deck.snapshot().as_ref().position, before.as_ref().position);
    assert_eq!(deck.entry(Bound::AtOrAfter(SessionFrame::new(1))), entry);
}

#[kithara::test]
fn grid_admission_follows_the_current_load_not_an_old_answer() {
    let (mut deck, control, mut rig, old_load) = sounding();
    let new_load = load(&mut deck, &mut rig, 6_000);
    rig.apply(&mut deck, new_load, 0);
    control.clear();
    let before = deck.snapshot().sync;
    answer(
        &mut deck,
        &mut rig,
        old_load,
        grid(24_000, 0, None, 480_000),
    );
    assert_eq!(deck.snapshot().sync, before);
    assert!(control.commands().is_empty());
    assert_eq!(deck.entry(Bound::AtOrAfter(SessionFrame::new(0))), None);
    answer(
        &mut deck,
        &mut rig,
        new_load,
        grid(30_000, 0, None, 480_000),
    );
    assert_eq!(control.commands().len(), 2);
    assert_eq!(
        deck.snapshot().sync,
        SyncStatus::WaitingForGrid {
            required: position(30_000)
        }
    );
    assert_ne!(old_load, new_load);
}

#[kithara::test]
fn repeating_the_same_grid_does_not_restart_a_correction() {
    let (mut deck, control, mut rig, load) = sounding();
    let revised = grid(32_000, 0, Some((4, 0)), 960_000);
    answer(&mut deck, &mut rig, load, revised.clone());
    assert!(!control.commands().is_empty());
    let before = deck.snapshot().sync;
    assert!(matches!(before, SyncStatus::Correcting { .. }));
    control.clear();
    answer(&mut deck, &mut rig, load, revised);
    assert!(control.commands().is_empty());
    assert_eq!(deck.snapshot().sync, before);
}

#[kithara::test]
fn newer_analysis_can_skip_unpublished_revisions() {
    let mut raw = grid(30_000, 0, None, 480_000).as_raw().clone();
    raw.state = BeatGridState::Provisional;
    raw.revision = 1;
    let (mut deck, control, mut rig, load) =
        loaded(BeatGridModel::try_from(raw).expect("provisional grid"));
    control.edit(|script| script.snapshot.position = position(6_000));
    let seq = rig
        .run(|out| LinkedPlayer::sync(&mut deck, true, out))
        .expect("sync")
        .expect("speed");
    rig.apply(&mut deck, seq, 0);
    assert_eq!(
        deck.entry(Bound::AtOrAfter(SessionFrame::new(0))),
        Some(SessionFrame::new(4_800))
    );
    let mut raw = grid(24_000, 0, None, 480_000).as_raw().clone();
    raw.revision = 4;
    answer(
        &mut deck,
        &mut rig,
        load,
        BeatGridModel::try_from(raw).expect("final grid"),
    );
    assert_eq!(
        deck.entry(Bound::AtOrAfter(SessionFrame::new(0))),
        Some(SessionFrame::new(6_000))
    );
    assert_eq!(deck.snapshot().sync, SyncStatus::On);
    assert!(
        rig.run(|out| deck.apply(
            TrackCommand::Seek {
                to: position(24_000)
            },
            out
        ))
        .is_ok()
    );
}
