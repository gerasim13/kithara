mod cascade;
mod interpolate;
#[cfg(feature = "spectrum")]
mod spectrum;

pub(crate) use kithara_apple::accelerate::{
    deinterleave_pair_f32 as deinterleave_pair, interleave_pair_f32 as interleave_pair,
    max_magnitude_f32 as peak,
};

#[cfg(feature = "spectrum")]
pub(crate) use self::spectrum::{Dft, Work, correlate, magnitude, multiply};
pub(crate) use self::{cascade::Cascade, interpolate::interpolate};
