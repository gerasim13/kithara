use super::{
    AlignmentPlanRevision, LoadGeneration, PresentationFrontier, SyncOperationId, TopologyStamp,
    TransportRevision,
};
use crate::MapStamp;

/// An audible acknowledgement tied to every admission axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct SyncApplied {
    /// Synchronization operation whose activation became audible.
    #[field(get, copy)]
    operation: SyncOperationId,
    /// Track load for which the plan was admitted.
    #[field(get, copy)]
    load: LoadGeneration,
    /// Exact group-map identity and revision used by the plan.
    #[field(get, copy)]
    group: MapStamp,
    /// Exact ownership tree used by the plan.
    #[field(get, copy)]
    topology: TopologyStamp,
    /// Exact session transport state used by the plan.
    #[field(get, copy)]
    transport: TransportRevision,
    /// Exact immutable alignment plan consumed by the callback.
    #[field(get, copy)]
    plan: AlignmentPlanRevision,
    /// Actual source/output frontier consumed by the callback.
    #[field(get, copy)]
    frontier: PresentationFrontier,
}
