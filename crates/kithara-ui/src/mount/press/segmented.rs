/// A row of mutually exclusive segments, one of them picked.
#[derive(kithara_derive::Control)]
#[control(size = skin.segmented.size)]
pub(crate) struct Segmented;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::ids::InternId;

    #[derive(Builder)]
    pub(crate) struct Segmented<'a> {
        pub(crate) items: &'a [InternId],
    }

    use num_traits::ToPrimitive;

    use crate::{
        atoms::design::segmented::{Segmented as Face, SegmentedData},
        hosts::controls::{Draws, Grip, Reading},
        render::{ReadValue, Skin},
    };

    impl Draws for Segmented<'_> {
        type Painter = Face;

        /// The picked cell is the reading rounded to a whole cell; a reading
        /// past the end of the row picks nothing rather than the last one.
        fn data(&self, read: Reading<'_>) -> Option<SegmentedData> {
            let Some(ReadValue::Scalar(value)) = read.value else {
                return None;
            };
            Some(SegmentedData {
                active: value
                    .round()
                    .to_usize()
                    .filter(|index| *index < self.items.len()),
                items: self
                    .items
                    .iter()
                    .map(|item| read.ctx.ui.resolve(*item).to_owned())
                    .collect(),
            })
        }

        fn grip(&self, _skin: &Skin, data: &SegmentedData) -> Grip {
            Grip::Index {
                count: data.items.len(),
            }
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
