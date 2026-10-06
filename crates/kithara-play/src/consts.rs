use std::num::{NonZeroU32, NonZeroUsize};

use kithara_dsp::param::{DEFAULT_SETTLE_RATIO, SmootherConfig};
#[cfg(test)]
use kithara_events::TrackId;
use kithara_platform::time::Duration;

#[cfg(test)]
use crate::api::SlotId;

#[cfg(test)]
pub(crate) const BACKGROUND: TrackId = TrackId(9);

#[cfg(test)]
pub(crate) const OUTGOING: TrackId = TrackId(7);

#[cfg(test)]
pub(crate) const PROMOTED: TrackId = TrackId(8);

pub(crate) const DISCRIMINATOR_DOMAIN: &[u8] = b"kithara.play.query-discriminator.v1\0";
pub(crate) const HASH_BYTES: usize = 16;
pub(crate) const IDENTITY_DOMAIN: &[u8] = b"kithara.play.query-identity.v1\0";

#[cfg(test)]
pub(crate) const BLOCK_FRAMES: usize = 512;

#[cfg(test)]
pub(crate) const DECK_SLOT: SlotId = SlotId::new(0);

#[cfg(test)]
pub(crate) const SAMPLE_RATE: u32 = 44_100;

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

/// A lane executes only while it renders, so a paused or backpressured deck
/// keeps every speed change sent to it in flight.
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

pub(crate) const DEFAULT_EQ_BAND_COUNT: usize = 10;
pub(crate) const DEFAULT_MAX_SLOTS: usize = 4;

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

#[cfg(test)]
pub(crate) const DROPPED_AFTER_CANCEL: u8 = 2;

#[cfg(test)]
pub(crate) const DROPPED_BEFORE_CANCEL: u8 = 1;

#[cfg(test)]
pub(crate) const NOT_DROPPED: u8 = 0;
