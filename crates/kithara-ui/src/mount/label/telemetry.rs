use bon::Builder;

use crate::{module::ScalarFormat, mount::Control, size::SizeSpec, skin::SkinDoc};

/// One formatted number read from an endpoint.
#[derive(Builder)]
pub(crate) struct Telemetry {
    pub(crate) format: ScalarFormat,
    pub(crate) framed: bool,
}

impl Control for Telemetry {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.telemetry.size
    }
}
