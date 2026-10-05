use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::module::ScalarFormat;

/// One formatted number read from an endpoint.
#[derive(Builder, kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.telemetry.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Telemetry {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) format: ScalarFormat,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) framed: bool,
}

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Telemetry;
    use crate::{
        atoms::label::Telemetry as Face,
        render::{
            ReadValue, Skin,
            controls::{Draws, Reading},
        },
    };

    impl Draws for Telemetry {
        type Painter = Face;

        /// A reading is the number its endpoint reports, so one with no number
        /// draws nothing rather than a zero nobody measured.
        fn data(&self, read: Reading<'_>) -> Option<f64> {
            match read.value {
                Some(ReadValue::Scalar(value)) => Some(*value),
                _ => None,
            }
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.format, self.framed, skin)
        }
    }
}
