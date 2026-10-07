//! AVQueuePlayer-analogue orchestration facade.
//!
//! This module groups the implementation by responsibility:
//!
//! - [`mod@state`] — the [`Queue`] owner, the [`QueueControl`] handle, the
//!   state both read, and the inherent helpers shared by the impl-block split.
//! - [`mod@view`] — what the queue publishes of its state for its handles.
//! - [`mod@command`] — the commands a [`QueueControl`] posts and the owner runs.
//! - [`mod@handle`] — the [`QueueControl`] methods that post them.
//! - [`mod@types`] — shared free items (`Transition`, helpers, internal shapes).
//! - [`mod@access`] — read-only API (`len`, `current`, `subscribe`, navigation getters).
//! - [`mod@lifecycle`] — track creation/removal (`append`, `insert`, `remove`, …).
//! - [`mod@selection`] — selection state machine (`select`, `advance_to_next`, …).
//! - [`mod@playback`] — runtime tick (`tick`, `position_seconds`, crossfade arming, event drain).
//! - [`mod@passthrough`] — `delegate!`-forwarded `PlayerImpl` controls.

mod access;
mod command;
mod engine_events;
mod handle;
mod lifecycle;
mod passthrough;
mod playback;
mod player;
mod selection;
mod state;
mod types;
mod view;

pub(crate) use command::{QueueCommand, QueuePostbox};
use state::QueueRuntime;
#[cfg(test)]
pub(crate) use state::tests::test_session;
pub(crate) use view::QueueView;

pub use self::{
    state::{Queue, QueueControl},
    types::{PlaybackView, Transition},
};
