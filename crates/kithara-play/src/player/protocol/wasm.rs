use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridSnapshot, SyncAdmission, SyncApplied, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncOperation, SyncRejected, SyncStatusSnapshot,
};
use portable_atomic::{AtomicF32, Ordering};

use crate::sync::GroupState;

pub(crate) struct PlayerSync {
    owned: Option<GroupState<PlayerMember>>,
    grid: BeatGridSnapshot,
    topology: Result<SyncGroupSnapshot, SyncError>,
    status: SyncStatusSnapshot,
}

impl PlayerSync {
    pub(crate) fn new(owned: GroupState<PlayerMember>) -> Self {
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

    fn status(&self) -> SyncStatusSnapshot {
        self.owned.as_ref().map_or(self.status, SyncGroup::status)
    }

    fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        self.owned
            .as_mut()
            .map_or(Err(SyncError::OwnerUnavailable), |owned| {
                owned.acknowledge(applied)
            })
    }
}

/// Host-owned sendable synchronization state and desired level for a wasm player.
pub struct PlayerMember {
    sync: GroupState<PlayerMember>,
    level: AtomicF32,
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
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn transact(
                &mut self,
                operation: SyncOperation<Self>,
            ) -> Result<SyncAdmission, SyncRejected<Self>>;
            fn status(&self) -> SyncStatusSnapshot;
            fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }
}
