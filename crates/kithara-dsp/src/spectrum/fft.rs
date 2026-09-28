use std::f32::consts::TAU;

use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use num_traits::ToPrimitive;

use super::{FftLen, SpectrumError};
use crate::{backend::platform, consts};

/// A real FFT of one length behind a Hann window.
///
/// The window is tabulated once, so `forward` neither allocates nor
/// recomputes it. Every backend returns the unscaled DFT of the windowed
/// frame, `X[k] = Σ w[n]·x[n]·e^(−2πikn/N)`, and `Fft` moves and shares
/// across threads.
pub struct Fft {
    len: FftLen,
    window: Box<[f32]>,
    dft: platform::Dft,
}

/// One frame's spectrum: bins `0..=N/2` as real and imaginary planes, and
/// the scratch the transform writes through. Its sample planes belong to the
/// region it was taken from and return there on drop.
pub struct Spectrum {
    len: FftLen,
    re: SampleBuffer,
    im: SampleBuffer,
    work: platform::Work,
}

impl Fft {
    /// # Errors
    /// [`SpectrumError::Setup`] when the backend cannot build the transform.
    pub fn new(len: FftLen) -> Result<Self, SpectrumError> {
        Ok(Self {
            len,
            window: hann(len, platform::Dft::WINDOW_SCALE),
            dft: platform::Dft::try_from(len)?,
        })
    }

    #[must_use]
    pub const fn size(&self) -> FftLen {
        self.len
    }

    /// A zeroed spectrum sized for this transform, its planes taken from
    /// `pools`.
    ///
    /// # Errors
    /// [`PoolError`] when the planes do not fit the region.
    pub fn spectrum<S>(&self, pools: &PoolRegion<S>) -> Result<Spectrum, PoolError>
    where
        S: HasPool<f32>,
    {
        let bins = self.len.bins();
        Ok(Spectrum {
            len: self.len,
            re: pools.get_with_len::<f32>(bins)?,
            im: pools.get_with_len::<f32>(bins)?,
            work: self.dft.work(pools)?,
        })
    }

    /// Windows `frame`, fills it out with zeros to the FFT length and
    /// writes its spectrum into `spectrum`, without allocating.
    ///
    /// # Errors
    /// [`SpectrumError::Shape`] when `frame` is longer than the FFT or
    /// `spectrum` belongs to a transform of another length.
    pub fn forward(&self, frame: &[f32], spectrum: &mut Spectrum) -> Result<(), SpectrumError> {
        let Spectrum { len, re, im, work } = spectrum;
        if *len != self.len {
            return Err(SpectrumError::Shape);
        }
        let (signal, padding) = work
            .input_mut()
            .split_at_mut_checked(frame.len())
            .ok_or(SpectrumError::Shape)?;
        let _ = platform::multiply(frame, &self.window, signal);
        padding.fill(0.0);
        let (re, im): (&mut [f32], &mut [f32]) = (re, im);
        self.dft.forward(work, [re, im])
    }
}

impl Spectrum {
    /// Real parts of bins `0..=N/2`.
    #[must_use]
    pub fn re(&self) -> &[f32] {
        &self.re
    }

    /// Imaginary parts of bins `0..=N/2`; bins `0` and `N/2` are exactly
    /// zero.
    #[must_use]
    pub fn im(&self) -> &[f32] {
        &self.im
    }
}

/// `A0 − A0·cos(2πn / (N − 1))` times `scale`, the backend's gain
/// correction.
pub(super) fn hann(len: FftLen, scale: f32) -> Box<[f32]> {
    let step = TAU / len.get().saturating_sub(1).to_f32().unwrap_or(f32::NAN);
    (0..len.get())
        .map(|n| {
            let phase = step * n.to_f32().unwrap_or(f32::NAN);
            consts::HANN_A0.mul_add(-phase.cos(), consts::HANN_A0) * scale
        })
        .collect()
}
