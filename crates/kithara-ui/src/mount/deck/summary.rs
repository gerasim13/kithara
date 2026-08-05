use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The deck's headline: what is loaded and how it is playing.
pub(crate) struct Summary;

impl Control for Summary {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.deck.summary_size
    }
}
