use super::{
    BeatMap, BeatMapId, BeatMapRevision, BeatMapSnapshot, HostAxis, HostEpoch, MapAxis, MeterFacts,
    SessionAnchor,
};

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
}

impl BeatMap for HostBeatMap {
    delegate::delegate! {
        to self.snapshot {
            fn id(&self) -> BeatMapId;
            #[call(clone)]
            fn snapshot(&self) -> BeatMapSnapshot;
        }
    }
}
