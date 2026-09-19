#[cfg(feature = "ranged")]
mod derive;

#[cfg(feature = "ranged")]
pub(crate) use derive::expand;
