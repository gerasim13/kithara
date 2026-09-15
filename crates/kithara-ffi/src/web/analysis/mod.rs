//! Track analysis in the browser: one pass per queued track, published to JS.

#[cfg_attr(feature = "analysis", path = "live.rs")]
#[cfg_attr(not(feature = "analysis"), path = "disabled.rs")]
mod backend;

pub(crate) use backend::{AnalysisRoute, AnalysisRuns};
