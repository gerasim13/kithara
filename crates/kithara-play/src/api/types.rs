pub use kithara_events::{
    DjEvent, EngineEvent, InterruptionKind, ItemEvent, ItemRole, ItemStatus, PlaybackDirection,
    PlayerEvent, PlayerStatus, RouteChangeReason, SessionEvent, SlotId, TimeControlStatus,
    TimeRange, TrackId, TrackRef, TransportEvent, WaitingReason,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SessionDuckingMode {
    #[default]
    Off,
    Soft,
    Hard,
}

impl SessionDuckingMode {
    /// Session-output gain represented by this ducking policy.
    #[must_use]
    pub const fn gain(self) -> f32 {
        match self {
            Self::Off => 1.0,
            Self::Soft => 0.4,
            Self::Hard => 0.2,
        }
    }
}
