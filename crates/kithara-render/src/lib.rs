#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]
#![forbid(unsafe_code)]

#[cfg(test)]
pub(crate) use kithara_test_utils::bufpool as test_pools;

pub mod bridge;
mod consts;
pub mod context;
pub mod crossfade;
pub mod host_rt;
mod resource;
pub mod rt;
pub mod transport;
#[doc(hidden)]
pub mod transport_rt;
pub mod worker;
pub use resource::RenderResource;
mod error;
pub use error::{RenderError, ResponseError};
pub use worker::{
    EngineLoad, EngineLoadSnapshot, PlayWorker, PlayWorkerConfig, PlayWorkerConfigPatch,
    RegisteredAudio, ServiceClass, TrackConfig,
};
