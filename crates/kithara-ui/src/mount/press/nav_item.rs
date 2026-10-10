use crate::size::{Dim, SizeSpec};

/// One row of the navigation rail: an icon, a word, and a selected state.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::new(Dim::Fill, Dim::Fixed(skin.nav.item_height)))]
pub(crate) struct NavItem;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{
        ids::InternId,
        module::{IconName, TextStyle},
    };

    #[derive(Builder)]
    pub(crate) struct NavItem {
        pub(crate) icon: IconName,
        pub(crate) label: InternId,
        pub(crate) style: Option<TextStyle>,
    }

    use crate::{
        atoms::{nav_item::NavItem as Face, painter::NavData},
        hosts::controls::{Draws, Grip, Reading},
        render::{ReadValue, Skin},
    };

    impl Draws for NavItem {
        type Painter = Face;

        /// A rail item is nothing without the page it points at, so an item
        /// whose endpoint has not said which page is current draws nothing —
        /// and neither does one whose art could not be read.
        fn data(&self, read: Reading<'_>) -> Option<NavData> {
            let (Some(ReadValue::Bool(active)), Some(mark)) = (read.value, self.icon.mark()) else {
                return None;
            };
            Some(NavData {
                mark,
                active: *active,
                label: read.ctx.ui.resolve(self.label).to_owned(),
            })
        }

        fn grip(&self, _skin: &Skin, _data: &NavData) -> Grip {
            Grip::Press
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin, self.style)
        }
    }
}
