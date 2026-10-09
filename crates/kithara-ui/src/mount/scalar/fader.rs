/// A rail and a cap, dragged along the rail.
#[derive(kithara_derive::Control)]
#[control(size = skin.fader.size)]
pub(crate) struct Fader;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{ids::InternId, module::FaderStyle};

    #[derive(Builder)]
    pub(crate) struct Fader {
        pub(crate) style: FaderStyle,
        pub(crate) label: Option<InternId>,
    }

    use num_traits::cast::AsPrimitive;

    use crate::{
        atoms::{design::fader::Fader as Face, painter::Captioned},
        hosts::controls::{Drag, Draws, Grip, Reading},
        interact::{CursorShape, recognizers::Track},
        render::{ReadValue, Skin},
    };

    impl Draws for Fader {
        type Painter = Face;

        /// A fader is the fraction it sits at, so one whose endpoint has not
        /// reported a fraction draws nothing rather than a rail at zero.
        fn data(&self, read: Reading<'_>) -> Option<Captioned> {
            let Some(ReadValue::Scalar(value)) = read.value else {
                return None;
            };
            Some(Captioned {
                label: self
                    .label
                    .map(|label| read.ctx.ui.resolve(label).to_owned()),
                value: value.clamp(0.0, 1.0).as_(),
            })
        }

        /// The hand grabs a point on the rail, and the value walks in the steps
        /// the skin names rather than in whole pixels.
        fn grip(&self, skin: &Skin, _data: &Captioned) -> Grip {
            Grip::Drag(
                Drag::builder()
                    .cursor(CursorShape::Grab)
                    .track(Track::AbsoluteHorizontal)
                    .step(skin.fader.step)
                    .build(),
            )
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.style, skin)
        }
    }
}
