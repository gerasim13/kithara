#![forbid(unsafe_code)]

//! Neutral master-output and finite offline-rendering protocols.

mod offline;

pub use offline::{
    OfflineRenderConfig, OfflineRenderError, OfflineRenderReport, OfflineRenderRequest,
    OfflineRenderer, RenderSink, RenderSinkError,
};
