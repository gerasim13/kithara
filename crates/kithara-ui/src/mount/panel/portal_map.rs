/// A tempo axis carrying one arc from the master tempo to each portal target.
#[derive(kithara_derive::Control)]
#[control(size = skin.portal_map.size)]
pub(crate) struct PortalMap;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::PortalMap;
    use crate::{
        atoms::pivot::map::{PortalMap as Face, PortalMapData},
        hosts::controls::{Draws, Reading},
        render::{ReadValue, Skin},
    };

    impl Draws for PortalMap {
        type Painter = Face;

        /// A host that reports no map draws nothing: an axis with no tempo on
        /// it would claim a range the host never named.
        fn data(&self, read: Reading<'_>) -> Option<PortalMapData> {
            match read.value {
                Some(ReadValue::PortalMap(view)) => Some((*view).into()),
                _ => None,
            }
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
