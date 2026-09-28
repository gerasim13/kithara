use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use realfft::{RealToComplex, RealToComplexEven, num_complex::Complex};

use crate::spectrum::{FftLen, SpectrumError};

/// realfft's even-length real FFT, planned once per length.
pub(crate) struct Dft(RealToComplexEven<f32>);

/// The windowed frame, the complex bins and the scratch the FFT writes
/// through.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, vis = "pub(crate)")]
pub(crate) struct Work {
    /// the frame the next `forward` transforms
    #[field(get_mut, deref = "[f32]")]
    input: SampleBuffer,
    output: Box<[Complex<f32>]>,
    scratch: Box<[Complex<f32>]>,
}

impl TryFrom<FftLen> for Dft {
    type Error = SpectrumError;

    fn try_from(len: FftLen) -> Result<Self, Self::Error> {
        Ok(Self(RealToComplexEven::new(
            len.get(),
            &mut rustfft::FftPlanner::new(),
        )))
    }
}

impl Dft {
    /// realfft leaves its output unscaled, so the window keeps unit gain.
    pub(crate) const WINDOW_SCALE: f32 = 1.0;

    /// The frame plane comes from `pools`; the complex planes are realfft's
    /// own.
    pub(crate) fn work<S>(&self, pools: &PoolRegion<S>) -> Result<Work, PoolError>
    where
        S: HasPool<f32>,
    {
        Ok(Work {
            input: pools.get_with_len::<f32>(self.0.len())?,
            output: self.0.make_output_vec().into_boxed_slice(),
            scratch: self.0.make_scratch_vec().into_boxed_slice(),
        })
    }

    /// Bins `0..=N/2` of the work's frame into `re` and `im`; realfft uses
    /// the frame as scratch and leaves it unspecified.
    pub(crate) fn forward(
        &self,
        work: &mut Work,
        [re, im]: [&mut [f32]; 2],
    ) -> Result<(), SpectrumError> {
        let Work {
            input,
            output,
            scratch,
        } = work;
        self.0
            .process_with_scratch(input, output, scratch)
            .map_err(|_| SpectrumError::Shape)?;
        for ((bin, re), im) in output.iter().zip(re.iter_mut()).zip(im.iter_mut()) {
            *re = bin.re;
            *im = bin.im;
        }
        Ok(())
    }
}
