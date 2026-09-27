use kithara_warp::BeatGridId;

use crate::{ControlEnterError, ControlError, SyncError};

/// Why the session root refused an owner input or a cut.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RootError {
    /// Control could not be entered.
    #[error(transparent)]
    Enter(ControlEnterError),
    /// An install raced a source change the owner had not reconciled yet;
    /// its terminal rejection waits for the next cut.
    #[error("an install raced an unreconciled source change")]
    InstallRacedSourceChange,
    /// The deck or the member already has a cell.
    #[error("sync member is already registered: {0:?}")]
    MemberAlreadyRegistered(BeatGridId),
    /// No cell is registered for the member.
    #[error("sync member is not registered: {0:?}")]
    MemberNotRegistered(BeatGridId),
    /// A slot inbox carried a receipt only the executor sends.
    #[error("RT mailbox carried a non-audio sync receipt")]
    NonAudioReceipt,
    /// The executor sent a receipt only the audio callback sends.
    #[error("executor sent an audio receipt")]
    AudioReceiptFromExecutor,
    /// The member already holds a different gate failure the owner has not
    /// recorded.
    #[error("member already has an undrained gate failure")]
    GateFailurePending,
    /// A member cell refused the owner.
    #[error(transparent)]
    Control(#[from] ControlError),
    /// The root group refused the owner.
    #[error(transparent)]
    Sync(#[from] SyncError),
}
