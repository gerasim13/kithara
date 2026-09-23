use kithara_sync::{
    GroupState, ParentGridUpdate, SessionAxisUpdate, SyncAdmission, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncOperation, SyncReceipt, SyncRejected, SyncStatusSnapshot,
};
use kithara_warp::{BeatGrid, BeatGridId, BeatGridSnapshot};

use super::Player;

pub(crate) type PlayerSync = GroupState<PlayerMember>;

/// Host-owned synchronization member that retains one native player.
pub struct PlayerMember {
    inner: Box<dyn Player>,
}

impl PlayerMember {
    /// Erases one concrete player while retaining exclusive ownership.
    #[must_use]
    pub fn new<P: Player>(player: P) -> Self {
        Self {
            inner: Box::new(player),
        }
    }

    delegate::delegate! {
        to self.inner.as_ref() {
            /// Commits the Host-applied level after its graph batch succeeds.
            #[call(set_host_level)]
            pub fn commit_host_level(&self, level: f32);
            /// Reads the desired Host level used for later graph registration.
            #[must_use]
            pub fn host_level(&self) -> f32;
        }
    }
}

impl BeatGrid for PlayerMember {
    delegate::delegate! {
        to self.inner.as_ref() {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for PlayerMember {
    type NestedGroup = Self;

    fn status(&self) -> SyncStatusSnapshot {
        SyncGroup::status(self.inner.as_ref())
    }

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        self.inner.topology()
    }

    delegate::delegate! {
        to self.inner.as_ref() {
            fn check_axis(&self, update: SessionAxisUpdate) -> Result<(), SyncError>;
            fn check_parent(&self, update: ParentGridUpdate) -> Result<(), SyncError>;
        }
        to self.inner.as_mut() {
            fn accept_axis(&mut self, update: SessionAxisUpdate) -> Result<(), SyncError>;
            fn accept_parent(&mut self, update: ParentGridUpdate) -> Result<(), SyncError>;
            fn transact(
                &mut self,
                operation: SyncOperation<Self>,
            ) -> Result<SyncAdmission, SyncRejected<Self>>;
            fn acknowledge(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }
}
