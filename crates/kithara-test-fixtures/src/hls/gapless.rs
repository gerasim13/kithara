use std::sync::OnceLock;

use super::HlsBundle;
use crate::assets::{gapless_hls_drm, gapless_hls_plain};

fn load(cell: &'static OnceLock<HlsBundle>, encrypted: bool) -> &'static HlsBundle {
    cell.get_or_init(|| {
        let asset = if encrypted {
            gapless_hls_drm()
        } else {
            gapless_hls_plain()
        };
        HlsBundle::try_from(&asset).expect("invariant: generated gapless HLS bundle")
    })
}

/// Build-cached gapless plain HLS fixture.
#[must_use]
pub fn gapless_plain() -> &'static HlsBundle {
    static BUNDLE: OnceLock<HlsBundle> = OnceLock::new();
    load(&BUNDLE, false)
}

/// Build-cached gapless AES-128 HLS fixture.
#[must_use]
pub fn gapless_drm() -> &'static HlsBundle {
    static BUNDLE: OnceLock<HlsBundle> = OnceLock::new();
    load(&BUNDLE, true)
}
