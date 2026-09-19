mod bundle;
#[cfg(feature = "hls")]
mod gapless;
#[cfg(all(test, feature = "library"))]
pub(crate) mod hydrate;
#[cfg(feature = "hls")]
mod long;
pub(crate) mod manifest;
#[cfg(feature = "hls")]
mod rss;

pub use bundle::{HlsBundle, HlsBundleError, HlsResource};
#[cfg(feature = "hls")]
pub use gapless::{gapless_drm, gapless_plain};
#[cfg(feature = "hls")]
pub use long::{long_drm, long_plain};
#[cfg(feature = "hls")]
pub use rss::rss_plain;
