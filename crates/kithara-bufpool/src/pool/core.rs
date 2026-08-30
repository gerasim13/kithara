use std::array;

use crossbeam_queue::ArrayQueue;
use kithara_platform::{sync::Arc, thread::current_thread_id};

use super::shard::PoolShard;
use crate::{
    PoolConfig, PoolError,
    budget::{BudgetPair, RegionBudget, ReserveFailure},
    buffer::OwnedBuffer,
};

pub(crate) struct Core<const SHARDS: usize, T>
where
    T: Copy + Default,
{
    budgets: BudgetPair,
    cold: Option<ArrayQueue<Vec<T>>>,
    shards: [PoolShard<T>; SHARDS],
}

impl<const SHARDS: usize, T> Core<SHARDS, T>
where
    T: Copy + Default,
{
    const MAX_PROBE: usize = 4;

    pub(crate) fn new(
        config: PoolConfig,
        region_budget: RegionBudget,
        pool_limit: usize,
    ) -> Result<Self, PoolError> {
        if config.max_buffers < SHARDS {
            return Err(PoolError::InvalidConfig {
                field: "max_buffers",
                reason: "must provide at least one retained slot per shard",
            });
        }
        let buffers_per_shard = config.max_buffers / SHARDS;
        let effective_buffers = buffers_per_shard
            .min(PoolShard::<T>::MAX_SLOTS)
            .checked_mul(SHARDS)
            .ok_or(PoolError::InvalidConfig {
                field: "max_buffers",
                reason: "effective shard capacity overflows usize",
            })?;
        if config.initial_buffers > effective_buffers {
            return Err(PoolError::InvalidConfig {
                field: "initial_buffers",
                reason: "exceeds the effective retained-buffer capacity",
            });
        }
        config
            .initial_capacity
            .checked_mul(size_of::<T>())
            .and_then(|bytes| bytes.checked_mul(config.initial_buffers))
            .ok_or(PoolError::InvalidConfig {
                field: "initial_capacity",
                reason: "initial payload byte count overflows usize",
            })?;

        let cold = (config.initial_buffers > 0).then(|| ArrayQueue::new(config.initial_buffers));
        let core = Self {
            budgets: BudgetPair::new(region_budget, pool_limit),
            cold,
            shards: array::from_fn(|_| PoolShard::new(buffers_per_shard, config.trim_capacity)),
        };

        if let Some(cold) = &core.cold {
            for _ in 0..config.initial_buffers {
                let empty = Vec::new();
                let mut value = core.grow(&empty, config.initial_capacity, None)?;
                value.clear();
                if let Err(value) = cold.push(value) {
                    let bytes = Self::byte_size(&value);
                    drop(value);
                    core.budgets.release(bytes);
                    return Err(PoolError::InvalidConfig {
                        field: "initial_buffers",
                        reason: "cold-start queue rejected a validated payload",
                    });
                }
            }
        }
        Ok(core)
    }

    #[cfg_attr(feature = "perf", hotpath::measure)]
    pub(crate) fn acquire(self: &Arc<Self>) -> OwnedBuffer<SHARDS, T> {
        let shard_idx = Self::shard_index();
        let value = self.shards[shard_idx]
            .try_get()
            .or_else(|| self.try_steal(shard_idx))
            .or_else(|| self.cold.as_ref().and_then(ArrayQueue::pop));
        let value = value.unwrap_or_default();
        OwnedBuffer::new(Arc::clone(self), value, shard_idx)
    }

    pub(crate) fn grow(
        &self,
        current: &Vec<T>,
        new_len: usize,
        appended: Option<&[T]>,
    ) -> Result<Vec<T>, PoolError> {
        let old_capacity = current.capacity();
        let old_bytes =
            old_capacity
                .checked_mul(size_of::<T>())
                .ok_or(PoolError::CapacityOverflow {
                    elements: old_capacity,
                    element_size: size_of::<T>(),
                })?;
        let region_available = self
            .budgets
            .region_limit()
            .saturating_sub(self.budgets.region_current());
        let pool_available = self.budgets.limit().saturating_sub(self.budgets.current());
        let affordable_capacity =
            old_bytes.saturating_add(region_available.min(pool_available)) / size_of::<T>();
        let amortized_capacity = new_len.max(old_capacity.saturating_mul(2));
        let target_capacity = if affordable_capacity >= new_len {
            amortized_capacity.min(affordable_capacity)
        } else {
            new_len
        };
        let requested_bytes =
            target_capacity
                .checked_mul(size_of::<T>())
                .ok_or(PoolError::CapacityOverflow {
                    elements: target_capacity,
                    element_size: size_of::<T>(),
                })?;
        let requested_delta =
            requested_bytes
                .checked_sub(old_bytes)
                .ok_or(PoolError::InvalidConfig {
                    field: "buffer growth",
                    reason: "new capacity is smaller than the current capacity",
                })?;
        let mut reservation = self.reserve(requested_delta)?;

        let mut grown = Vec::new();
        if grown.try_reserve_exact(target_capacity).is_err() {
            return Err(PoolError::AllocationFailed {
                additional_bytes: requested_delta,
                allocated_bytes: self.budgets.region_current(),
                max_bytes: self.budgets.region_limit(),
            });
        }
        let actual_bytes = grown
            .capacity()
            .checked_mul(size_of::<T>())
            .ok_or_else(|| PoolError::CapacityOverflow {
                elements: grown.capacity(),
                element_size: size_of::<T>(),
            })?;
        let actual_delta = actual_bytes
            .checked_sub(old_bytes)
            .ok_or(PoolError::InvalidConfig {
                field: "buffer growth",
                reason: "allocator returned less capacity than the current buffer",
            })?;
        let extra = if actual_delta > requested_delta {
            Some(self.reserve(actual_delta - requested_delta)?)
        } else {
            reservation.reduce(requested_delta - actual_delta);
            None
        };

        grown.extend_from_slice(current);
        if let Some(appended) = appended {
            grown.extend_from_slice(appended);
        } else {
            grown.resize(new_len, T::default());
        }
        if let Some(extra) = extra {
            extra.commit();
        }
        reservation.commit();
        Ok(grown)
    }

    pub(crate) fn put(&self, value: Vec<T>, shard_idx: usize) {
        let before = Self::byte_size(&value);
        match self.shards[shard_idx].try_put(value) {
            Ok(kept) => self.budgets.release(before - kept),
            Err(value) => {
                drop(value);
                self.budgets.release(before);
            }
        }
    }

    fn byte_size(value: &Vec<T>) -> usize {
        value.capacity() * size_of::<T>()
    }

    fn reserve(&self, amount: usize) -> Result<crate::budget::Reservation<'_>, PoolError> {
        self.budgets
            .reserve(amount)
            .map_err(|failure| match failure {
                ReserveFailure::Overall { amount, snapshot } => PoolError::OverallBudgetExceeded {
                    additional_bytes: amount,
                    allocated_bytes: snapshot.current,
                    max_bytes: snapshot.limit,
                },
                ReserveFailure::Pool { amount, snapshot } => PoolError::PoolBudgetExceeded {
                    additional_bytes: amount,
                    allocated_bytes: snapshot.current,
                    max_bytes: snapshot.limit,
                },
            })
    }

    fn shard_index() -> usize {
        let shards = SHARDS as u64;
        usize::try_from(current_thread_id() % shards).unwrap_or(0)
    }

    fn try_steal(&self, home: usize) -> Option<Vec<T>> {
        let probes = Self::MAX_PROBE.min(SHARDS.saturating_sub(1));
        (1..=probes).find_map(|offset| self.shards[(home + offset) % SHARDS].try_get())
    }
}

impl<const SHARDS: usize, T> Drop for Core<SHARDS, T>
where
    T: Copy + Default,
{
    fn drop(&mut self) {
        if let Some(cold) = &self.cold {
            while let Some(value) = cold.pop() {
                let bytes = Self::byte_size(&value);
                drop(value);
                self.budgets.release(bytes);
            }
        }
        for shard in &self.shards {
            shard.drain(|value| {
                let bytes = Self::byte_size(&value);
                drop(value);
                self.budgets.release(bytes);
            });
        }
    }
}
