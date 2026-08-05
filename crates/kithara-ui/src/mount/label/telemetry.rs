use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// One formatted number read from an endpoint.
pub(crate) struct Telemetry;

impl Control for Telemetry {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.telemetry.size
    }
}
