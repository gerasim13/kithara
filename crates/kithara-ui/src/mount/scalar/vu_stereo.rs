/// A horizontal pair of level bars with a volume cap.
#[derive(kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.vu_stereo.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct VuStereo;

#[cfg(feature = "render")]
mod host {
    use super::VuStereo;
    use crate::{
        atoms::vu::StereoMeter,
        interact::{CursorShape, recognizers::Track},
        render::{
            ReadValue, Skin, StereoLevels,
            controls::{Drag, Draws, Grip, Reading},
        },
    };

    impl Draws for VuStereo {
        type Painter = StereoMeter;

        fn data(&self, read: Reading<'_>) -> Option<StereoLevels> {
            match read.value {
                Some(ReadValue::Stereo(levels)) => Some(*levels),
                _ => None,
            }
        }

        fn grip(&self, _skin: &Skin, _data: &StereoLevels) -> Grip {
            Grip::Drag(
                Drag::builder()
                    .cursor(CursorShape::ResizeH)
                    .track(Track::AbsoluteHorizontal)
                    .build(),
            )
        }

        fn painter(&self, skin: &Skin) -> StereoMeter {
            StereoMeter::new(skin)
        }
    }
}
