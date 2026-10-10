/// One box of a grid, optionally captioned and optionally picked out.
#[derive(kithara_derive::Control)]
#[control(size = skin.cell.size)]
pub(crate) struct Cell;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::ids::InternId;

    #[derive(Builder)]
    pub(crate) struct Cell {
        pub(crate) label: Option<InternId>,
        pub(crate) highlighted: bool,
    }

    use crate::{
        atoms::{design::cell::Cell as Face, painter::CellData},
        hosts::controls::{Draws, Reading},
        render::Skin,
    };

    impl Draws for Cell {
        type Painter = Face;

        fn data(&self, read: Reading<'_>) -> Option<CellData> {
            Some(CellData {
                highlighted: self.highlighted,
                label: self
                    .label
                    .map(|label| read.ctx.ui.resolve(label).to_owned()),
            })
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
