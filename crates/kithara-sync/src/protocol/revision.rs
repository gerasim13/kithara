use kithara_signal::Revision;
use kithara_warp::BeatGridId;

/// The revision domain for synchronization-group topology.
pub enum TopologyRevisionTag {}

/// Monotonic revision of one synchronization-group topology.
pub type TopologyRevision = Revision<TopologyRevisionTag>;

/// The revision domain for synchronization operations.
pub enum SyncOperationIdTag {}

/// Monotonic identity of one synchronization operation.
pub type SyncOperationId = Revision<SyncOperationIdTag>;

/// The revision domain for track loads into stable decks.
pub enum LoadGenerationTag {}

/// Monotonic identity of one track load into a stable deck.
pub type LoadGeneration = Revision<LoadGenerationTag>;

/// Identity and immutable revision of one group topology snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct TopologyStamp {
    /// Returns the stable identity of the group grid.
    #[field(get, copy)]
    pub(crate) group_id: BeatGridId,
    /// Returns the immutable topology revision.
    #[field(get, copy)]
    revision: TopologyRevision,
}

impl TopologyStamp {
    /// Creates a composite topology stamp.
    #[must_use]
    pub const fn new(group_id: BeatGridId, revision: TopologyRevision) -> Self {
        Self { group_id, revision }
    }
}
