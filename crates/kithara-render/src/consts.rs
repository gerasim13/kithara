use std::num::{NonZeroU32, NonZeroUsize};

use kithara_dsp::param::{DEFAULT_SETTLE_RATIO, SmootherConfig};
use kithara_platform::time::Duration;
use kithara_signal::FrameCount;

pub(crate) const DEFAULT_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(value) => value,
    None => unreachable!(),
};

/// Tracks a deck holds at once.
pub(crate) const DEFAULT_DECK_SLOTS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => unreachable!(),
};

/// The ramp a track starts and stops with: 5 ms.
pub(crate) const DEFAULT_DECLICK: SmootherConfig = SmootherConfig {
    smooth_seconds: 0.005,
    settle_ratio: DEFAULT_SETTLE_RATIO,
};

/// Frames of a replaced consumer a slot plays out of its tail.
pub(crate) const DEFAULT_EVICT_FADE: FrameCount = FrameCount::new(512);

/// Frames in one source chunk of a test lane.
#[cfg(test)]
pub(crate) const LANE_CHUNK_FRAMES: u32 = 4096;

pub(crate) const ACTIVE_WAIT_TIMEOUT: Duration = Duration::from_millis(1);
pub(crate) const BACKPRESSURE_POLL_INTERVAL: Duration = Duration::from_micros(250);

pub(crate) const CAPACITY: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(value) => value,
    None => unreachable!(),
};

pub(crate) const FAIRNESS_YIELD_INTERVAL: NonZeroU32 = match NonZeroU32::new(16) {
    Some(value) => value,
    None => unreachable!(),
};

/// Maximum unsettled commands admitted by one lane channel.
pub(crate) const LANE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(128) {
    Some(value) => value,
    None => unreachable!(),
};

pub(crate) const TASK_BURST: NonZeroU32 = match NonZeroU32::new(32) {
    Some(value) => value,
    None => unreachable!(),
};

/// EWMA weight for per-chunk samples (≈ last ~10 chunks dominate).
pub(crate) const LOAD_ALPHA: f32 = 0.2;

pub(crate) const MS_PER_SEC: f64 = 1000.0;
