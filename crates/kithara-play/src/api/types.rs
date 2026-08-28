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
