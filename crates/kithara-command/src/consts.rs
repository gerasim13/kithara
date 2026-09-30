use std::num::NonZeroUsize;

/// Batches a channel holds in flight when its config names no capacity.
pub(crate) const CAPACITY: NonZeroUsize = match NonZeroUsize::new(64) {
    Some(value) => value,
    None => unreachable!(),
};
