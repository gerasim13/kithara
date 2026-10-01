use std::num::{NonZeroU32, NonZeroUsize};

use kithara_platform::time::Duration;

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

pub(crate) const TASK_BURST: NonZeroU32 = match NonZeroU32::new(32) {
    Some(value) => value,
    None => unreachable!(),
};

/// EWMA weight for per-chunk samples (≈ last ~10 chunks dominate).
pub(crate) const LOAD_ALPHA: f32 = 0.2;

pub(crate) const MS_PER_SEC: f64 = 1000.0;
pub(crate) const SLOT_TRACKS: usize = crate::rt::PlayerNodeProcessor::MAX_TRACKS;
