use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};

pub(crate) struct GridBuffers {
    pub(super) gaps: SampleBuffer,
    pub(super) marks: SampleBuffer,
    pub(super) neighbors: SampleBuffer,
    pub(super) outliers: SampleBuffer,
    pub(super) positions: SampleBuffer,
    pub(super) sorted: SampleBuffer,
}

impl GridBuffers {
    pub(crate) fn new<S>(pools: &PoolRegion<S>) -> Self
    where
        S: HasPool<f32>,
    {
        Self {
            gaps: pools.get::<f32>(),
            marks: pools.get::<f32>(),
            neighbors: pools.get::<f32>(),
            outliers: pools.get::<f32>(),
            positions: pools.get::<f32>(),
            sorted: pools.get::<f32>(),
        }
    }
}

pub(super) fn fill(
    buffer: &mut SampleBuffer,
    values: impl ExactSizeIterator<Item = f32>,
) -> Result<(), PoolError> {
    buffer.clear();
    buffer.ensure_len(values.len())?;
    for (slot, value) in buffer.iter_mut().zip(values) {
        *slot = value;
    }
    Ok(())
}

pub(super) fn retain(buffer: &mut SampleBuffer, mut keep: impl FnMut(f32) -> bool) {
    let mut write = 0;
    for read in 0..buffer.len() {
        let value = buffer[read];
        if keep(value) {
            buffer[write] = value;
            write += 1;
        }
    }
    buffer.truncate(write);
}
