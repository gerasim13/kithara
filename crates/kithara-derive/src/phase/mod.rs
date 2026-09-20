#[cfg(feature = "phase")]
mod derive;

#[cfg(feature = "phase")]
pub(crate) use derive::expand;
