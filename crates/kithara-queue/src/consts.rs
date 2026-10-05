#[cfg(test)]
use std::num::NonZeroU32;
use std::num::NonZeroUsize;

/// Default parallelism cap for async track loads.
pub(crate) const DEFAULT_MAX_CONCURRENT_LOADS: NonZeroUsize = match NonZeroUsize::new(3) {
    Some(n) => n,
    None => unreachable!(),
};

/// Default session seconds before a track ends at which the queue reloads a
/// consumed successor.
pub(crate) const DEFAULT_PREFETCH_DURATION: f32 = 3.5;

#[cfg(test)]
pub(crate) const TEST_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};
