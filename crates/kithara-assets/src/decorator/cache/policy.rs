use std::num::NonZeroUsize;

use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;

use crate::{consts, store::AssetStoreConfig};

/// Capacity settings read by a cache from its retained configuration.
pub trait CachePolicy: Clone + Send + Sync + 'static {
    /// Base number of entries before pinned entries extend the cache.
    fn capacity(&self) -> NonZeroUsize;
    /// Optional byte limit used when a cached reader or writer is released.
    fn max_bytes(&self) -> Option<u64>;
}

impl CachePolicy for (NonZeroUsize, Option<u64>) {
    fn capacity(&self) -> NonZeroUsize {
        self.0
    }

    fn max_bytes(&self) -> Option<u64> {
        self.1
    }
}

impl<S> CachePolicy for Arc<AssetStoreConfig<S>>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn capacity(&self) -> NonZeroUsize {
        self.cache_capacity
            .unwrap_or(consts::DEFAULT_CACHE_CAPACITY)
    }

    fn max_bytes(&self) -> Option<u64> {
        self.max_bytes
    }
}
