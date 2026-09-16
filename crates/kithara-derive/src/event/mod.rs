#[cfg(feature = "event")]
mod derive;

#[cfg(feature = "event-set")]
mod event_set;

#[cfg(feature = "event")]
pub(crate) use derive::derive;
#[cfg(feature = "event-set")]
pub(crate) use event_set::derive as derive_set;
