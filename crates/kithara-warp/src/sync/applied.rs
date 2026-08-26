use super::{
    LoadGeneration, PresentationFrontier, SyncOperationId, TopologyStamp, TransportRevision,
    WarpMapRevision,
};
use crate::BeatGridStamp;

/// An audible acknowledgement tied to every admission axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct SyncApplied {
    /// Synchronization operation whose activation became audible.
    #[field(get, copy)]
    operation: SyncOperationId,
    /// Track load for which the warp map was admitted.
    #[field(get, copy)]
    load: LoadGeneration,
    /// Exact group-grid identity and revision used by the warp map.
    #[field(get, copy)]
    group: BeatGridStamp,
    /// Exact ownership tree used by the warp map.
    #[field(get, copy)]
    topology: TopologyStamp,
    /// Exact session transport state used by the warp map.
    #[field(get, copy)]
    transport: TransportRevision,
    /// Exact immutable warp map consumed by the callback.
    #[field(get, copy)]
    warp_map: WarpMapRevision,
    /// Actual source/output frontier consumed by the callback.
    #[field(get, copy)]
    frontier: PresentationFrontier,
}
