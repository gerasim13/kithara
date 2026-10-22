#[cfg(not(feature = "client-host"))]
mod client;
mod net;

#[cfg(not(feature = "client-host"))]
pub(crate) use client::{RawNet, RetryClient};
pub(crate) use net::RetryNet;
