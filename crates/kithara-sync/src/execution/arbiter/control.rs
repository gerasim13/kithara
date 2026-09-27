use kithara_platform::sync::atomic::Ordering;

use super::{
    ArmPermit, PermitCell, SourceRevision, SyncArbiter,
    cell::{change_of, revision_of},
    consts,
};
use crate::{SourceChange, SyncExecutionStamp};

/// One committed source change as the Host read it from a member cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingSourceChange {
    change: SourceChange,
    source: u64,
}

impl PendingSourceChange {
    /// The strongest change committed since the last reconciliation.
    #[must_use]
    pub const fn change(self) -> SourceChange {
        self.change
    }
}

/// A checked revocation held under the same Control phase as the owner
/// transaction. Preflight every affected cell before mutating owner state.
#[must_use]
pub struct PreparedRevocation<'cell, 'guard, 'arbiter> {
    cell: &'cell PermitCell,
    _guard: &'guard ControlGuard<'arbiter>,
    next_revision: u64,
}

impl PreparedRevocation<'_, '_, '_> {
    /// Publish the preflighted revocation after the owner transition commits.
    pub fn revoke(self) {
        self.cell
            .permit_revision
            .store(self.next_revision, Ordering::Release);
    }
}

/// Refusal of an owner-side permit or source mutation operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ControlError {
    #[error("permit cell belongs to another member")]
    WrongMember,
    #[error("member cell is retired")]
    CellRetired,
    #[error("member revision space is exhausted")]
    RevisionExhausted,
    #[error("member source is reserved by its player")]
    SourceReserved,
    #[error("member source changed since the owner last reconciled it")]
    SourceChanged,
}

/// Short owner phase. Host drains available RT receipts before transacting the
/// synchronization group under this guard.
#[must_use]
pub struct ControlGuard<'a> {
    pub(super) arbiter: &'a SyncArbiter,
}

impl<'arbiter> ControlGuard<'arbiter> {
    /// Mint the permit in the same owner reply that accepts Installed.
    ///
    /// A player editing the source does not refuse the permit: it names the
    /// committed source, so the edit parks it until the change is reconciled
    /// and the permit withdrawn, or the edit aborts and the permit claims.
    ///
    /// # Errors
    ///
    /// Returns an error when the cell belongs to another member or is
    /// retired, or when its source changed unreconciled.
    pub fn mint_permit(
        &self,
        cell: &PermitCell,
        stamp: SyncExecutionStamp,
    ) -> Result<ArmPermit, ControlError> {
        if stamp.member().grid_id() != cell.member {
            return Err(ControlError::WrongMember);
        }
        if cell.retired.load(Ordering::Acquire) {
            return Err(ControlError::CellRetired);
        }
        let source = cell.source.load(Ordering::Acquire);
        if change_of(source) != consts::UNCHANGED {
            return Err(ControlError::SourceChanged);
        }
        Ok(ArmPermit {
            stamp,
            permit_revision: cell.permit_revision.load(Ordering::Acquire),
            source_revision: revision_of(source),
        })
    }

    /// The source revision a Host decision may rely on. A reservation taken
    /// after this read parks every permit minted since until its change is
    /// reconciled or aborted.
    ///
    /// # Errors
    /// Returns an error when the source is reserved or changed unreconciled.
    pub fn current_source(&self, cell: &PermitCell) -> Result<SourceRevision, ControlError> {
        if cell.reserved.load(Ordering::Acquire) {
            return Err(ControlError::SourceReserved);
        }
        let source = cell.source.load(Ordering::Acquire);
        if change_of(source) != consts::UNCHANGED {
            return Err(ControlError::SourceChanged);
        }
        Ok(SourceRevision(revision_of(source)))
    }

    /// The committed source change the owner has not reconciled.
    #[must_use]
    pub fn source_change(&self, cell: &PermitCell) -> Option<PendingSourceChange> {
        let source = cell.source.load(Ordering::Acquire);
        let bits = change_of(source);
        if bits == consts::UNCHANGED {
            return None;
        }
        let change = if bits & consts::DISCONTINUITY == 0 {
            SourceChange::Timing
        } else {
            SourceChange::Discontinuity
        };
        Some(PendingSourceChange { change, source })
    }

    /// Mark `observed` reconciled after the owner transition that withdrew
    /// its stale decisions has committed. A change committed after the read
    /// stays pending for the next reconciliation.
    pub fn acknowledge_source_change(&self, cell: &PermitCell, observed: PendingSourceChange) {
        let _ = cell.source.compare_exchange(
            observed.source,
            observed.source & !consts::CHANGE_MASK,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Check one affected member before the owner changes group state. The
    /// caller must preflight every distinct affected cell exactly once before
    /// that transaction, then consume every token without interleaving other
    /// mutations of those cells.
    ///
    /// # Errors
    ///
    /// Returns an error when this cell is retired or its revision is spent.
    pub fn preflight_revoke<'cell, 'guard>(
        &'guard self,
        cell: &'cell PermitCell,
    ) -> Result<PreparedRevocation<'cell, 'guard, 'arbiter>, ControlError> {
        if cell.retired.load(Ordering::Acquire) {
            return Err(ControlError::CellRetired);
        }
        let next = cell
            .permit_revision
            .load(Ordering::Relaxed)
            .checked_add(1)
            .ok_or(ControlError::RevisionExhausted)?;
        Ok(PreparedRevocation {
            cell,
            _guard: self,
            next_revision: next,
        })
    }

    /// Permanently retire one member cell after that player's RT users have
    /// quiesced. Other member cells remain available in this session.
    ///
    /// # Errors
    ///
    /// Returns an error if the cell is already retired.
    pub fn retire_cell(&self, cell: &PermitCell) -> Result<(), ControlError> {
        if cell.retired.load(Ordering::Relaxed) {
            return Err(ControlError::CellRetired);
        }
        cell.retired.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for ControlGuard<'_> {
    fn drop(&mut self) {
        let _ = self.arbiter.phase.compare_exchange(
            consts::CONTROL,
            consts::OPEN,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}
