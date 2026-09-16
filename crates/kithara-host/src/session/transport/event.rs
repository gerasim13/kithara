use kithara_events::Event;

/// Facts committed by the session transport owner.
#[derive(Clone, Debug, Event, PartialEq)]
#[non_exhaustive]
pub enum TransportEvent {
    TempoCommitted {
        beats_per_minute: f64,
        revision: u64,
    },
    PlayStateCommitted {
        playing: bool,
        revision: u64,
    },
    SeekCommitted {
        position_beats: f64,
        revision: u64,
    },
    Failed {
        revision: Option<u64>,
        reason: String,
    },
}
