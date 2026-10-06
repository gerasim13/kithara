#[cfg(feature = "built-default")]
pub(crate) mod built;
mod entrypoints;
#[cfg(feature = "config")]
mod field;
#[cfg(feature = "patch")]
mod patch;

pub(crate) use entrypoints::config_derives;
#[cfg(feature = "patch")]
pub(crate) use patch::expand;

#[cfg(feature = "config")]
pub(crate) mod retained;

#[cfg(feature = "config")]
pub(crate) mod owner;
