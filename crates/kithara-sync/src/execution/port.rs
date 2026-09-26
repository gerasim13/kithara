use kithara_platform::{CancelToken, maybe_send::MaybeSendFuture, tokio::runtime::Handle};
use kithara_warp::WarpPlan;

use crate::{SyncExecutionReject, SyncReceipt};

/// Opens the staged lanes of one load of a member's media.
pub trait StagePort: Clone + Send + 'static {
    /// Identifies one load of the member's media.
    type Media: Copy + Eq + Send + 'static;
    /// Holds a staged lane's prepared PCM until the preparation it serves
    /// ends.
    type Lane: Send + 'static;

    /// The runtime staging and receipt delivery run on.
    fn runtime(&self) -> &Handle;

    /// Opens a lane that plays `plan` and resolves once its prepared PCM is
    /// proven, or with the reason the lane cannot be held.
    fn stage(
        self,
        plan: WarpPlan,
        cancel: CancelToken,
    ) -> impl MaybeSendFuture<Output = Result<Self::Lane, SyncExecutionReject>> + 'static;
}

/// The group owner an executor reports the outcome of each staged lane to.
pub trait ReceiptSink: Send + Sync {
    /// Whether an owner is bound to take receipts at all.
    fn is_bound(&self) -> bool;

    /// Hands one receipt to the owner and blocks until it answers; returns
    /// whether the owner recorded it.
    fn acknowledge(&self, receipt: SyncReceipt) -> bool;
}
