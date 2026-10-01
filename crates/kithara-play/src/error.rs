use kithara_bufpool::PoolError;
use kithara_platform::time::Duration;
use kithara_render::{RenderError, ResponseError};

use crate::{api::SlotId, session::SessionError};

#[derive(Clone, Debug, kithara_derive::Mirror, thiserror::Error)]
#[mirror(from = RenderError)]
#[non_exhaustive]
pub enum PlayError {
    #[error("player is closed")]
    #[mirror(skip)]
    Closed,

    #[error("player not ready")]
    #[mirror(skip)]
    NotReady,

    #[error("no active slot")]
    #[mirror(skip)]
    NoActiveSlot,

    #[error("slot command channel full: {slot:?}")]
    #[mirror(skip)]
    SlotChannelFull { slot: SlotId },

    #[error("item {index} has no resource (already consumed)")]
    #[mirror(skip)]
    ItemConsumed { index: usize },

    #[error("item index out of range: {index} (len {len})")]
    #[mirror(skip)]
    IndexOutOfRange { index: usize, len: usize },

    #[error("commit index mismatch: requested {requested}, armed {armed}")]
    #[mirror(skip)]
    ArmIndexMismatch { requested: usize, armed: usize },

    #[error("eq band out of range: {band} (bands: {bands})")]
    EqBandOutOfRange { band: usize, bands: usize },

    #[error("item failed to load: {reason}")]
    #[mirror(skip)]
    ItemFailed { reason: String },

    #[error("seek failed to position {position:?}")]
    #[mirror(skip)]
    SeekFailed { position: Duration },

    #[error("engine not running")]
    #[mirror(skip)]
    EngineNotRunning,

    #[error("engine already running")]
    #[mirror(skip)]
    EngineAlreadyRunning,

    #[error("slot not found: {0:?}")]
    #[mirror(skip)]
    SlotNotFound(SlotId),

    #[error("slot already occupied: {0:?}")]
    #[mirror(skip)]
    SlotOccupied(SlotId),

    #[error("no available slots in arena")]
    #[mirror(skip)]
    ArenaFull,

    #[error("crossfade already in progress")]
    #[mirror(skip)]
    CrossfadeActive,

    #[error("no active crossfade to cancel")]
    #[mirror(skip)]
    NoCrossfade,

    #[error("BPM analysis failed: {reason}")]
    #[mirror(skip)]
    BpmAnalysisFailed { reason: String },

    #[error("BPM sync requires detected BPM on both slots")]
    #[mirror(skip)]
    BpmUnknown,

    #[error("session activation failed: {reason}")]
    #[mirror(skip)]
    SessionActivationFailed { reason: String },

    #[error("session category not supported: {reason}")]
    #[mirror(skip)]
    SessionCategoryUnsupported { reason: String },

    #[error("audio route unavailable: {reason}")]
    #[mirror(skip)]
    RouteUnavailable { reason: String },

    #[error("effect parameter not found: {name}")]
    #[mirror(skip)]
    EffectParameterNotFound { name: String },

    #[error("invalid parameter value: {name}={value}")]
    InvalidParameter { name: String, value: f32 },

    #[error("mix level {level} is not a finite value in 0.0..=1.0")]
    #[mirror(skip)]
    MixLevel { level: f32 },

    #[error("crossfader position {position} is not a finite value in 0.0..=1.0")]
    #[mirror(skip)]
    MixPosition { position: f32 },

    #[error("mix input player belongs to a different audio session")]
    #[mirror(skip)]
    MixForeignSession,

    #[error("player belongs to a different audio session")]
    #[mirror(skip)]
    ForeignSession,

    #[error("player is not attached to an audio session")]
    #[mirror(skip)]
    SessionUnbound,

    #[error("player is already attached to an audio session")]
    #[mirror(skip)]
    SessionAlreadyBound,

    #[error("player sample rate {player} does not match audio session sample rate {session}")]
    #[mirror(skip)]
    SessionSampleRateMismatch { player: u32, session: u32 },

    #[error("an audio session is already active on this thread")]
    #[mirror(skip)]
    SessionAlreadyActive,

    #[error("mix input lists the same player more than once")]
    #[mirror(skip)]
    MixDuplicatePlayer,

    #[error("end of resource")]
    #[mirror(skip)]
    Eof,

    #[error("playback buffer allocation failed: {0}")]
    #[mirror(skip)]
    Pool(#[from] PoolError),

    #[error("audio session is gone: {reason}")]
    #[mirror(skip)]
    SessionGone { reason: &'static str },

    #[error(transparent)]
    #[mirror(skip)]
    Session(SessionError),

    #[error("{0}")]
    #[mirror(skip)]
    Internal(String),
}

impl From<SessionError> for PlayError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::EqBandOutOfRange { band, bands } => {
                Self::EqBandOutOfRange { band, bands }
            }
            error => Self::Session(error),
        }
    }
}

impl From<ResponseError> for PlayError {
    fn from(error: ResponseError) -> Self {
        Self::Session(error.into())
    }
}
