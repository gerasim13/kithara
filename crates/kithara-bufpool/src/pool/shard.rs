use crossbeam_queue::ArrayQueue;

/// One bounded lock-free free list.
pub(super) struct PoolShard<T> {
    free: ArrayQueue<Vec<T>>,
    trim_capacity: usize,
}

impl<T> PoolShard<T> {
    pub(super) const MAX_SLOTS: usize = 1024;

    pub(super) fn new(max_buffers: usize, trim_capacity: usize) -> Self {
        Self {
            free: ArrayQueue::new(max_buffers.min(Self::MAX_SLOTS)),
            trim_capacity,
        }
    }

    pub(super) fn try_get(&self) -> Option<Vec<T>> {
        self.free.pop()
    }

    pub(super) fn try_put(&self, mut value: Vec<T>) -> Result<usize, Vec<T>> {
        const TRIM_HYSTERESIS: usize = 2;

        value.clear();
        if self.trim_capacity > 0
            && value.capacity() > self.trim_capacity.saturating_mul(TRIM_HYSTERESIS)
        {
            value.shrink_to(self.trim_capacity);
        }
        if value.capacity() == 0 {
            return Err(value);
        }
        let kept = value.capacity().saturating_mul(size_of::<T>());
        self.free.push(value).map(|()| kept)
    }

    pub(super) fn drain(&self, mut release: impl FnMut(Vec<T>)) {
        while let Some(value) = self.free.pop() {
            release(value);
        }
    }
}
