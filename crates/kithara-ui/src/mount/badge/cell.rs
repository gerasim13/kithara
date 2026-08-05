use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// One box of a grid, optionally captioned and optionally picked out.
pub(crate) struct Cell;

impl Control for Cell {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.cell.size
    }
}
