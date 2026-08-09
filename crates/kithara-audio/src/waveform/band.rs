/// The three concentric envelopes one waveform column carries. The
/// discriminants are the indices into every `[T; Band::COUNT]` band array in
/// this module, and the order is the wire order: low, mid, high.
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub(crate) enum Band {
    Low = 0,
    Mid = 1,
    High = 2,
}

impl Band {
    /// Every band, in index order.
    pub(crate) const ALL: [Self; Self::COUNT] = [Self::Low, Self::Mid, Self::High];
    /// Length of a band array.
    pub(crate) const COUNT: usize = 3;

    /// Index of this band in a `[T; Band::COUNT]` array.
    pub(crate) const fn idx(self) -> usize {
        self as usize
    }
}
