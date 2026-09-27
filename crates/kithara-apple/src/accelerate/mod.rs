mod biquad;
mod ffi;
mod interpolation;
mod layout;
#[cfg(test)]
mod tests;
mod vector;

pub use biquad::{BiquadError, MultichannelBiquad};
pub use interpolation::{OutOfWindow, linear_interpolate_f32, quadratic_interpolate_f32};
pub use layout::{deinterleave_pair_f32, interleave_pair_f32};
pub use vector::max_magnitude_f32;
