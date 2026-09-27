use std::{
    num::NonZeroUsize,
    ops::{DerefMut, Range},
};

use kithara_apple::accelerate::{
    BiquadError, MultichannelBiquad, OutOfWindow, linear_interpolate_f32, quadratic_interpolate_f32,
};
pub(crate) use kithara_apple::accelerate::{
    deinterleave_pair_f32 as deinterleave_pair, interleave_pair_f32 as interleave_pair,
    max_magnitude_f32 as peak,
};

use crate::{
    filter::FilterError,
    interp::{InterpError, Interpolation},
};

/// `vDSP_biquadm` behind the interface of the portable cascade.
pub(crate) struct Cascade(MultichannelBiquad);

impl Cascade {
    pub(crate) fn new(channels: NonZeroUsize, sections: NonZeroUsize) -> Result<Self, FilterError> {
        MultichannelBiquad::new(channels, sections)
            .map(Self)
            .map_err(filter_error)
    }

    pub(crate) fn set_section(
        &mut self,
        section: usize,
        coefficients: [f32; 5],
    ) -> Result<(), FilterError> {
        self.0
            .set_section(section, coefficients)
            .map_err(filter_error)
    }

    pub(crate) fn process<P: DerefMut<Target = [f32]>>(
        &mut self,
        planes: &mut [P],
        range: Range<usize>,
    ) -> Result<(), FilterError> {
        self.0.process(planes, range).map_err(filter_error)
    }

    pub(crate) fn reset(&mut self) {
        self.0.reset();
    }

    pub(crate) fn copy_state_from(&mut self, source: &Self) -> Result<(), FilterError> {
        self.0.copy_state_from(&source.0).map_err(filter_error)
    }
}

const fn filter_error(error: BiquadError) -> FilterError {
    match error {
        BiquadError::Setup => FilterError::Setup,
        BiquadError::Shape => FilterError::Shape,
    }
}

/// Linear and Quadratic on `vDSP_vlint`/`vDSP_vqint`, Hermite and Watte on
/// `fearless_simd`.
pub(crate) fn interpolate(
    method: Interpolation,
    window: &[f32],
    positions: &[f32],
    output: &mut [f32],
) -> Result<usize, InterpError> {
    match method {
        Interpolation::Linear => {
            linear_interpolate_f32(window, positions, output).map_err(out_of_window)
        }
        Interpolation::Quadratic => {
            quadratic_interpolate_f32(window, positions, output).map_err(out_of_window)
        }
        Interpolation::Hermite | Interpolation::Watte => {
            super::interpolate::interpolate(method, window, positions, output)
        }
    }
}

const fn out_of_window(_: OutOfWindow) -> InterpError {
    InterpError::OutOfWindow
}
