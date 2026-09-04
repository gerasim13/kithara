#![forbid(unsafe_code)]

//! Neutral master-output and finite offline-rendering protocols.

mod live;
mod offline;

pub use live::{LiveOutput, OutputGroup};
pub use offline::{
    OfflineRenderError, OfflineRenderReport, OfflineRenderRequest, OfflineRenderer, RenderSink,
    RenderSinkError,
};
