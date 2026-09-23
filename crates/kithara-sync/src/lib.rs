#![forbid(unsafe_code)]

//! Recursive synchronization-group ownership and its control-plane protocol.

mod owner;
mod protocol;

pub use owner::GroupState;
pub use protocol::{
    AlignmentSource, LoadGeneration, ParentGridUpdate, SessionAxisUpdate, SyncAdmission,
    SyncApplied, SyncCapability, SyncEffect, SyncError, SyncExecutionStamp, SyncGroup,
    SyncGroupSnapshot, SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind,
    SyncMemberSnapshot, SyncMode, SyncOperation, SyncOperationId, SyncPreparation, SyncRejected,
    SyncStatusSnapshot, TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation,
};
