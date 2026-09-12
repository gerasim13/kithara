//! AVQueuePlayer-analogue orchestration layer on top of `kithara-play`.

mod attempts;
mod config;
mod error;
mod event;
mod loader;
mod navigation;
mod queue;
#[cfg(test)]
pub(crate) use kithara_bufpool::testing as test_pools;
mod track;

pub use config::{CueIn, QueueConfig, QueueConfigPatch};
pub use error::QueueError;
pub use event::{AdvanceReason, ItemEvent, QueueEvent, QueueRepeatMode, TrackStatus};
pub use kithara_events::TrackId;
pub use navigation::{NavigationState, RepeatMode};
#[cfg(any(test, feature = "usdt"))]
pub use queue::test_utils;
pub use queue::{PlaybackView, Queue, QueueControl, Transition};
pub use track::{TrackEntry, TrackSource};
