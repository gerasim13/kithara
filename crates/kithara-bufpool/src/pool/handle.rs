use std::fmt;

use crate::{PoolError, PoolKey, PoolKeyWithLen, PoolStats, key::PoolAccess};

/// Cloneable access to one typed slot in an existing buffer region.
///
/// A handle shares its originating slot and hard budgets. It keeps that slot
/// alive without requiring consumers to carry the application's full schema.
/// Buffers acquired here return to the same slot when dropped.
#[derive_where::derive_where(Clone)]
pub struct Pool<K>
where
    K: PoolKey,
{
    core: K::Core,
}

impl<K> Pool<K>
where
    K: PoolKey,
{
    pub(crate) const fn new(core: K::Core) -> Self {
        Self { core }
    }

    /// Acquire an empty guard from the originating typed slot.
    #[must_use]
    pub fn get(&self) -> K::Buffer {
        K::__get(&self.core, PoolAccess::new())
    }

    /// Snapshot the originating slot's reuse counters.
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        K::__stats(&self.core, PoolAccess::new())
    }
}

impl<K> Pool<K>
where
    K: PoolKeyWithLen,
{
    /// Acquire a guard of at least `len` elements under both hard budgets.
    ///
    /// # Errors
    /// Returns an error when capacity overflows, either hard budget is exceeded,
    /// or the allocator refuses the requested growth.
    #[inline]
    pub fn get_with_len(&self, len: usize) -> Result<K::Buffer, PoolError> {
        K::__get_with_len(&self.core, len, PoolAccess::new())
    }
}

impl<K> fmt::Debug for Pool<K>
where
    K: PoolKey,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}
