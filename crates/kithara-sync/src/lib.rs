#![forbid(unsafe_code)]

//! Recursive synchronization-group ownership and its control-plane protocol.

mod execution;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod owner;
mod protocol;
mod root;

pub use execution::{
    ActivationAudio, ActivationControl, ActivationDeck, ActivationHead, ActivationResident,
    AppliedSource, ArmPermit, BlockSource, ControlEnterError, ControlError, ExecutedGroup,
    LoadedMedia, PreparedFirst, ReceiptSink, ReturnRoom, ReturnedTicket, SourceReservation,
    SourceRevision, StagePort, Staged, SyncAttachment, SyncAttempt, SyncCallback, SyncExecution,
    SyncExecutor, SyncGateBinding, SyncKind, SyncReceiptAck, SyncReceiptInbox, SyncReceiptTx,
    SyncReturn, SyncTicket, TicketRoom, TrackDisposal, activation_channels, sync_receipts,
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
