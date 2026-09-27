use std::{cell::Cell, num::NonZeroU32};

use kithara_platform::time::Duration;
use kithara_signal::{SessionEpoch, SessionFrame, TransportRevision};
use kithara_test_utils::kithara;
use kithara_warp::{
    AssetAxis, AssetExtent, AssetFrame, BeatGrid, BeatGridId, BeatGridRevision, BeatGridSnapshot,
    BeatGridStamp, BeatsPerMinute, MapAxis, PresentationFrontier,
};

use super::*;
use crate::{
    AlignmentSource, ControlEnterError, GroupState, LoadGeneration, PublicOperation, SourceChange,
    SyncAdmission, SyncError, SyncExecutionReject, SyncExecutionStamp, SyncGroup, SyncIntent,
    SyncMember, SyncMemberKind, SyncMode, SyncOperation, SyncOperationId, SyncReceipt,
    SyncReceiptInbox, SyncStatusSnapshot, TopologyOperation, TopologyRevision, TopologyStamp,
    TransportOperation,
    owner::tests::fixtures::{
        TestGrid, TestGroup, deck_with_a_preparation, root_with_a_waiting_deck,
    },
    sync_receipts,
};

/// One deck of live slots and the retiring slots of any deck, with every
/// publication and every read of an audio clock that never processed
/// anything counted.
#[derive(Default)]
struct FakePort {
    live: Vec<SyncReceiptInbox>,
    retiring: Vec<(BeatGridId, SyncReceiptInbox)>,
    publishes: Cell<usize>,
    clock_reads: Cell<usize>,
}

impl EntryPort for FakePort {
    fn processed(&mut self) -> Option<ProcessedTransport> {
        self.clock_reads.set(self.clock_reads.get() + 1);
        None
    }

    fn commit_boundary(&self) -> Result<SessionFrame, ClockRefusal> {
        self.clock_reads.set(self.clock_reads.get() + 1);
        Err(ClockRefusal::Unavailable)
    }
}

impl RootPort<TestGroup> for FakePort {
    fn decks(&self) -> usize {
        1
    }

    fn slots(&self, _deck: usize) -> usize {
        self.live.len()
    }

    delegate::delegate! {
        to self.retiring {
            #[call(len)]
            fn retiring_len(&self) -> usize;
            #[expr($.0)]
            #[call(remove)]
            fn remove_retiring(&mut self, index: usize) -> BeatGridId;
        }
    }

    fn inbox(&mut self, at: InboxAt) -> Option<&mut SyncReceiptInbox> {
        match at {
            InboxAt::Live { slot, .. } => self.live.get_mut(slot),
            InboxAt::Retiring(index) => self.retiring.get_mut(index).map(|(_, inbox)| inbox),
        }
    }

    fn is_quiesced(&self, group: BeatGridId) -> bool {
        self.retiring.iter().all(|(retiring, _)| *retiring != group)
    }

    fn is_projected(&self, _id: BeatGridId) -> bool {
        false
    }

    fn publish(&self, _root: &GroupState<TestGroup>) {
        self.publishes.set(self.publishes.get() + 1);
    }
}

fn id() -> BeatGridId {
    BeatGridId::allocate().expect("fixture grid id")
}

fn rate() -> NonZeroU32 {
    NonZeroU32::new(48_000).expect("fixture sample rate")
}

/// A root whose members are track grids, as a deck's are.
fn root(owner_wait: Duration) -> SyncRoot<TestGroup> {
    let group = GroupState::unavailable(
        id(),
        rate(),
        SessionEpoch::new(0),
        SyncMemberKind::Grid,
        SyncMode::Off,
    );
    SyncRoot::new(
        group,
        SyncRootConfig::builder().owner_wait(owner_wait).build(),
    )
}

/// Attaching `track`, still without geometry, to the root at `base`.
fn attach(base: TopologyStamp, track: BeatGridId) -> SyncOperation<TestGroup> {
    let grid = BeatGridSnapshot::unavailable(
        track,
        BeatGridRevision::first(),
        MapAxis::Asset(AssetAxis::new(rate(), AssetExtent::Bounded(480_000))),
    );
    SyncOperation::Topology {
        base,
        operations: Box::new([TopologyOperation::Attach {
            member: SyncMember::Grid {
                alignment: None,
                grid: Box::new(TestGrid(grid)),
            },
        }]),
    }
}

/// Attaches a track still without geometry under one owner cut.
fn attach_track(root: &mut SyncRoot<TestGroup>, port: &mut FakePort, track: BeatGridId) {
    let entered = root.enter().expect("the owner is free");
    let admission = entered
        .run(port, |cut, port| {
            let base = cut.group().topology().expect("root topology").stamp();
            let attach = PublicOperation::try_from(attach(base, track)).expect("a public edit");
            cut.transact(&*port, attach)
        })
        .expect("the cut drains nothing")
        .expect("the root admits a track grid");
    assert!(matches!(admission, SyncAdmission::TopologyChanged { .. }));
}

/// A root over one deck whose decision waits for its Host, with the deck's
/// track registered; with the deck and the waiting operation.
fn waiting_root() -> (SyncRoot<TestGroup>, BeatGridId, SyncOperationId) {
    let (group, deck, track, operation) = root_with_a_waiting_deck();
    let mut root = SyncRoot::new(group, SyncRootConfig::builder().build());
    let _ = root.register(deck, track).expect("registration");
    (root, deck, operation)
}

/// A Host that cannot observe any deck's track afresh.
fn unobserved(_: &TestGroup) -> Option<ResidentLoadObservation<u32>> {
    None
}

/// An execution stamp of `member` for an operation the root never prepared.
fn stamp(root: &SyncRoot<TestGroup>, member: BeatGridId) -> SyncExecutionStamp {
    let group = root.group().id();
    SyncExecutionStamp::new(
        SyncOperationId::first(),
        BeatGridStamp::new(member, BeatGridRevision::first()),
        BeatGridStamp::new(group, BeatGridRevision::first()),
        TopologyStamp::new(group, TopologyRevision::first()),
        LoadGeneration::first(),
        TransportRevision::first(),
    )
}

#[kithara::test]
fn a_deck_and_its_member_register_once() {
    let mut root = root(DEFAULT_OWNER_WAIT);
    let (deck, member) = (id(), id());
    root.register(deck, member).expect("first registration");

    let other = id();
    assert_eq!(
        root.register(deck, other).err(),
        Some(RootError::MemberAlreadyRegistered(other))
    );
    assert_eq!(
        root.register(id(), member).err(),
        Some(RootError::MemberAlreadyRegistered(member))
    );
    assert!(root.gate(deck).is_some());
    assert!(root.gate(other).is_none());
}

#[kithara::test]
fn a_busy_owner_keeps_one_terminal_rejection_for_its_next_cut() {
    let (deck, preparation) = deck_with_a_preparation();
    let mut root = SyncRoot::new(
        deck,
        SyncRootConfig::builder().owner_wait(Duration::ZERO).build(),
    );
    let mut port = FakePort::default();
    let deck = root.group().id();
    let member = preparation.stamp().member().grid_id();
    let binding = root.register(deck, member).expect("registration");
    let installed = SyncReceipt::Installed(preparation.stamp());
    let held = binding.arbiter().try_control().expect("the gate is open");

    for _ in 0..2 {
        assert_eq!(
            root.enter_to_acknowledge(installed).err(),
            Some(RootError::Enter(ControlEnterError::Busy))
        );
    }
    let different = SyncReceipt::Rejected {
        stamp: preparation.stamp(),
        reason: SyncExecutionReject::Late,
    };
    assert_eq!(
        root.enter_to_acknowledge(different).err(),
        Some(RootError::GateFailurePending)
    );
    drop(held);
    assert!(matches!(
        root.group().status(),
        SyncStatusSnapshot::Prepared { .. }
    ));

    let busy = |status: SyncStatusSnapshot| {
        matches!(
            status,
            SyncStatusSnapshot::Rejected {
                operation,
                reason: SyncExecutionReject::ControlBusy,
                ..
            } if operation == preparation.stamp().operation()
        )
    };
    let entered = root.enter().expect("the owner is free");
    let seen = entered
        .run(&mut port, |cut, _| cut.group().status())
        .expect("the kept rejection is recorded");
    assert!(busy(seen), "the body sees the rejection recorded: {seen:?}");
    assert_eq!(port.publishes.get(), 1);

    let entered = root.enter().expect("the owner is free");
    let seen = entered
        .run(&mut port, |cut, _| cut.group().status())
        .expect("nothing is left to record");
    assert!(busy(seen));
    assert_eq!(
        port.publishes.get(),
        1,
        "a recorded rejection is not recorded again"
    );
}

#[kithara::test]
fn a_receipt_the_root_cannot_record_stays_in_its_inbox_and_the_body_does_not_run() {
    let mut root = root(DEFAULT_OWNER_WAIT);
    let installed = SyncReceipt::Installed(stamp(&root, id()));
    let (_tx, mut inbox) = sync_receipts();
    inbox.keep(installed);
    let mut port = FakePort {
        live: vec![inbox],
        ..FakePort::default()
    };

    let mut ran = false;
    let entered = root.enter().expect("the owner is free");
    let outcome = entered.run(&mut port, |_, _| ran = true);

    assert_eq!(outcome, Err(RootError::NonAudioReceipt));
    assert!(!ran);
    assert_eq!(port.live[0].next_receipt(), Some(installed));
    assert_eq!(port.publishes.get(), 0);
}

#[kithara::test]
fn closing_drains_every_inbox_publishes_once_and_refuses_later_entry() {
    let mut root = root(DEFAULT_OWNER_WAIT);
    let rejected = SyncReceipt::Rejected {
        stamp: stamp(&root, id()),
        reason: SyncExecutionReject::Late,
    };
    let (_live_tx, mut live) = sync_receipts();
    live.keep(rejected);
    let (_retiring_tx, mut retiring) = sync_receipts();
    retiring.keep(rejected);
    let mut port = FakePort {
        live: vec![live],
        retiring: vec![(id(), retiring)],
        ..FakePort::default()
    };

    root.close(&mut port);

    assert_eq!(port.live[0].next_receipt(), None);
    assert_eq!(port.retiring[0].1.next_receipt(), None);
    assert_eq!(port.publishes.get(), 1);
    assert_eq!(root.enter().err(), Some(ControlEnterError::Closed));
}

#[kithara::test]
fn public_operations_refuse_every_owner_only_operation() {
    let target = id();
    let (load, transport) = (LoadGeneration::first(), TransportRevision::first());
    let source = AlignmentSource::Prepared(AssetFrame::default());
    let window = SessionFrame::new(0)..SessionFrame::new(1);
    let owner_only: [(SyncOperation<TestGroup>, SyncError); 4] = [
        (
            SyncOperation::WithdrawQuiescedMember { target },
            SyncError::QuiescenceRequired { member_id: target },
        ),
        (
            SyncOperation::InvalidateSource {
                target,
                change: SourceChange::Timing,
            },
            SyncError::SourceChangeUnverified { member_id: target },
        ),
        (
            SyncOperation::Replan {
                target,
                operation: SyncOperationId::first(),
                load,
                transport,
                source,
                activation: SessionFrame::new(0),
            },
            SyncError::ReplanUnverified { group_id: target },
        ),
        (
            SyncOperation::AbandonReplan {
                target,
                operation: SyncOperationId::first(),
            },
            SyncError::ReplanUnverified { group_id: target },
        ),
    ];
    for (operation, refusal) in owner_only {
        let rejected = PublicOperation::try_from(operation)
            .err()
            .expect("only the root's own observation issues it");
        assert_eq!(rejected.error(), &refusal);
        assert_eq!(rejected.operation().target(), target);
    }

    let public: [SyncOperation<TestGroup>; 6] = [
        SyncOperation::Topology {
            base: TopologyStamp::new(target, TopologyRevision::first()),
            operations: Box::new([]),
        },
        SyncOperation::Transport {
            target,
            load,
            transport,
            operation: TransportOperation::Play,
        },
        SyncOperation::Sync {
            target,
            load,
            transport,
            source,
            activation: SessionFrame::new(0),
            intent: SyncIntent::Enable,
        },
        SyncOperation::Prepare {
            target,
            load,
            transport,
            source,
            window: window.clone(),
        },
        SyncOperation::Relocate {
            target,
            load,
            transport,
            cue: AssetFrame::default(),
            frontier: PresentationFrontier::builder()
                .source(0)
                .output(SessionFrame::new(0))
                .build(),
            window,
        },
        SyncOperation::Tempo {
            target,
            tempo: BeatsPerMinute::try_from(120.0).expect("finite positive bpm"),
            commit: SessionFrame::new(0),
            smoothing: 0.0,
        },
    ];
    for operation in public {
        let converted = PublicOperation::try_from(operation).expect("any caller may ask it");
        assert_eq!(converted.0.target(), target);
    }
}

#[kithara::test]
fn a_public_operation_publishes_only_when_the_root_admits_it() {
    let mut root = root(DEFAULT_OWNER_WAIT);
    let mut port = FakePort::default();
    let base = root.group().topology().expect("root topology").stamp();
    attach_track(&mut root, &mut port, id());
    let attached = root.group().topology().expect("root topology");
    assert_eq!(port.publishes.get(), 1, "an admission publishes the root");

    let entered = root.enter().expect("the owner is free");
    let rejected = entered
        .run(&mut port, |cut, port| {
            let stale = PublicOperation::try_from(attach(base, id())).expect("a public edit");
            cut.transact(&*port, stale)
        })
        .expect("the cut drains nothing")
        .expect_err("a stale base is refused");

    assert!(matches!(rejected.error(), SyncError::StaleTopology { .. }));
    assert_eq!(port.publishes.get(), 1, "a refusal publishes nothing");
    assert_eq!(
        root.group().topology().expect("root topology"),
        attached,
        "a refusal changes nothing"
    );
}

#[kithara::test]
fn an_entry_refused_on_its_own_evidence_never_reads_the_host_clock() {
    let mut root = root(DEFAULT_OWNER_WAIT);
    let mut port = FakePort::default();
    let (target, member) = (id(), id());
    let observed = |render, staging| {
        ResidentLoadObservation::builder()
            .item_id(0_u32)
            .load(LoadGeneration::first())
            .requested_speed(1.0)
            .render(render)
            .source(None)
            .staging(staging)
            .build()
    };
    let stale = ResidentRender::Stale {
        bound_load: LoadGeneration::first(),
    };
    let other = ResidentStaging::DifferentLoad {
        item_id: 1,
        load: LoadGeneration::first(),
    };
    let cases = [
        (SyncIntent::Enable, observed(stale.clone(), other)),
        (
            SyncIntent::AlignNow,
            observed(stale.clone(), ResidentStaging::Unavailable),
        ),
        (
            SyncIntent::Enable,
            observed(ResidentRender::Missing, ResidentStaging::Available),
        ),
        (
            SyncIntent::Disable,
            observed(stale, ResidentStaging::Unavailable),
        ),
        (
            SyncIntent::Free,
            observed(ResidentRender::Missing, ResidentStaging::Unavailable),
        ),
    ];

    let entered = root.enter().expect("the owner is free");
    let refusals = entered
        .run(&mut port, |cut, port| {
            cases.map(|(intent, resident)| {
                cut.requested_sync(port, target, member, intent, &resident)
                    .err()
            })
        })
        .expect("the cut drains nothing");

    assert_eq!(refusals, [Some(EntryRefusal::NotReady); 5]);
    assert_eq!(port.clock_reads.get(), 0);
}

#[kithara::test]
fn a_waiting_deck_with_no_observation_ends_its_decision_and_the_root_publishes_once() {
    let (mut root, deck, operation) = waiting_root();
    let mut port = FakePort::default();
    let waiting = root.waiting(unobserved);
    assert_eq!(waiting.len(), 1, "the deck waits for its Host");

    let entered = root.enter().expect("the owner is free");
    let settled = entered
        .run(&mut port, |cut, port| cut.replan_waiting(port, waiting))
        .expect("the cut drains nothing");

    assert_eq!(settled, Ok(()));
    assert!(matches!(
        root.group().with_group(deck, SyncGroup::status),
        Some(SyncStatusSnapshot::Rejected {
            operation: ended,
            reason: SyncExecutionReject::Late,
            ..
        }) if ended == operation
    ));
    assert!(root.waiting(unobserved).is_empty());
    assert_eq!(port.publishes.get(), 1);
    assert_eq!(port.clock_reads.get(), 0);
}

#[kithara::test]
fn a_decision_that_stopped_waiting_is_left_alone() {
    let (mut root, deck, _) = waiting_root();
    let mut port = FakePort::default();
    let first = root.waiting(unobserved);
    let stale = root.waiting(unobserved);
    let entered = root.enter().expect("the owner is free");
    let settled = entered
        .run(&mut port, |cut, port| cut.replan_waiting(port, first))
        .expect("the cut drains nothing");
    assert_eq!(settled, Ok(()));
    let ended = root.group().with_group(deck, SyncGroup::status);

    let entered = root.enter().expect("the owner is free");
    let settled = entered
        .run(&mut port, |cut, port| cut.replan_waiting(port, stale))
        .expect("the cut drains nothing");

    assert_eq!(settled, Ok(()), "an ended decision is not ended again");
    assert_eq!(root.group().with_group(deck, SyncGroup::status), ended);
    assert_eq!(port.publishes.get(), 2, "each pass publishes the root once");
}
