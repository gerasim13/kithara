use kithara_platform::sync::Arc;

use super::{BeatMapId, BeatMapRevision, MapStamp, MapState, MapUnavailable};
use crate::{HostAxis, HostEpoch, MapAxis, MeterFacts, SegmentSet, SessionAnchor};

/// One immutable, revisioned musical-map observation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BeatMapSnapshot {
    pub(crate) data: Arc<BeatMapSnapshotData>,
}

/// A caller-supplied snapshot violates the public musical-map contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BeatMapSnapshotError {
    /// The proposed replacement was derived from any snapshot but the current one.
    #[error("beat map successor is stale: expected {expected:?}, got {given:?}")]
    Stale { expected: MapStamp, given: MapStamp },
    /// A successor changed its native coordinate axis outside a host restart.
    #[error("beat map successor changed axis from {expected:?} to {given:?}")]
    AxisChanged { expected: MapAxis, given: MapAxis },
    /// A proposed successor did not advance the owner's published revision.
    #[error("beat map revision {given} does not advance {current}")]
    RevisionNotAdvanced {
        current: BeatMapRevision,
        given: BeatMapRevision,
    },
    /// The lifecycle state is incompatible with the snapshot coordinate axis.
    #[error("state {state:?} is invalid for segment geometry on axis {axis:?}")]
    InvalidState { axis: MapAxis, state: MapState },
    /// A complete bounded map cannot return to an incomplete lifecycle state.
    #[error("beat map cannot transition from {from:?} to {to:?}")]
    InvalidTransition { from: MapState, to: MapState },
}

#[derive(Debug)]
pub(crate) struct BeatMapSnapshotData {
    pub(crate) id: BeatMapId,
    pub(crate) revision: BeatMapRevision,
    pub(crate) state: MapState,
    pub(crate) axis: MapAxis,
    pub(crate) geometry: BeatMapGeometry,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BeatMapGeometry {
    Segments(SegmentSet),
    Host {
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    },
}

impl BeatMapSnapshot {
    /// Creates the first validated segment-backed snapshot for a map owner.
    ///
    /// # Errors
    ///
    /// Returns [`BeatMapSnapshotError`] when `state` is invalid for the
    /// coordinate axis carried by `segments`.
    pub fn initial(
        id: BeatMapId,
        state: MapState,
        segments: SegmentSet,
    ) -> Result<Self, BeatMapSnapshotError> {
        Self::try_new_segments(id, BeatMapRevision::first(), state, segments)
    }

    /// Creates the first empty snapshot for a map owner without geometry.
    #[must_use]
    pub fn unavailable(id: BeatMapId, axis: MapAxis) -> Self {
        Self::new_segments(
            id,
            BeatMapRevision::first(),
            MapState::Unavailable(MapUnavailable::NoGeometry),
            SegmentSet::empty(axis),
        )
    }

    /// Advances to a newer unavailable snapshot, optionally entering a newer host epoch.
    ///
    /// # Errors
    ///
    /// Returns [`BeatMapSnapshotError`] for a stale base, a non-advancing
    /// revision, an asset-axis change, a non-advancing host-axis change, or a
    /// terminal-map transition.
    pub fn unavailable_successor(
        &self,
        base: MapStamp,
        revision: BeatMapRevision,
        axis: MapAxis,
    ) -> Result<Self, BeatMapSnapshotError> {
        let expected = self.stamp();
        if base != expected {
            return Err(BeatMapSnapshotError::Stale {
                expected,
                given: base,
            });
        }
        if revision <= self.revision() {
            return Err(BeatMapSnapshotError::RevisionNotAdvanced {
                current: self.revision(),
                given: revision,
            });
        }
        if self.state() == MapState::Complete {
            return Err(BeatMapSnapshotError::InvalidTransition {
                from: self.state(),
                to: MapState::Unavailable(MapUnavailable::NoGeometry),
            });
        }
        let axis_is_valid = self.axis() == axis
            || matches!(
                (self.axis(), axis),
                (MapAxis::Host(current), MapAxis::Host(next))
                    if next.epoch() > current.epoch()
            );
        if !axis_is_valid {
            return Err(BeatMapSnapshotError::AxisChanged {
                expected: self.axis(),
                given: axis,
            });
        }
        Ok(Self::new_segments(
            self.id(),
            revision,
            MapState::Unavailable(MapUnavailable::NoGeometry),
            SegmentSet::empty(axis),
        ))
    }

    /// Derives a newer live host snapshot from this owner's current state.
    ///
    /// # Errors
    ///
    /// Returns [`BeatMapSnapshotError`] for a stale base, a non-advancing
    /// revision, or a non-host or non-advancing host axis.
    pub fn host_successor(
        &self,
        base: MapStamp,
        revision: BeatMapRevision,
        epoch: HostEpoch,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Result<Self, BeatMapSnapshotError> {
        let expected = self.stamp();
        if base != expected {
            return Err(BeatMapSnapshotError::Stale {
                expected,
                given: base,
            });
        }
        if revision <= self.revision() {
            return Err(BeatMapSnapshotError::RevisionNotAdvanced {
                current: self.revision(),
                given: revision,
            });
        }
        let axis = MapAxis::Host(HostAxis::new(anchor.sample_rate(), epoch));
        let axis_is_valid = self.axis() == axis
            || matches!(
                (self.axis(), axis),
                (MapAxis::Host(current), MapAxis::Host(next))
                    if next.epoch() > current.epoch()
            );
        if !axis_is_valid {
            return Err(BeatMapSnapshotError::AxisChanged {
                expected: self.axis(),
                given: axis,
            });
        }
        Ok(Self::new_host(self.id(), revision, axis, anchor, meter))
    }

    fn try_new_segments(
        id: BeatMapId,
        revision: BeatMapRevision,
        state: MapState,
        segments: SegmentSet,
    ) -> Result<Self, BeatMapSnapshotError> {
        let axis = segments.axis();
        if matches!(
            (axis, state),
            (MapAxis::Asset(_), MapState::Live)
                | (MapAxis::Host(_), MapState::Complete)
                | (_, MapState::Unavailable(_))
        ) {
            return Err(BeatMapSnapshotError::InvalidState { axis, state });
        }
        Ok(Self::new_segments(id, revision, state, segments))
    }

    fn new_segments(
        id: BeatMapId,
        revision: BeatMapRevision,
        state: MapState,
        segments: SegmentSet,
    ) -> Self {
        let axis = segments.axis();
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id,
                revision,
                state,
                axis,
                geometry: BeatMapGeometry::Segments(segments),
            }),
        }
    }

    pub(crate) fn new_host(
        id: BeatMapId,
        revision: BeatMapRevision,
        axis: MapAxis,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Self {
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id,
                revision,
                state: MapState::Live,
                axis,
                geometry: BeatMapGeometry::Host { anchor, meter },
            }),
        }
    }
}

impl PartialEq for BeatMapSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
            && self.revision() == other.revision()
            && self.state() == other.state()
            && self.axis() == other.axis()
            && self.data.geometry == other.data.geometry
    }
}
