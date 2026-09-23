#![forbid(unsafe_code)]

//! Recursive synchronization-group ownership and its control-plane protocol.

mod owner;
mod protocol;

pub use owner::{GroupState, SyncStaged};
pub use protocol::{
    AlignmentSource, LoadGeneration, ParentFact, ParentGridUpdate, ParentWithdrawal,
    SessionAxisUpdate, SyncAdmission, SyncApplied, SyncCapability, SyncEffect, SyncError,
    SyncExecutionReject, SyncExecutionStamp, SyncGroup, SyncGroupSnapshot, SyncGroupTopologyError,
    SyncIntent, SyncMember, SyncMemberKind, SyncMemberSnapshot, SyncMode, SyncOperation,
    SyncOperationId, SyncPreparation, SyncReceipt, SyncRejected, SyncStatusSnapshot,
    SyncTransition, TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation,
};
