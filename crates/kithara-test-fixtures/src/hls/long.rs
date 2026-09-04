use std::sync::OnceLock;

use super::HlsBundle;
use crate::assets::{long_hls_drm, long_hls_plain};

fn load(cell: &'static OnceLock<HlsBundle>, encrypted: bool) -> &'static HlsBundle {
    cell.get_or_init(|| {
        let asset = if encrypted {
            long_hls_drm()
        } else {
            long_hls_plain()
        };
        HlsBundle::try_from(&asset).expect("invariant: generated long HLS bundle")
    })
}

/// Build-cached long plain HLS fixture.
#[must_use]
pub fn long_plain() -> &'static HlsBundle {
    static BUNDLE: OnceLock<HlsBundle> = OnceLock::new();
    load(&BUNDLE, false)
}

/// Build-cached long AES-128 HLS fixture.
#[must_use]
pub fn long_drm() -> &'static HlsBundle {
    static BUNDLE: OnceLock<HlsBundle> = OnceLock::new();
    load(&BUNDLE, true)
}
