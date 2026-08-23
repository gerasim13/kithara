use kithara_bufpool::{BudgetExhausted, PcmBuf, PcmPool};

pub(super) struct GridBuffers {
    pub(super) gaps: PcmBuf,
    pub(super) marks: PcmBuf,
    pub(super) neighbors: PcmBuf,
    pub(super) outliers: PcmBuf,
    pub(super) positions: PcmBuf,
    pub(super) sorted: PcmBuf,
}

impl GridBuffers {
    pub(super) fn new(pool: &PcmPool) -> Self {
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
    buffer: &mut PcmBuf,
    values: impl ExactSizeIterator<Item = f32>,
) -> Result<(), BudgetExhausted> {
    buffer.clear();
    buffer.ensure_len(values.len())?;
    for (slot, value) in buffer.iter_mut().zip(values) {
        *slot = value;
    }
    Ok(())
}

pub(super) fn retain(buffer: &mut PcmBuf, mut keep: impl FnMut(f32) -> bool) {
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
