mod dft;
mod kernels;
#[cfg(test)]
mod tests;

pub(crate) use dft::{Dft, Work};
pub(crate) use kernels::{correlate, magnitude};
