use kithara_platform::{CancelToken, maybe_send::MaybeSendFuture, tokio::runtime::Handle};
use kithara_warp::WarpPlan;

use super::{ActivationHead, Staged, SyncTicket};
use crate::{ArmPermit, SyncExecutionReject, SyncGateBinding, SyncReceipt};

mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

/// Opens the staged lanes of one load of a member's media and hands each
/// installed lane to the member's audio path.
pub trait StagePort: Clone + Send + 'static {
    /// Identifies the Player's item a load of media is of.
    type Item: Copy + Eq + Send + 'static;
    /// Holds a staged lane's prepared PCM until the preparation it serves
    /// ends.
    type Lane: Send + 'static;

    /// The runtime staging and receipt delivery run on.
    fn runtime(&self) -> &Handle;

    /// The gate the member's audio path claims each activation through.
    fn gate(&self) -> &SyncGateBinding;

    /// Opens a lane that plays `plan` from `head` and resolves once its
    /// prepared PCM is proven, with the first frame it decoded, or with the
    /// reason the lane cannot be held.
    fn stage(
        self,
        plan: WarpPlan,
        head: ActivationHead,
        cancel: CancelToken,
    ) -> impl MaybeSendFuture<Output = Result<Staged<Self::Lane>, SyncExecutionReject>> + 'static;

    /// Transfer one exact installed lane to the member's audio path before
    /// its activation can be claimed.
    ///
    /// # Errors
    ///
    /// Returns a pre-Armed rejection if the ready span or bounded handoff
    /// capacity is no longer available. The lane is retired off RT.
    fn handoff(self, ticket: SyncTicket<Self::Item, Self::Lane>)
    -> Result<(), SyncExecutionReject>;
}

/// The owner's answer to one executor receipt. An Installed answer carries
/// the exact permit minted in the same owner acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncReceiptAck {
    /// A non-installation receipt was recorded.
    Recorded,
    /// The installed preparation was recorded and may enter the audio path.
    Installed(ArmPermit),
    /// The owner refused the receipt.
    Refused,
    /// The owner gate or session failed before recording the receipt.
    GateFailed,
}

/// The group owner an executor reports the outcome of each staged lane to.
#[kithara::mock(api = ReceiptSinkMock)]
pub trait ReceiptSink: Send + Sync {
    /// Whether an owner is bound to take receipts at all.
    fn is_bound(&self) -> bool;

    /// Hands one receipt to the owner and blocks until its exact answer.
    fn acknowledge(&self, receipt: SyncReceipt) -> SyncReceiptAck;
}
