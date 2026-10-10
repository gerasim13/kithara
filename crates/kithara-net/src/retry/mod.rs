#[cfg(not(feature = "client-host"))]
mod client;
mod net;

#[cfg(not(feature = "client-host"))]
pub(crate) use client::RetryClient;
pub(crate) use net::RetryNet;
