#![forbid(unsafe_code)]

//! Neutral master-output and finite offline-rendering protocols.

mod offline;

pub use offline::{
    OfflineRenderError, OfflineRenderReport, OfflineRenderRequest, OfflineRenderer, RenderSink,
    RenderSinkError,
};
