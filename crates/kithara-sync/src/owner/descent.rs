use kithara_warp::{BeatGridSnapshot, MapAxis};

use super::{
    state::{GroupState, Withdrawal, validate_successor},
    timeline::Timeline,
};
use crate::{ParentGridUpdate, SessionAxisUpdate, SyncError, SyncGroup, SyncMember};

/// A timeline fact one group passes on to its direct child groups.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Descent {
    Parent(ParentGridUpdate),
    Axis(SessionAxisUpdate),
}

/// One group's reaction to a fact from its parent, computed before mutation.
pub(super) struct Staged {
    grid: BeatGridSnapshot,
    timeline: Timeline,
    parent: Option<ParentGridUpdate>,
    descent: Option<Descent>,
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
                let grid = self.derived_grid(update.epoch(), update.anchor(), update.meter())?;
                validate_successor(&self.grid, &grid, Withdrawal::Refused)?;
                let segment = ParentGridUpdate::new(
                    grid.stamp(),
                    update.epoch(),
                    update.anchor(),
                    update.meter(),
                );
                (grid, Some(Descent::Parent(segment)))
            }
            Timeline::Off | Timeline::Local(_) => (self.grid.clone(), None),
        };
        Ok(Some(Staged {
            grid,
            timeline: self.timeline,
            parent: Some(update),
            descent,
        }))
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
        Ok(Some(Staged {
            grid,
            timeline: self.timeline.on_new_axis(),
            parent: None,
            descent: Some(Descent::Axis(update)),
        }))
    }

    pub(super) fn check_staged(&self, staged: Option<&Staged>) -> Result<(), SyncError> {
        staged.map_or(Ok(()), |staged| self.check_descent(staged.descent))
    }

    pub(super) fn commit_staged(&mut self, staged: Option<Staged>) -> Result<(), SyncError> {
        let Some(staged) = staged else {
            return Ok(());
        };
        self.check_descent(staged.descent)?;
        if matches!(staged.descent, Some(Descent::Axis(_))) {
            self.pending.clear();
        }
        self.grid = staged.grid;
        self.timeline = staged.timeline;
        self.parent = staged.parent;
        self.descend(staged.descent)
    }
}
