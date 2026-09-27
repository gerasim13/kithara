#![forbid(unsafe_code)]

//! Recursive synchronization-group ownership and its control-plane protocol.

mod execution;
mod owner;
mod protocol;
mod root;

pub use execution::{
    AppliedSource, ArmPermit, AudioClaim, ClaimError, ControlEnterError, ControlError,
    ControlGuard, ExecutedGroup, PendingSourceChange, PermitCell, PermitState, PreparedRevocation,
    ReceiptReservation, ReceiptSink, SourceReservation, SourceRevision, StagePort, SyncArbiter,
    SyncAttachment, SyncExecution, SyncExecutor, SyncGateBinding, SyncReceiptAck, SyncReceiptInbox,
    SyncReceiptTx, sync_receipts,
};
pub use owner::{GroupState, SyncStaged};
pub use protocol::{
    AlignmentSource, LoadGeneration, ObservedEntry, ParentFact, ParentGridUpdate, ParentWithdrawal,
    PublicOperation, SessionAxisUpdate, SourceChange, SyncAdmission, SyncApplied, SyncCapability,
    SyncEffect, SyncError, SyncExecutionReject, SyncExecutionStamp, SyncGroup, SyncGroupSnapshot,
    SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind, SyncMemberSnapshot, SyncMode,
    SyncOperation, SyncOperationId, SyncPreparation, SyncReceipt, SyncRejected, SyncStatusSnapshot,
    SyncTransition, TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation,
};
pub use root::{
    DEFAULT_OWNER_WAIT, EnteredCut, InboxAt, RegisteredCell, RootCut, RootError, RootPort,
    SyncRoot, SyncRootConfig,
};
mod consts;
