use std::{cell::Cell, num::NonZeroU32};

use kithara_platform::time::Duration;
use kithara_signal::{SessionEpoch, TransportRevision};
use kithara_test_utils::kithara;
use kithara_warp::{
    AssetAxis, AssetExtent, BeatGrid, BeatGridId, BeatGridRevision, BeatGridSnapshot,
    BeatGridStamp, MapAxis,
};

use super::*;
use crate::{
    ControlEnterError, GroupState, LoadGeneration, SyncAdmission, SyncExecutionReject,
    SyncExecutionStamp, SyncGroup, SyncMember, SyncMemberKind, SyncMode, SyncOperation,
    SyncOperationId, SyncReceipt, SyncReceiptInbox, TopologyOperation, TopologyRevision,
    TopologyStamp,
    owner::tests::fixtures::{TestGrid, TestGroup},
    sync_receipts,
};

/// One deck of live slots and the retiring slots of any deck, with every
/// publication counted.
#[derive(Default)]
struct FakePort {
    live: Vec<SyncReceiptInbox>,
    retiring: Vec<(BeatGridId, SyncReceiptInbox)>,
    publishes: Cell<usize>,
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

/// Attaches a track still without geometry under one owner cut.
fn attach_track(root: &mut SyncRoot<TestGroup>, port: &mut FakePort, track: BeatGridId) {
    let grid = BeatGridSnapshot::unavailable(
        track,
        BeatGridRevision::first(),
        MapAxis::Asset(AssetAxis::new(rate(), AssetExtent::Bounded(480_000))),
    );
    let entered = root.enter().expect("the owner is free");
    let admission = entered
        .run(port, |cut, port| {
            let base = cut.group().topology().expect("root topology").stamp();
            cut.transact_verified(
                &*port,
                SyncOperation::Topology {
                    base,
                    operations: Box::new([TopologyOperation::Attach {
                        member: SyncMember::Grid {
                            alignment: None,
                            grid: Box::new(TestGrid(grid)),
                        },
                    }]),
                },
            )
        })
        .expect("the cut drains nothing")
        .expect("the root admits a track grid");
    assert!(matches!(admission, SyncAdmission::TopologyChanged { .. }));
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
    let mut root = root(Duration::ZERO);
    let mut port = FakePort::default();
    let (deck, member) = (id(), id());
    attach_track(&mut root, &mut port, member);
    let binding = root.register(deck, member).expect("registration");
    let installed = SyncReceipt::Installed(stamp(&root, member));
    let held = binding.arbiter().try_control().expect("the gate is open");

    for _ in 0..2 {
        assert_eq!(
            root.enter_to_acknowledge(installed).err(),
            Some(RootError::Enter(ControlEnterError::Busy))
        );
    }
    let different = SyncReceipt::Rejected {
        stamp: stamp(&root, member),
        reason: SyncExecutionReject::Late,
    };
    assert_eq!(
        root.enter_to_acknowledge(different).err(),
        Some(RootError::GateFailurePending)
    );
    drop(held);

    let entered = root.enter().expect("the owner is free");
    let seen = entered.run(&mut port, |_, port| port.publishes.get());
    assert_eq!(
        seen,
        Ok(1),
        "the kept rejection is recorded before the body"
    );
    let entered = root.enter().expect("the owner is free");
    let seen = entered.run(&mut port, |_, port| port.publishes.get());
    assert_eq!(seen, Ok(1), "a recorded rejection is not recorded again");
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
