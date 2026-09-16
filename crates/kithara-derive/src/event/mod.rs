#[cfg(feature = "event")]
mod derive;

mod event_set;

#[cfg(feature = "event")]
pub(crate) use derive::derive;
pub(crate) use event_set::derive as derive_set;
