#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "probe-capture")))]
pub mod capture;

#[cfg(not(any(test, feature = "probe-capture", feature = "usdt")))]
mod noop;
#[cfg(any(test, feature = "probe-capture", feature = "usdt"))]
mod real;

#[cfg(not(any(test, feature = "probe-capture", feature = "usdt")))]
pub use noop::*;
#[cfg(any(test, feature = "probe-capture", feature = "usdt"))]
pub use real::*;
