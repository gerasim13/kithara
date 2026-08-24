use kithara::{
    audio::{Beat, BeatMapSnapshot, MapStamp, MapState, MapUnavailable},
    queue::Queue,
};

/// Musical quantum requested by a synchronization oracle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyncQuantum(f64);

impl SyncQuantum {
    /// Creates a finite positive beat quantum.
    pub fn new(beats: f64) -> Result<Self, SyncControlError> {
        if beats.is_finite() && beats > 0.0 {
            Ok(Self(beats))
        } else {
            Err(SyncControlError::InvalidQuantum { beats })
        }
    }
}

/// Control-plane seam used by the transferred synchronization matrix.
pub trait SyncDeckControl {
    /// Requests exact activation of one queue entry against an immutable map.
    fn start_at_map(
        &self,
        index: usize,
        map: BeatMapSnapshot,
        track_beat: Beat,
        quantum: SyncQuantum,
    ) -> Result<(), SyncControlError>;

    /// Detaches the deck from its synchronization group.
    fn unbind_from_map(&self) -> Result<(), SyncControlError>;
}

/// A transferred synchronization control cannot be admitted.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SyncControlError {
    /// The requested quantum is not finite and positive.
    #[error("sync quantum must be finite and positive, got {beats}")]
    InvalidQuantum { beats: f64 },
    /// The Session-owned synchronization runtime has not been connected yet.
    #[error(
        "SYNC-ORACLE runtime is unavailable for queue entry {index}, map {map:?}, target {track_beat:?}, quantum {quantum:?}"
    )]
    RuntimeUnavailable {
        index: usize,
        map: MapStamp,
        track_beat: Beat,
        quantum: SyncQuantum,
    },
    /// The supplied map cannot currently answer synchronization queries.
    #[error("map is unavailable for synchronization: {reason:?}")]
    MapUnavailable { reason: MapUnavailable },
    /// The Session-owned synchronization runtime has not been connected yet.
    #[error("SYNC-ORACLE runtime is unavailable for group detach")]
    DetachUnavailable,
}

impl SyncDeckControl for Queue {
    fn start_at_map(
        &self,
        index: usize,
        map: BeatMapSnapshot,
        track_beat: Beat,
        quantum: SyncQuantum,
    ) -> Result<(), SyncControlError> {
        if let MapState::Unavailable(reason) = map.state() {
            return Err(SyncControlError::MapUnavailable { reason });
        }
        Err(SyncControlError::RuntimeUnavailable {
            index,
            map: map.stamp(),
            track_beat,
            quantum,
        })
    }

    fn unbind_from_map(&self) -> Result<(), SyncControlError> {
        Err(SyncControlError::DetachUnavailable)
    }
}
