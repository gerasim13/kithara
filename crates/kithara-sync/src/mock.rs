//! Owner stand-ins for tests of the member side of the gate: a player, its
//! audio callback and its executor, without a session root.

use kithara_platform::sync::Arc;
use kithara_warp::BeatGridId;

pub use crate::execution::ReceiptSinkMock;
use crate::{
    ArmPermit, ControlEnterError, RootError, SourceChange, SyncExecutionStamp, SyncGateBinding,
    execution::{ControlGuard, PendingSourceChange, PermitCell, SyncArbiter},
};

/// The owner of one member behind its own gate. It enters the gate as a
/// session root does, reconciles what the member's player changed, revokes
/// the member's permit, and mints the permit of an installed lane.
#[derive(Clone)]
pub struct MemberOwner {
    gate: SyncGateBinding,
}

impl MemberOwner {
    /// The owner of `member` behind a fresh open gate.
    #[must_use]
    pub fn new(member: BeatGridId) -> Self {
        Self {
            gate: SyncGateBinding::new(
                Arc::new(SyncArbiter::new()),
                Arc::new(PermitCell::new(member)),
            ),
        }
    }

    /// The gate the member's player and audio callback claim through.
    #[must_use]
    pub fn gate(&self) -> SyncGateBinding {
        self.gate.clone()
    }

    /// The member this owner holds.
    #[must_use]
    pub fn member(&self) -> BeatGridId {
        self.gate.cell().member()
    }

    /// The source change the member's player committed and the owner has
    /// not reconciled, left pending.
    ///
    /// # Errors
    ///
    /// Returns `Enter(Busy)` while an audio claim holds the gate.
    pub fn pending_change(&self) -> Result<Option<SourceChange>, RootError> {
        let control = self.enter()?;
        Ok(self
            .gate
            .cell()
            .source_change(&control)
            .map(PendingSourceChange::change))
    }

    /// Reconciles the source change the member's player committed, as a
    /// session root does before any work, and returns it.
    ///
    /// # Errors
    ///
    /// Returns `Enter(Busy)` while an audio claim holds the gate.
    pub fn reconcile(&self) -> Result<Option<SourceChange>, RootError> {
        let control = self.enter()?;
        let Some(observed) = self.gate.cell().source_change(&control) else {
            return Ok(None);
        };
        self.gate
            .cell()
            .acknowledge_source_change(&control, observed);
        Ok(Some(observed.change()))
    }

    /// Revokes the member's permit, withdrawing any ticket minted against it.
    ///
    /// # Errors
    ///
    /// Returns `Enter(Busy)` while an audio claim holds the gate, and the
    /// cell's refusal once it is retired or its revisions are spent.
    pub fn revoke(&self) -> Result<(), RootError> {
        let control = self.enter()?;
        self.gate.cell().preflight_revoke(&control)?.revoke();
        Ok(())
    }

    /// Mints the permit of the installed lane `stamp` names, as a session
    /// root does in the answer to its install receipt.
    ///
    /// # Errors
    ///
    /// Returns `Enter(Busy)` while an audio claim holds the gate, and the
    /// cell's refusal when the stamp names another member, the cell is
    /// retired, or its source changed unreconciled.
    pub fn mint(&self, stamp: SyncExecutionStamp) -> Result<ArmPermit, RootError> {
        let control = self.enter()?;
        Ok(self.gate.cell().mint_permit(&control, stamp)?)
    }

    fn enter(&self) -> Result<ControlGuard<'_>, RootError> {
        self.gate
            .arbiter()
            .try_control()
            .ok_or(RootError::Enter(ControlEnterError::Busy))
    }
}
