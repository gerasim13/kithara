use std::num::NonZeroU32;

use kithara_signal::SessionEpoch;
use kithara_sync::{
    GroupState, ParentFact, SyncAdmission, SyncError, SyncGroup, SyncGroupSnapshot, SyncMember,
    SyncOperation, SyncReceipt, SyncRejected, SyncStaged, SyncStatusSnapshot, SyncTransition,
};
use kithara_warp::{BeatGrid, BeatGridId, BeatGridSnapshot};
use portable_atomic::{AtomicF32, Ordering};

pub(crate) struct PlayerSync {
    grid: BeatGridSnapshot,
    owned: Option<GroupState<PlayerMember>>,
    topology: Result<SyncGroupSnapshot, SyncError>,
    status: SyncStatusSnapshot,
}

impl PlayerSync {
    pub(crate) fn owning(
        id: BeatGridId,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
        member: SyncMember<PlayerMember>,
    ) -> Self {
        let owned = GroupState::owning(id, sample_rate, epoch, member);
        Self {
            grid: owned.snapshot(),
            topology: owned.topology(),
            status: owned.status(),
            owned: Some(owned),
        }
    }
    pub(crate) fn take(&mut self) -> Option<GroupState<PlayerMember>> {
        let owned = self.owned.take()?;
        self.grid = owned.snapshot();
        self.topology = owned.topology();
        self.status = owned.status();
        Some(owned)
    }
}

impl BeatGrid for PlayerSync {
    fn id(&self) -> BeatGridId {
        self.owned.as_ref().map_or(self.grid.id(), BeatGrid::id)
    }

    fn snapshot(&self) -> BeatGridSnapshot {
        self.owned
            .as_ref()
            .map_or_else(|| self.grid.clone(), BeatGrid::snapshot)
    }
}

impl SyncGroup for PlayerSync {
    type NestedGroup = PlayerMember;

    /// A staged fact reaches only the owner that staged it; once the owner
    /// is taken there is nothing left to apply it to.
    fn apply_staged(&mut self, staged: SyncStaged) -> SyncTransition {
        self.owned
            .as_mut()
            .map_or_else(SyncTransition::default, |owned| owned.apply_staged(staged))
    }

    fn acknowledge(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError> {
        self.owned
            .as_mut()
            .map_or(Err(SyncError::OwnerUnavailable), |owned| {
                owned.acknowledge(receipt)
            })
    }

    fn stage_fact(&self, fact: ParentFact) -> Result<SyncStaged, SyncError> {
        self.owned
            .as_ref()
            .map_or(Err(SyncError::OwnerUnavailable), |owned| {
                owned.stage_fact(fact)
            })
    }

    fn status(&self) -> SyncStatusSnapshot {
        self.owned.as_ref().map_or(self.status, SyncGroup::status)
    }

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        self.owned
            .as_ref()
            .map_or_else(|| self.topology.clone(), SyncGroup::topology)
    }

    fn transact(
        &mut self,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        match self.owned.as_mut() {
            Some(owned) => owned.transact(operation),
            None => Err(SyncRejected::new(SyncError::OwnerUnavailable, operation)),
        }
    }
}

/// Host-owned sendable synchronization state and desired level for a wasm player.
pub struct PlayerMember {
    level: AtomicF32,
    sync: GroupState<PlayerMember>,
}

impl PlayerMember {
    pub(crate) fn new(sync: GroupState<Self>, level: f32) -> Self {
        Self {
            sync,
            level: AtomicF32::new(level),
        }
    }

    /// Commits the Host-applied level after its graph batch succeeds.
    pub fn commit_host_level(&self, level: f32) {
        self.level.store(level, Ordering::Relaxed);
    }

    /// Reads the desired Host level used for later graph registration.
    #[must_use]
    pub fn host_level(&self) -> f32 {
        self.level.load(Ordering::Relaxed)
    }
}

impl BeatGrid for PlayerMember {
    delegate::delegate! {
        to self.sync {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for PlayerMember {
    type NestedGroup = Self;

    delegate::delegate! {
        to self.sync {
            fn stage_fact(&self, fact: ParentFact) -> Result<SyncStaged, SyncError>;
            fn apply_staged(&mut self, staged: SyncStaged) -> SyncTransition;
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn transact(
                &mut self,
                operation: SyncOperation<Self>,
            ) -> Result<SyncAdmission, SyncRejected<Self>>;
            fn status(&self) -> SyncStatusSnapshot;
            fn acknowledge(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }
}
