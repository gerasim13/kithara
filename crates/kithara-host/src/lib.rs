#![forbid(unsafe_code)]
#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]

//! Multi-player session ownership and output-graph runtime.

pub mod api;
pub mod bridge;
mod effects;
mod error;
mod host;
mod rt;
mod session;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use api::{CrossfaderBus, HostLevel, crossfader_gain};
pub use error::PlayError;
#[cfg(feature = "offline")]
pub use host::OfflineSessionConfig;
pub use host::{Host, HostConfig, HostOwned, RealtimeSessionConfig, SessionConfig};
pub use kithara_play::SessionSampleRate;
#[cfg(any(test, feature = "probe"))]
pub use session::testing;
