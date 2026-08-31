use std::{cell::RefCell, num::NonZeroU32, rc::Rc};

use kithara_audio::{ConsumerWakeMode, SeekOutcome};
use kithara_bufpool::testing::TestPools;
use kithara_platform::sync::Arc;
use kithara_play::{
    GroupState, PlayError, SessionBinding, SessionDispatcher,
    player::{PlaybackView, Player, PlayerControlSource, PlayerMember},
};
use kithara_test_utils::kithara;
use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridSnapshot, SessionEpoch, SyncAdmission, SyncApplied, SyncError,
    SyncGroup, SyncGroupSnapshot, SyncMember, SyncMemberKind, SyncOperation, SyncRejected,
    SyncStatusSnapshot, TopologyOperation,
};

use super::{Host, HostOwned, Platform};
use crate::session::{
    HostCmd, HostDispatcher, HostReply, Reply, RootView,
    protocol::{HostDispatchError, SyncCmd},
};

const SAMPLE_RATE: u32 = 44_100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    SessionGone,
    OtherError,
}

struct Resident {
    sync: GroupState<PlayerMember>,
    close: Outcome,
    drops: Rc<RefCell<usize>>,
}

impl Resident {
    fn new(id: BeatGridId, close: Outcome, drops: Rc<RefCell<usize>>) -> Self {
        Self {
            sync: group(id),
            close,
            drops,
        }
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        *self.drops.borrow_mut() += 1;
    }
}

impl BeatGrid for Resident {
    fn id(&self) -> BeatGridId {
        self.sync.id()
    }

    fn snapshot(&self) -> BeatGridSnapshot {
        self.sync.snapshot()
    }
}

impl SyncGroup for Resident {
    type NestedGroup = PlayerMember;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        self.sync.topology()
    }

    fn transact(
        &mut self,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        self.sync.transact(operation)
    }

    fn status(&self) -> SyncStatusSnapshot {
        self.sync.status()
    }

    fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        self.sync.acknowledge(applied)
    }
}

impl Player for Resident {
    fn play(&self) {}

    fn pause(&self) {}

    fn seek_seconds(&self, _seconds: f64) -> Result<SeekOutcome, PlayError> {
        Err(PlayError::Internal("fixture seek is unavailable".into()))
    }

    fn tick(&self) -> Result<(), PlayError> {
        Ok(())
    }

    fn playback_view(&self) -> PlaybackView {
        PlaybackView::default()
    }

    fn set_host_level(&self, _level: f32) {}

    fn host_level(&self) -> f32 {
        1.0
    }

    fn close(&mut self) -> Result<(), PlayError> {
        match self.close {
            Outcome::Ok => Ok(()),
            Outcome::SessionGone => Err(PlayError::SessionGone {
                reason: "fixture resident close",
            }),
            Outcome::OtherError => Err(PlayError::Internal("fixture resident close failed".into())),
        }
    }
}

impl PlayerControlSource for Resident {
    type Schema = TestPools;
    type Control = ();

    fn control(&self) -> Self::Control {}

    fn attach_session(&mut self, _binding: SessionBinding<TestPools>) -> Result<(), PlayError> {
        Ok(())
    }

    fn close_control(_control: &Self::Control) -> Result<(), PlayError> {
        Ok(())
    }

    fn take_host_member(&mut self) -> Result<PlayerMember, PlayError> {
        Err(PlayError::Internal(
            "fixture synchronization ownership is unavailable".into(),
        ))
    }
}

struct Dispatcher {
    root: RefCell<GroupState<PlayerMember>>,
    detach: Outcome,
}

impl SessionDispatcher<TestPools> for Dispatcher {
    fn exec(&self, _cmd: kithara_play::Cmd<TestPools>) -> Result<Reply, PlayError> {
        Ok(Reply::Ok)
    }

    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }
}

impl HostDispatcher<TestPools> for Dispatcher {
    fn exec_host(
        &self,
        cmd: HostCmd<TestPools>,
    ) -> Result<HostReply, HostDispatchError<TestPools>> {
        let HostCmd::Sync(SyncCmd::TransactCurrent(operations)) = cmd else {
            panic!("unexpected fixture Host command")
        };
        match self.detach {
            Outcome::SessionGone => Err(HostDispatchError::before_send(
                PlayError::SessionGone {
                    reason: "fixture detach",
                },
                HostCmd::Sync(SyncCmd::TransactCurrent(operations)),
            )),
            Outcome::OtherError => Ok(HostReply::Err(PlayError::Internal(
                "fixture detach failed".into(),
            ))),
            Outcome::Ok => {
                let mut root = self.root.borrow_mut();
                let base = root.topology().expect("fixture topology").stamp();
                Ok(HostReply::Admission(
                    root.transact(SyncOperation::Topology { base, operations }),
                ))
            }
        }
    }
}

fn group(id: BeatGridId) -> GroupState<PlayerMember> {
    GroupState::unavailable(
        id,
        NonZeroU32::new(SAMPLE_RATE).expect("fixture sample rate"),
        SessionEpoch::new(0),
        SyncMemberKind::Group,
    )
}

fn fixture(
    close: Outcome,
    detach: Outcome,
) -> (Host<TestPools>, HostOwned<Resident>, Rc<RefCell<usize>>) {
    let host_id = BeatGridId::allocate().expect("fixture Host grid id");
    let resident_id = BeatGridId::allocate().expect("fixture resident grid id");
    let mut root = group(host_id);
    let base = root.topology().expect("fixture root topology").stamp();
    let admission = root
        .transact(SyncOperation::Topology {
            base,
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(crate::session::testing::fixture_member(resident_id)),
                },
            }]),
        })
        .expect("fixture resident attachment");
    assert!(matches!(admission, SyncAdmission::TopologyChanged { .. }));

    let root_view = RootView::new(&root);
    let dispatcher: Arc<dyn HostDispatcher<TestPools>> = Arc::new(Dispatcher {
        root: RefCell::new(root),
        detach,
    });
    let drops = Rc::new(RefCell::new(0));
    let mut platform = Platform::remote();
    let replaced = platform
        .insert_resident(
            resident_id,
            Resident::new(resident_id, close, Rc::clone(&drops)),
        )
        .expect("fixture resident registry");
    assert!(replaced.is_none());
    let host = Host {
        id: host_id,
        owns_session: false,
        root_view,
        dispatcher,
        platform,
    };
    let owner = host.owned::<Resident>(resident_id, ());
    (host, owner, drops)
}

#[kithara::test(wasm, flash(false))]
fn successful_remove_releases_resident() {
    let (mut host, owner, drops) = fixture(Outcome::Ok, Outcome::Ok);

    host.remove(&owner).expect("remove resident");

    assert_eq!(*drops.borrow(), 1);
}

#[kithara::test(wasm, flash(false))]
fn session_gone_while_closing_releases_resident() {
    let (mut host, owner, drops) = fixture(Outcome::SessionGone, Outcome::Ok);

    assert!(matches!(
        host.remove(&owner),
        Err(PlayError::SessionGone { .. })
    ));
    assert_eq!(*drops.borrow(), 1);
}

#[kithara::test(wasm, flash(false))]
fn session_gone_while_detaching_releases_resident() {
    let (mut host, owner, drops) = fixture(Outcome::Ok, Outcome::SessionGone);

    assert!(matches!(
        host.remove(&owner),
        Err(PlayError::SessionGone { .. })
    ));
    assert_eq!(*drops.borrow(), 1);
}

#[kithara::test(wasm, flash(false))]
fn other_errors_retain_resident() {
    for (close, detach) in [
        (Outcome::OtherError, Outcome::Ok),
        (Outcome::Ok, Outcome::OtherError),
    ] {
        let (mut host, owner, drops) = fixture(close, detach);

        assert!(matches!(host.remove(&owner), Err(PlayError::Internal(_))));
        assert_eq!(*drops.borrow(), 0);
        assert!(
            host.platform
                .remote_residents
                .as_ref()
                .is_some_and(|residents| residents.contains_key(&owner.id()))
        );
        drop(host);
        assert_eq!(*drops.borrow(), 0);
    }
}
