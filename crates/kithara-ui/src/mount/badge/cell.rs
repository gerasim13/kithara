use bon::Builder;

use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// One box of a grid, optionally captioned and optionally picked out.
#[derive(Builder)]
pub(crate) struct Cell {
    pub(crate) highlighted: bool,
    pub(crate) label: Option<InternId>,
}

impl Control for Cell {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.cell.size
    }
}
