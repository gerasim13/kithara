#![forbid(unsafe_code)]

//! Beat-grid warping and synchronization contracts.

mod anchor;
mod beat_grid;
mod coordinate;
mod segment;
mod sync;

pub use anchor::{CoordinateError, SessionAnchor, SessionBeat, SessionFrame};
pub use beat_grid::{
    BeatEstimate, BeatGrid, BeatGridId, BeatGridIdAllocationError, BeatGridQuery, BeatGridRegion,
    BeatGridRevision, BeatGridSnapshot, BeatGridSnapshotError, BeatGridStamp, BeatGridState,
    BeatGridUnavailable, BeatGridView,
};
pub(crate) use coordinate::AxisKind;
pub use coordinate::{
    AssetAxis, AssetFrame, Beat, BeatOrdinal, FrameUncertainty, MapAxis, MapCoordinateError,
    MapPoint, MapPosition, SessionAxis, SessionEpoch,
};
pub use segment::{
    BeatEvidence, BeatMarker, BeatsPerMinute, BeatsPerMinuteError, MapRegion, MapRegionError,
    MapSegment, Meter, MeterError, MeterFacts, SegmentEndpoint, SegmentError, SegmentFacts,
    SegmentSet,
};
pub use sync::{
    AlignmentSource, BeatAlignment, LoadGeneration, PresentationFrontier, ReconcileCause,
    SyncAdmission, SyncApplied, SyncCapability, SyncError, SyncGroup, SyncGroupSnapshot,
    SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind, SyncMemberSnapshot,
    SyncOperation, SyncOperationId, SyncRejected, SyncStatusSnapshot, TopologyOperation,
    TopologyRevision, TopologyStamp, TransportOperation, TransportRevision, WarpMapRevision,
};
