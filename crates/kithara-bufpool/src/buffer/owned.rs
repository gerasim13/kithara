use std::ops::RangeBounds;

use kithara_platform::sync::Arc;

use crate::{PoolError, pool::Core};

pub(crate) struct OwnedBuffer<const SHARDS: usize, T>
where
    T: Copy + Default,
{
    core: Arc<Core<SHARDS, T>>,
    shard_idx: usize,
    pub(super) value: Vec<T>,
}

impl<const SHARDS: usize, T> OwnedBuffer<SHARDS, T>
where
    T: Copy + Default,
{
    pub(crate) fn new(core: Arc<Core<SHARDS, T>>, value: Vec<T>, shard_idx: usize) -> Self {
        Self {
            core,
            shard_idx,
            value,
        }
    }

    pub(super) fn drain<R>(&mut self, range: R) -> std::vec::Drain<'_, T>
    where
        R: RangeBounds<usize>,
    {
        self.value.drain(range)
    }

    pub(super) fn ensure_len(&mut self, min_len: usize) -> Result<(), PoolError> {
        if min_len <= self.value.len() {
            return Ok(());
        }
        if min_len <= self.value.capacity() {
            self.value.resize(min_len, T::default());
            return Ok(());
        }

        let grown = self.core.grow(&self.value, min_len, None)?;
        self.value = grown;
        Ok(())
    }

    pub(super) fn retain<F>(&mut self, keep: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.value.retain(keep);
    }

    pub(super) fn renew(&mut self) {
        let core = Arc::clone(&self.core);
        let replacement = core.acquire();
        *self = replacement;
    }

    pub(super) fn try_extend_from_slice(&mut self, values: &[T]) -> Result<(), PoolError> {
        let new_len =
            self.value
                .len()
                .checked_add(values.len())
                .ok_or(PoolError::CapacityOverflow {
                    elements: usize::MAX,
                    element_size: size_of::<T>(),
                })?;
        if new_len <= self.value.capacity() {
            self.value.extend_from_slice(values);
            return Ok(());
        }

        let grown = self.core.grow(&self.value, new_len, Some(values))?;
        self.value = grown;
        Ok(())
    }

    delegate::delegate! {
        to self.value {
            pub(super) fn capacity(&self) -> usize;
            pub(super) fn clear(&mut self);
            pub(super) fn dedup(&mut self)
            where
                T: PartialEq;
            pub(super) fn truncate(&mut self, len: usize);
        }
    }
}

impl<const SHARDS: usize, T> Drop for OwnedBuffer<SHARDS, T>
where
    T: Copy + Default,
{
    fn drop(&mut self) {
        self.core
            .put(std::mem::take(&mut self.value), self.shard_idx);
    }
}
