use kithara_platform::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use kithara_warp::BeatGridId;

use super::{ControlError, SyncArbiter, consts};
use crate::{SourceChange, SyncExecutionStamp};

const fn change_bits(change: SourceChange) -> u64 {
    match change {
        SourceChange::Timing => consts::TIMING,
        SourceChange::Discontinuity => consts::DISCONTINUITY,
    }
}

pub(super) const fn revision_of(source: u64) -> u64 {
    source >> consts::CHANGE_SHIFT
}

pub(super) const fn change_of(source: u64) -> u64 {
    source & consts::CHANGE_MASK
}

/// The Host's stable gate and member cell carried to one attached player.
#[derive(Clone)]
pub struct SyncGateBinding {
    arbiter: Arc<SyncArbiter>,
    cell: Arc<PermitCell>,
}

impl SyncGateBinding {
    /// Bind the Host-owned arbiter to the cell of one attached member.
    #[must_use]
    pub fn new(arbiter: Arc<SyncArbiter>, cell: Arc<PermitCell>) -> Self {
        Self { arbiter, cell }
    }

    /// Session arbiter shared with the audio callback.
    #[must_use]
    pub fn arbiter(&self) -> &SyncArbiter {
        &self.arbiter
    }

    /// Stable cell for this player's track member.
    #[must_use]
    pub fn cell(&self) -> &PermitCell {
        &self.cell
    }

    /// What this exact owner-issued permit allows the audio callback now.
    /// The audio claim repeats these checks under the gate before changing PCM.
    #[must_use]
    pub fn permit_state(&self, permit: &ArmPermit) -> PermitState {
        if permit.stamp.member().grid_id() != self.cell.member
            || self.cell.retired.load(Ordering::Acquire)
            || !permit.matches(&self.cell)
        {
            PermitState::Withdrawn
        } else if permit.source_current(&self.cell) {
            PermitState::Current
        } else {
            PermitState::Parked
        }
    }

    /// The member's current source, as a player observes it before asking
    /// the Host to act on that observation.
    #[must_use]
    pub fn source_revision(&self) -> SourceRevision {
        SourceRevision(revision_of(self.cell.source.load(Ordering::Acquire)))
    }

    /// Take the exclusive right to change this member's source, without
    /// waiting on the Host or the audio callback. While the right is held,
    /// every permit for this member parks instead of claiming, including one
    /// the Host mints meanwhile. Other members are unaffected.
    ///
    /// # Errors
    /// Returns an error when the source is already reserved or its revision
    /// space is spent.
    pub fn reserve_source(&self) -> Result<SourceReservation, ControlError> {
        if self.cell.reserved.swap(true, Ordering::AcqRel) {
            return Err(ControlError::SourceReserved);
        }
        let reservation = SourceReservation {
            binding: self.clone(),
        };
        if revision_of(self.cell.source.load(Ordering::Acquire)) == consts::MAX_REVISION {
            return Err(ControlError::RevisionExhausted);
        }
        Ok(reservation)
    }
}

/// What an owner-issued permit allows the audio callback at one moment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermitState {
    /// The permit is current and its source unchanged: the ticket may claim.
    Current,
    /// The player is changing, or has changed, the source the permit names.
    /// The ticket waits without a terminal receipt until the owner withdraws
    /// it or the change aborts.
    Parked,
    /// The owner withdrew the permit or retired the member.
    Withdrawn,
}

/// One observed source identity of a member. It moves on every committed
/// change of that source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceRevision(pub(super) u64);

/// The newest source revision one audio callback applied and rendered.
///
/// The callback reads the member's revision before draining its commands and
/// publishes it once that block's render evidence is out. A reader loads it
/// before the evidence, which then reflects every change committed up to it.
#[derive(Debug, Default)]
pub struct AppliedSource(AtomicU64);

impl AppliedSource {
    /// Record `source` as applied by a block whose evidence is published.
    pub fn publish(&self, source: SourceRevision) {
        self.0.store(source.0 + 1, Ordering::Release);
    }

    /// The newest applied revision; `None` before the first rendered block.
    #[must_use]
    pub fn load(&self) -> Option<SourceRevision> {
        self.0
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(SourceRevision)
    }
}

/// A player's exclusive right to change one member's source.
///
/// Publishing reports the committed change and releases the member. Dropping
/// it unpublished aborts: the source is unchanged and nothing is reported.
#[must_use]
pub struct SourceReservation {
    binding: SyncGateBinding,
}

impl SourceReservation {
    /// Report `change` as committed and release the member. The change stays
    /// pending until the Host reconciles it; later changes coalesce into the
    /// strongest one.
    pub fn publish(self, change: SourceChange) {
        let bits = change_bits(change);
        let _ =
            self.binding
                .cell
                .source
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    Some(
                        ((revision_of(current) + 1) << consts::CHANGE_SHIFT)
                            | change_of(current)
                            | bits,
                    )
                });
    }
}

impl Drop for SourceReservation {
    fn drop(&mut self) {
        self.binding.cell.reserved.store(false, Ordering::Release);
    }
}

/// Stable per-member claim identity. The Host allocates and retains the cell;
/// an RT ticket carries a reference to that same allocation.
pub struct PermitCell {
    pub(super) member: BeatGridId,
    pub(super) permit_revision: AtomicU64,
    pub(super) retired: AtomicBool,
    /// A player holds the member's source reservation.
    pub(super) reserved: AtomicBool,
    /// Revision of the source the member plays above the strongest committed
    /// change the Host has not reconciled. One word, so a change committed
    /// after the Host read it cannot be lost to its acknowledgement.
    pub(super) source: AtomicU64,
}

impl PermitCell {
    /// Allocate one cell for the lifetime of `member`'s Host registration.
    #[must_use]
    pub const fn new(member: BeatGridId) -> Self {
        Self {
            member,
            permit_revision: AtomicU64::new(1),
            retired: AtomicBool::new(false),
            reserved: AtomicBool::new(false),
            source: AtomicU64::new(consts::UNCHANGED),
        }
    }

    /// Identity this cell was allocated for.
    #[must_use]
    pub const fn member(&self) -> BeatGridId {
        self.member
    }
}

/// Owner-minted authority for one exact installed preparation and source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArmPermit {
    pub(super) stamp: SyncExecutionStamp,
    pub(super) permit_revision: u64,
    pub(super) source_revision: u64,
}

impl ArmPermit {
    /// The preparation admitted by the owner in the Installed reply.
    #[must_use]
    pub const fn stamp(&self) -> SyncExecutionStamp {
        self.stamp
    }

    pub(super) fn matches(&self, cell: &PermitCell) -> bool {
        self.permit_revision == cell.permit_revision.load(Ordering::Acquire)
    }

    pub(super) fn source_current(&self, cell: &PermitCell) -> bool {
        !cell.reserved.load(Ordering::Acquire)
            && self.source_revision == revision_of(cell.source.load(Ordering::Acquire))
    }
}
