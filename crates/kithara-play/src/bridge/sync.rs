use kithara_events::TrackId;

use crate::rt::track::{PlayerResource, PlayerTrack, SyncFadeTail};

/// The Player's kinds of one deck's sync activation.
pub(crate) enum PlayerSync {}

impl kithara_sync::SyncKind for PlayerSync {
    type Item = TrackId;
    type Lane = Box<PlayerResource>;
    type Track = PlayerTrack;
    type Tail = SyncFadeTail;

    fn holds_lane(track: &PlayerTrack) -> bool {
        track.has_sync_lane()
    }

    fn settled(tail: &SyncFadeTail) -> bool {
        tail.settled()
    }
}

/// Exact installed lane sent from the executor into one player callback.
pub(crate) type SyncTicket = kithara_sync::SyncTicket<TrackId, Box<PlayerResource>>;

/// Audio-owned objects returned to the control thread without RT destruction.
pub(crate) type SyncReturn = kithara_sync::SyncReturn<PlayerSync>;

/// The callback's owner of one deck's activation.
pub(crate) type PlaySync = kithara_sync::SyncCallback<PlayerSync>;
