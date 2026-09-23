use kithara_signal::SessionFrame;
use kithara_warp::{BeatGridSnapshot, MapAxis, WarpMapRevision};

use super::{
    lifecycle::Applied,
    preparation::{Pending, Refreshed},
    state::{GroupState, Withdrawal, validate_successor},
    timeline::Timeline,
};
use crate::{
    ParentGridUpdate, SessionAxisUpdate, SyncError, SyncGroup, SyncMember, SyncOperationId,
};

/// A timeline fact one group passes on to its direct child groups.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Descent {
    Parent(ParentGridUpdate),
    Axis(SessionAxisUpdate),
}

/// One group's successor grid together with everything it moves: the
/// decisions and applied maps of its direct members, the identities they
/// spend, and the segment its child groups follow, computed before mutation.
pub(super) struct Staged {
    pub(super) grid: BeatGridSnapshot,
    timeline: Timeline,
    parent: Option<ParentGridUpdate>,
    descent: Option<Descent>,
    pub(super) pending: Vec<Pending>,
    applied: Vec<Applied>,
    next_map: Option<WarpMapRevision>,
    next_operation: Option<SyncOperationId>,
}

/// Where a staged grid takes over, and the identities a retarget may spend.
#[derive(Clone, Copy)]
pub(super) struct Takeover {
    pub(super) commit: Option<SessionFrame>,
    pub(super) next_operation: Option<SyncOperationId>,
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Checks that every direct child group accepts `descent` before any of
    /// them changes.
    pub(super) fn check_descent(&self, descent: Option<Descent>) -> Result<(), SyncError> {
        let Some(descent) = descent else {
            return Ok(());
        };
        self.members.iter().try_for_each(|member| match member {
            SyncMember::Group { group, .. } => match descent {
                Descent::Parent(update) => group.check_parent(update),
                Descent::Axis(update) => group.check_axis(update),
            },
            SyncMember::Grid { .. } => Ok(()),
        })
    }

    /// Passes `descent` on to every direct child group; call only after
    /// [`Self::check_descent`] accepted it on the same members.
    pub(super) fn descend(&mut self, descent: Option<Descent>) -> Result<(), SyncError> {
        let Some(descent) = descent else {
            return Ok(());
        };
        self.members.iter_mut().try_for_each(|member| match member {
            SyncMember::Group { group, .. } => match descent {
                Descent::Parent(update) => group.accept_parent(update),
                Descent::Axis(update) => group.accept_axis(update),
            },
            SyncMember::Grid { .. } => Ok(()),
        })
    }

    pub(super) fn stage_parent(
        &self,
        update: ParentGridUpdate,
    ) -> Result<Option<Staged>, SyncError> {
        if let Some(current) = self.parent {
            let (current_stamp, given) = (current.parent(), update.parent());
            if current == update {
                return Ok(None);
            }
            if current_stamp.grid_id() == given.grid_id()
                && given.revision() <= current_stamp.revision()
            {
                return Err(SyncError::StaleGridRevision {
                    current: current_stamp,
                    given,
                });
            }
        }
        let (grid, descent) = match self.timeline {
            Timeline::Host => {
                let (grid, descent) =
                    self.derived_grid(update.epoch(), update.anchor(), update.meter())?;
                validate_successor(&self.grid, &grid, Withdrawal::Refused)?;
                (grid, Some(descent))
            }
            Timeline::Off | Timeline::Local(_) => (self.grid.clone(), None),
        };
        let takeover = Takeover {
            commit: Some(update.anchor().frame()),
            next_operation: self.next_operation,
        };
        self.stage(grid, self.timeline, Some(update), descent, takeover)
            .map(Some)
    }

    pub(super) fn stage_axis(
        &self,
        update: SessionAxisUpdate,
    ) -> Result<Option<Staged>, SyncError> {
        let axis = MapAxis::Session(update.axis());
        if axis == self.grid.axis() {
            return Ok(None);
        }
        let grid = BeatGridSnapshot::unavailable(self.grid.id(), self.next_revision()?, axis);
        validate_successor(&self.grid, &grid, Withdrawal::Refused)?;
        self.stage(
            grid,
            self.timeline.on_new_axis(),
            None,
            Some(Descent::Axis(update)),
            Takeover {
                commit: None,
                next_operation: self.next_operation,
            },
        )
        .map(Some)
    }

    /// Stages `grid` under `timeline` as this group's successor, carrying
    /// every member decision and applied map onto it.
    pub(super) fn stage(
        &self,
        grid: BeatGridSnapshot,
        timeline: Timeline,
        parent: Option<ParentGridUpdate>,
        descent: Option<Descent>,
        takeover: Takeover,
    ) -> Result<Staged, SyncError> {
        let Refreshed {
            pending,
            applied,
            next_map,
            next_operation,
        } = self.refreshed(&grid, timeline, takeover)?;
        Ok(Staged {
            grid,
            timeline,
            parent,
            descent,
            pending,
            applied,
            next_map,
            next_operation,
        })
    }

    pub(super) fn check_staged(&self, staged: Option<&Staged>) -> Result<(), SyncError> {
        staged.map_or(Ok(()), |staged| self.check_descent(staged.descent))
    }

    pub(super) fn commit_staged(&mut self, staged: Option<Staged>) -> Result<(), SyncError> {
        let Some(staged) = staged else {
            return Ok(());
        };
        self.check_descent(staged.descent)?;
        self.grid = staged.grid;
        self.timeline = staged.timeline;
        self.parent = staged.parent;
        self.pending = staged.pending;
        self.applied = staged.applied;
        self.next_map = staged.next_map;
        self.next_operation = staged.next_operation;
        self.descend(staged.descent)
    }
}
