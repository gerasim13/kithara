use kithara_signal::{SessionEpoch, SessionFrame};
use kithara_warp::{
    BeatGridQuery, BeatGridSnapshot, BeatsPerMinute, MapAxis, MapPoint, MapPosition, MapRegion,
    MeterFacts, SessionAnchor, SessionBeat,
};

use super::{
    state::{GroupState, Withdrawal, validate_successor},
    transaction::take_operation,
};
use crate::{
    ParentGridUpdate, SyncAdmission, SyncCapability, SyncError, SyncGroup, SyncIntent, SyncMode,
    SyncOperationId,
};

const SECONDS_PER_MINUTE: f64 = 60.0;

/// The beat timeline one group follows, owned together with its mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Timeline {
    /// No musical timeline; an external owner may publish the grid.
    Off,
    /// The group's own tempo and phase, once a first tempo established them.
    Local(Option<LocalTimeline>),
    /// The parent's accepted segment, recorded on the group state.
    Host,
}

/// A local tempo trajectory together with the meter it carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LocalTimeline {
    anchor: SessionAnchor,
    meter: Option<MeterFacts>,
}

/// A status a group reports until a later operation supersedes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Blocked {
    /// The operation needs grid coverage not yet published.
    Waiting {
        operation: SyncOperationId,
        required: MapRegion,
    },
    /// The operation needs a capability this group does not have.
    Unavailable {
        operation: SyncOperationId,
        capability: SyncCapability,
    },
}

/// A mode operation evaluated against frozen state, before any mutation.
enum ModeEffect {
    Changed {
        timeline: Timeline,
        grid: BeatGridSnapshot,
    },
    Unchanged,
    Deferred {
        required: MapRegion,
    },
}

impl Timeline {
    pub(super) const fn without_geometry(mode: SyncMode) -> Self {
        match mode {
            SyncMode::Off => Self::Off,
            SyncMode::LocalSync => Self::Local(None),
            SyncMode::HostSync => Self::Host,
        }
    }

    pub(super) const fn mode(self) -> SyncMode {
        match self {
            Self::Off => SyncMode::Off,
            Self::Local(_) => SyncMode::LocalSync,
            Self::Host => SyncMode::HostSync,
        }
    }

    pub(super) fn tempo(self, parent: Option<&ParentGridUpdate>) -> Option<BeatsPerMinute> {
        let beats_per_second = match self {
            Self::Off | Self::Local(None) => return None,
            Self::Local(Some(local)) => local.anchor.target_beats_per_second(),
            Self::Host => parent?.anchor().target_beats_per_second(),
        };
        BeatsPerMinute::try_from(beats_per_second * SECONDS_PER_MINUTE).ok()
    }

    /// The same mode on a new physical axis, where no frame of the old one
    /// is meaningful.
    pub(super) const fn on_new_axis(self) -> Self {
        match self {
            Self::Local(_) => Self::Local(None),
            Self::Off | Self::Host => self,
        }
    }
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Applies one mode intent addressed to this group.
    pub(super) fn transact_intent(
        &mut self,
        intent: SyncIntent,
        activation: SessionFrame,
    ) -> Result<SyncAdmission, SyncError> {
        self.reserve_operation()?;
        let effect = match intent {
            SyncIntent::Enable | SyncIntent::AlignNow => self.follow_parent()?,
            SyncIntent::Disable => self.latch(activation)?,
            SyncIntent::Free => self.leave_timeline()?,
        };
        self.commit(effect)
    }

    /// Commits a tempo on a group that owns its timeline.
    pub(super) fn transact_tempo(
        &mut self,
        tempo: BeatsPerMinute,
        commit: SessionFrame,
        smoothing: f64,
    ) -> Result<SyncAdmission, SyncError> {
        let beats_per_second = f64::from(tempo) / SECONDS_PER_MINUTE;
        let local = match self.timeline {
            Timeline::Off => {
                return Err(SyncError::CapabilityUnavailable {
                    capability: SyncCapability::Transport,
                });
            }
            Timeline::Host => {
                return Err(SyncError::TempoInherited {
                    owner: self.grid.id(),
                });
            }
            Timeline::Local(Some(local)) => LocalTimeline {
                anchor: local.anchor.retarget(commit, beats_per_second, smoothing)?,
                meter: local.meter,
            },
            Timeline::Local(None) => {
                let MapAxis::Session(axis) = self.grid.axis() else {
                    return Err(SyncError::InvalidGroupGridState {
                        state: self.grid.state(),
                    });
                };
                LocalTimeline {
                    anchor: SessionAnchor::new(
                        commit,
                        SessionBeat::default(),
                        beats_per_second,
                        axis.sample_rate(),
                    )?,
                    meter: None,
                }
            }
        };
        self.reserve_operation()?;
        let effect = self.local_effect(local)?;
        self.commit(effect)
    }

    fn follow_parent(&self) -> Result<ModeEffect, SyncError> {
        if matches!(self.timeline, Timeline::Host) {
            return Ok(ModeEffect::Unchanged);
        }
        let grid = match self.parent {
            Some(parent) => self.derived_grid(parent.epoch(), parent.anchor(), parent.meter())?,
            None => self.withdrawn_grid()?,
        };
        validate_successor(&self.grid, &grid, Withdrawal::Allowed)?;
        Ok(ModeEffect::Changed {
            timeline: Timeline::Host,
            grid,
        })
    }

    /// Fixes the beat and tempo actually playing at `activation` as the
    /// group's own timeline, including mid-way through a tempo approach.
    fn latch(&self, activation: SessionFrame) -> Result<ModeEffect, SyncError> {
        if matches!(self.timeline, Timeline::Local(_)) {
            return Ok(ModeEffect::Unchanged);
        }
        let Some(local) = latch_at(&self.grid, activation)? else {
            return Ok(ModeEffect::Deferred {
                required: MapRegion::point(MapPosition::Session(activation)),
            });
        };
        self.local_effect(local)
    }

    fn leave_timeline(&self) -> Result<ModeEffect, SyncError> {
        if matches!(self.timeline, Timeline::Off) {
            return Ok(ModeEffect::Unchanged);
        }
        let grid = self.withdrawn_grid()?;
        validate_successor(&self.grid, &grid, Withdrawal::Allowed)?;
        Ok(ModeEffect::Changed {
            timeline: Timeline::Off,
            grid,
        })
    }

    fn local_effect(&self, local: LocalTimeline) -> Result<ModeEffect, SyncError> {
        let MapAxis::Session(axis) = self.grid.axis() else {
            return Err(SyncError::InvalidGroupGridState {
                state: self.grid.state(),
            });
        };
        let grid = self.derived_grid(axis.epoch(), local.anchor, local.meter)?;
        validate_successor(&self.grid, &grid, Withdrawal::Refused)?;
        Ok(ModeEffect::Changed {
            timeline: Timeline::Local(Some(local)),
            grid,
        })
    }

    pub(super) fn derived_grid(
        &self,
        epoch: SessionEpoch,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Result<BeatGridSnapshot, SyncError> {
        Ok(BeatGridSnapshot::session(
            self.grid.id(),
            self.next_revision()?,
            epoch,
            anchor,
            meter,
        ))
    }

    fn withdrawn_grid(&self) -> Result<BeatGridSnapshot, SyncError> {
        Ok(BeatGridSnapshot::unavailable(
            self.grid.id(),
            self.next_revision()?,
            self.grid.axis(),
        ))
    }

    fn reserve_operation(&self) -> Result<SyncOperationId, SyncError> {
        self.next_operation
            .ok_or_else(|| SyncError::OperationIdExhausted {
                group_id: self.grid.id(),
            })
    }

    fn commit(&mut self, effect: ModeEffect) -> Result<SyncAdmission, SyncError> {
        let operation = take_operation(self.grid.id(), &mut self.next_operation)?;
        let topology = self.topology_stamp();
        Ok(match effect {
            ModeEffect::Changed { timeline, grid } => {
                self.timeline = timeline;
                self.grid = grid;
                self.blocked = None;
                SyncAdmission::StateChanged {
                    operation,
                    topology,
                    mode: timeline.mode(),
                    grid: self.grid.stamp(),
                }
            }
            ModeEffect::Unchanged => {
                self.blocked = None;
                SyncAdmission::Unchanged {
                    operation,
                    topology,
                }
            }
            ModeEffect::Deferred { required } => {
                self.blocked = Some(Blocked::Waiting {
                    operation,
                    required,
                });
                SyncAdmission::Deferred {
                    operation,
                    topology,
                    required,
                }
            }
        })
    }
}

/// Reads the beat, tempo, and meter actually playing at `frame`, or `None`
/// while the grid cannot answer there yet.
fn latch_at(
    grid: &BeatGridSnapshot,
    frame: SessionFrame,
) -> Result<Option<LocalTimeline>, SyncError> {
    let MapAxis::Session(axis) = grid.axis() else {
        return Ok(None);
    };
    let position = MapPoint::new(grid.stamp(), MapPosition::Session(frame));
    let (BeatGridQuery::Resolved(beat), BeatGridQuery::Resolved(tempo)) =
        (grid.beat_at(position), grid.tempo_at(position))
    else {
        return Ok(None);
    };
    let meter = match grid.meter_at(*beat.value()) {
        BeatGridQuery::Resolved(meter) => Some(MeterFacts::new(
            *meter.value(),
            meter.evidence(),
            meter.uncertainty(),
        )),
        _ => None,
    };
    let anchor = SessionAnchor::new(
        frame,
        SessionBeat::new(f64::from(*beat.value().value()))?,
        f64::from(*tempo.value()) / SECONDS_PER_MINUTE,
        axis.sample_rate(),
    )?;
    Ok(Some(LocalTimeline { anchor, meter }))
}
