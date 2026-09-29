use kithara_apple::accelerate::{OutOfWindow, linear_interpolate_f32, quadratic_interpolate_f32};

use crate::{
    backend,
    interp::{InterpError, Interpolation},
};

/// Linear and Quadratic on `vDSP_vlint`/`vDSP_vqint`, Hermite and Watte on
/// the scalar kernel.
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
            backend::interpolate::interpolate(method, window, positions, output)
        }
    }
}

const fn out_of_window(_: OutOfWindow) -> InterpError {
    InterpError::OutOfWindow
}
