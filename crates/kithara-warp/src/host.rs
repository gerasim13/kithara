use super::{
    AlignmentPlan, AlignmentRequest, BeatMap, BeatMapId, BeatMapRevision, BeatMapSnapshot,
    HostAxis, HostEpoch, MapAxis, MapStamp, MeterFacts, PlanTransition, PresentationFrontier,
    SessionAnchor, SyncError,
};
use crate::beat_map::BeatMapGeometry;

/// Immutable ephemeral host-map view over one committed session anchor.
#[derive(Clone, Debug)]
pub struct HostBeatMap {
    snapshot: BeatMapSnapshot,
}

impl HostBeatMap {
    /// Creates a live host map for one committed session-clock epoch.
    #[must_use]
    pub fn new(
        id: BeatMapId,
        revision: BeatMapRevision,
        epoch: HostEpoch,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Self {
        let axis = MapAxis::Host(HostAxis::new(anchor.sample_rate(), epoch));
        Self {
            snapshot: BeatMapSnapshot::new_host(id, revision, axis, anchor, meter),
        }
    }

    /// Returns the immutable identity and revision carried by this observation.
    #[must_use]
    pub fn stamp(&self) -> MapStamp {
        self.snapshot.stamp()
    }

    /// Returns the session-frame generation carried by this observation.
    #[must_use]
    pub fn epoch(&self) -> HostEpoch {
        match self.snapshot.axis() {
            MapAxis::Host(axis) => axis.epoch(),
            MapAxis::Asset(_) => unreachable!("HostBeatMap always carries a host axis"),
        }
    }

    /// Returns the committed session-clock anchor carried by this observation.
    #[must_use]
    pub fn anchor(&self) -> SessionAnchor {
        match &self.snapshot.data.geometry {
            BeatMapGeometry::Host { anchor, .. } => *anchor,
            BeatMapGeometry::Segments(_) => {
                unreachable!("HostBeatMap always carries host geometry")
            }
        }
    }

    /// Returns the optional meter carried by this observation.
    #[must_use]
    pub fn meter(&self) -> Option<MeterFacts> {
        match &self.snapshot.data.geometry {
            BeatMapGeometry::Host { meter, .. } => *meter,
            BeatMapGeometry::Segments(_) => {
                unreachable!("HostBeatMap always carries host geometry")
            }
        }
    }
}

impl BeatMap for HostBeatMap {
    delegate::delegate! {
        to self.snapshot {
            fn id(&self) -> BeatMapId;
            #[call(clone)]
            fn snapshot(&self) -> BeatMapSnapshot;
            fn align_to(
                &self,
                target: &dyn BeatMap,
                request: AlignmentRequest,
            ) -> Result<AlignmentPlan, SyncError>;
            fn reconcile_to(
                &self,
                target: &dyn BeatMap,
                active: &AlignmentPlan,
                frontier: PresentationFrontier,
            ) -> Result<PlanTransition, SyncError>;
        }
    }
}
