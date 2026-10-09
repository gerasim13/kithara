/// One formatted number read from an endpoint.
#[derive(kithara_derive::Control)]
#[control(size = skin.telemetry.size)]
pub(crate) struct Telemetry;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::module::ScalarFormat;

    #[derive(Builder)]
    pub(crate) struct Telemetry {
        pub(crate) format: ScalarFormat,
        pub(crate) framed: bool,
    }

    use crate::{
        atoms::label::Telemetry as Face,
        hosts::controls::{Draws, Reading},
        render::{ReadValue, Skin},
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
