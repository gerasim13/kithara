//! Queue orchestration over player tracks, loading, navigation and selection.

mod access;
mod engine_events;
mod lifecycle;
mod owner;
mod passthrough;
mod playback;
mod player;
mod selection;
mod state;
mod types;

#[cfg(test)]
pub(crate) use state::tests::test_session;

pub use self::{
    state::{Queue, QueueControl},
    types::{PlaybackView, Transition},
};
