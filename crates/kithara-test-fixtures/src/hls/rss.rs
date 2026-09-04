use std::sync::OnceLock;

use super::HlsBundle;
use crate::assets::rss_hls_plain;

/// Build-cached HLS fixture used by playback RSS measurements.
///
/// # Panics
///
/// Panics when the generated bundle manifest violates the fixture contract.
#[must_use]
pub fn rss_plain() -> &'static HlsBundle {
    static BUNDLE: OnceLock<HlsBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        HlsBundle::try_from(&rss_hls_plain()).expect("invariant: generated RSS HLS bundle")
    })
}
