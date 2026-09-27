mod error;
mod interpolation;
#[cfg(test)]
mod tests;

pub use error::InterpError;
pub use interpolation::{Interpolation, interpolate};
