use std::num::NonZeroU32;

use kithara_signal::SessionEpoch;
use kithara_warp::{BeatGrid, BeatGridId, BeatGridStamp, MapAxis};

use super::{RegisteredCell, RootError, RootPort};
use crate::{
    ControlError, ControlGuard, GroupState, ParentGridUpdate, PermitCell, PreparedRevocation,
    PublicOperation, SyncAdmission, SyncCapability, SyncError, SyncGroup, SyncMode, SyncOperation,
    SyncRejected, SyncTransition, TopologyOperation,
};

/// An owner cut that holds Control but has not yet heard the audio callback,
/// the players, or the executor: [`Self::run`] records them first.
#[must_use = "an entered cut holds Control until it runs"]
pub struct EnteredCut<'r, G: SyncGroup<NestedGroup = G>> {
    cut: RootCut<'r, G>,
}

impl<'r, G: SyncGroup<NestedGroup = G>> EnteredCut<'r, G> {
    pub(super) const fn new(
        control: ControlGuard<'r>,
        group: &'r mut GroupState<G>,
        cells: &'r mut Vec<RegisteredCell>,
    ) -> Self {
        Self {
            cut: RootCut {
                control,
                group,
                cells,
            },
        }
    }

    /// Records what the audio callback, the players and the executor left
    /// for the owner, then runs `body` under the same Control, which is
    /// released on return.
    ///
    /// # Errors
    ///
    /// Returns the first input the root could not record. That input stays
    /// where it waited, and `body` does not run.
    pub fn run<P, R, F>(self, port: &mut P, body: F) -> Result<R, RootError>
    where
        P: RootPort<G>,
        F: FnOnce(&mut RootCut<'r, G>, &mut P) -> R,
    {
        let mut cut = self.cut;
        cut.drain(port)?;
        Ok(body(&mut cut, port))
    }
}

/// The session root under Control, with the owner up to date. Every change
/// to the root is one of its methods.
pub struct RootCut<'r, G: SyncGroup<NestedGroup = G>> {
    pub(super) control: ControlGuard<'r>,
    pub(super) group: &'r mut GroupState<G>,
    pub(super) cells: &'r mut Vec<RegisteredCell>,
}

impl<G: SyncGroup<NestedGroup = G>> RootCut<'_, G> {
    /// The root group as this cut has left it so far.
    #[must_use]
    pub fn group(&self) -> &GroupState<G> {
        self.group
    }

    /// Publishes the session segment the render graph committed.
    ///
    /// # Errors
    ///
    /// Returns the refusal of a member cell or of the root group; nothing
    /// changes then.
    pub fn publish_session<P: RootPort<G>>(
        &mut self,
        port: &P,
        update: ParentGridUpdate,
    ) -> Result<(), SyncError> {
        let axis_changed = self.group.snapshot().axis() != MapAxis::Session(update.axis());
        let fence = transition_fence(&self.control, self.group, self.cells, axis_changed)?;
        let transition = self.group.publish_session(update)?;
        fence.commit(|member| axis_changed || transition_replaces_ticket(&transition, member));
        port.publish(self.group);
        Ok(())
    }

    /// Publishes a later session grid that is not available yet, on a new
    /// axis.
    ///
    /// # Errors
    ///
    /// Returns the refusal of a member cell or of the root group; nothing
    /// changes then.
    pub fn publish_unavailable_grid<P: RootPort<G>>(
        &mut self,
        port: &P,
        stamp: BeatGridStamp,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
    ) -> Result<(), SyncError> {
        let fence = transition_fence(&self.control, self.group, self.cells, true)?;
        self.group
            .publish_unavailable_grid(stamp, sample_rate, epoch)?;
        fence.commit(|_| true);
        port.publish(self.group);
        Ok(())
    }

    /// The member's end of life: its cell is retired, so no permit minted
    /// for it claims again. Returns the deck group it played in, whose
    /// retiring slots can no longer report anything the owner would apply,
    /// or `None` when the member was never registered.
    ///
    /// # Errors
    ///
    /// Returns [`RootError::Control`] when the cell is already retired.
    pub fn retire(&mut self, member: BeatGridId) -> Result<Option<BeatGridId>, RootError> {
        let Some(index) = self.cells.iter().position(|entry| entry.member() == member) else {
            return Ok(None);
        };
        self.control.retire_cell(&self.cells[index].cell)?;
        Ok(Some(self.cells.remove(index).group))
    }

    /// Transacts one operation a caller asked of the root group and publishes
    /// the root it admits. A topology edit of a grid the render graph
    /// projects is refused. A refusal changes and publishes nothing.
    ///
    /// # Errors
    ///
    /// Returns the refusal with its operation.
    pub fn transact<P: RootPort<G>>(
        &mut self,
        port: &P,
        operation: PublicOperation<G>,
    ) -> Result<SyncAdmission, SyncRejected<G>> {
        let PublicOperation(operation) = operation;
        if topology_conflicts_with_projection(port, &operation) {
            return Err(SyncRejected::new(
                SyncError::CapabilityUnavailable {
                    capability: SyncCapability::Topology,
                },
                operation,
            ));
        }
        let admission = self.verified(operation)?;
        port.publish(self.group);
        Ok(admission)
    }

    /// Preflights every affected cell before the owner changes, then revokes
    /// the tickets the admission replaces. A refusal changes nothing.
    pub(super) fn verified(
        &mut self,
        operation: SyncOperation<G>,
    ) -> Result<SyncAdmission, SyncRejected<G>> {
        let affected = affected_cells(self.group, self.cells, &operation);
        let fence = match Fence::prepare(&self.control, affected) {
            Ok(fence) => fence,
            Err(error) => {
                return Err(SyncRejected::new(
                    SyncError::ExecutionControl(error),
                    operation,
                ));
            }
        };
        let admission = self.group.transact(operation)?;
        fence.commit(|member| admission_replaces_ticket(&admission, member));
        Ok(admission)
    }
}

/// Revocations preflighted under the cut's Control for every cell an owner
/// change may replace, before that change, and published after it commits.
pub(super) struct Fence<'cut> {
    prepared: Vec<(BeatGridId, PreparedRevocation<'cut, 'cut, 'cut>)>,
}

impl<'cut> Fence<'cut> {
    /// Revokes the ticket of every member the committed change `replaced`.
    pub(super) fn commit<F>(self, replaced: F)
    where
        F: Fn(BeatGridId) -> bool,
    {
        for (member, revoke) in self.prepared {
            if replaced(member) {
                revoke.revoke();
            }
        }
    }

    /// Preflights each cell once, in order.
    ///
    /// # Errors
    ///
    /// Returns the first cell's refusal; nothing is revoked then.
    pub(super) fn prepare<I>(
        control: &'cut ControlGuard<'cut>,
        cells: I,
    ) -> Result<Self, ControlError>
    where
        I: IntoIterator<Item = &'cut PermitCell>,
    {
        let prepared = cells
            .into_iter()
            .map(|cell| {
                control
                    .preflight_revoke(cell)
                    .map(|revoke| (cell.member(), revoke))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { prepared })
    }
}

/// A root grid publication fences the cells of every deck that follows the
/// root, and every cell when the axis or epoch changes.
fn transition_fence<'cut, G: SyncGroup<NestedGroup = G>>(
    control: &'cut ControlGuard<'cut>,
    group: &GroupState<G>,
    cells: &'cut [RegisteredCell],
    axis_changed: bool,
) -> Result<Fence<'cut>, SyncError> {
    let fenced = cells
        .iter()
        .filter(|entry| {
            axis_changed
                || group.with_group(entry.group, SyncGroup::mode) == Some(SyncMode::HostSync)
        })
        .map(|entry| &*entry.cell);
    Fence::prepare(control, fenced).map_err(SyncError::ExecutionControl)
}

/// Only cells belonging to the transaction's affected deck or subtree are
/// preflighted. An unrelated deck's reserved source never blocks this edit.
fn affected_cells<'c, G: SyncGroup<NestedGroup = G>>(
    group: &GroupState<G>,
    cells: &'c [RegisteredCell],
    operation: &SyncOperation<G>,
) -> Vec<&'c PermitCell> {
    let mut targets: Vec<BeatGridId> = Vec::new();
    match operation {
        SyncOperation::Topology { operations, .. } => {
            for edit in operations {
                match edit {
                    TopologyOperation::Attach { member } => targets.push(member.id()),
                    TopologyOperation::Detach { member } => targets.push(*member),
                    TopologyOperation::Replace {
                        member,
                        replacement,
                    } => {
                        targets.push(*member);
                        targets.push(replacement.id());
                    }
                }
            }
        }
        SyncOperation::Tempo { target, .. } if *target == group.id() => {
            return cells
                .iter()
                .filter(|entry| {
                    group.with_group(entry.group, SyncGroup::mode) == Some(SyncMode::HostSync)
                })
                .map(|entry| &*entry.cell)
                .collect();
        }
        _ => targets.push(operation.target()),
    }
    cells
        .iter()
        .filter(|entry| {
            targets
                .iter()
                .any(|target| *target == entry.group || *target == entry.member())
        })
        .map(|entry| &*entry.cell)
        .collect()
}

fn admission_replaces_ticket(admission: &SyncAdmission, member: BeatGridId) -> bool {
    match admission {
        SyncAdmission::Prepared(preparation) => preparation.stamp().member().grid_id() == member,
        SyncAdmission::StateChanged { transition, .. }
        | SyncAdmission::TopologyChanged { transition, .. } => {
            transition_replaces_ticket(transition, member)
        }
        _ => false,
    }
}

fn transition_replaces_ticket(transition: &SyncTransition, member: BeatGridId) -> bool {
    transition
        .issued()
        .iter()
        .any(|preparation| preparation.stamp().member().grid_id() == member)
        || transition
            .withdrawn()
            .iter()
            .any(|stamp| stamp.member().grid_id() == member)
}

fn topology_conflicts_with_projection<G: SyncGroup<NestedGroup = G>, P: RootPort<G>>(
    port: &P,
    operation: &SyncOperation<G>,
) -> bool {
    let SyncOperation::Topology { operations, .. } = operation else {
        return false;
    };
    operations.iter().any(|operation| match operation {
        TopologyOperation::Attach { member } => port.is_projected(member.id()),
        TopologyOperation::Detach { member } => port.is_projected(*member),
        TopologyOperation::Replace {
            member,
            replacement,
        } => port.is_projected(*member) || port.is_projected(replacement.id()),
    })
}
