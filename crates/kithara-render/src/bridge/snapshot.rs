use super::{RtMetricsSnapshot, SlotMark, SlotState};

/// What a deck's mixer last published of each slot and of itself, once per block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckSnapshot {
    /// One entry per slot, in slot order.
    pub slots: Vec<SlotSnapshot>,
    /// Current output sample rate.
    pub sample_rate: u32,
    /// Blocks the mixer rendered.
    pub blocks: u64,
    /// The mixer's real-time counters.
    pub metrics: RtMetricsSnapshot,
}

/// One slot as its mixer last published it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlotSnapshot {
    pub state: SlotState,
    pub mark: Option<SlotMark>,
    /// Media position in seconds.
    pub position: f64,
    /// Visible media duration in seconds; `0.0` when unknown.
    pub duration: f64,
    /// Decoded-ahead frontier in seconds, never behind `position`.
    pub frontier: f64,
    /// How much of the source is on disk, in seconds.
    pub cached: f64,
    /// The slot's envelope gain on its last mixed frame.
    pub gain: f32,
}
