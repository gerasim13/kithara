use kithara_bufpool::{BudgetExhausted, SampleBuffer, SamplePool};

pub(super) struct GridBuffers {
    pub(super) gaps: SampleBuffer,
    pub(super) marks: SampleBuffer,
    pub(super) neighbors: SampleBuffer,
    pub(super) outliers: SampleBuffer,
    pub(super) positions: SampleBuffer,
    pub(super) sorted: SampleBuffer,
}

impl GridBuffers {
    pub(super) fn new(pool: &SamplePool) -> Self {
        Self {
            gaps: pool.get(),
            marks: pool.get(),
            neighbors: pool.get(),
            outliers: pool.get(),
            positions: pool.get(),
            sorted: pool.get(),
        }
    }
}

pub(super) fn fill(
    buffer: &mut SampleBuffer,
    values: impl ExactSizeIterator<Item = f32>,
) -> Result<(), BudgetExhausted> {
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
