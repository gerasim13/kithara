mod config;
mod core;
#[cfg(feature = "fdk-aac")]
mod fdk;
#[cfg(feature = "opus")]
mod opus;
mod registry;

pub(crate) use core::SymphoniaCodec;

pub(crate) use config::SymphoniaConfig;
pub(super) use registry::get_probe;
