use super::{
    AlignmentPlanRevision, LoadGeneration, PresentationFrontier, SyncOperationId, TopologyStamp,
    TransportRevision,
};
use crate::MapStamp;

/// An audible acknowledgement tied to every admission axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SyncApplied {
    /// Synchronization operation whose activation became audible.
    operation: SyncOperationId,
    /// Track load for which the plan was admitted.
    load: LoadGeneration,
    /// Exact group-map identity and revision used by the plan.
    group: MapStamp,
    /// Exact ownership tree used by the plan.
    topology: TopologyStamp,
    /// Exact session transport state used by the plan.
    transport: TransportRevision,
    /// Exact immutable alignment plan consumed by the callback.
    plan: AlignmentPlanRevision,
    /// Actual source/output frontier consumed by the callback.
    frontier: PresentationFrontier,
}

impl SyncApplied {
    /// Returns the synchronization operation that became audible.
    #[must_use]
    pub const fn operation(self) -> SyncOperationId {
        self.operation
    }

    /// Returns the track-load generation that reached the renderer.
    #[must_use]
    pub const fn load(self) -> LoadGeneration {
        self.load
    }

    /// Returns the group-map identity and revision used by the audible plan.
    #[must_use]
    pub const fn group(self) -> MapStamp {
        self.group
    }

    /// Returns the topology identity and revision used by the audible plan.
    #[must_use]
    pub const fn topology(self) -> TopologyStamp {
        self.topology
    }

    /// Returns the committed transport revision used by the renderer.
    #[must_use]
    pub const fn transport(self) -> TransportRevision {
        self.transport
    }

    /// Returns the immutable alignment-plan revision that became audible.
    #[must_use]
    pub const fn plan(self) -> AlignmentPlanRevision {
        self.plan
    }

    /// Returns the actual consumed source/output frontier.
    #[must_use]
    pub const fn frontier(self) -> PresentationFrontier {
        self.frontier
    }
}
