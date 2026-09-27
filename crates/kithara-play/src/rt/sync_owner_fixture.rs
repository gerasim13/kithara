//! A real Sync owner decision for RT unit tests that need an owner-minted permit.

use std::num::{NonZeroU32, NonZeroU64};

use kithara_events::TrackId;
use kithara_signal::{SessionEpoch, SessionFrame, SourceSpan, TransportRevision};
use kithara_sync::{
    ActivationHead, AlignmentSource, GroupState, LoadGeneration, LoadedMedia, ParentFact,
    ParentGridUpdate, PreparedFirst, SyncAdmission, SyncEffect, SyncError, SyncExecutionStamp,
    SyncGateBinding, SyncGroup, SyncGroupSnapshot, SyncIntent, SyncMember, SyncMode, SyncOperation,
    SyncReceipt, SyncRejected, SyncStaged, SyncStatusSnapshot, SyncTransition,
};
use kithara_warp::{
    AssetAxis, AssetExtent, AssetFrame, BeatEvidence, BeatGrid, BeatGridId, BeatGridRevision,
    BeatGridSnapshot, BeatGridStamp, BeatGridState, BeatMarker, BeatOrdinal, FrameUncertainty,
    MapAxis, MapPosition, MapSegment, SegmentFacts, SegmentSet, SessionAnchor, SessionBeat,
};

use crate::{bridge::sync::SyncTicket, rt::track::PlayerResource};

struct FixtureGroup(GroupState<Self>);

impl BeatGrid for FixtureGroup {
    delegate::delegate! {
        to self.0 {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for FixtureGroup {
    type NestedGroup = Self;

    delegate::delegate! {
        to self.0 {
            fn stage_fact(&self, fact: ParentFact) -> Result<SyncStaged, SyncError>;
            fn apply_staged(&mut self, staged: SyncStaged) -> SyncTransition;
            fn acknowledge(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError>;
            fn status(&self) -> SyncStatusSnapshot;
            fn mode(&self) -> SyncMode;
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn transact(&mut self, operation: SyncOperation<Self>) -> Result<SyncAdmission, SyncRejected<Self>>;
        }
    }
}

struct FixtureGrid(BeatGridSnapshot);

impl BeatGrid for FixtureGrid {
    delegate::delegate! {
        to self.0 {
            fn id(&self) -> BeatGridId;
            #[call(clone)]
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

fn source_grid(member: BeatGridId, rate: NonZeroU32) -> BeatGridSnapshot {
    let exact = FrameUncertainty::new(0.0).expect("fixture uncertainty");
    let marker = |frame: u64, ordinal: i64| {
        BeatMarker::new(
            MapPosition::Asset(AssetFrame::new(frame as f64).expect("fixture source frame")),
            Some(BeatOrdinal::new(ordinal)),
            BeatEvidence::Observed,
            exact,
        )
    };
    let segment = MapSegment::new(
        marker(0, 0),
        marker(24_000, 1),
        SegmentFacts::new(BeatEvidence::Observed, exact, None),
    )
    .expect("fixture beat interval");
    let segments = SegmentSet::new(
        MapAxis::Asset(AssetAxis::new(rate, AssetExtent::Bounded(24_000))),
        vec![segment],
    )
    .expect("fixture segment set");
    BeatGridSnapshot::segments(
        member,
        BeatGridRevision::first(),
        BeatGridState::Complete,
        segments,
    )
    .expect("fixture source grid")
}

/// Return the exact stamp and activation head from one public owner Enable
/// at output frame 32, prepared from source frame `cue`; no RT test
/// manufactures a private execution stamp or a first frame off its head.
pub(crate) fn prepared_entry(
    member: BeatGridId,
    group: BeatGridId,
    load: LoadGeneration,
    transport: TransportRevision,
    output_transport: Option<TransportRevision>,
    rate: NonZeroU32,
    cue: u64,
) -> (SyncExecutionStamp, ActivationHead) {
    let mut owner: GroupState<FixtureGroup> = GroupState::owning(
        group,
        rate,
        SessionEpoch::new(1),
        SyncMember::Grid {
            alignment: None,
            grid: Box::new(FixtureGrid(source_grid(member, rate))),
        },
    );
    let parent = BeatGridId::allocate().expect("fixture parent identity");
    let anchor = SessionAnchor::new(SessionFrame::new(32), SessionBeat::default(), 2.0, rate)
        .expect("fixture parent anchor");
    let mut update = ParentGridUpdate::new(
        BeatGridStamp::new(parent, BeatGridRevision::first()),
        SessionEpoch::new(1),
        anchor,
        None,
    );
    if let Some(revision) = output_transport {
        update = update.with_output_transport(revision);
    }
    let fact = ParentFact::Segment(update);
    let staged = owner.stage_fact(fact).expect("fixture parent publication");
    owner.apply_staged(staged);
    let admission = owner
        .transact(SyncOperation::Sync {
            target: group,
            load,
            transport,
            source: AlignmentSource::Prepared(AssetFrame::new(cue as f64).expect("fixture cue")),
            activation: SessionFrame::new(32),
            intent: SyncIntent::Enable,
        })
        .expect("public owner Enable");
    let SyncAdmission::StateChanged { transition, .. } = admission else {
        panic!("public Enable must issue a preparation");
    };
    let [preparation] = transition.issued() else {
        panic!("one direct member must receive one preparation");
    };
    let SyncEffect::Projection { plan, .. } = preparation.effect() else {
        panic!("public Enable must project the member");
    };
    let head = ActivationHead::of(plan).expect("public Enable enters the session");
    assert_eq!(head.activation().output(), SessionFrame::new(32));
    (preparation.stamp(), head)
}

/// The frame `stereo`, decoded exactly at `head`.
pub(crate) fn first_at(head: ActivationHead, stereo: [f32; 2]) -> PreparedFirst {
    let cursor = head.activation();
    let source = SourceSpan::new(cursor.source(), cursor.source() + 1, head.source_rate(), 1)
        .expect("one source frame")
        .with_mapping_revision(NonZeroU64::new(u64::from(cursor.revision())));
    head.first(stereo, source).expect("a frame at its head")
}

/// The ticket of `lane`, loaded as `item_id` at `load`, entering silently
/// at `head` with the permit the owner mints for `stamp` through `gate`.
pub(crate) fn ticket(
    item_id: TrackId,
    load: LoadGeneration,
    lane: Box<PlayerResource>,
    stamp: SyncExecutionStamp,
    head: ActivationHead,
    gate: SyncGateBinding,
) -> SyncTicket {
    let owner = gate.arbiter().try_control().expect("owner phase");
    let permit = owner.mint_permit(gate.cell(), stamp).expect("exact permit");
    drop(owner);
    let first = first_at(head, [0.0; 2]);
    SyncTicket::new(LoadedMedia::new(item_id, load), lane, first, permit, gate)
}
