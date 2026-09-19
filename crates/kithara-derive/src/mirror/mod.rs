#[cfg(feature = "mirror")]
mod derive;

#[cfg(feature = "mirror")]
pub(crate) use derive::expand;
