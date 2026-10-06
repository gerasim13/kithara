use kithara_warp::{BeatGrid, BeatGridId, BeatGridSnapshot};

use super::SyncExecution;
use crate::{
    ParentFact, SyncAdmission, SyncError, SyncGroup, SyncGroupSnapshot, SyncOperation, SyncReceipt,
    SyncRejected, SyncStaged, SyncStatusSnapshot, SyncTransition,
};

/// A synchronization group whose staged preparations an executor carries
/// out: it refuses what the executor cannot stage before the group admits
/// it, and hands the executor every preparation the group issues or
/// withdraws.
pub struct ExecutedGroup<G> {
    group: G,
    execution: SyncExecution,
}

impl<G: SyncGroup> ExecutedGroup<G> {
    /// `group`, its staged preparations carried out through `execution`.
    #[must_use]
    pub const fn new(group: G, execution: SyncExecution) -> Self {
        Self { group, execution }
    }
}

impl<G: SyncGroup> BeatGrid for ExecutedGroup<G> {
    delegate::delegate! {
        to self.group {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl<G: SyncGroup> SyncGroup for ExecutedGroup<G> {
    type NestedGroup = G::NestedGroup;

    fn apply_staged(&mut self, staged: SyncStaged) -> SyncTransition {
        let transition = self.group.apply_staged(staged);
        self.execution.follow_transition(&transition);
        transition
    }

    fn transact(
        &mut self,
        operation: SyncOperation<Self::NestedGroup>,
    ) -> Result<SyncAdmission, SyncRejected<Self::NestedGroup>> {
        if let Err(error) = self.execution.admit(&operation) {
            return Err(SyncRejected::new(error, operation));
        }
        let relocation = matches!(operation, SyncOperation::Relocate { .. });
        let admission = self.group.transact(operation)?;
        self.execution.follow_admission(&admission, relocation);
        Ok(admission)
    }

    delegate::delegate! {
        to self.group {
            fn stage_fact(&self, fact: ParentFact) -> Result<SyncStaged, SyncError>;
            fn status(&self) -> SyncStatusSnapshot;
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn acknowledge(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }
}
