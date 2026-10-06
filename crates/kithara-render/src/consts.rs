use std::num::NonZeroUsize;

use kithara_dsp::param::{DEFAULT_SETTLE_RATIO, SmootherConfig};

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

/// Frames in one source chunk of a test lane.
#[cfg(test)]
pub(crate) const LANE_CHUNK_FRAMES: u32 = 4096;
