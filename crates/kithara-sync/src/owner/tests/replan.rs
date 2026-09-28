use kithara_signal::{SessionEpoch, SessionFrame};
use kithara_test_utils::kithara;
use kithara_warp::{AssetFrame, BeatGrid, BeatGridId, PresentationFrontier};

use super::{
    Accept,
    lifecycle::{
        PublishingGrid, acknowledge, map, pending_public_entry, plan, presented, publish_next,
        raw_successor, sound, source_at, transact, transition,
    },
    modes::{
        Group, anchor_at_rate, owning_deck_with_parent, parent_id, parent_stamp, parent_update,
        rate, sync_at,
    },
    preparation::asset_grid,
};
use crate::{
    AlignmentSource, GroupState, ReplanCause, SourceChange, SyncAdmission, SyncError,
    SyncExecutionReject, SyncGroup, SyncIntent, SyncMember, SyncMode, SyncOperation,
    SyncOperationId, SyncPreparation, SyncReceipt, SyncStatusSnapshot, owner::timeline::Custodian,
};

/// Output frames one beat of the 120 BPM parent lasts at 48 kHz.
const BEAT_FRAMES: i64 = 24_000;

/// Installs `preparation` and has its executor reject it for `reason`.
fn missed(
    group: &mut Group,
    preparation: &SyncPreparation,
    reason: SyncExecutionReject,
) -> SyncStatusSnapshot {
    let _ = acknowledge(group, SyncReceipt::Installed(preparation.stamp()));
    acknowledge(
        group,
        SyncReceipt::Rejected {
            stamp: preparation.stamp(),
            reason,
        },
    )
}

/// The Host's fresh observation of a silent track, from `activation` on.
fn replan(
    deck: BeatGridId,
    missed: &SyncPreparation,
    activation: i64,
) -> SyncOperation<super::TestGroup> {
    SyncOperation::Replan {
        target: deck,
        operation: missed.stamp().operation(),
        load: missed.stamp().load(),
        transport: missed.stamp().transport(),
        source: AlignmentSource::Prepared(AssetFrame::default()),
        activation: SessionFrame::new(activation),
    }
}

fn activation(preparation: &SyncPreparation) -> i64 {
    i64::from(preparation.activation().1)
}

/// The one preparation a replan issues.
fn replanned(admission: SyncAdmission) -> SyncPreparation {
    let transition = transition(admission);
    let [issued] = transition.issued() else {
        panic!("a replan issues one preparation, got {transition:?}");
    };
    issued.clone()
}

/// A Host-synced deck whose track grid publishes new revisions.
fn publishing_deck_with_parent() -> (Group, PublishingGrid) {
    let deck = BeatGridId::allocate().expect("deck id");
    let track = BeatGridId::allocate().expect("track id");
    let live = PublishingGrid::new(asset_grid(track, 960_000, 24_000));
    let mut group: Group = GroupState::owning(
        deck,
        rate(48_000),
        SessionEpoch::new(0),
        SyncMember::Grid {
            alignment: None,
            grid: Box::new(live.clone()),
        },
    );
    group
        .accept_parent(parent_update(
            parent_stamp(parent_id(), 1),
            anchor_at_rate(2.0, 48_000),
        ))
        .expect("host beat grid");
    (group, live)
}

#[kithara::test]
fn a_late_entry_waits_for_its_host_then_presents_at_the_next_beat() {
    let (mut group, _, entry) = pending_public_entry();
    let deck = group.id();
    let operation = entry.stamp().operation();

    let status = missed(&mut group, &entry, SyncExecutionReject::Late);

    assert_eq!(
        status,
        SyncStatusSnapshot::Replanning {
            operation,
            topology: entry.stamp().topology(),
            load: entry.stamp().load(),
            cause: ReplanCause::Missed(SyncExecutionReject::Late),
        }
    );
    assert_eq!(group.mode(), SyncMode::HostSync);
    assert_eq!(
        group.before_entry.map(|(custodian, _)| custodian),
        Some(Custodian::Decision(operation)),
        "the waiting decision keeps the prior timeline in custody"
    );

    let next = replanned(transact(
        &mut group,
        replan(deck, &entry, activation(&entry) + 1),
    ));
    assert_eq!(activation(&next), activation(&entry) + BEAT_FRAMES);
    assert!(matches!(
        group.status(),
        SyncStatusSnapshot::Prepared { .. }
    ));
    assert!(matches!(
        sound(&mut group, &next),
        SyncStatusSnapshot::Locked { .. }
    ));
    assert!(group.before_entry.is_none());
}

#[kithara::test]
fn a_second_miss_ends_the_entry_and_restores_the_prior_mode() {
    let (mut group, _, entry) = pending_public_entry();
    let deck = group.id();
    let _ = missed(&mut group, &entry, SyncExecutionReject::ControlBusy);
    let next = replanned(transact(
        &mut group,
        replan(deck, &entry, activation(&entry) + 1),
    ));

    let status = missed(&mut group, &next, SyncExecutionReject::Late);

    assert_eq!(
        status,
        SyncStatusSnapshot::Rejected {
            operation: next.stamp().operation(),
            topology: next.stamp().topology(),
            reason: SyncExecutionReject::Late,
        }
    );
    assert_eq!(group.mode(), SyncMode::Off);
    assert!(group.pending.is_empty());
    assert!(group.before_entry.is_none());
}

#[kithara::test]
fn a_miss_that_is_not_transient_ends_the_entry_at_once() {
    let (mut group, _, entry) = pending_public_entry();

    let status = missed(&mut group, &entry, SyncExecutionReject::Media);

    assert_eq!(
        status,
        SyncStatusSnapshot::Rejected {
            operation: entry.stamp().operation(),
            topology: entry.stamp().topology(),
            reason: SyncExecutionReject::Media,
        }
    );
    assert_eq!(group.mode(), SyncMode::Off);
    assert!(group.pending.is_empty());
}

#[kithara::test]
fn a_missed_launch_window_is_not_planned_again() {
    let (mut group, track, _) = pending_public_entry();
    let launch = raw_successor(&mut group, track);

    let status = missed(&mut group, &launch, SyncExecutionReject::Late);

    assert!(matches!(
        status,
        SyncStatusSnapshot::Rejected {
            reason: SyncExecutionReject::Late,
            ..
        }
    ));
    assert_eq!(group.mode(), SyncMode::Off);
}

#[kithara::test]
fn an_abandoned_replan_ends_the_entry_rejected() {
    let (mut group, _, entry) = pending_public_entry();
    let deck = group.id();
    let operation = entry.stamp().operation();
    let _ = missed(&mut group, &entry, SyncExecutionReject::ControlBusy);

    let admission = transact(
        &mut group,
        SyncOperation::AbandonReplan {
            target: deck,
            operation,
        },
    );

    assert!(transition(admission).issued().is_empty());
    assert_eq!(group.mode(), SyncMode::Off);
    assert!(group.pending.is_empty());
    assert_eq!(
        group.status(),
        SyncStatusSnapshot::Rejected {
            operation,
            topology: entry.stamp().topology(),
            reason: SyncExecutionReject::ControlBusy,
        }
    );
}

#[kithara::test]
fn a_newer_intent_supersedes_the_decision_waiting_to_be_planned_again() {
    let (mut group, _, entry) = pending_public_entry();
    let deck = group.id();
    let _ = missed(&mut group, &entry, SyncExecutionReject::Late);

    let _ = transact(
        &mut group,
        sync_at(deck, SyncIntent::AlignNow, SessionFrame::new(30_000)),
    );
    let pending = group.pending.clone();
    let next_operation = group.next_operation;

    let rejected = group
        .transact(replan(deck, &entry, 30_000))
        .expect_err("the superseded decision no longer waits");
    assert_eq!(
        rejected.error(),
        &SyncError::NotReplanning {
            operation: entry.stamp().operation(),
        }
    );
    assert_eq!(group.pending, pending);
    assert_eq!(group.next_operation, next_operation);
    assert!(matches!(
        group.status(),
        SyncStatusSnapshot::Prepared { .. }
    ));
}

#[kithara::test]
fn a_replan_for_another_load_changes_nothing() {
    let (mut group, _, entry) = pending_public_entry();
    let deck = group.id();
    let _ = missed(&mut group, &entry, SyncExecutionReject::Late);
    let pending = group.pending.clone();
    let reloaded = entry.stamp().load().checked_next().expect("load");

    let rejected = group
        .transact(SyncOperation::Replan {
            target: deck,
            operation: entry.stamp().operation(),
            load: reloaded,
            transport: entry.stamp().transport(),
            source: AlignmentSource::Prepared(AssetFrame::default()),
            activation: SessionFrame::new(activation(&entry) + 1),
        })
        .expect_err("a new load needs a new public decision");
    assert!(matches!(rejected.error(), SyncError::LoadMismatch { .. }));
    assert_eq!(group.pending, pending);
}

#[kithara::test]
fn a_presentation_on_a_refined_track_grid_waits_to_catch_up() {
    let (mut group, live) = publishing_deck_with_parent();
    let deck = group.id();
    let entry = replanned(transact(
        &mut group,
        sync_at(deck, SyncIntent::Enable, SessionFrame::new(2_048)),
    ));
    let _ = acknowledge(&mut group, SyncReceipt::Installed(entry.stamp()));
    let refined = publish_next(&live).stamp();

    let _ = acknowledge(&mut group, SyncReceipt::Armed(entry.stamp()));
    let status = acknowledge(&mut group, presented(&entry));

    let SyncStatusSnapshot::Replanning { operation, .. } = status else {
        panic!("a presentation on a refined track grid catches up, got {status:?}");
    };
    assert!(operation > entry.stamp().operation());
    assert!(group.applied_of(live.id()).is_some());
    let frontier = activation(&entry) + 2_048;
    let caught_up = replanned(transact(
        &mut group,
        SyncOperation::Replan {
            target: deck,
            operation,
            load: entry.stamp().load(),
            transport: entry.stamp().transport(),
            source: AlignmentSource::Audible {
                frontier: PresentationFrontier::builder()
                    .warp_map(map(&entry))
                    .source(source_at(plan(&entry), frontier))
                    .output(SessionFrame::new(frontier))
                    .build(),
                speed: 1.0,
            },
            activation: SessionFrame::new(frontier + 2_048),
        },
    ));
    assert_eq!(caught_up.stamp().member(), refined);
    assert!(matches!(
        sound(&mut group, &caught_up),
        SyncStatusSnapshot::Locked { .. }
    ));
}

#[kithara::test]
fn a_track_grid_refined_before_the_claim_replans_the_entry() {
    let (mut group, live) = publishing_deck_with_parent();
    let deck = group.id();
    let entry = replanned(transact(
        &mut group,
        sync_at(deck, SyncIntent::Enable, SessionFrame::new(2_048)),
    ));
    let refined = publish_next(&live).stamp();
    assert_eq!(
        group.acknowledge(SyncReceipt::Installed(entry.stamp())),
        Err(SyncError::StaleGridRevision {
            current: refined,
            given: entry.stamp().member(),
        })
    );

    let status = acknowledge(
        &mut group,
        SyncReceipt::Rejected {
            stamp: entry.stamp(),
            reason: SyncExecutionReject::Cancelled,
        },
    );

    assert!(matches!(status, SyncStatusSnapshot::Replanning { .. }));
    let next = replanned(transact(
        &mut group,
        replan(deck, &entry, activation(&entry) + 1),
    ));
    assert_eq!(next.stamp().member(), refined);
}

#[kithara::test]
fn a_refined_track_grid_does_not_make_a_media_refusal_retryable() {
    let (mut group, live) = publishing_deck_with_parent();
    let deck = group.id();
    let entry = replanned(transact(
        &mut group,
        sync_at(deck, SyncIntent::Enable, SessionFrame::new(2_048)),
    ));
    let _ = publish_next(&live);

    let status = acknowledge(
        &mut group,
        SyncReceipt::Rejected {
            stamp: entry.stamp(),
            reason: SyncExecutionReject::Media,
        },
    );

    assert_eq!(
        status,
        SyncStatusSnapshot::Rejected {
            operation: entry.stamp().operation(),
            topology: entry.stamp().topology(),
            reason: SyncExecutionReject::Media,
        }
    );
    assert_eq!(group.mode(), SyncMode::Off);
    assert!(group.pending.is_empty());
}

#[kithara::test]
fn a_parent_update_leaves_a_sounding_decks_missed_retarget_to_its_host() {
    let (mut group, track, parent) = owning_deck_with_parent();
    let deck = group.id();
    let entry = replanned(transact(
        &mut group,
        sync_at(deck, SyncIntent::Enable, SessionFrame::new(2_048)),
    ));
    let _ = sound(&mut group, &entry);
    let moved = group
        .accept_parent(parent_update(
            parent_stamp(parent, 2),
            anchor_at_rate(2.2, 48_000),
        ))
        .expect("a faster Host grid");
    let [retarget] = moved.issued() else {
        panic!("the sounding deck follows its Host, got {moved:?}");
    };
    let SyncStatusSnapshot::Replanning { operation, .. } =
        missed(&mut group, retarget, SyncExecutionReject::Late)
    else {
        panic!("a missed retarget waits for its Host");
    };

    let moved = group
        .accept_parent(parent_update(
            parent_stamp(parent, 3),
            anchor_at_rate(2.4, 48_000),
        ))
        .expect("a faster Host grid again");

    assert!(
        moved.issued().is_empty(),
        "only the Host plans the waiting retarget once more"
    );
    assert!(matches!(
        group.status(),
        SyncStatusSnapshot::Replanning { operation: waiting, .. } if waiting == operation
    ));
    let frontier = activation(retarget) + 2_048;
    let next = replanned(transact(
        &mut group,
        SyncOperation::Replan {
            target: deck,
            operation,
            load: entry.stamp().load(),
            transport: entry.stamp().transport(),
            source: AlignmentSource::Audible {
                frontier: PresentationFrontier::builder()
                    .warp_map(map(&entry))
                    .source(source_at(plan(&entry), frontier))
                    .output(SessionFrame::new(frontier))
                    .build(),
                speed: 1.0,
            },
            activation: SessionFrame::new(frontier + 2_048),
        },
    ));
    assert!(matches!(
        missed(&mut group, &next, SyncExecutionReject::Late),
        SyncStatusSnapshot::Rejected {
            reason: SyncExecutionReject::Late,
            ..
        }
    ));
    assert!(
        group.applied_of(track).is_some(),
        "the entry's map sounds on"
    );
}

#[kithara::test]
fn a_changed_source_forgets_the_rejection_placed_against_it() {
    let (mut group, track, entry) = pending_public_entry();
    let _ = missed(&mut group, &entry, SyncExecutionReject::Media);

    let _ = transact(
        &mut group,
        SyncOperation::InvalidateSource {
            target: track,
            change: SourceChange::Discontinuity,
        },
    );

    assert!(matches!(group.status(), SyncStatusSnapshot::Off { .. }));
}

#[kithara::test]
fn a_withdrawn_track_takes_its_rejection_along() {
    let (mut group, track, entry) = pending_public_entry();
    let _ = missed(&mut group, &entry, SyncExecutionReject::Media);

    let _ = transact(
        &mut group,
        SyncOperation::WithdrawQuiescedMember { target: track },
    );

    assert!(matches!(group.status(), SyncStatusSnapshot::Off { .. }));
}

/// A deck sounding its Host through `entry`, and the operation of the
/// decision a discontinuity of its track then leaves waiting.
fn broken_sounding_deck() -> (Group, BeatGridId, SyncPreparation, SyncOperationId) {
    let (mut group, track, _) = owning_deck_with_parent();
    let deck = group.id();
    let entry = replanned(transact(
        &mut group,
        sync_at(deck, SyncIntent::Enable, SessionFrame::new(2_048)),
    ));
    let _ = sound(&mut group, &entry);
    let _ = transact(
        &mut group,
        SyncOperation::InvalidateSource {
            target: track,
            change: SourceChange::Discontinuity,
        },
    );
    let SyncStatusSnapshot::Replanning {
        operation,
        cause: ReplanCause::Break,
        ..
    } = group.status()
    else {
        panic!(
            "a broken deck waits to be planned again, got {:?}",
            group.status()
        );
    };
    (group, track, entry, operation)
}

/// The Host's fresh observation of a track playing by hand from `source`
/// at `output`.
fn by_hand(
    deck: BeatGridId,
    operation: SyncOperationId,
    entry: &SyncPreparation,
    source: u64,
    output: i64,
) -> SyncOperation<super::TestGroup> {
    SyncOperation::Replan {
        target: deck,
        operation,
        load: entry.stamp().load(),
        transport: entry.stamp().transport(),
        source: AlignmentSource::Audible {
            frontier: PresentationFrontier::builder()
                .source(source)
                .output(SessionFrame::new(output))
                .build(),
            speed: 1.0,
        },
        activation: SessionFrame::new(output + 2_048),
    }
}

#[kithara::test]
fn a_break_of_a_sounding_deck_enters_afresh_from_where_it_plays_by_hand() {
    let (mut group, track, entry, operation) = broken_sounding_deck();
    let deck = group.id();

    assert!(operation > entry.stamp().operation());
    assert!(
        group.applied_of(track).is_none(),
        "the break released the map"
    );
    assert_eq!(group.mode(), SyncMode::HostSync);
    let output = activation(&entry) + BEAT_FRAMES;
    let next = replanned(transact(
        &mut group,
        by_hand(deck, operation, &entry, 144_000, output),
    ));
    assert!(matches!(
        sound(&mut group, &next),
        SyncStatusSnapshot::Locked { .. }
    ));
}

#[kithara::test]
fn a_timing_change_leaves_a_sounding_deck_on_its_map() {
    let (mut group, track, _) = owning_deck_with_parent();
    let deck = group.id();
    let entry = replanned(transact(
        &mut group,
        sync_at(deck, SyncIntent::Enable, SessionFrame::new(2_048)),
    ));
    let _ = sound(&mut group, &entry);

    let _ = transact(
        &mut group,
        SyncOperation::InvalidateSource {
            target: track,
            change: SourceChange::Timing,
        },
    );

    assert!(matches!(group.status(), SyncStatusSnapshot::Locked { .. }));
    assert!(group.pending.is_empty());
}

#[kithara::test]
fn a_second_break_waits_in_place_of_the_first() {
    let (mut group, track, entry, first) = broken_sounding_deck();
    let deck = group.id();

    let _ = transact(
        &mut group,
        SyncOperation::InvalidateSource {
            target: track,
            change: SourceChange::Discontinuity,
        },
    );

    let SyncStatusSnapshot::Replanning {
        operation: second,
        cause: ReplanCause::Break,
        ..
    } = group.status()
    else {
        panic!("the deck still waits after a second break");
    };
    assert!(second > first);
    let rejected = group
        .transact(by_hand(deck, first, &entry, 144_000, activation(&entry)))
        .expect_err("the first break waits no longer");
    assert_eq!(
        rejected.error(),
        &SyncError::NotReplanning { operation: first }
    );
}

#[kithara::test]
fn a_speed_change_keeps_a_break_waiting() {
    let (mut group, track, entry, operation) = broken_sounding_deck();
    let deck = group.id();

    let _ = transact(
        &mut group,
        SyncOperation::InvalidateSource {
            target: track,
            change: SourceChange::Timing,
        },
    );

    assert!(matches!(
        group.status(),
        SyncStatusSnapshot::Replanning {
            operation: waiting,
            cause: ReplanCause::Break,
            ..
        } if waiting == operation
    ));
    let output = activation(&entry) + BEAT_FRAMES;
    let next = replanned(transact(
        &mut group,
        by_hand(deck, operation, &entry, 144_000, output),
    ));
    assert!(matches!(
        sound(&mut group, &next),
        SyncStatusSnapshot::Locked { .. }
    ));
}

#[kithara::test]
fn an_abandoned_break_leaves_the_deck_synced_without_a_rejection() {
    let (mut group, _, _, operation) = broken_sounding_deck();
    let deck = group.id();

    let _ = transact(
        &mut group,
        SyncOperation::AbandonReplan {
            operation,
            target: deck,
        },
    );

    assert!(!matches!(
        group.status(),
        SyncStatusSnapshot::Rejected { .. } | SyncStatusSnapshot::Replanning { .. }
    ));
    assert_eq!(group.mode(), SyncMode::HostSync);
}
