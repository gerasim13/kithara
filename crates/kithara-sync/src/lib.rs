#![forbid(unsafe_code)]

//! Recursive synchronization-group ownership and its control-plane protocol.

mod execution;
mod owner;
mod protocol;
mod root;

pub use execution::{
    ActivationHead, AppliedSource, ArmPermit, AudioClaim, ClaimError, ControlEnterError,
    ControlError, ControlGuard, ExecutedGroup, LoadedMedia, PendingSourceChange, PermitCell,
    PermitState, PreparedFirst, PreparedRevocation, ReceiptReservation, ReceiptSink,
    SourceReservation, SourceRevision, StagePort, Staged, SyncArbiter, SyncAttachment,
    SyncExecution, SyncExecutor, SyncGateBinding, SyncReceiptAck, SyncReceiptInbox, SyncReceiptTx,
    SyncTicket, sync_receipts,
};
pub use owner::{GroupState, SyncStaged};
pub use protocol::{
    AlignmentSource, LoadGeneration, ParentFact, ParentGridUpdate, ParentWithdrawal,
    PublicOperation, SessionAxisUpdate, SourceChange, SyncAdmission, SyncApplied, SyncCapability,
    SyncEffect, SyncError, SyncExecutionReject, SyncExecutionStamp, SyncGroup, SyncGroupSnapshot,
    SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind, SyncMemberSnapshot, SyncMode,
    SyncOperation, SyncOperationId, SyncPreparation, SyncReceipt, SyncRejected, SyncStatusSnapshot,
    SyncTransition, TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation,
};
pub use root::{
    ClockRefusal, DEFAULT_OWNER_WAIT, EnteredCut, EntryPort, EntryRefusal, InboxAt,
    ProcessedTransport, ResidentLoadObservation, ResidentRender, ResidentStaging, RootCut,
    RootError, RootPort, SyncRoot, SyncRootConfig, Waiting,
};
mod consts;
