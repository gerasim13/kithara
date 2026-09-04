use bon::Builder;

use crate::{
    ids::InternId,
    module::IconName,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// One row of the navigation rail: an icon, a word, and a selected state.
#[derive(Builder)]
pub(crate) struct NavItem {
    pub(crate) icon: IconName,
    pub(crate) label: InternId,
}

impl Control for NavItem {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.nav.item_height))
    }
}

#[cfg(feature = "render")]
mod host {
    use super::NavItem;
    use crate::{
        atoms::{nav_item::NavItem as Face, painter::NavData},
        render::{
            ReadValue, Skin,
            controls::{Draws, Grip, Reading},
        },
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
            Face::new(skin)
        }
    }
}
