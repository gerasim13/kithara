use kithara_sync::{ReceiptSink, SyncExecutionReject, SyncExecutor, SyncReceipt, SyncReceiptAck};
use kithara_test_utils::kithara;
use tracing::debug;

use crate::{PlayError, resource::SlotStaging, session::SessionHandle};

/// Executor of the preparations a player's group issues for its track.
pub(crate) type SyncStaging = SyncExecutor<SlotStaging>;

impl<S: 'static> ReceiptSink for SessionHandle<S>
where
    Self: Send + Sync,
{
    fn is_bound(&self) -> bool {
        self.dispatcher().is_ok()
    }

    fn acknowledge(&self, receipt: SyncReceipt) -> SyncReceiptAck {
        let answer = self.acknowledge_sync(receipt).unwrap_or_else(|error| {
            debug!(%error, "sync: an executor receipt did not reach its owner");
            match error {
                PlayError::SessionGone { .. } => SyncReceiptAck::GateFailed,
                _ => SyncReceiptAck::Refused,
            }
        });
        let delivered = match receipt {
            SyncReceipt::Installed(stamp) => Some((stamp, 0)),
            SyncReceipt::Rejected { stamp, reason } => Some((stamp, reject_code(reason))),
            _ => None,
        };
        if let Some((stamp, rejected)) = delivered {
            let accepted = matches!(
                answer,
                SyncReceiptAck::Recorded | SyncReceiptAck::Installed(_)
            );
            kithara::probe_event!(
                sync_receipt_delivered,
                operation = u64::from(stamp.operation()),
                rejected = rejected,
                accepted = u64::from(accepted)
            );
        }
        answer
    }
}

/// Probe code of a rejection; `0` stands for an installed lane.
const fn reject_code(reason: SyncExecutionReject) -> u64 {
    match reason {
        SyncExecutionReject::Geometry => 1,
        SyncExecutionReject::Late => 2,
        SyncExecutionReject::Capacity => 3,
        SyncExecutionReject::ControlBusy => 6,
        SyncExecutionReject::Cancelled => 4,
        SyncExecutionReject::Media => 5,
        _ => u64::MAX,
    }
}
