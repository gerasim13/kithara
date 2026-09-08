use std::num::NonZeroU32;

use kithara_test_utils::kithara;
use kithara_warp::{
    AlignmentSource, BeatGrid, BeatGridId, BeatsPerMinute, SessionAnchor, SessionBeat,
    SessionEpoch, SessionFrame, SyncError, SyncGroup, SyncIntent, SyncMemberKind, SyncMode,
    SyncOperation,
};

use super::GroupState;
use crate::{player::PlayerMember, sync::TempoSource};

#[kithara::test]
#[case::tempo(None)]
#[case::enable(Some(SyncIntent::Enable))]
#[case::free(Some(SyncIntent::Free))]
fn rejected_state_change_preserves_mode_and_tempo(#[case] intent: Option<SyncIntent>) {
    let mut group = GroupState::<PlayerMember>::unavailable(
        BeatGridId::allocate().expect("group identity"),
        NonZeroU32::new(48_000).expect("sample rate"),
        SessionEpoch::new(0),
        SyncMemberKind::Grid,
        SyncMode::LocalSync,
    );
    let tempo = TempoSource::Local(BeatsPerMinute::try_from(90.0).expect("local tempo"));
    group.tempo = tempo;
    group.next_operation = None;
    let operation = match intent {
        Some(intent) => SyncOperation::Sync {
            target: group.id(),
            load: group.generations.0,
            transport: group.generations.1,
            source: AlignmentSource::Prepared,
            activation: SessionFrame::new(0),
            intent,
        },
        None => SyncOperation::Tempo {
            target: group.id(),
            tempo: BeatsPerMinute::try_from(120.0).expect("requested tempo"),
        },
    };
    let (error, _): (SyncError, SyncOperation<PlayerMember>) = group
        .transact(operation)
        .expect_err("operation identities are exhausted")
        .into();
    assert!(matches!(error, SyncError::OperationIdExhausted { .. }));
    assert_eq!(group.mode, SyncMode::LocalSync, "rejected mode changed");
    assert_eq!(group.tempo, tempo, "rejected tempo changed");
}

#[kithara::test]
fn rejected_parent_anchor_preserves_the_committed_grid_and_anchor() {
    let mut group = GroupState::<PlayerMember>::unavailable(
        BeatGridId::allocate().expect("group identity"),
        NonZeroU32::new(48_000).expect("sample rate"),
        SessionEpoch::new(0),
        SyncMemberKind::Grid,
        SyncMode::HostSync,
    );
    let anchor = |rate| {
        SessionAnchor::new(
            SessionFrame::new(0),
            SessionBeat::new(0.0).expect("beat"),
            2.0,
            NonZeroU32::new(rate).expect("sample rate"),
        )
        .expect("session anchor")
    };
    let committed = anchor(48_000);
    group
        .publish_session_anchor(committed)
        .expect("first anchor");
    let stamp = group.snapshot().stamp();
    assert!(matches!(
        group.publish_session_anchor(anchor(44_100)),
        Err(SyncError::GridAxisChanged { .. })
    ));
    assert_eq!(group.snapshot().stamp(), stamp);
    assert_eq!(group.parent_anchor, Some(committed));
}
