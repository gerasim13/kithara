use kithara_signal::{SessionEpoch, SessionFrame, TransportRevision};
use kithara_test_utils::kithara;
use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridQuery, PresentationFrontier, SessionAxis, WarpMapRevision,
    WarpPlan,
};
use num_traits::ToPrimitive;

use super::{
    modes::{Group, attach_group, group_in, nested, rate, sync_at, synced_deck, tempo_at},
    preparation::{asset_grid, attach_grid, cue, prepare, prepare_in, replace_grid, window},
    refresh::{pending_members, prepared},
};
use crate::{
    AlignmentSource, SessionAxisUpdate, SyncAdmission, SyncApplied, SyncEffect, SyncError,
    SyncExecutionReject, SyncExecutionStamp, SyncGroup, SyncIntent, SyncMemberKind, SyncMode,
    SyncOperation, SyncOperationId, SyncPreparation, SyncReceipt, SyncStatusSnapshot,
    TopologyOperation,
};

/// A deck at 120 BPM holding one track grid.
fn deck_with_track() -> (Group, BeatGridId) {
    let mut group = synced_deck();
    let track = BeatGridId::allocate().expect("grid id");
    attach_grid(&mut group, asset_grid(track, 960_000, 24_000));
    (group, track)
}

fn launched(group: &mut Group, track: BeatGridId, earliest: i64) -> SyncPreparation {
    match prepare(group, track, cue(0), earliest) {
        SyncAdmission::Prepared(preparation) => preparation,
        admission => panic!("expected a prepared member, got {admission:?}"),
    }
}

fn plan(preparation: &SyncPreparation) -> &WarpPlan {
    let SyncEffect::Projection { plan, .. } = preparation.effect() else {
        panic!("expected a projection, got {preparation:?}");
    };
    plan
}

fn map(preparation: &SyncPreparation) -> WarpMapRevision {
    plan(preparation).activation().revision()
}

/// The recording frame `plan` reaches at `output`, rounded as a renderer
/// consumes it.
fn source_at(plan: &WarpPlan, output: i64) -> u64 {
    let BeatGridQuery::Resolved(source) = plan.source_at(SessionFrame::new(output)) else {
        panic!("the plan covers frame {output}");
    };
    f64::from(source).round().to_u64().expect("source frame")
}

/// The receipt of `preparation` sounding exactly from its activation.
fn presented(preparation: &SyncPreparation) -> SyncReceipt {
    let (warp_map, activation) = preparation.activation();
    let source = match preparation.effect() {
        SyncEffect::Projection { plan, .. } => plan.activation().source(),
        SyncEffect::Handoff { source, .. } => {
            f64::from(*source).round().to_u64().expect("source frame")
        }
    };
    SyncReceipt::Presented(
        SyncApplied::builder()
            .stamp(preparation.stamp())
            .frontier(
                PresentationFrontier::builder()
                    .maybe_warp_map(warp_map)
                    .source(source)
                    .output(activation)
                    .build(),
            )
            .build(),
    )
}

fn acknowledge(group: &mut Group, receipt: SyncReceipt) -> SyncStatusSnapshot {
    group.acknowledge(receipt).expect("the receipt is recorded")
}

/// Installs, arms and presents `preparation`.
fn sound(group: &mut Group, preparation: &SyncPreparation) -> SyncStatusSnapshot {
    let _ = acknowledge(group, SyncReceipt::Installed(preparation.stamp()));
    let _ = acknowledge(group, SyncReceipt::Armed(preparation.stamp()));
    acknowledge(group, presented(preparation))
}

/// The same facts as `stamp` under another transport revision.
fn restamped(stamp: SyncExecutionStamp, transport: TransportRevision) -> SyncExecutionStamp {
    SyncExecutionStamp::new(
        stamp.operation(),
        stamp.member(),
        stamp.group(),
        stamp.topology(),
        stamp.load(),
        transport,
    )
}

fn operation(admission: &SyncAdmission) -> SyncOperationId {
    match admission {
        SyncAdmission::StateChanged { operation, .. } => *operation,
        admission => panic!("expected a state change, got {admission:?}"),
    }
}

fn transact(group: &mut Group, operation: SyncOperation<super::TestGroup>) -> SyncAdmission {
    group
        .transact(operation)
        .expect("the group admits the operation")
}

/// Commits 130 BPM on the deck from frame 96 000 on.
fn commit_tempo(group: &mut Group) -> SyncAdmission {
    let id = group.id();
    transact(group, tempo_at(id, 130.0, SessionFrame::new(96_000)))
}

fn free_at(group: &mut Group, frame: i64) -> SyncAdmission {
    let id = group.id();
    transact(
        group,
        sync_at(id, SyncIntent::Free, SessionFrame::new(frame)),
    )
}

#[kithara::test]
fn a_preparation_is_installed_armed_and_presented_before_it_locks() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let (warp_map, activation) = preparation.activation();
    let issued = SyncStatusSnapshot::Prepared {
        operation: preparation.stamp().operation(),
        topology: preparation.stamp().topology(),
        warp_map,
        activation,
    };

    assert_eq!(
        acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp())),
        issued
    );
    assert_eq!(
        acknowledge(&mut group, SyncReceipt::Armed(preparation.stamp())),
        issued
    );
    let SyncStatusSnapshot::Locked {
        applied,
        phase_error_frames,
    } = acknowledge(&mut group, presented(&preparation))
    else {
        panic!("a presented map on the current grid is locked");
    };

    assert_eq!(applied.stamp(), preparation.stamp());
    assert!(phase_error_frames.abs() < 1.0, "{phase_error_frames}");
    assert!(group.pending.is_empty());
}

#[kithara::test]
fn a_repeated_receipt_is_a_duplicate_and_changes_nothing() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let operation = preparation.stamp().operation();
    let _ = acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp()));
    let installed = group.pending.clone();

    assert_eq!(
        group.acknowledge(SyncReceipt::Installed(preparation.stamp())),
        Err(SyncError::DuplicateAcknowledgement { operation })
    );
    assert_eq!(group.pending, installed);

    let _ = acknowledge(&mut group, SyncReceipt::Armed(preparation.stamp()));
    let locked = acknowledge(&mut group, presented(&preparation));
    assert_eq!(
        group.acknowledge(presented(&preparation)),
        Err(SyncError::DuplicateAcknowledgement { operation })
    );
    assert_eq!(group.status(), locked);
}

#[kithara::test]
fn a_receipt_skipping_a_phase_is_refused() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let operation = preparation.stamp().operation();
    let issued = group.pending.clone();

    assert_eq!(
        group.acknowledge(SyncReceipt::Armed(preparation.stamp())),
        Err(SyncError::ReceiptOutOfOrder { operation })
    );
    assert_eq!(
        group.acknowledge(presented(&preparation)),
        Err(SyncError::ReceiptOutOfOrder { operation })
    );
    assert_eq!(group.pending, issued);

    let _ = acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp()));
    let _ = acknowledge(&mut group, SyncReceipt::Armed(preparation.stamp()));
    assert_eq!(
        group.acknowledge(SyncReceipt::Rejected {
            stamp: preparation.stamp(),
            reason: SyncExecutionReject::Late,
        }),
        Err(SyncError::ReceiptOutOfOrder { operation })
    );
}

#[kithara::test]
fn a_rejected_launch_leaves_the_member_silent() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp()));

    let status = acknowledge(
        &mut group,
        SyncReceipt::Rejected {
            stamp: preparation.stamp(),
            reason: SyncExecutionReject::Geometry,
        },
    );

    assert_eq!(
        status,
        SyncStatusSnapshot::Off {
            topology: preparation.stamp().topology(),
        }
    );
    assert!(group.pending.is_empty());
}

#[kithara::test]
fn a_receipt_under_other_facts_or_an_older_operation_commits_nothing() {
    let (mut group, track) = deck_with_track();
    let first = launched(&mut group, track, 0);
    let second = launched(&mut group, track, 0);
    let held = group.pending.clone();

    assert_eq!(
        group.acknowledge(SyncReceipt::Installed(first.stamp())),
        Err(SyncError::StaleAcknowledgement {
            expected: second.stamp().operation(),
            given: first.stamp().operation(),
        })
    );
    let other = restamped(
        second.stamp(),
        TransportRevision::first().checked_next().expect("rev"),
    );
    assert_eq!(
        group.acknowledge(SyncReceipt::Installed(other)),
        Err(SyncError::ReceiptMismatch {
            expected: Box::new(second.stamp()),
            given: Box::new(other),
        })
    );
    assert_eq!(group.pending, held);
}

#[kithara::test]
fn a_receipt_for_a_member_that_left_the_group_is_refused() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let base = group.topology().expect("topology").stamp();
    let _ = transact(
        &mut group,
        SyncOperation::Topology {
            base,
            operations: Box::new([TopologyOperation::Detach { member: track }]),
        },
    );

    assert_eq!(
        group.acknowledge(SyncReceipt::Installed(preparation.stamp())),
        Err(SyncError::MemberNotFound {
            group_id: group.id(),
            member_id: track,
        })
    );
}

#[kithara::test]
fn a_receipt_reaches_the_nested_group_that_issued_it() {
    let (deck, track) = deck_with_track();
    let deck_id = deck.id();
    let mut root = group_in(SyncMode::Off, SyncMemberKind::Group);
    attach_group(&mut root, deck);
    let SyncAdmission::Prepared(preparation) = prepare(&mut root, track, cue(0), 0) else {
        panic!("the nested deck prepares its track");
    };

    let status = acknowledge(&mut root, SyncReceipt::Installed(preparation.stamp()));

    let (warp_map, activation) = preparation.activation();
    assert_eq!(
        status,
        SyncStatusSnapshot::Prepared {
            operation: preparation.stamp().operation(),
            topology: preparation.stamp().topology(),
            warp_map,
            activation,
        }
    );
    assert_eq!(
        root.status(),
        SyncStatusSnapshot::Off {
            topology: root.topology().expect("topology").stamp(),
        }
    );
    assert_eq!(nested(&root, &[deck_id], SyncGroup::status), status);
}

#[kithara::test]
fn a_sounding_member_cannot_launch_again() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);

    assert_eq!(
        prepare_in(&mut group, track, cue(0), window(0, i64::MAX)),
        Err(SyncError::MemberAudible { member_id: track })
    );
    let unmapped = AlignmentSource::Audible(
        PresentationFrontier::builder()
            .source(0)
            .output(SessionFrame::new(0))
            .build(),
    );
    assert_eq!(
        prepare_in(&mut group, track, unmapped, window(0, i64::MAX)),
        Err(SyncError::AudibleMapMismatch {
            member_id: track,
            expected: Some(map(&preparation)),
            given: None,
        })
    );
}

#[kithara::test]
fn an_audible_retarget_continues_the_recording_its_map_plays() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);
    let heard = AlignmentSource::Audible(
        PresentationFrontier::builder()
            .warp_map(map(&preparation))
            .source(source_at(plan(&preparation), 48_000))
            .output(SessionFrame::new(48_000))
            .build(),
    );

    let SyncAdmission::Prepared(retarget) =
        prepare_in(&mut group, track, heard, window(96_000, i64::MAX))
            .expect("a sounding member retargets")
    else {
        panic!("the retarget is prepared");
    };

    let SyncEffect::Projection {
        replaces,
        plan: next,
        ..
    } = retarget.effect()
    else {
        panic!("a retarget is a projection");
    };
    assert_eq!(*replaces, Some(map(&preparation)));
    assert_eq!(next.activation().output(), SessionFrame::new(96_000));
    assert_eq!(
        next.activation().source(),
        source_at(plan(&preparation), 96_000)
    );
}

#[kithara::test]
fn a_rejected_retarget_returns_the_member_to_its_applied_map() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let locked = sound(&mut group, &preparation);
    let _ = commit_tempo(&mut group);
    let retarget = prepared(&group, track);

    let status = acknowledge(
        &mut group,
        SyncReceipt::Rejected {
            stamp: retarget.stamp(),
            reason: SyncExecutionReject::Late,
        },
    );

    let SyncStatusSnapshot::Locked { applied, .. } = locked else {
        panic!("the first map locked");
    };
    let SyncStatusSnapshot::Converging { applied: still, .. } = status else {
        panic!("the old map sounds against a grid that moved on, got {status:?}");
    };
    assert_eq!(still, applied);
    assert!(group.pending.is_empty());
}

#[kithara::test]
fn a_tempo_commit_retargets_every_sounding_member_and_carries_the_silent_one() {
    let mut group = synced_deck();
    let tracks: Vec<BeatGridId> = (0..3)
        .map(|_| BeatGridId::allocate().expect("grid id"))
        .collect();
    for track in &tracks {
        attach_grid(&mut group, asset_grid(*track, 960_000, 24_000));
    }
    let first = launched(&mut group, tracks[0], 0);
    let second = launched(&mut group, tracks[1], 0);
    let _ = sound(&mut group, &first);
    let _ = sound(&mut group, &second);
    let silent = launched(&mut group, tracks[2], 200_000);

    let admission = commit_tempo(&mut group);

    assert_eq!(pending_members(&group), tracks);
    let tempo = operation(&admission);
    for (applied, expected) in [
        (&first, tempo.checked_next()),
        (
            &second,
            tempo.checked_next().and_then(SyncOperationId::checked_next),
        ),
    ] {
        let retarget = prepared(&group, applied.stamp().member().grid_id());
        assert_eq!(Some(retarget.stamp().operation()), expected);
        assert_eq!(retarget.stamp().group(), group.snapshot().stamp());
        let SyncEffect::Projection {
            replaces,
            plan: next,
            ..
        } = retarget.effect()
        else {
            panic!("a retarget is a projection");
        };
        assert_eq!(*replaces, Some(map(applied)));
        assert_eq!(next.activation().output(), SessionFrame::new(96_000));
        assert_eq!(next.activation().source(), source_at(plan(applied), 96_000));
    }
    let carried = prepared(&group, tracks[2]);
    assert_eq!(carried.stamp().operation(), silent.stamp().operation());
    let SyncEffect::Projection { replaces, .. } = carried.effect() else {
        panic!("a launch is a projection");
    };
    assert_eq!(*replaces, None);
}

#[kithara::test]
fn a_presented_retarget_locks_the_member_on_its_new_map() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);
    let _ = commit_tempo(&mut group);
    let retarget = prepared(&group, track);

    let SyncStatusSnapshot::Locked { applied, .. } = sound(&mut group, &retarget) else {
        panic!("the retarget sounds on the current grid");
    };

    assert_eq!(applied.stamp(), retarget.stamp());
    assert_eq!(
        group.applied_of(track).map(|lane| lane.map()),
        Some(map(&retarget))
    );
}

#[kithara::test]
fn an_armed_preparation_survives_a_tempo_commit_and_then_converges() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp()));
    let _ = acknowledge(&mut group, SyncReceipt::Armed(preparation.stamp()));
    let armed = group.pending.clone();

    let _ = commit_tempo(&mut group);
    assert_eq!(group.pending, armed);
    assert_eq!(
        prepare_in(&mut group, track, cue(0), window(0, i64::MAX)),
        Err(SyncError::ArmedOperation {
            member_id: track,
            operation: preparation.stamp().operation(),
        })
    );

    let status = acknowledge(&mut group, presented(&preparation));
    assert!(
        matches!(status, SyncStatusSnapshot::Converging { applied, .. } if applied.stamp() == preparation.stamp()),
        "{status:?}"
    );
}

#[kithara::test]
fn a_late_receipt_of_a_moved_preparation_does_not_complete_its_successor() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 200_000);
    let _ = acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp()));
    let _ = commit_tempo(&mut group);
    let moved = prepared(&group, track);
    assert_eq!(moved.stamp().operation(), preparation.stamp().operation());
    let held = group.pending.clone();

    assert_eq!(
        group.acknowledge(SyncReceipt::Armed(preparation.stamp())),
        Err(SyncError::ReceiptMismatch {
            expected: Box::new(moved.stamp()),
            given: Box::new(preparation.stamp()),
        })
    );
    assert_eq!(group.pending, held);
}

#[kithara::test]
fn replacing_a_member_grid_keeps_what_already_sounds_or_is_armed() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);
    let retarget = {
        let _ = commit_tempo(&mut group);
        prepared(&group, track)
    };
    let _ = acknowledge(&mut group, SyncReceipt::Installed(retarget.stamp()));
    let _ = acknowledge(&mut group, SyncReceipt::Armed(retarget.stamp()));
    let armed = group.pending.clone();

    replace_grid(&mut group, asset_grid(track, 960_000, 24_000));

    assert_eq!(group.pending, armed);
    assert_eq!(
        group.applied_of(track).map(|lane| lane.map()),
        Some(map(&preparation))
    );
}

#[kithara::test]
fn leaving_the_timeline_hands_every_sounding_member_off_where_its_map_plays() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);

    let admission = free_at(&mut group, 96_000);

    let handoff = prepared(&group, track);
    assert_eq!(handoff.stamp().operation(), operation(&admission));
    assert_eq!(handoff.stamp().group(), group.snapshot().stamp());
    let SyncEffect::Handoff {
        replaces,
        source,
        activation,
    } = handoff.effect()
    else {
        panic!("leaving the timeline hands the member off");
    };
    assert_eq!(*replaces, map(&preparation));
    assert_eq!(*activation, SessionFrame::new(96_000));
    assert_eq!(
        f64::from(*source).round().to_u64(),
        Some(source_at(plan(&preparation), 96_000))
    );
    assert_eq!(
        group.status(),
        SyncStatusSnapshot::Prepared {
            operation: operation(&admission),
            topology: handoff.stamp().topology(),
            warp_map: None,
            activation: SessionFrame::new(96_000),
        }
    );

    let status = sound(&mut group, &handoff);

    assert_eq!(
        status,
        SyncStatusSnapshot::Off {
            topology: handoff.stamp().topology(),
        }
    );
    assert!(group.applied_of(track).is_none());
}

#[kithara::test]
fn a_rejected_handoff_drops_only_itself() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);
    let _ = free_at(&mut group, 96_000);
    let handoff = prepared(&group, track);

    let status = acknowledge(
        &mut group,
        SyncReceipt::Rejected {
            stamp: handoff.stamp(),
            reason: SyncExecutionReject::Late,
        },
    );

    assert!(group.pending.is_empty());
    assert!(
        matches!(status, SyncStatusSnapshot::Converging { applied, .. } if applied.stamp() == preparation.stamp()),
        "{status:?}"
    );
}

#[kithara::test]
fn a_handoff_receipt_is_consumed_once_and_goes_stale_after_the_next_release() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);
    let _ = free_at(&mut group, 96_000);
    let handoff = prepared(&group, track);
    let _ = acknowledge(&mut group, SyncReceipt::Installed(handoff.stamp()));
    let other = restamped(
        handoff.stamp(),
        TransportRevision::first().checked_next().expect("rev"),
    );

    assert_eq!(
        group.acknowledge(SyncReceipt::Installed(handoff.stamp())),
        Err(SyncError::DuplicateAcknowledgement {
            operation: handoff.stamp().operation(),
        })
    );
    assert_eq!(
        group.acknowledge(SyncReceipt::Armed(other)),
        Err(SyncError::ReceiptMismatch {
            expected: Box::new(handoff.stamp()),
            given: Box::new(other),
        })
    );

    let id = group.id();
    let _ = transact(
        &mut group,
        sync_at(id, SyncIntent::Enable, SessionFrame::new(96_000)),
    );
    let released = free_at(&mut group, 120_000);

    assert_eq!(
        group.acknowledge(SyncReceipt::Armed(handoff.stamp())),
        Err(SyncError::StaleAcknowledgement {
            expected: operation(&released),
            given: handoff.stamp().operation(),
        })
    );
}

#[kithara::test]
fn an_armed_member_keeps_the_timeline_from_being_left() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = acknowledge(&mut group, SyncReceipt::Installed(preparation.stamp()));
    let _ = acknowledge(&mut group, SyncReceipt::Armed(preparation.stamp()));
    let id = group.id();

    let refused = group
        .transact(sync_at(id, SyncIntent::Free, SessionFrame::new(96_000)))
        .map_err(|rejected| rejected.error().clone());

    assert_eq!(
        refused.map(|_| ()),
        Err(SyncError::ArmedOperation {
            member_id: track,
            operation: preparation.stamp().operation(),
        })
    );
    assert_eq!(group.mode(), SyncMode::LocalSync);
}

#[kithara::test]
fn a_new_session_epoch_drops_every_decision_and_applied_map() {
    let (mut group, track) = deck_with_track();
    let preparation = launched(&mut group, track, 0);
    let _ = sound(&mut group, &preparation);
    let _ = free_at(&mut group, 96_000);
    let handoff = prepared(&group, track);
    let _ = acknowledge(&mut group, SyncReceipt::Installed(handoff.stamp()));
    let _ = acknowledge(&mut group, SyncReceipt::Armed(handoff.stamp()));

    group
        .accept_axis(SessionAxisUpdate::new(SessionAxis::new(
            rate(48_000),
            SessionEpoch::new(1),
        )))
        .expect("a group follows the next session epoch");

    assert!(group.pending.is_empty());
    assert!(group.applied_of(track).is_none());
    assert_eq!(
        group.acknowledge(presented(&handoff)),
        Err(SyncError::NoPreparedOperation)
    );
    assert!(matches!(group.status(), SyncStatusSnapshot::Off { .. }));
}
