use std::num::NonZeroUsize;

use super::SpectrumError;
use crate::consts;

/// A real FFT length every backend runs: `f·2ⁿ` with `f` in 1, 3, 5, 15
/// and `n ≥ 4`, the lengths vDSP's real DFT builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FftLen(NonZeroUsize);

impl FftLen {
    /// # Errors
    /// [`SpectrumError::Length`] when `len` is not such a length.
    pub const fn new(len: usize) -> Result<Self, SpectrumError> {
        let twos = len.trailing_zeros();
        let Some(odd) = len.checked_shr(twos) else {
            return Err(SpectrumError::Length);
        };
        match NonZeroUsize::new(len) {
            Some(len) if twos >= consts::FFT_MIN_TWOS && matches!(odd, 1 | 3 | 5 | 15) => {
                Ok(Self(len))
            }
            _ => Err(SpectrumError::Length),
        }
    }

    /// Samples per frame.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }

    /// Bins of the spectrum, `0..=N/2`.
    #[must_use]
    pub const fn bins(self) -> usize {
        (self.0.get() / 2).saturating_add(1)
    }
}
