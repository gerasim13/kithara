use std::mem;

use kithara_apple::accelerate::{DftError, RealDft};
pub(crate) use kithara_apple::accelerate::{correlate_f32 as correlate, multiply_f32 as multiply};
use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};

use super::deinterleave_pair;
use crate::spectrum::{FftLen, SpectrumError};

mod consts {
    use super::RealDft;

    /// Samples a pool buffer may hold before its first `RealDft::ALIGN`
    /// boundary: an `f32` sits on four bytes.
    pub(super) const SLACK: usize = RealDft::ALIGN / size_of::<f32>() - 1;
}

/// `vDSP_DFT_zrop` behind the interface of the portable DFT.
pub(crate) struct Dft {
    dft: RealDft,
    len: usize,
    half: usize,
}

/// The windowed frame and the four planes `vDSP_DFT_zrop` reads and writes:
/// the frame's even and odd samples and the packed bins.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, vis = "pub(crate)")]
pub(crate) struct Work {
    /// the frame the next `forward` transforms
    #[field(get_mut, deref = "[f32]")]
    input: SampleBuffer,
    even: Plane,
    odd: Plane,
    re: Plane,
    im: Plane,
}

/// A pool buffer with room for `len` samples from its first
/// `RealDft::ALIGN` boundary on.
struct Plane {
    buffer: SampleBuffer,
    len: usize,
}

impl TryFrom<FftLen> for Dft {
    type Error = SpectrumError;

    fn try_from(len: FftLen) -> Result<Self, Self::Error> {
        Ok(Self {
            dft: RealDft::new(len.get()).map_err(dft_error)?,
            len: len.get(),
            half: len.get() / 2,
        })
    }
}

impl Dft {
    /// vDSP doubles every bin; the window takes the halving, so the spectrum
    /// matches the portable backend's.
    pub(crate) const WINDOW_SCALE: f32 = 0.5;

    /// Every plane comes from `pools`.
    pub(crate) fn work<S>(&self, pools: &PoolRegion<S>) -> Result<Work, PoolError>
    where
        S: HasPool<f32>,
    {
        Ok(Work {
            input: pools.get_with_len::<f32>(self.len)?,
            even: Plane::new(pools, self.half)?,
            odd: Plane::new(pools, self.half)?,
            re: Plane::new(pools, self.half)?,
            im: Plane::new(pools, self.half)?,
        })
    }

    /// Bins `0..=N/2` of the work's frame into `re` and `im`: the Nyquist
    /// bin vDSP packs into `im[0]` moves to the last bin, and both edge bins
    /// are real.
    pub(crate) fn forward(
        &self,
        work: &mut Work,
        [re, im]: [&mut [f32]; 2],
    ) -> Result<(), SpectrumError> {
        let (even, odd) = (work.even.aligned()?, work.odd.aligned()?);
        let _ = deinterleave_pair(&work.input, even, odd);
        let (packed_re, packed_im) = (work.re.aligned()?, work.im.aligned()?);
        self.dft
            .execute([&*even, &*odd], [&mut *packed_re, &mut *packed_im])
            .map_err(dft_error)?;
        let (Some((re_last, re_bins)), Some((im_last, im_bins))) =
            (re.split_last_mut(), im.split_last_mut())
        else {
            return Err(SpectrumError::Shape);
        };
        for ((bin_re, bin_im), (value_re, value_im)) in re_bins
            .iter_mut()
            .zip(im_bins.iter_mut())
            .zip(packed_re.iter().zip(packed_im.iter()))
        {
            *bin_re = *value_re;
            *bin_im = *value_im;
        }
        let nyquist = im_bins
            .first_mut()
            .map(mem::take)
            .ok_or(SpectrumError::Shape)?;
        *re_last = nyquist;
        *im_last = 0.0;
        Ok(())
    }
}

impl Plane {
    fn new<S>(pools: &PoolRegion<S>, len: usize) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        Ok(Self {
            buffer: pools.get_with_len::<f32>(len.saturating_add(consts::SLACK))?,
            len,
        })
    }

    /// The `len` samples from the boundary on.
    fn aligned(&mut self) -> Result<&mut [f32], SpectrumError> {
        let start = self.buffer.as_ptr().align_offset(RealDft::ALIGN);
        self.buffer
            .get_mut(start..start.saturating_add(self.len))
            .ok_or(SpectrumError::Shape)
    }
}

const fn dft_error(error: DftError) -> SpectrumError {
    match error {
        DftError::Setup => SpectrumError::Setup,
        DftError::Shape => SpectrumError::Shape,
    }
}
