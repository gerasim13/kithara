/// A caption with a value beside it, toned by the document.
#[derive(kithara_derive::Control)]
#[control(size = skin.readout.size)]
pub(crate) struct Readout;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{ids::InternId, module::Tone};

    #[derive(Builder)]
    pub(crate) struct Readout {
        pub(crate) label: Option<InternId>,
        pub(crate) tone: Tone,
        pub(crate) framed: bool,
    }

    use crate::{
        atoms::readout::{Readout as Face, ReadoutData},
        hosts::controls::{Draws, Reading},
        render::{ReadValue, Skin},
    };

    impl Draws for Readout {
        type Painter = Face;

        /// A readout is a caption and the number under it, so one missing
        /// either draws nothing rather than half of itself.
        fn data(&self, read: Reading<'_>) -> Option<ReadoutData> {
            Some(ReadoutData {
                label: read.ctx.ui.resolve(self.label?).to_owned(),
                value: match read.value? {
                    ReadValue::Text(value) => (*value).to_owned(),
                    ReadValue::Scalar(value) => format!("{value:.2}"),
                    _ => return None,
                },
            })
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.tone, self.framed, skin)
        }
    }
}
