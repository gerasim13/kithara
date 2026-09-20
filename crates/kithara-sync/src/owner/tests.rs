use std::num::NonZeroU32;

use kithara_signal::{SessionEpoch, SessionFrame};
use kithara_test_utils::kithara;
use kithara_warp::{
    AssetAxis, BeatGrid, BeatGridId, BeatGridRevision, BeatGridSnapshot, BeatGridState,
    BeatGridUnavailable, MapAxis, SessionAnchor, SessionAxis, SessionBeat,
};

use crate::{
    GroupState, SyncAdmission, SyncApplied, SyncError, SyncGroup, SyncGroupSnapshot,
    SyncMemberKind, SyncOperation, SyncRejected, SyncStatusSnapshot,
};

/// Test-only recursive group that delegates to the real owner state.
///
/// The production nested-group representation lives in `kithara-play`, which
/// this crate must not depend on.
struct TestGroup(GroupState<Self>);

impl TestGroup {
    fn unavailable(
        id: BeatGridId,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
        member_kind: SyncMemberKind,
    ) -> Self {
        Self(GroupState::unavailable(id, sample_rate, epoch, member_kind))
    }

    delegate::delegate! {
        to self.0 {
            fn publish_grid(&mut self, candidate: BeatGridSnapshot) -> Result<(), SyncError>;
        }
    }
}

impl BeatGrid for TestGroup {
    delegate::delegate! {
        to self.0 {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for TestGroup {
    type NestedGroup = Self;

    delegate::delegate! {
        to self.0 {
            fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError>;
            fn status(&self) -> SyncStatusSnapshot;
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn transact(
                &mut self,
                operation: SyncOperation<Self>,
            ) -> Result<SyncAdmission, SyncRejected<Self>>;
        }
    }
}

fn session_grid(
    id: BeatGridId,
    revision: BeatGridRevision,
    epoch: SessionEpoch,
    beats_per_second: f64,
) -> BeatGridSnapshot {
    session_grid_at_rate(id, revision, epoch, beats_per_second, 48_000)
}

fn session_grid_at_rate(
    id: BeatGridId,
    revision: BeatGridRevision,
    epoch: SessionEpoch,
    beats_per_second: f64,
    sample_rate: u32,
) -> BeatGridSnapshot {
    let sample_rate =
        NonZeroU32::new(sample_rate).expect("invariant: fixture sample rate is non-zero");
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: fixture beat is finite"),
        beats_per_second,
        sample_rate,
    )
    .expect("invariant: fixture session anchor is valid");
    BeatGridSnapshot::session(id, revision, epoch, anchor, None)
}

fn fixture_group() -> TestGroup {
    TestGroup::unavailable(
        BeatGridId::allocate().expect("invariant: fixture group id is available"),
        NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero"),
        SessionEpoch::new(0),
        SyncMemberKind::Grid,
    )
}

#[kithara::test]
fn group_rejects_foreign_grid_identity() {
    let mut group = fixture_group();
    let before = group.snapshot();
    let foreign = session_grid(
        BeatGridId::allocate().expect("invariant: foreign grid id is available"),
        before
            .revision()
            .checked_next()
            .expect("invariant: fixture grid revision can advance"),
        SessionEpoch::new(0),
        2.0,
    );

    assert_eq!(
        group.publish_grid(foreign.clone()),
        Err(SyncError::GridIdentityMismatch {
            expected: before.id(),
            given: foreign.id(),
        })
    );
    assert_eq!(group.snapshot(), before);
}

#[kithara::test]
fn group_enforces_grid_successors() {
    let mut group = fixture_group();
    let initial = group.snapshot();
    let published_revision = initial
        .revision()
        .checked_next()
        .expect("invariant: fixture grid revision can advance");
    let published = session_grid(initial.id(), published_revision, SessionEpoch::new(0), 2.0);
    group
        .publish_grid(published.clone())
        .expect("invariant: newer fixture publication is valid");
    let stale = session_grid(initial.id(), initial.revision(), SessionEpoch::new(0), 1.5);

    assert_eq!(
        group.publish_grid(stale.clone()),
        Err(SyncError::StaleGridRevision {
            current: published.stamp(),
            given: stale.stamp(),
        })
    );
    assert_eq!(group.snapshot(), published);

    let withdrawn_revision = published_revision
        .checked_next()
        .expect("invariant: fixture grid revision can advance twice");
    let withdrawn =
        BeatGridSnapshot::unavailable(initial.id(), withdrawn_revision, published.axis());
    assert_eq!(
        group.publish_grid(withdrawn.clone()),
        Err(SyncError::InvalidGroupGridTransition {
            from: BeatGridState::Live,
            to: BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry),
        })
    );
    assert_eq!(group.snapshot(), published);

    let wrong_axis_revision = published_revision
        .checked_next()
        .expect("invariant: fixture grid revision can advance twice");
    let sample_rate = NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero");
    let wrong_axis = BeatGridSnapshot::unavailable(
        initial.id(),
        wrong_axis_revision,
        MapAxis::Asset(AssetAxis::new(sample_rate, 0)),
    );
    assert_eq!(
        group.publish_grid(wrong_axis.clone()),
        Err(SyncError::GridAxisChanged {
            expected: published.axis(),
            given: wrong_axis.axis(),
        })
    );
    assert_eq!(group.snapshot(), published);

    let mut negotiated = fixture_group();
    let initial = negotiated.snapshot();
    let live_revision = initial
        .revision()
        .checked_next()
        .expect("invariant: fixture grid revision can advance");
    let observed = session_grid_at_rate(
        initial.id(),
        live_revision,
        SessionEpoch::new(0),
        2.0,
        44_100,
    );
    negotiated
        .publish_grid(observed.clone())
        .expect("an unavailable axis admits the negotiated live sample rate");
    assert_eq!(negotiated.snapshot(), observed);

    let changed_rate = session_grid_at_rate(
        initial.id(),
        live_revision
            .checked_next()
            .expect("invariant: fixture grid revision can advance twice"),
        SessionEpoch::new(0),
        2.0,
        32_000,
    );
    assert_eq!(
        negotiated.publish_grid(changed_rate.clone()),
        Err(SyncError::GridAxisChanged {
            expected: observed.axis(),
            given: changed_rate.axis(),
        })
    );
    assert_eq!(negotiated.snapshot(), observed);
}

#[kithara::test]
fn group_treats_same_grid_stamp_as_idempotent_publication() {
    let mut group = fixture_group();
    let current = group.snapshot();
    let revision = current
        .revision()
        .checked_next()
        .expect("invariant: fixture grid revision can advance");
    let published = session_grid(current.id(), revision, SessionEpoch::new(0), 2.0);
    group
        .publish_grid(published.clone())
        .expect("invariant: newer fixture publication is valid");

    group
        .publish_grid(published.clone())
        .expect("publishing the same immutable grid revision is idempotent");

    assert_eq!(group.snapshot(), published);
}

#[kithara::test]
fn group_accepts_latest_grid_after_unpublished_revisions() {
    let mut group = fixture_group();
    let current = group.snapshot();
    let skipped = current
        .revision()
        .checked_next()
        .expect("invariant: fixture grid revision can advance");
    let published_revision = skipped
        .checked_next()
        .expect("invariant: fixture grid revision can advance twice");
    let published = session_grid(current.id(), published_revision, SessionEpoch::new(0), 2.0);

    group
        .publish_grid(published.clone())
        .expect("a newer published observation may skip invisible revisions");

    assert_eq!(group.snapshot(), published);
    assert_eq!(group.snapshot().revision(), published_revision);
}

#[kithara::test]
fn group_requires_each_unavailable_route_boundary() {
    let mut group = fixture_group();
    let initial = group.snapshot();
    let live_revision = initial
        .revision()
        .checked_next()
        .expect("invariant: fixture grid revision can advance");
    let live = session_grid(initial.id(), live_revision, SessionEpoch::new(0), 2.0);
    group
        .publish_grid(live.clone())
        .expect("the initial session grid becomes live in its current epoch");

    let boundary_revision = live_revision
        .checked_next()
        .expect("invariant: fixture grid revision can advance twice");
    let sample_rate = NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero");
    let skipped_axis = MapAxis::Session(SessionAxis::new(sample_rate, SessionEpoch::new(2)));
    let skipped = BeatGridSnapshot::unavailable(live.id(), boundary_revision, skipped_axis);
    assert_eq!(
        group.publish_grid(skipped),
        Err(SyncError::GridAxisChanged {
            expected: live.axis(),
            given: skipped_axis,
        })
    );

    let successor_live = session_grid(live.id(), boundary_revision, SessionEpoch::new(1), 2.0);
    assert_eq!(
        group.publish_grid(successor_live.clone()),
        Err(SyncError::GridAxisChanged {
            expected: live.axis(),
            given: successor_live.axis(),
        })
    );

    let boundary_axis = MapAxis::Session(SessionAxis::new(sample_rate, SessionEpoch::new(1)));
    let boundary = BeatGridSnapshot::unavailable(live.id(), boundary_revision, boundary_axis);
    group
        .publish_grid(boundary.clone())
        .expect("the exact successor epoch is admitted through an unavailable boundary");

    let next_live = session_grid_at_rate(
        live.id(),
        boundary_revision
            .checked_next()
            .expect("invariant: fixture grid revision can advance three times"),
        SessionEpoch::new(1),
        2.0,
        44_100,
    );
    group
        .publish_grid(next_live.clone())
        .expect("the unavailable boundary admits the negotiated live axis");
    assert_eq!(group.snapshot(), next_live);
}
