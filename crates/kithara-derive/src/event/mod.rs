#[cfg(feature = "event")]
mod derive;

#[cfg(feature = "event")]
mod event_set;

#[cfg(feature = "event")]
pub(crate) use derive::derive;
#[cfg(feature = "event")]
pub(crate) use event_set::derive as derive_set;
