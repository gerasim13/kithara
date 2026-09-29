mod cascade;
mod interpolate;
#[cfg(feature = "spectrum")]
mod spectrum;

pub(crate) use kithara_apple::accelerate::{
    deinterleave_pair_f32 as deinterleave_pair, downmix_pair_f32 as downmix_pair,
    interleave_pair_f32 as interleave_pair, max_magnitude_f32 as peak,
    sum_squares_f32 as sum_squares,
};

#[cfg(feature = "spectrum")]
pub(crate) use self::spectrum::{Dft, Work, correlate, multiply};
pub(crate) use self::{cascade::Cascade, interpolate::interpolate};
