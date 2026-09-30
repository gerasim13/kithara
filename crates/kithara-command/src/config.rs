use std::num::NonZeroUsize;

use bon::Builder;

use crate::consts;

/// Sizes of one channel, fixed when it is built.
#[derive(Clone, Copy, Debug, Builder)]
#[non_exhaustive]
pub struct ChannelConfig {
    /// Batches in flight at once: sent and not yet answered by a receipt.
    #[builder(default = consts::CAPACITY)]
    pub(crate) capacity: NonZeroUsize,
    /// Targets whose time batches shift; every target index is below it.
    #[builder(default)]
    pub(crate) targets: usize,
}
