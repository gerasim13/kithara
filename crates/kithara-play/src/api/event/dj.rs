use kithara_events::{Event, SlotId};

use super::{BpmInfo, MediaTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StretchBackendKind {
    Signalsmith,
    Bungee,
    Unknown,
}

/// Audible movement through a track's beat map.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackDirection {
    /// Session beats advance toward higher track beats.
    #[default]
    Forward,
    /// Session beats advance toward lower track beats.
    Reverse,
}

#[derive(Clone, Debug, Event)]
pub enum DjEvent {
    BpmDetected {
        slot: SlotId,
        info: BpmInfo,
    },
    BeatTick {
        slot: SlotId,
        beat_number: u64,
        timestamp: MediaTime,
    },
    KeylockChanged {
        on: bool,
    },
    StretchBackendChanged {
        kind: StretchBackendKind,
    },
    BpmSyncEngaged {
        leader: SlotId,
        follower: SlotId,
    },
    BpmSyncDisengaged {
        slot: SlotId,
    },
    PhaseAligned {
        leader: SlotId,
        follower: SlotId,
        offset_beats: f64,
    },
}
