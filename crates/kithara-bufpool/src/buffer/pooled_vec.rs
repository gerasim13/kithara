use std::fmt;

use super::OwnedBuffer;
use crate::PoolError;

/// A checked vector guard returned by a registered [`crate::VecKey`].
pub struct PooledVec<T, const SHARDS: usize>(pub(super) OwnedBuffer<SHARDS, Vec<T>, true>);

impl<T, const SHARDS: usize> PooledVec<T, SHARDS> {
    pub(crate) fn new(inner: OwnedBuffer<SHARDS, Vec<T>, true>) -> Self {
        Self(inner)
    }

    delegate::delegate! {
        to self.0 {
            /// Return the allocated element capacity.
            #[must_use]
            pub fn capacity(&self) -> usize;
            /// Remove every element while retaining capacity.
            pub fn clear(&mut self);
            /// Append one element under both hard budgets.
            ///
            /// # Errors
            ///
            /// Returns an error when capacity overflows, exceeds either hard budget,
            /// or cannot be allocated.
            pub fn try_push(&mut self, value: T) -> Result<(), PoolError>;
            /// Grow to at least `min_len` default elements under both hard budgets.
            ///
            /// # Errors
            ///
            /// Returns an error when capacity overflows, exceeds either hard budget,
            /// or cannot be allocated.
            pub fn ensure_len(&mut self, min_len: usize) -> Result<(), PoolError>
            where
                T: Clone + Default,;
        }
    }

    /// Append elements under both hard budgets.
    ///
    /// # Errors
    ///
    /// Returns an error when capacity overflows, exceeds either hard budget,
    /// or cannot be allocated.
    pub fn try_extend<I>(&mut self, values: I) -> Result<(), PoolError>
    where
        I: IntoIterator<Item = T>,
    {
        self.0.try_extend(values)
    }
}

impl<T, const SHARDS: usize> std::ops::Deref for PooledVec<T, SHARDS> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0.value
    }
}

impl<T, const SHARDS: usize> std::ops::DerefMut for PooledVec<T, SHARDS> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.value
    }
}

impl<T, const SHARDS: usize> fmt::Debug for PooledVec<T, SHARDS>
where
    T: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.value.fmt(formatter)
    }
}
