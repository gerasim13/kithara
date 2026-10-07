#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]

//! Render stages for Kithara playback: the producer side that renders decoded
//! audio through Warp, and the deck that mixes loaded tracks on the audio
//! thread.

pub mod bridge;
mod consts;
mod crossfade;
mod lane;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod priority;
pub mod rt;
mod source;
mod worker;
pub use crossfade::{CrossfadeCurve, CrossfadeSettings, InvalidCrossfade};
#[cfg(test)]
pub(crate) use kithara_test_utils::bufpool as test_pools;
pub use lane::{LaneCommand, LaneFrame, LaneProtocol};
pub use priority::{ServiceClass, TrackPriority};
pub use source::WarpSource;
pub use worker::{
    EngineLoad, EngineLoadSnapshot, LoadRefusal, PlayWorker, PlayWorkerConfig,
    PlayWorkerConfigPatch, RegisteredAudio, TrackConfig,
};
