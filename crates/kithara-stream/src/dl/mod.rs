//! Unified download orchestrator.
//!
//! [`Downloader`] owns the sole [`HttpClient`](kithara_net::HttpClient) and
//! routes fetch commands from registered peers. Protocols register through a
//! [`FetchScope`] — the pool itself, or another peer's handle when one
//! download is carried by several registrations — and issue fetches through
//! [`PeerHandle::execute`].

mod batch;
mod cmd;
mod config;
mod downloader;
mod peer;
mod registry;
mod response;
mod scope;
#[cfg(test)]
mod tests;

pub use cmd::{FetchCmd, OnCompleteFn, OnResponseFn, OnSlowFn, WriterFn, reject_html_response};
pub use config::DownloaderConfig;
pub use downloader::Downloader;
pub use kithara_events::{RequestMethod, RequestPriority};
pub use peer::{Peer, PeerHandle, PeerRef};
pub use response::{BodyStream, FetchResponse};
pub use scope::FetchScope;
