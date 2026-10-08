#[cfg(feature = "built-default")]
pub(crate) mod built;
#[cfg(feature = "config")]
mod field;
#[cfg(feature = "patch")]
mod patch;

#[cfg(feature = "patch")]
pub(crate) use patch::expand;

#[cfg(feature = "config")]
pub(crate) mod retained;

#[cfg(feature = "config")]
pub(crate) mod owner;
