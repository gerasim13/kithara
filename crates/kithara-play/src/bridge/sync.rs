use kithara_events::TrackId;

use crate::rt::track::{PlayerResource, PlayerTrack, SyncFadeTail};

/// Exact installed lane sent from the executor into one player callback.
pub(crate) type SyncTicket = kithara_sync::SyncTicket<TrackId, Box<PlayerResource>>;

/// Audio-owned objects returned to the control thread without RT destruction.
pub(crate) enum SyncReturn {
    Ticket(SyncTicket),
    Track(PlayerTrack),
    Tail(SyncFadeTail),
}
