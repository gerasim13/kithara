use kithara_platform::{
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, WallInstant},
};

use super::{ArmPermit, ControlGuard, PermitCell, consts};
use crate::ReceiptReservation;

/// Arbitrates one session's owner mutations and audio claims without owning
/// the synchronization ledger. The Host owns this value for the session.
pub struct SyncArbiter {
    pub(super) phase: AtomicU64,
    /// Owner entries waiting for an in-progress claim; each blocks new claims.
    pub(super) owner_waiting: AtomicU64,
}

impl Default for SyncArbiter {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncArbiter {
    /// Create an open session arbiter before attaching any member.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            phase: AtomicU64::new(consts::OPEN),
            owner_waiting: AtomicU64::new(0),
        }
    }

    /// Enter the owner phase with one attempt. A busy owner may retry off RT.
    #[must_use]
    pub fn try_control(&self) -> Option<ControlGuard<'_>> {
        self.phase
            .compare_exchange(
                consts::OPEN,
                consts::CONTROL,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()
            .map(|_| ControlGuard { arbiter: self })
    }

    /// Enter the sole Host dispatcher phase after an in-progress audio claim.
    /// A bounded wait lets a shutdown command run if a callback abandoned its
    /// claim. Waiting blocks later RT claims so new candidates cannot starve
    /// an owner receipt or member retirement.
    ///
    /// # Errors
    /// Returns `Busy` when the callback has not finished its claim within
    /// `wait`; the caller may retry off RT. Returns `Closed` after callback
    /// quiescence has tombstoned this session.
    pub(crate) fn enter_host_control(
        &self,
        wait: Duration,
    ) -> Result<ControlGuard<'_>, ControlEnterError> {
        let deadline = WallInstant::now() + wait;
        self.owner_waiting.fetch_add(1, Ordering::AcqRel);
        let entered = loop {
            if self.phase.load(Ordering::Acquire) == consts::CLOSED {
                break Err(ControlEnterError::Closed);
            }
            if let Some(control) = self.try_control() {
                break Ok(control);
            }
            if WallInstant::now() >= deadline {
                break Err(ControlEnterError::Busy);
            }
            thread::yield_now();
        };
        self.owner_waiting.fetch_sub(1, Ordering::AcqRel);
        entered
    }

    /// Claim a ready audio candidate with one gate CAS and no wait or lock.
    /// The claim takes the candidate's reserved receipt pair, so the gate
    /// reopens only once both receipts are written. First-span validity and
    /// the activation frame are the RT caller's preconditions.
    ///
    /// # Errors
    ///
    /// Returns the precise busy, closed, wrong-member, stale-permit, or
    /// parked-source refusal before an audio change. The reservation is
    /// released with the refusal.
    pub fn try_claim<'r>(
        &self,
        permit: &ArmPermit,
        cell: &PermitCell,
        receipts: ReceiptReservation<'r>,
    ) -> Result<AudioClaim<'_, 'r>, ClaimError> {
        debug_assert_eq!(
            receipts.stamp(),
            permit.stamp,
            "the reserved receipts report the permit's preparation"
        );
        if permit.stamp.member().grid_id() != cell.member {
            return Err(ClaimError::WrongMember);
        }
        if cell.retired.load(Ordering::Acquire) {
            return Err(ClaimError::CellRetired);
        }
        if self.owner_waiting.load(Ordering::Acquire) != 0 {
            return Err(ClaimError::Busy);
        }
        if !permit.matches(cell) {
            return Err(ClaimError::StalePermit);
        }
        if !permit.source_current(cell) {
            return Err(ClaimError::SourceParked);
        }
        self.phase
            .compare_exchange(
                consts::OPEN,
                consts::AUDIO_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|phase| {
                if phase == consts::CLOSED {
                    ClaimError::Closed
                } else {
                    ClaimError::Busy
                }
            })?;

        // Control could have entered and left between the first check and
        // the CAS. No owner mutation can run after this check until release.
        let result = if cell.retired.load(Ordering::Acquire) {
            Err(ClaimError::CellRetired)
        } else if self.owner_waiting.load(Ordering::Acquire) != 0 {
            Err(ClaimError::Busy)
        } else if !permit.matches(cell) {
            Err(ClaimError::StalePermit)
        } else if !permit.source_current(cell) {
            Err(ClaimError::SourceParked)
        } else {
            Ok(AudioClaim {
                arbiter: self,
                receipts,
            })
        };
        if result.is_err() {
            let _ = self.phase.compare_exchange(
                consts::AUDIO_CLAIMED,
                consts::OPEN,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        result
    }

    /// Tombstone the session after its audio callback and owner work have
    /// quiesced. This also closes a claim abandoned before both receipts.
    pub(crate) fn close_quiescent(&self) {
        self.phase.store(consts::CLOSED, Ordering::Release);
    }
}

/// An owner entry failed before any group state was changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ControlEnterError {
    /// An audio callback did not release its claim within the owner budget.
    #[error("audio claim did not complete within the owner wait budget")]
    Busy,
    /// Callback quiescence closed this session's gate.
    #[error("session claim gate is closed")]
    Closed,
}

/// Refusal of an RT claim before any audio state changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ClaimError {
    #[error("session commit gate is busy")]
    Busy,
    #[error("session is closed")]
    Closed,
    #[error("permit cell belongs to another member")]
    WrongMember,
    #[error("member cell is retired")]
    CellRetired,
    #[error("arm permit is stale")]
    StalePermit,
    #[error("member source is changing or changed")]
    SourceParked,
}

/// The sole RT owner of the session gate until both claim receipts are queued.
/// Dropping this value without finishing leaves the gate claimed, so no owner
/// may proceed on an audio change it has not heard about.
#[must_use]
pub struct AudioClaim<'a, 'r> {
    pub(super) arbiter: &'a SyncArbiter,
    receipts: ReceiptReservation<'r>,
}

impl AudioClaim<'_, '_> {
    /// Complete after a nonempty prefetched span was consumed: write Armed
    /// and Presented into the reserved slots, then reopen the gate.
    #[inline]
    pub fn finish(self) {
        self.receipts.publish();
        let _ = self.arbiter.phase.compare_exchange(
            consts::AUDIO_CLAIMED,
            consts::OPEN,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}
