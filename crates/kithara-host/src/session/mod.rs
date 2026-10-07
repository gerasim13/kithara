//! Concrete session state, graph dispatch, and platform backends.

pub(crate) mod decks;
mod dispatch;
mod graph;
pub(crate) mod protocol;
mod queue;
pub(crate) mod state;
#[cfg(test)]
pub(crate) mod tests;
mod transport;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native;
#[cfg(feature = "offline")]
pub(crate) mod offline;

#[cfg(target_arch = "wasm32")]
pub(crate) mod web;

pub(crate) use protocol::{HostCmd, HostDispatcher, Reply, SessionError, SessionSampleRate, ask};
pub(crate) use queue::HostProtocol;
pub(crate) use state::{HostRoot, RootView};
pub use transport::TransportEvent;
pub(crate) use transport::{Span, applied_spans};
#[cfg(target_arch = "wasm32")]
pub(crate) use web::{
    bridge_duration_secs, bridge_is_playing, bridge_position_secs, bridge_process_calls,
    bridge_underruns, remote, tick_and_poll_remote, warm_up_audio, worker_channel,
};
