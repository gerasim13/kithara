#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "usdt")))]
pub mod capture;

#[cfg(not(feature = "usdt"))]
mod noop;
#[cfg(feature = "usdt")]
mod real;

#[cfg(not(feature = "usdt"))]
pub use noop::*;
#[cfg(feature = "usdt")]
pub use real::*;
