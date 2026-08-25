#![forbid(unsafe_code)]

//! Beat-map alignment and synchronization contracts.

mod anchor;
mod beat_map;
mod coordinate;
mod host;
mod map;
mod segment;
mod sync;

pub use anchor::{SessionAnchor, SessionBeat, SessionFrame};
pub use beat_map::{
    BeatEstimate, BeatMap, BeatMapId, BeatMapIdAllocationError, BeatMapRevision, BeatMapSnapshot,
    BeatMapSnapshotError, MapQuery, MapStamp, MapState, MapUnavailable,
};
pub(crate) use coordinate::AxisKind;
pub use coordinate::{
    AssetAxis, AssetFrame, Beat, BeatOrdinal, FrameUncertainty, HostAxis, HostEpoch, MapAxis,
    MapCoordinateError, MapPoint, MapPosition,
};
pub use host::HostBeatMap;
pub use map::CoordinateError;
pub use segment::{
    BeatEvidence, BeatMarker, BeatsPerMinute, MapRegion, MapSegment, Meter, MeterError, MeterFacts,
    SegmentEndpoint, SegmentError, SegmentFacts, SegmentSet,
};
pub use sync::{
    AlignmentCursor, AlignmentPlan, AlignmentPlanError, AlignmentPlanRevision, AlignmentRequest,
    AlignmentSource, AlignmentTransition, BeatAlignment, LoadGeneration, PlanSpan, PlanSpanSlot,
    PlanTransition, PlannedRenderSpan, PresentationFrontier, ReconcileCause, RenderFrontier,
    RenderPlan, SourceFrameRange, SyncAdmission, SyncApplied, SyncCapability, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind,
    SyncMemberSnapshot, SyncOperation, SyncOperationId, SyncRejected, SyncStatusSnapshot,
    TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation, TransportRevision,
};
