use std::num::NonZeroU32;

use kithara_events::TrackId;
use kithara_signal::{SessionEpoch, SessionFrame, SourceSpan};
use kithara_sync::{ArmPermit, LoadGeneration, SyncGateBinding};
use kithara_warp::WarpMapRevision;

use crate::rt::track::{PlayerResource, PlayerTrack, SyncFadeTail};

/// PCM already consumed from the staged reader off the audio callback.
pub(crate) struct PreparedFirst {
    pub(crate) stereo: [f32; 2],
    pub(crate) source: SourceSpan,
}

/// Exact installed lane sent from the executor into one player callback.
pub(crate) struct SyncTicket {
    pub(crate) item_id: TrackId,
    pub(crate) load: LoadGeneration,
    pub(crate) resource: Box<PlayerResource>,
    pub(crate) first: PreparedFirst,
    pub(crate) permit: ArmPermit,
    pub(crate) gate: SyncGateBinding,
    pub(crate) activation: SessionFrame,
    pub(crate) source_start: u64,
    pub(crate) epoch: SessionEpoch,
    pub(crate) output_rate: NonZeroU32,
    pub(crate) map: WarpMapRevision,
}

/// Audio-owned objects returned to the control thread without RT destruction.
pub(crate) enum SyncReturn {
    Ticket(SyncTicket),
    Track(PlayerTrack),
    Tail(SyncFadeTail),
}
