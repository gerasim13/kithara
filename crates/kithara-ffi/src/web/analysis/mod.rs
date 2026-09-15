//! Track analysis in the browser: one pass per queued track, published to JS.

#[cfg(feature = "analysis")]
pub(crate) mod encode;
#[cfg(feature = "analysis")]
pub(crate) mod runs;
#[cfg(not(feature = "analysis"))]
#[path = "disabled.rs"]
pub(crate) mod runs;
