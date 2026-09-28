use std::mem;

use kithara_apple::accelerate::{DftError, RealDft};
pub(crate) use kithara_apple::accelerate::{
    correlate_f32 as correlate, magnitude_f32 as magnitude, multiply_f32 as multiply,
};
use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};

use super::deinterleave_pair;
use crate::spectrum::{FftLen, SpectrumError};

/// `vDSP_DFT_zrop` behind the interface of the portable DFT.
pub(crate) struct Dft {
    dft: RealDft,
    len: usize,
    half: usize,
}

/// The windowed frame and the even and odd samples `vDSP_DFT_zrop` reads.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, vis = "pub(crate)")]
pub(crate) struct Work {
    /// the frame the next `forward` transforms
    #[field(get_mut, deref = "[f32]")]
    input: SampleBuffer,
    even: SampleBuffer,
    odd: SampleBuffer,
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
            even: pools.get_with_len::<f32>(self.half)?,
            odd: pools.get_with_len::<f32>(self.half)?,
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
        let _ = deinterleave_pair(&work.input, &mut work.even, &mut work.odd);
        let (Some((re_last, re_packed)), Some((im_last, im_packed))) =
            (re.split_last_mut(), im.split_last_mut())
        else {
            return Err(SpectrumError::Shape);
        };
        self.dft
            .execute([&*work.even, &*work.odd], [re_packed, &mut *im_packed])
            .map_err(dft_error)?;
        let nyquist = im_packed
            .first_mut()
            .map(mem::take)
            .ok_or(SpectrumError::Shape)?;
        *re_last = nyquist;
        *im_last = 0.0;
        Ok(())
    }
}

const fn dft_error(error: DftError) -> SpectrumError {
    match error {
        DftError::Setup => SpectrumError::Setup,
        DftError::Shape => SpectrumError::Shape,
    }
}
