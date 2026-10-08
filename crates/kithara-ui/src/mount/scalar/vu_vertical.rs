/// A vertical pair of level bars with a volume cap.
#[derive(kithara_derive::Control)]
#[control(size = skin.vu_vertical.size)]
pub(crate) struct VuVertical;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    #[derive(Builder)]
    pub(crate) struct VuVertical {
        pub(crate) ticks: bool,
    }

    use crate::{
        atoms::vu::VerticalVu,
        hosts::controls::{Drag, Draws, Grip, Reading},
        interact::{CursorShape, recognizers::Track},
        render::{ReadValue, Skin, StereoLevels},
    };

    impl Draws for VuVertical {
        type Painter = VerticalVu;

        fn data(&self, read: Reading<'_>) -> Option<StereoLevels> {
            match read.value {
                Some(ReadValue::Stereo(levels)) => Some(*levels),
                _ => None,
            }
        }

        fn grip(&self, _skin: &Skin, _data: &StereoLevels) -> Grip {
            Grip::Drag(
                Drag::builder()
                    .cursor(CursorShape::ResizeV)
                    .track(Track::AbsoluteVertical)
                    .build(),
            )
        }

        fn painter(&self, skin: &Skin) -> VerticalVu {
            VerticalVu::new(self.ticks, skin)
        }
    }
}
