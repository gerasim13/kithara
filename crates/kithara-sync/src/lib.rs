#![forbid(unsafe_code)]

//! Recursive synchronization-group ownership and its control-plane protocol.

mod owner;
mod protocol;

pub use owner::GroupState;
pub use protocol::{
    AlignmentSource, LoadGeneration, ReconcileCause, SyncAdmission, SyncApplied, SyncCapability,
    SyncError, SyncGroup, SyncGroupSnapshot, SyncGroupTopologyError, SyncIntent, SyncMember,
    SyncMemberKind, SyncMemberSnapshot, SyncOperation, SyncOperationId, SyncRejected,
    SyncStatusSnapshot, TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation,
};
