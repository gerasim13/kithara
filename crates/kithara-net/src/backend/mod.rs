mod guards;

pub(crate) mod common;
#[cfg(not(reqwest_backend))]
mod pooled;

#[cfg(feature = "client-host")]
pub(crate) mod host;
#[cfg(feature = "client-host")]
pub(crate) use self::host::Transport as SelectedTransport;

#[cfg(apple_backend)]
mod apple;
#[cfg(all(apple_backend, not(feature = "client-host")))]
pub(crate) use self::apple::Transport as SelectedTransport;

#[cfg(reqwest_backend)]
mod reqwest;
#[cfg(reqwest_backend)]
pub(crate) use self::reqwest::Transport as SelectedTransport;
